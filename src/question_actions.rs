//! The app boundary of the questions tab: store reads and writes, durability recovery, and
//! the identifier-only Herdr hints. The pure tab state lives in [`crate::ui::questions`].
//!
//! Every write goes through [`QuestionStore`] with the revision the user read. A save the
//! store cannot prove durable stays pending on the page: `Enter` confirms that exact
//! revision (or reads the record back first when the commit itself is unknown). Nothing
//! here retries a write or recreates a task.

use uuid::Uuid;

use crate::domain::{
    AnswerDraft, AnswerOrigin, DomainState, FindingAction, FindingLink, HumanStatus,
    ProvenanceOrigin, QuestionKind, QuestionRecord,
};
use crate::store::{
    default_state_dir, notify_question, QuestionDurability, QuestionError, QuestionNotification,
    QuestionSaveResult, QuestionStore, TaskStore,
};
use crate::threads::herdr;
use crate::ui::board::BoardModel;
use crate::ui::links::{task_links, TaskLink, ThreadRef};
use crate::ui::questions::{
    choices, AnswerInput, FollowUp, PendingCommit, QuestionFeed, QuestionIntent, FINDING_ACTIONS,
};

/// The answering actor the board records. Origin `UserUi` carries the authority.
const USER: &str = "user";

/// Read the board's question records once.
pub fn load_feed() -> QuestionFeed {
    match QuestionStore::new(default_state_dir()) {
        Err(QuestionError::AgentStoreRequired) => QuestionFeed::Disabled,
        Err(error) => QuestionFeed::Unavailable(error.to_string()),
        Ok(store) => match store.list(None) {
            Ok(records) => QuestionFeed::Loaded(records),
            Err(error) => QuestionFeed::Unavailable(error.to_string()),
        },
    }
}

fn reload(model: &mut BoardModel) {
    model.set_question_feed(load_feed());
}

fn reload_tasks(store: &TaskStore, domain: &mut DomainState, model: &mut BoardModel) {
    if let Ok(disk) = store.load() {
        domain.merge_tasks_from_disk(&disk);
        model.sync_from_domain(domain);
    }
}

fn question_store(model: &mut BoardModel) -> Option<QuestionStore> {
    match QuestionStore::new(default_state_dir()) {
        Ok(store) => Some(store),
        Err(error) => {
            model.set_message(format!("questions: {error}"));
            None
        }
    }
}

fn page_record(model: &BoardModel) -> Option<QuestionRecord> {
    let questions = model.questions();
    questions
        .page
        .and_then(|id| questions.feed.get(id))
        .cloned()
}

fn draft(revision: Uuid, text: String) -> AnswerDraft {
    AnswerDraft {
        expected_revision: revision,
        text,
        actor: USER.to_string(),
        origin: AnswerOrigin::UserUi,
        corrects_revision: None,
        approve: false,
        confirmed: false,
        approval: None,
        user_chat: None,
        source: None,
        action: None,
    }
}

