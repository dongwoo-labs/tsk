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
    ProvenanceOrigin, QuestionKind, QuestionRecord, TaskScope,
};
use crate::store::{
    default_state_dir, notify_question, valid_secret_question, QuestionDurability, QuestionError,
    QuestionNotification, QuestionSaveResult, QuestionStore, TaskStore, SECRET_SET,
};
use crate::threads::herdr;
use crate::ui::board::BoardModel;
use crate::ui::links::{task_links, TaskLink, ThreadRef};
use crate::ui::questions::{
    choices, AnswerAttempt, AnswerInput, FollowUp, PageClick, PendingCommit, QuestionFeed,
    QuestionIntent, FINDING_ACTIONS,
};

/// The answering actor the board records. Origin `UserUi` carries the authority.
pub(crate) const USER: &str = "user";

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
        comment: None,
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
                model.questions().scroll_page(forward);
            } else {
                model.questions_mut().move_cursor(forward);
            }
        }
        QuestionIntent::Open => {
            if let Some(id) = model.questions().cursor {
                if crate::ui::board::refuse_answer_elsewhere(model, id) {
                    return;
                }
                let questions = model.questions_mut();
                questions.return_task = None;
                questions.open(id);
            }
        }
        QuestionIntent::Row(id) => {
            if crate::ui::board::refuse_answer_elsewhere(model, id) {
                return;
            }
            let questions = model.questions_mut();
            if wide && questions.cursor != Some(id) {
                questions.cursor = Some(id);
                questions.page = None;
                questions.scroll.set(0);
                questions.follow.set(false);
            } else {
                questions.return_task = None;
                questions.open(id);
            }
        }
        QuestionIntent::Page(click) => {
            // At 110 and up the detail previews the cursor's record: open it first.
            if model.questions().page.is_none() {
                let Some(id) = model.questions().shown(wide).map(|r| r.reference.id) else {
                    return;
                };
                if crate::ui::board::refuse_answer_elsewhere(model, id) {
                    return;
                }
                let questions = model.questions_mut();
                questions.return_task = None;
                questions.open(id);
            }
            let next = match click {
                PageClick::Option(index) => {
                    model.questions_mut().choose(index);
                    return;
                }
                PageClick::Enter => QuestionIntent::Enter,
                PageClick::Check => QuestionIntent::ToggleCheck,
                PageClick::OpenTask => QuestionIntent::OpenTask,
                PageClick::Correct => QuestionIntent::Correct,
                PageClick::Comment => QuestionIntent::Comment,
                PageClick::Resend => QuestionIntent::Resend,
            };
            handle(store, domain, model, next);
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
        QuestionIntent::Correct => {
            correct(model);
            model.questions().follow.set(true);
        }
        QuestionIntent::Comment => {
            comment(model);
            model.questions().follow.set(true);
        }
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
            model.questions().follow.set(true);
        }
        QuestionIntent::AnswerBackspace => {
            if let Some(input) = model.questions_mut().answer.as_mut() {
                input.text.pop();
            }
            model.questions().follow.set(true);
        }
        QuestionIntent::AnswerCancel => {
            // Dropping a secret input wipes its buffer. A comment edit leaves whatever was
            // staged before untouched: Esc discards the edit, not the earlier text.
            let input = model.questions_mut().answer.take();
            model.set_message(match input {
                Some(input) if input.secret => {
                    "secret discarded · nothing sent to fnox · nothing saved"
                }
                Some(input) if input.comment => "comment edit discarded · earlier text kept",
                _ => "answer discarded · nothing saved",
            });
        }
        QuestionIntent::Enter => {
            enter(store, domain, model);
            model.questions().follow.set(true);
        }
        QuestionIntent::Approve => decide_approval(store, domain, model, true),
        QuestionIntent::Reject => decide_approval(store, domain, model, false),
        QuestionIntent::Resend => resend(model),
        QuestionIntent::ToggleClosed => model.questions_mut().toggle_closed(),
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