/// One questions-tab action.
pub fn handle(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    intent: QuestionIntent,
) {
    let wide = model.frame_wide();
    match intent {
        QuestionIntent::Move(forward) => {
            if model.questions().page.is_some() {
                model.questions_mut().move_on_page(forward);
            } else {
                model.questions_mut().move_cursor(forward);
            }
        }
        QuestionIntent::Scroll(forward) => {
            if model.questions().page.is_some() {
                let scroll = &model.questions().scroll;
                scroll.set(if forward {
                    scroll.get() + 1
                } else {
                    scroll.get().saturating_sub(1)
                });
            } else {
                model.questions_mut().move_cursor(forward);
            }
        }
        QuestionIntent::Open => {
            if let Some(id) = model.questions().cursor {
                let questions = model.questions_mut();
                questions.return_task = None;
                questions.open(id);
            }
        }
        QuestionIntent::Row(id) => {
            let questions = model.questions_mut();
            if wide && questions.cursor != Some(id) {
                questions.cursor = Some(id);
                questions.page = None;
            } else {
                questions.return_task = None;
                questions.open(id);
            }
        }
        QuestionIntent::Back => back(model),
        QuestionIntent::Step(forward) => {
            if let Some(message) = model.questions_mut().step(forward) {
                model.set_message(message);
            }
        }
        QuestionIntent::ToggleCheck => {
            if let Some(message) = model.questions_mut().toggle_check() {
                model.set_message(message);
            }
        }
        QuestionIntent::Correct => correct(model),
        QuestionIntent::OpenTask => {
            if let Some(record) = page_record(model) {
                crate::ui::board::open_task_from_questions(
                    domain,
                    model,
                    record.reference.task_id,
                    Some(record.reference.id),
                );
            }
        }
        QuestionIntent::FromTask(id) => {
            reload(model);
            crate::ui::board::open_question_from_task(model, id);
        }
        QuestionIntent::Link(task, index) => link(domain, model, task, index),
        QuestionIntent::AnswerInsert(text) => {
            if let Some(input) = model.questions_mut().answer.as_mut() {
                input.text.push_str(&text);
            }
        }
        QuestionIntent::AnswerBackspace => {
            if let Some(input) = model.questions_mut().answer.as_mut() {
                input.text.pop();
            }
        }
        QuestionIntent::AnswerCancel => {
            model.questions_mut().answer = None;
            model.set_message("answer discarded · nothing saved");
        }
        QuestionIntent::Enter => enter(store, domain, model),
        QuestionIntent::Approve => decide_approval(store, domain, model, true),
        QuestionIntent::Reject => decide_approval(store, domain, model, false),
        QuestionIntent::Resend => resend(model),
    }
}

fn back(model: &mut BoardModel) {
    let questions = model.questions_mut();
    if questions.answer.is_some() {
        questions.answer = None;
        return;
    }
    if questions.correcting.is_some() {
        questions.correcting = None;
        return;
    }
    questions.page = None;
    questions.check = None;
    if questions.return_task.take().is_some() {
        // The task page the question came from is still parked on the board.
        questions.active = false;
    }
}

/// Refuse a new write while another save on this board is still unresolved.
fn pending_elsewhere(model: &mut BoardModel, id: Uuid) -> bool {
    match model.questions().pending {
        Some(pending) if pending.id() != id => {
            let number = model
                .questions()
                .feed
                .get(pending.id())
                .map(|record| format!("Q{}", record.reference.number))
                .unwrap_or_else(|| "another question".to_string());
            model.set_message(format!("resolve the pending save on {number} first"));
            true
        }
        _ => false,
    }
}

fn refuse_unrouted(model: &mut BoardModel, record: &QuestionRecord) -> bool {
    if record.draft.audience.is_none() && record.is_open() {
        model.set_message(format!(
            "Q{} has no target · route it with tsk route before answering",
            record.reference.number
        ));
        return true;
    }
    false
}

fn correct(model: &mut BoardModel) {
    let Some(record) = page_record(model) else {
        return;
    };
    if pending_elsewhere(model, record.reference.id) {
        return;
    }
    if model
        .questions()
        .pending
        .is_some_and(|pending| pending.id() == record.reference.id)
    {
        model.set_message("confirm the pending save first (enter)");
        return;
    }
    let Some(current) = record.current_answer() else {
        model.set_message("nothing to correct yet");
        return;
    };
    if record.reference.kind == QuestionKind::Approval {
        model.set_message("an approval is not corrected · ask a new question for new terms");
        return;
    }
    let id = record.reference.id;
    let revision = record.reference.revision;
    let questions = model.questions_mut();
    if choices(&record).is_empty() {
        questions.answer = Some(AnswerInput {
            id,
            revision,
            corrects: Some(current.revision),
            text: String::new(),
        });
    } else {
        questions.correcting = Some((id, revision));
    }
    model.set_message(format!(
        "correcting Q{} · the new answer is a new revision, the earlier one stays",
        record.reference.number
    ));
}

fn enter(store: &TaskStore, domain: &mut DomainState, model: &mut BoardModel) {
    let Some(record) = page_record(model) else {
        return;
    };
    let id = record.reference.id;
    if model
        .questions()
        .pending
        .is_some_and(|pending| pending.id() == id)
    {
        resolve_pending(store, domain, model);
        return;
    }
    if pending_elsewhere(model, id) {
        return;
    }
    if let Some(input) = model.questions().answer.clone() {
        if input.text.trim().is_empty() {
            model.set_message("type an answer first · esc cancels");
            return;
        }
        let mut answer = draft(input.revision, input.text.trim().to_string());
        answer.corrects_revision = input.corrects;
        save(
            store,
            domain,
            model,
            &record,
            answer,
            FollowUp::NotifyOrigin,
            None,
        );
        return;
    }
    if refuse_unrouted(model, &record) {
        return;
    }
    let questions = model.questions();
    if record.reference.kind == QuestionKind::Approval {
        if record.is_open() {
            model.set_message("space checks the terms · y approves · n rejects");
        }
        return;
    }
    if questions.choosing(&record) {
        let choice = questions.choice(&record);
        let corrects = questions
            .correcting
            .filter(|(cid, _)| *cid == id)
            .and_then(|_| record.current_answer().map(|answer| answer.revision));
        if record.reference.kind == QuestionKind::Finding {
            let (action, label) = FINDING_ACTIONS[choice];
            let mut answer = draft(record.reference.revision, label.to_string());
            answer.action = Some(action);
            answer.corrects_revision = corrects;
            let follow = if action == FindingAction::CreateAndRun {
                FollowUp::NotifyOriginAndLead
            } else {
                FollowUp::NotifyOrigin
            };
            let create = match action {
                FindingAction::Close => None,
                FindingAction::CreateAndRun => Some(true),
                FindingAction::CreateOnly => Some(false),
            };
            save(store, domain, model, &record, answer, follow, create);
        } else {
            let Some(text) = record.draft.options.get(choice).cloned() else {
                return;
            };
            let mut answer = draft(record.reference.revision, text);
            answer.corrects_revision = corrects;
            save(
                store,
                domain,
                model,
                &record,
                answer,
                FollowUp::NotifyOrigin,
                None,
            );
        }
        return;
    }
    if record.is_open() && record.reference.kind == QuestionKind::Question {
        model.questions_mut().answer = Some(AnswerInput {
            id,
            revision: record.reference.revision,
            corrects: None,
            text: String::new(),
        });
        model.set_message("type the answer · enter saves · esc cancels");
    }
}

fn decide_approval(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    approve: bool,
) {
    let Some(record) = page_record(model) else {
        return;
    };
    if record.reference.kind != QuestionKind::Approval || !record.is_open() {
        return;
    }
    if pending_elsewhere(model, record.reference.id) || refuse_unrouted(model, &record) {
        return;
    }
    if model
        .questions()
        .pending
        .is_some_and(|pending| pending.id() == record.reference.id)
    {
        model.set_message("confirm the pending save first (enter)");
        return;
    }
    let mut answer = draft(
        record.reference.revision,
        if approve { "approved" } else { "rejected" }.to_string(),
    );
    if approve {
        if !model.questions().checked(&record) {
            model.set_message("check the target, scope, and revision first (space)");
            return;
        }
        answer.approve = true;
        answer.confirmed = true;
        answer.approval = record.draft.approval.clone();
    }
    save(
        store,
        domain,
        model,
        &record,
        answer,
        FollowUp::NotifyOrigin,
        None,
    );
}

/// Write one answer. `create` is `Some(run)` for a finding's task actions.
fn save(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    record: &QuestionRecord,
    answer: AnswerDraft,
    follow: FollowUp,
    create: Option<bool>,
) {
    let Some(questions) = question_store(model) else {
        return;
    };
    let id = record.reference.id;
    let source_task = record.reference.task_id;
    let title = record.reference.summary.clone();
    let result = match create {
        None => questions.answer(id, answer),
        Some(run) => questions.answer_with_task(id, answer, |state| {
            let scope = state
                .get(source_task)
                .filter(|task| !task.soft_deleted)
                .map(|task| task.scope.clone())
                .ok_or_else(|| "the finding's task is missing".to_string())?;
            let task = state
                .create(&title, None, scope, ProvenanceOrigin::Manual)
                .map_err(|error| error.to_string())?;
            if run {
                state
                    .set_status(task, HumanStatus::Doing)
                    .map_err(|error| error.to_string())?;
            }
            Ok(Some(task))
        }),
    };
    let number = record.reference.number;
    match result {
        Ok(saved) => {
            {
                let questions = model.questions_mut();
                questions.answer = None;
                questions.correcting = None;
                questions.check = None;
            }
            match saved.durability {
                QuestionDurability::Durable => {
                    let report = follow_up(&saved, follow);
                    record_report(model, id, format!("saved · {report}"));
                    model.set_message(format!("Q{number} saved · hints below"));
                }
                QuestionDurability::Uncertain => {
                    model.questions_mut().pending = Some(PendingCommit::Uncertain {
                        id,
                        revision: saved.record.reference.revision,
                        follow_up: follow,
                    });
                    model.set_message(format!(
                        "Q{number} saved but durability is not proven · enter confirms · no hint sent"
                    ));
                }
            }
        }
        Err(QuestionError::CommitUnknown) => {
            model.questions_mut().pending = Some(PendingCommit::Unknown {
                id,
                prior_revision: record.reference.revision,
                follow_up: follow,
            });
            model.set_message(format!(
                "Q{number}: save result unknown · enter reads it back · nothing retried"
            ));
        }
        Err(QuestionError::StaleRevision) => {
            let questions = model.questions_mut();
            questions.answer = None;
            questions.correcting = None;
            questions.check = None;
            model.set_message(format!(
                "Q{number} changed since you read it · read it again · nothing saved"
            ));
        }
        Err(error) => model.set_message(format!("Q{number}: {error}")),
    }
    reload(model);
    if create.is_some() {
        reload_tasks(store, domain, model);
    }
}