/// No board write on a record without an audience, open or closed: tsk never guesses its
/// target, so neither an answer nor a correction is offered until `tsk route` sets one.
fn refuse_unrouted(model: &mut BoardModel, record: &QuestionRecord) -> bool {
    if record.draft.audience.is_none() {
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
    if refuse_unrouted(model, &record) {
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
    if record.reference.kind == QuestionKind::Secret {
        model.set_message("a secret is not corrected · ask a new secret question to set it again");
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
            secret: false,
            comment: false,
        });
    } else {
        questions.correcting = Some((id, revision));
    }
    model.set_message(format!(
        "correcting Q{} · the new answer is a new revision, the earlier one stays",
        record.reference.number
    ));
}

/// `m`: write or edit the comment staged beside an open approval's y/n decision. The text
/// is kept in session state until `y`/`n` saves it, or the record moves past this revision.
fn comment(model: &mut BoardModel) {
    let Some(record) = page_record(model) else {
        return;
    };
    if record.reference.kind != QuestionKind::Approval || !record.is_open() {
        return;
    }
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
    if refuse_unrouted(model, &record) {
        return;
    }
    let id = record.reference.id;
    let revision = record.reference.revision;
    let staged = model
        .questions()
        .comments
        .get(&id)
        .cloned()
        .unwrap_or_default();
    model.questions_mut().answer = Some(AnswerInput::comment(id, revision, staged));
    model.set_message("type a comment for this decision · enter stages it · esc discards the edit");
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
    if pending_elsewhere(model, id) || refuse_unrouted(model, &record) {
        return;
    }
    if record.reference.kind == QuestionKind::Secret {
        secret(store, domain, model, &record);
        return;
    }
    if let Some(input) = model.questions().answer.clone() {
        if input.comment {
            // A comment never saves on its own: it stages beside the still-open decision
            // until `y`/`n` writes it with the approve/reject answer.
            let text = input.text.trim().to_string();
            let questions = model.questions_mut();
            questions.answer = None;
            if text.is_empty() {
                questions.comments.remove(&input.id);
                model.set_message("comment cleared");
            } else {
                questions.comments.insert(input.id, text);
                model.set_message("comment staged · y approves · n rejects, either carries it");
            }
            return;
        }
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
    let questions = model.questions();
    if record.reference.kind == QuestionKind::Approval {
        if record.is_open() {
            model.set_message("space checks the terms · y approves · n rejects · m comments");
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
            secret: false,
            comment: false,
        });
        model.set_message("type the answer · enter saves · esc cancels");
    }
}

/// `Enter` on a secret question: open the masked input, or hand the typed value to fnox
/// and record only `set`. The value never reaches the store, a message, or a log.
fn secret(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    record: &QuestionRecord,
) {
    let id = record.reference.id;
    let number = record.reference.number;
    let typing = model
        .questions()
        .answer
        .as_ref()
        .filter(|input| input.id == id && input.secret)
        .map(|input| input.text.is_empty());
    match typing {
        None => {
            if record.is_open() {
                model.questions_mut().answer =
                    Some(AnswerInput::secret(id, record.reference.revision));
                model.set_message(
                    "type the secret · shown as • · enter hands it to fnox set · esc cancels",
                );
            }
            return;
        }
        Some(true) => {
            model.set_message("type the secret first · esc cancels");
            return;
        }
        Some(false) => {}
    }
    // The input leaves the page now: whatever happens next, its buffer is wiped on drop.
    let Some(input) = model.questions_mut().answer.take() else {
        return;
    };
    let Some(questions) = question_store(model) else {
        return;
    };
    let handed = hand_to_fnox(&questions, &input, &crate::fnox::binary());
    drop(input);
    let fresh = match handed {
        Ok(fresh) => fresh,
        Err(reason) => {
            model.set_message(format!("Q{number}: {reason} · nothing saved"));
            reload(model);
            return;
        }
    };
    save(
        store,
        domain,
        model,
        &fresh,
        draft(fresh.reference.revision, SECRET_SET.to_string()),
        FollowUp::NotifyOrigin,
        None,
    );
    if model
        .questions()
        .feed
        .get(id)
        .is_some_and(QuestionRecord::is_open)
    {
        let reason = model.message().unwrap_or_default().to_string();
        model.set_message(format!("fnox set the secret, but {reason}"));
    }
}