fn resolve_pending(store: &TaskStore, domain: &mut DomainState, model: &mut BoardModel) {
    let Some(pending) = model.questions().pending else {
        return;
    };
    let Some(questions) = question_store(model) else {
        return;
    };
    match pending {
        PendingCommit::Uncertain {
            id,
            revision,
            follow_up: follow,
        } => match questions.confirm_durability(id, revision) {
            Ok(saved) => {
                model.questions_mut().pending = None;
                let report = follow_up(&saved, follow);
                record_report(model, id, format!("durable · {report}"));
                model.set_message(format!(
                    "Q{} durable · hints below",
                    saved.record.reference.number
                ));
            }
            Err(QuestionError::StaleRevision) => {
                model.questions_mut().pending = None;
                model.set_message(
                    "the question moved on since that save · read it again · no hint sent",
                );
            }
            Err(error) => model.set_message(format!("still not proven durable: {error}")),
        },
        PendingCommit::Unknown {
            id,
            prior_revision,
            follow_up: follow,
        } => match questions.read(id) {
            Ok(record) if record.reference.revision == prior_revision => {
                model.questions_mut().pending = None;
                model.set_message(format!(
                    "Q{}: nothing was saved · you can answer again",
                    record.reference.number
                ));
            }
            Ok(record)
                if record
                    .current_answer()
                    .is_some_and(|answer| answer.origin == AnswerOrigin::UserUi) =>
            {
                model.questions_mut().pending = Some(PendingCommit::Uncertain {
                    id,
                    revision: record.reference.revision,
                    follow_up: follow,
                });
                model.set_message(format!(
                    "Q{}: the save is visible · enter confirms its durability",
                    record.reference.number
                ));
            }
            Ok(record) => {
                model.questions_mut().pending = None;
                model.set_message(format!(
                    "Q{} changed by someone else · read it again · no hint sent",
                    record.reference.number
                ));
            }
            Err(error) => model.set_message(format!("read back failed: {error}")),
        },
    }
    reload(model);
    reload_tasks(store, domain, model);
}

/// Keep the last hint outcome on the question page, where it can wrap in full.
fn record_report(model: &mut BoardModel, id: Uuid, text: String) {
    model.questions_mut().report = Some((id, text));
}

fn notification_text(notification: QuestionNotification, to: &str) -> String {
    match notification {
        QuestionNotification::Notified => format!("hint submitted to {to}"),
        QuestionNotification::Failed => format!("hint to {to} not delivered"),
        QuestionNotification::NotAttempted => "no hint sent".to_string(),
    }
}

/// What a durable save owes: the asking Thread's hint, then (create-and-run) the run request.
fn follow_up(saved: &QuestionSaveResult, follow: FollowUp) -> String {
    let origin = notification_text(
        notify_question(saved, false),
        &saved.record.draft.origin_thread,
    );
    match follow {
        FollowUp::NotifyOrigin => origin,
        FollowUp::NotifyOriginAndLead => format!("{origin} · {}", request_lead(&saved.record)),
    }
}

/// Send the run request of a finding decided as create-and-run to its explicit Lead
/// address, only when exactly one live agent carries that name.
fn request_lead(record: &QuestionRecord) -> String {
    let Some(name) = record.draft.lead_recipient.as_deref() else {
        return "run request not sent: no lead address on this finding".to_string();
    };
    let FindingLink::One(task) = record.finding_link() else {
        return "run request not sent: no single linked task".to_string();
    };
    if std::env::var("HERDR_ENV").as_deref() != Ok("1") {
        return format!("run request to {name} not sent: outside Herdr");
    }
    let rows = match herdr::list_agents() {
        Ok(rows) => rows,
        Err(_) => return format!("run request to {name} not sent: cannot list live agents"),
    };
    let live = rows.iter().filter(|row| row.name == name).count();
    match live {
        0 => format!("run request to {name} not sent: no live agent by that name"),
        1 => {
            let hint = format!(
                "tsk finding Q{} id={} revision={} task={task} action=create_and_run; read back from the Agent state directory before acting.",
                record.reference.number, record.reference.id, record.reference.revision
            );
            match herdr::prompt(name, &hint) {
                Ok(()) => format!("run request submitted to {name} (not a started Thread)"),
                Err(_) => format!("run request to {name} failed to send"),
            }
        }
        n => format!("run request to {name} not sent: {n} live agents share that name"),
    }
}

/// `r`: resend a create-and-run finding's run request. Reads the record back and confirms
/// the visible revision is durable first; never writes, never creates a task.
fn resend(model: &mut BoardModel) {
    let Some(record) = page_record(model) else {
        return;
    };
    let Some(questions) = question_store(model) else {
        return;
    };
    let fresh = match questions.read(record.reference.id) {
        Ok(fresh) => fresh,
        Err(error) => {
            model.set_message(format!("read back failed: {error} · nothing sent"));
            return;
        }
    };
    let decided_run = fresh
        .current_answer()
        .is_some_and(|answer| answer.action == Some(FindingAction::CreateAndRun));
    if !decided_run {
        model.set_message("only a finding decided as create-and-run has a run request");
        reload(model);
        return;
    }
    match questions.confirm_durability(fresh.reference.id, fresh.reference.revision) {
        Ok(saved) => {
            let report = request_lead(&saved.record);
            record_report(
                model,
                saved.record.reference.id,
                format!("resent · {report}"),
            );
            model.set_message(format!(
                "Q{} · run request resent · see below",
                saved.record.reference.number
            ));
        }
        Err(error) => model.set_message(format!("not resent: {error}")),
    }
    reload(model);
}

fn link(domain: &DomainState, model: &mut BoardModel, task: Uuid, index: usize) {
    let Some(found) = domain.get(task) else {
        return;
    };
    let links = task_links(found.notes.as_deref());
    let message = match links.get(index) {
        None => return,
        Some(TaskLink::Issue { id, url }) => format!("{id} · {url}"),
        Some(TaskLink::Pr { url }) | Some(TaskLink::Ci { url }) => url.clone(),
        Some(TaskLink::UnverifiableThread { reason, .. }) => {
            format!("this Thread line cannot be verified ({reason}) · nothing focused")
        }
        Some(TaskLink::Thread(reference)) => focus_thread(reference),
    };
    model.set_message(message);
}

/// Focus the one live pane that matches the whole tuple, or say why not.
fn focus_thread(reference: &ThreadRef) -> String {
    if std::env::var("HERDR_ENV").as_deref() != Ok("1") {
        return format!("{} · outside Herdr · nothing focused", reference.name);
    }
    let rows = match herdr::list_agents() {
        Ok(rows) => rows,
        Err(_) => return "cannot list live agents · nothing focused".to_string(),
    };
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.name == reference.name
                && row.workspace_id == reference.workspace_id
                && row.tab_id == reference.tab_id
                && row.pane_id == reference.pane_id
        })
        .collect();
    match matches.as_slice() {
        [row] => match herdr::focus(row) {
            Ok(()) => format!("focused {}", reference.name),
            Err(error) => format!("focus failed: {error}"),
        },
        [] => format!(
            "no live pane matches {} {} {} {} · nothing focused",
            reference.name, reference.workspace_id, reference.tab_id, reference.pane_id
        ),
        _ => "several live panes match · nothing focused".to_string(),
    }
}