/// Read the secret question back, re-check its revision and fnox target, then run
/// `fnox set` with the typed value on stdin. Returns the record the answer must go to.
/// Errors are short reasons for the status row; none carries the value or fnox output.
pub(crate) fn hand_to_fnox(
    questions: &QuestionStore,
    input: &AnswerInput,
    bin: &std::ffi::OsStr,
) -> Result<QuestionRecord, String> {
    let fresh = questions
        .read(input.id)
        .map_err(|error| format!("read back failed: {error}"))?;
    if fresh.reference.revision != input.revision || !fresh.is_open() {
        return Err("the question changed since you read it · read it again".to_string());
    }
    let target = fresh
        .draft
        .secret
        .as_ref()
        .filter(|_| valid_secret_question(&fresh.draft))
        .ok_or_else(|| {
            "the fnox target is no longer valid (config missing or not a regular file)".to_string()
        })?;
    crate::fnox::set(bin, target, input.text.as_bytes()).map_err(|failure| failure.to_string())?;
    Ok(fresh)
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
    // A staged comment rides the decision either way; it is never itself an approval.
    answer.comment = model.questions_mut().comments.remove(&record.reference.id);
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
    // Kept so a failed save can restore a decision's staged comment rather than lose it.
    let comment = answer.comment.clone();
    let attempt = AnswerAttempt {
        answers_before: record.answers.len(),
        routes_before: record.routes.len(),
        text_hash: text_hash(&answer.text),
        corrects: answer.corrects_revision,
        action: answer.action,
        approve: answer.approve,
        comment_hash: comment.as_deref().map(text_hash),
    };
    let mut refusal = None;
    let result = match create {
        None => questions.answer(id, answer),
        Some(run) => questions.answer_with_task(id, answer, |state| {
            let created = create_finding_task(state, source_task, &title, run);
            if let Err(reason) = &created {
                refusal = Some(reason.clone());
            }
            created
        }),
    };
    let number = record.reference.number;
    // Where the record sat in the list, so the cursor keeps that place once it leaves.
    let slot = model.questions().order().iter().position(|row| *row == id);
    let mut landed = false;
    if let (Err(QuestionError::InvalidInput), Some(reason)) = (&result, refusal) {
        model.set_message(format!("Q{number}: {reason} · nothing saved"));
        reload(model);
        return;
    }
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
                    model.set_message(format!("Q{number} saved · {report}"));
                    landed = true;
                }
                QuestionDurability::Uncertain => {
                    model.questions_mut().pending_slot = slot;
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
            model.questions_mut().pending_slot = slot;
            model.questions_mut().pending = Some(PendingCommit::Unknown {
                id,
                prior_revision: record.reference.revision,
                follow_up: follow,
                attempt,
            });
            model.set_message(format!(
                "Q{number}: save result unknown · enter reads it back · nothing retried"
            ));
        }
        Err(QuestionError::StaleRevision) => {
            // The typed text and the revision it was written against stay as they were:
            // nothing retargets it to the newer record, and only Esc discards it.
            let questions = model.questions_mut();
            questions.correcting = None;
            questions.check = None;
            // The comment was pulled off the staged map to attempt this save; restore it
            // since nothing was actually written.
            if let Some(text) = comment.clone() {
                questions.comments.insert(id, text);
            }
            let kept = if questions.answer.is_some() {
                " · your text stays · esc discards it"
            } else {
                ""
            };
            model.set_message(format!(
                "Q{number} changed since you read it · read it again · nothing saved{kept}"
            ));
        }
        Err(error) => {
            if let Some(text) = comment.clone() {
                model.questions_mut().comments.insert(id, text);
            }
            model.set_message(format!("Q{number}: {error}"));
        }
    }
    reload(model);
    if landed {
        model.questions_mut().back_to_list(id, slot);
    }
    if create.is_some() {
        reload_tasks(store, domain, model);
    }
}

/// The finding's task, created inside the question store's lock: the source task's scope,
/// refused like an ordinary add when that project is archived. Never falls back to the desk.
fn create_finding_task(
    state: &mut DomainState,
    source_task: Uuid,
    title: &str,
    run: bool,
) -> Result<Option<Uuid>, String> {
    let scope = state
        .get(source_task)
        .filter(|task| !task.soft_deleted)
        .map(|task| task.scope.clone())
        .ok_or_else(|| "the finding's task is missing".to_string())?;
    if let TaskScope::Project { path } = &scope {
        if state.is_project_archived(path) {
            return Err(format!(
                "project {} is archived · no task created",
                crate::ui::render::short_project(path)
            ));
        }
    }
    let task = state
        .create(title, None, scope, ProvenanceOrigin::Manual)
        .map_err(|error| error.to_string())?;
    if run {
        state
            .set_status(task, HumanStatus::Doing)
            .map_err(|error| error.to_string())?;
    }
    Ok(Some(task))
}

fn resolve_pending(store: &TaskStore, domain: &mut DomainState, model: &mut BoardModel) {
    let Some(pending) = model.questions().pending else {
        return;
    };
    let Some(questions) = question_store(model) else {
        return;
    };
    let mut landed = None;
    // The slot from before the first save: the reload since may have closed the record.
    let slot = model.questions().pending_slot;
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
                    "Q{} durable · {report}",
                    saved.record.reference.number
                ));
                landed = Some(id);
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
            attempt,
        } => match questions.read(id) {
            Ok(record) if record.reference.revision == prior_revision => {
                model.questions_mut().pending = None;
                model.set_message(format!(
                    "Q{}: nothing was saved · you can answer again",
                    record.reference.number
                ));
            }
            Ok(record) if readback_is_attempt(&record, &attempt) => {
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
                    "Q{} changed and the change cannot be shown to be this save · read it again · no hint sent",
                    record.reference.number
                ));
            }
            Err(error) => model.set_message(format!("read back failed: {error}")),
        },
    }
    reload(model);
    if let Some(id) = landed {
        model.questions_mut().back_to_list(id, slot);
    }
    reload_tasks(store, domain, model);
}

fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Whether a changed record shows exactly the save this board attempted: one new answer and
/// no route since, at the head, from this board's actor, with the same text, correction
/// link, finding action, and approval flag. Anything else is someone else's change.
fn readback_is_attempt(record: &QuestionRecord, attempt: &AnswerAttempt) -> bool {
    let Some(answer) = record.current_answer() else {
        return false;
    };
    record.answers.len() == attempt.answers_before + 1
        && record.routes.len() == attempt.routes_before
        && record.reference.revision == answer.revision
        && answer.origin == AnswerOrigin::UserUi
        && answer.actor == USER
        && text_hash(&answer.text) == attempt.text_hash
        && answer.corrects_revision == attempt.corrects
        && answer.action == attempt.action
        && answer.approval.is_some() == attempt.approve
        && answer.comment.as_deref().map(text_hash) == attempt.comment_hash
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
    // The record's own current answer decides, never the action this board meant to save.
    let decided_run = record
        .current_answer()
        .is_some_and(|answer| answer.action == Some(FindingAction::CreateAndRun));
    if !decided_run {
        return "run request not sent: the current answer is not create-and-run".to_string();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{QuestionAnswer, QuestionDraft, QuestionRef, QuestionRoute};

    #[test]
    fn a_finding_task_is_never_created_in_an_archived_project() {
        let mut state = DomainState::new();
        let scope = TaskScope::Project {
            path: "/repos/old".into(),
        };
        let source = state
            .create("source", None, scope, ProvenanceOrigin::Manual)
            .expect("source task");
        state.archive_project("/repos/old").expect("archive");
        let before = state.tasks().len();
        for run in [false, true] {
            let refused = create_finding_task(&mut state, source, "summary", run);
            assert!(refused.is_err_and(|reason| reason.contains("archived")));
            assert_eq!(state.tasks().len(), before, "no task, run={run}");
        }
        let desk = state
            .create(
                "desk source",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
            )
            .expect("desk task");
        let created = create_finding_task(&mut state, desk, "summary", false)
            .expect("live scope creates")
            .expect("task id");
        assert_eq!(
            state.get(created).map(|task| &task.scope),
            Some(&TaskScope::Global)
        );
    }

    fn finding(answers: Vec<QuestionAnswer>, routes: Vec<QuestionRoute>) -> QuestionRecord {
        let revision = answers
            .last()
            .map(|answer| answer.revision)
            .unwrap_or_else(|| Uuid::from_u128(1));
        QuestionRecord {
            reference: QuestionRef {
                id: Uuid::from_u128(2),
                number: 4,
                task_id: Uuid::from_u128(3),
                kind: QuestionKind::Finding,
                summary: "summary".into(),
                revision,
            },
            draft: QuestionDraft {
                task_id: Uuid::from_u128(3),
                kind: QuestionKind::Finding,
                summary: "summary".into(),
                origin_thread: "thr-x".into(),
                recipient: "user".into(),
                text: "body".into(),
                options: Vec::new(),
                recommendation: None,
                impact: None,
                blocking_scope: None,
                approval: None,
                finding: None,
                source: None,
                audience: None,
                lead_recipient: Some("lead-x".into()),
                secret: None,
            },
            created_at: std::time::SystemTime::now(),
            answers,
            routes,
        }
    }

    fn answer(text: &str, action: FindingAction, origin: AnswerOrigin) -> QuestionAnswer {
        QuestionAnswer {
            revision: Uuid::new_v4(),
            created_task_id: None,
            corrects_revision: None,
            text: text.into(),
            actor: USER.into(),
            origin,
            at: std::time::SystemTime::now(),
            approval: None,
            user_chat: None,
            source: None,
            action: Some(action),
            comment: None,
        }
    }

    #[test]
    fn an_unknown_commit_is_this_save_only_when_the_readback_matches_the_attempt() {
        let (run_label, close_label) = (FINDING_ACTIONS[0].1, FINDING_ACTIONS[2].1);
        let attempt = AnswerAttempt {
            answers_before: 0,
            routes_before: 0,
            text_hash: text_hash(run_label),
            corrects: None,
            action: Some(FindingAction::CreateAndRun),
            approve: false,
            comment_hash: None,
        };
        let mine = finding(
            vec![answer(
                run_label,
                FindingAction::CreateAndRun,
                AnswerOrigin::UserUi,
            )],
            Vec::new(),
        );
        assert!(readback_is_attempt(&mine, &attempt));
        // Another board (also UserUi) closed it: origin alone proves nothing.
        let closed = finding(
            vec![answer(
                close_label,
                FindingAction::Close,
                AnswerOrigin::UserUi,
            )],
            Vec::new(),
        );
        assert!(!readback_is_attempt(&closed, &attempt));
        // The same decision plus a later route, or a second answer, is not this save alone.
        let mut routed = mine.clone();
        routed.routes.push(QuestionRoute {
            revision: Uuid::new_v4(),
            after_answers: 1,
            audience: crate::domain::Audience::Lead,
            lead_recipient: None,
            at: std::time::SystemTime::now(),
        });
        routed.reference.revision = routed.routes[0].revision;
        assert!(!readback_is_attempt(&routed, &attempt));
        let mut twice = mine.clone();
        twice.answers.push(answer(
            run_label,
            FindingAction::CreateAndRun,
            AnswerOrigin::UserUi,
        ));
        twice.reference.revision = twice.answers[1].revision;
        assert!(!readback_is_attempt(&twice, &attempt));
        // The run request checks the record's own current action.
        assert_eq!(
            request_lead(&closed),
            "run request not sent: the current answer is not create-and-run"
        );
    }
}

#[cfg(all(test, unix))]
mod secret_tests {
    use super::*;
    use crate::domain::{Audience, QuestionDraft, SecretTarget};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    const VALUE: &str = "s3cr3t-Value-9f1c";

    struct Temp(PathBuf);
    impl Temp {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("tsk-secret-{label}-{nanos}-{seq}"));
            std::fs::create_dir_all(dir.join("state")).expect("state dir");
            std::fs::create_dir_all(dir.join("record")).expect("record dir");
            Self(dir)
        }
        fn state(&self) -> PathBuf {
            self.0.join("state")
        }
        fn record(&self) -> PathBuf {
            self.0.join("record")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fake fnox that records its argv and stdin outside the state dir, then exits `code`.
    fn fake_fnox(temp: &Temp, code: i32) -> PathBuf {
        let script = temp.0.join("fnox");
        let record = temp.record();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{argv}'\ncat > '{stdin}'\necho noisy-stdout\necho noisy-stderr >&2\nexit {code}\n",
                argv = record.join("argv").display(),
                stdin = record.join("stdin").display(),
            ),
        )
        .expect("write fake fnox");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake fnox");
        script
    }

    fn ask_secret(temp: &Temp, target: SecretTarget) -> (QuestionStore, QuestionRecord) {
        let store = TaskStore::new(temp.state());
        let mut domain = DomainState::new();
        let task = domain
            .create(
                "needs a key",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
            )
            .expect("task");
        store.save(&domain).expect("seed store");
        let questions = QuestionStore::new(temp.state()).expect("agent store");
        let saved = questions
            .ask(QuestionDraft {
                task_id: task,
                kind: QuestionKind::Secret,
                summary: "API key for staging".into(),
                origin_thread: "thr-a".into(),
                recipient: "lead-a".into(),
                text: "Paste the staging API key".into(),
                options: Vec::new(),
                recommendation: None,
                impact: None,
                blocking_scope: None,
                approval: None,
                finding: None,
                source: None,
                audience: Some(Audience::User),
                lead_recipient: None,
                secret: Some(target),
            })
            .expect("ask secret");
        (questions, saved.record)
    }

    fn typed(record: &QuestionRecord) -> AnswerInput {
        let mut input = AnswerInput::secret(record.reference.id, record.reference.revision);
        input.text.push_str(VALUE);
        input
    }

    fn files_containing(dir: &Path, needle: &[u8]) -> Vec<PathBuf> {
        let mut hits = Vec::new();
        for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                hits.extend(files_containing(&path, needle));
            } else if std::fs::read(&path)
                .expect("read file")
                .windows(needle.len())
                .any(|window| window == needle)
            {
                hits.push(path);
            }
        }
        hits
    }

    #[test]
    fn the_board_hands_the_value_to_fnox_on_stdin_and_records_only_set() {
        let temp = Temp::new("ok");
        let config = temp.0.join("fnox.toml");
        std::fs::write(&config, "").expect("config");
        let (questions, record) = ask_secret(
            &temp,
            SecretTarget {
                key: "STAGING_API_KEY".into(),
                profile: Some("stage".into()),
                config: Some(config.to_string_lossy().into_owned()),
            },
        );
        let fnox = fake_fnox(&temp, 0);
        let fresh = hand_to_fnox(&questions, &typed(&record), fnox.as_os_str()).expect("fnox ok");
        let argv = std::fs::read_to_string(temp.record().join("argv")).expect("argv");
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            [
                "set",
                "-c",
                config.to_str().expect("utf8"),
                "--profile",
                "stage",
                "STAGING_API_KEY"
            ]
        );
        assert!(!argv.contains(VALUE), "the value never rides argv");
        assert_eq!(
            std::fs::read(temp.record().join("stdin")).expect("stdin"),
            VALUE.as_bytes(),
            "the value is exactly the child's stdin"
        );
        let saved = questions
            .answer(
                fresh.reference.id,
                draft(fresh.reference.revision, SECRET_SET.to_string()),
            )
            .expect("record set");
        assert_eq!(saved.record.answers.len(), 1);
        assert_eq!(saved.record.answers[0].text, "set");
        assert!(!saved.record.is_open());
        assert_eq!(
            files_containing(&temp.state(), VALUE.as_bytes()),
            Vec::<PathBuf>::new(),
            "no file in the state dir holds the value"
        );
    }

    #[test]
    fn a_global_target_runs_fnox_set_g_without_a_profile() {
        let temp = Temp::new("global");
        let (questions, record) = ask_secret(
            &temp,
            SecretTarget {
                key: "TOKEN".into(),
                profile: None,
                config: None,
            },
        );
        let fnox = fake_fnox(&temp, 0);
        hand_to_fnox(&questions, &typed(&record), fnox.as_os_str()).expect("fnox ok");
        let argv = std::fs::read_to_string(temp.record().join("argv")).expect("argv");
        assert_eq!(argv.lines().collect::<Vec<_>>(), ["set", "-g", "TOKEN"]);
    }

    #[test]
    fn a_failed_fnox_records_nothing_and_says_only_that_it_failed() {
        let temp = Temp::new("fail");
        let (questions, record) = ask_secret(
            &temp,
            SecretTarget {
                key: "TOKEN".into(),
                profile: None,
                config: None,
            },
        );
        let fnox = fake_fnox(&temp, 3);
        let reason =
            hand_to_fnox(&questions, &typed(&record), fnox.as_os_str()).expect_err("fnox fails");
        assert_eq!(reason, "fnox set failed · exit 3");
        let after = questions.read(record.reference.id).expect("read back");
        assert!(after.is_open(), "the question stays open");
        assert_eq!(after.reference.revision, record.reference.revision);
        assert!(files_containing(&temp.state(), VALUE.as_bytes()).is_empty());
    }

    #[test]
    fn a_vanished_config_or_a_moved_revision_never_runs_fnox() {
        let temp = Temp::new("recheck");
        let config = temp.0.join("fnox.toml");
        std::fs::write(&config, "").expect("config");
        let (questions, record) = ask_secret(
            &temp,
            SecretTarget {
                key: "TOKEN".into(),
                profile: None,
                config: Some(config.to_string_lossy().into_owned()),
            },
        );
        let fnox = fake_fnox(&temp, 0);
        let mut stale = typed(&record);
        stale.revision = Uuid::from_u128(1);
        assert!(hand_to_fnox(&questions, &stale, fnox.as_os_str()).is_err());
        // A symlink in place of the config is refused right before the run.
        std::fs::remove_file(&config).expect("remove config");
        std::os::unix::fs::symlink(temp.0.join("elsewhere.toml"), &config).expect("symlink");
        std::fs::write(temp.0.join("elsewhere.toml"), "").expect("target");
        let reason =
            hand_to_fnox(&questions, &typed(&record), fnox.as_os_str()).expect_err("refused");
        assert!(reason.contains("no longer valid"), "{reason}");
        assert!(
            !temp.record().join("argv").exists(),
            "fnox never ran for a stale or invalid target"
        );
    }

    #[test]
    fn no_other_answer_closes_a_secret_question() {
        let temp = Temp::new("guard");
        let (questions, record) = ask_secret(
            &temp,
            SecretTarget {
                key: "TOKEN".into(),
                profile: None,
                config: None,
            },
        );
        let mut agent = draft(record.reference.revision, SECRET_SET.to_string());
        agent.origin = AnswerOrigin::Agent;
        assert_eq!(
            questions.answer(record.reference.id, agent).err(),
            Some(QuestionError::SecretNeedsBoard)
        );
        let typed_value = draft(record.reference.revision, VALUE.to_string());
        assert_eq!(
            questions.answer(record.reference.id, typed_value).err(),
            Some(QuestionError::SecretNeedsBoard),
            "a UserUi answer other than `set` would store a value"
        );
        assert!(files_containing(&temp.state(), VALUE.as_bytes()).is_empty());
    }
}
