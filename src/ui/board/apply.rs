//! Board intent reducer.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::context::InvocationSnapshot;
use crate::domain::{
    status_word, DomainError, DomainState, HumanStatus, Task, TaskScope, UndoEntry,
};
use crate::ui::capture::{CaptureField, TITLE_REQUIRED_MESSAGE};
use crate::ui::edit::{delete_last_word, flatten_line_breaks, EditBuffer};
use crate::ui::input::{BoardIntent, MarkDirection};
use crate::ui::mouse::BoardPopup;
use crate::ui::queue::{ARCHIVED_HEADER_ROW_ID, INBOX_HEADER_ROW_ID};
use crate::ui::tier::{FocusedSurface, WideStage};

use super::commands::{resolve_board_command, CommandSurface};
use super::model::{relation_scroll_to, steps_scroll_to_cursor, steps_scroll_to_row};
use super::model::{
    BoardForm, BoardInputMode, BoardLocation, BoardModel, IntentOutcome, ListPickerValue,
    OnPickerTarget, StepEditor, StepEditorSave, TaskEditSave, DIRTY_TASK_SWITCH_REFUSAL,
};

/// What the row says when an action that aims at the selection is asked for on a board that
/// has none. One wording, so the same refusal always reads the same way.
const NO_SELECTION: &str = "select a task first";
/// `@` with no `config.toml` profile: the picker would offer only **none**.
/// Enter on an empty `type a name…` row.
pub(crate) const TYPE_A_NAME_FIRST: &str = "type a name first";

fn base_target_project(domain: &DomainState, ids: &[Uuid]) -> Result<PathBuf, String> {
    for id in ids {
        let task = domain
            .get(*id)
            .ok_or_else(|| format!("unknown task {id}"))?;
        if let TaskScope::Project { path } = &task.scope {
            return Ok(PathBuf::from(path));
        }
    }
    Err("base requires a project task".to_string())
}

/// What `J` / `K` say at either end of the list, and for a page whose task the list no
/// longer shows.
const LAST_TASK: &str = "last task in this list";
const FIRST_TASK: &str = "first task in this list";
const TASK_NOT_IN_LIST: &str = "this task is not in the list · esc back";

/// What `P` / `U` and the parent chooser say. Changing a parent never changes either task's
/// status, scope, or children.
const NO_PARENT: &str = "this task has no parent";
const PARENT_CLEARED: &str = "parent cleared";
const PARENT_UNCHANGED: &str = "parent unchanged";
const PARENT_NEEDS_CLEAN_PAGE: &str = "save or cancel edits before changing the parent";

/// What the row says when Shift+Enter reaches a form as Ctrl+J (#125).
pub const SHIFT_ENTER_AS_CTRL_J_HINT: &str = "Shift+Enter sent Ctrl+J: check terminal keys";

fn take_verb_targets(model: &mut BoardModel) -> (Vec<Uuid>, bool) {
    let bulk = model.task_list_owns_input() && model.mark_mode_active() && model.marked_count() > 0;
    let targets = model.verb_target_ids();
    model.clear_marks();
    (targets, bulk)
}

/// Whether `ctrl+s` moves a task in `status` to started. A blocked task starts too: its block
/// closes as an unblock in the same save and the same undo entry.
pub fn ctrl_s_starts(status: HumanStatus) -> bool {
    matches!(
        status,
        HumanStatus::Open | HumanStatus::Ready | HumanStatus::Blocked
    )
}

/// Why `ctrl+s` leaves the one cursor task `label` in `status` alone, for the status row.
fn ctrl_s_refusal(label: &str, status: HumanStatus) -> String {
    match status {
        HumanStatus::Review => format!("{label} is in review: ctrl+r or the task page"),
        HumanStatus::Done => format!("{label} is done: ctrl+n or ctrl+o reopens it"),
        _ => format!("{label} is already started"),
    }
}

/// Block and review targets without their done tasks, and how many were dropped.
fn without_done(domain: &DomainState, targets: Vec<Uuid>) -> (Vec<Uuid>, usize) {
    let count = targets.len();
    let rest: Vec<Uuid> = targets
        .into_iter()
        .filter(|id| {
            !domain
                .get(*id)
                .is_some_and(|task| task.status == HumanStatus::Done)
        })
        .collect();
    let skipped = count - rest.len();
    (rest, skipped)
}

/// The status row's note for a marked set whose done tasks a block or review skipped.
fn note_skipped_done(model: &mut BoardModel, skipped: usize) {
    if skipped > 0 {
        model.set_message(format!(
            "skipped {skipped} done (ctrl+n or ctrl+o reopens them)"
        ));
    }
}

/// What one `ctrl+u` reverts, read from the top entry before it is undone: the action word, the
/// tasks it is about, and, for a done, the tasks it started that go back to ready.
struct UndoNote {
    action: &'static str,
    subjects: Vec<Uuid>,
    released: Vec<Uuid>,
    /// Every other task the entry touches (a deletion's unlinked `after` lists).
    others: Vec<Uuid>,
    /// Tasks that were started before the undo: only these can leave a running agent behind.
    was_started: Vec<Uuid>,
    restores_status: bool,
    /// What the undo says instead when it only moves a name on or off the assignee picker.
    name_note: Option<String>,
}

/// The board's word for the action an undo leaf recorded. `domain` is the state before the
/// undo, which still shows what the action did (a block card versus a review card).
fn undo_action_word(domain: &DomainState, leaf: &UndoEntry) -> &'static str {
    match leaf {
        UndoEntry::Complete { .. } => "done",
        UndoEntry::SoftDelete { .. } => "delete",
        UndoEntry::Archive {
            previous: false, ..
        } => "archive",
        UndoEntry::Archive { previous: true, .. } => "unarchive",
        UndoEntry::Start { .. } => "start",
        UndoEntry::Status { .. } => "status",
        UndoEntry::Block { id, .. } => {
            if domain
                .get(*id)
                .is_some_and(|task| task.status == HumanStatus::Review)
            {
                "review"
            } else {
                "block"
            }
        }
        UndoEntry::Assign { .. } => "assignee",
        UndoEntry::SetBase { .. } => "base",
        UndoEntry::SetAfter { .. } => "after",
        UndoEntry::SetOn { .. } => "waiting on",
        UndoEntry::Steps { .. } => "steps",
        UndoEntry::HideAssignee { .. } => "remove",
        UndoEntry::ShowAssignee { .. } => "assignee",
        UndoEntry::Batch { .. } => "undo",
    }
}

fn leaf_target(leaf: &UndoEntry) -> Option<Uuid> {
    leaf.targets().first().map(|(id, _)| *id)
}

impl UndoNote {
    fn read(domain: &DomainState, entry: &UndoEntry) -> Option<Self> {
        let leaves = entry.leaf_entries();
        let first = *leaves.first()?;
        let kind = std::mem::discriminant(first);
        let mut subjects = Vec::new();
        let mut released = Vec::new();
        let mut others = Vec::new();
        for leaf in &leaves {
            let Some(id) = leaf_target(leaf) else {
                continue;
            };
            if std::mem::discriminant(*leaf) == kind {
                if !subjects.contains(&id) {
                    subjects.push(id);
                }
            } else if matches!(first, UndoEntry::Complete { .. })
                && matches!(leaf, UndoEntry::Start { .. })
            {
                if !released.contains(&id) {
                    released.push(id);
                }
            } else if !others.contains(&id) {
                others.push(id);
            }
        }
        others.retain(|id| !subjects.contains(id) && !released.contains(id));
        let was_started = subjects
            .iter()
            .chain(&released)
            .copied()
            .filter(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Started)
            })
            .collect();
        Some(Self {
            action: undo_action_word(domain, first),
            subjects,
            released,
            others,
            was_started,
            restores_status: matches!(
                first,
                UndoEntry::Complete { .. }
                    | UndoEntry::Start { .. }
                    | UndoEntry::Status { .. }
                    | UndoEntry::Block { .. }
            ),
            name_note: match first {
                UndoEntry::HideAssignee { name } => Some(format!(
                    "{} back on the list",
                    crate::domain::display_name(name)
                )),
                UndoEntry::ShowAssignee { name } => Some(format!(
                    "{} removed from the list",
                    crate::domain::display_name(name)
                )),
                _ => None,
            },
        })
    }

    /// `done undone · T202 back to started · T203 back to ready · @claude kept running`, read
    /// from the state after the undo.
    fn message(&self, domain: &DomainState) -> String {
        if let Some(note) = &self.name_note {
            return note.clone();
        }
        let mut message = format!(
            "{} undone · {}",
            self.action,
            task_names(domain, &self.subjects)
        );
        if self.restores_status {
            if let [id] = self.subjects.as_slice() {
                if let Some(task) = domain.get(*id) {
                    message.push_str(&format!(" back to {}", status_word(task.status)));
                }
            }
        }
        if !self.released.is_empty() {
            message.push_str(&format!(
                " · {} back to ready",
                task_names(domain, &self.released)
            ));
        }
        // Undo never stops an agent: a task the undo takes out of started (the board's `◉`
        // rule: started with a live dispatch) leaves its agent running.
        let mut running: Vec<String> = self
            .was_started
            .iter()
            .filter(|_| self.restores_status)
            .filter_map(|id| domain.get(*id))
            .filter(|task| task.status != HumanStatus::Started)
            .filter(|task| task.dispatch.as_ref().is_some_and(|record| !record.cleaned))
            .filter_map(|task| task.agent_assignee().map(str::to_string))
            .collect();
        running.sort();
        running.dedup();
        for assignee in running {
            message.push_str(&format!(" · @{assignee} kept running"));
        }
        message
    }
}

/// `T3`, `T3, T4, T5`, or `5 tasks`: the tasks an undo is about, the way the board names them.
fn task_names(domain: &DomainState, ids: &[Uuid]) -> String {
    if ids.len() > 3 {
        return format!("{} tasks", ids.len());
    }
    let mut ids = ids.to_vec();
    ids.sort_by_key(|id| {
        domain
            .get(*id)
            .and_then(|task| task.number)
            .unwrap_or(u64::MAX)
    });
    let names: Vec<String> = ids
        .iter()
        .map(|id| {
            domain
                .get(*id)
                .and_then(crate::domain::Task::board_identifier)
                .unwrap_or_else(|| "a task".to_string())
        })
        .collect();
    if names.is_empty() {
        "nothing".to_string()
    } else {
        names.join(", ")
    }
}

/// The status row when the undo stack is empty.
const NOTHING_TO_UNDO: &str = "nothing to undo";

/// The refusal when **chain in order** has fewer than two marked tasks to chain.
const CHAIN_NEEDS_MARKS: &str = "mark two or more tasks, in order, to chain them";

/// `T202, T205`.
fn after_list(numbers: &[u64]) -> String {
    numbers
        .iter()
        .map(|number| format!("T{number}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the row says when an Undo is refused because its target moved on.
///
/// The domain's [`DomainError::StaleUndo`] names the task by uuid, which is right for a log
/// line but means nothing on the board. The row names the task the way the board paints it,
/// says which recorded action went stale, adds the visible change that most likely caused it
/// (an archive is the common one), and states that nothing moved, naming every task of a
/// marked set's entry.
fn stale_undo_message(domain: &DomainState, id: Uuid) -> String {
    let task = domain.get(id);
    let subject = task
        .and_then(crate::domain::Task::board_identifier)
        .unwrap_or_else(|| "this task".to_string());
    let entry = domain.last_undo();
    let leaf = entry.and_then(|entry| {
        entry
            .leaf_entries()
            .into_iter()
            .find(|leaf| leaf_target(leaf) == Some(id))
    });
    let since = match leaf {
        Some(UndoEntry::Complete { .. }) => "since it was completed",
        Some(UndoEntry::SoftDelete { .. }) => "since it was deleted",
        Some(UndoEntry::Archive {
            previous: false, ..
        }) => "since it was archived",
        Some(UndoEntry::Archive { previous: true, .. }) => "since it was unarchived",
        Some(UndoEntry::Start { .. }) => "since it was started",
        Some(UndoEntry::Status { .. }) => "since its status change",
        Some(UndoEntry::Block { .. }) => "since its block or review",
        Some(UndoEntry::Assign { .. }) => "since it was assigned",
        Some(UndoEntry::SetBase { .. }) => "since its base change",
        Some(UndoEntry::SetAfter { .. }) => "since its after change",
        Some(UndoEntry::SetOn { .. }) => "since its waiting-on change",
        Some(UndoEntry::Steps { .. }) => "since its step change",
        Some(
            UndoEntry::HideAssignee { .. }
            | UndoEntry::ShowAssignee { .. }
            | UndoEntry::Batch { .. },
        )
        | None => "since the undoable action",
    };
    let cause = task.and_then(|task| match leaf {
        Some(UndoEntry::Archive {
            previous: false, ..
        }) if !task.archived => Some("unarchived"),
        Some(UndoEntry::Archive { .. }) => None,
        _ if task.archived => Some("archived"),
        Some(UndoEntry::Complete { .. }) if task.soft_deleted => Some("deleted"),
        Some(UndoEntry::Complete { .. }) if task.status != HumanStatus::Done => Some("reopened"),
        Some(UndoEntry::SoftDelete { .. }) if !task.soft_deleted => Some("restored"),
        _ => None,
    });
    let cause = cause.map(|cause| format!(" ({cause})")).unwrap_or_default();
    let note = entry.and_then(|entry| UndoNote::read(domain, entry));
    match note {
        // An entry about more than the task that moved on names what it would have undone:
        // `done of T1 not undone: T2 changed since it was started`.
        Some(note)
            if note.subjects != [id] || !note.released.is_empty() || !note.others.is_empty() =>
        {
            let extras: Vec<Uuid> = note
                .released
                .iter()
                .chain(&note.others)
                .copied()
                .filter(|extra| *extra != id)
                .collect();
            let with = if extras.is_empty() {
                String::new()
            } else {
                format!(" (with {})", task_names(domain, &extras))
            };
            format!(
                "{} of {}{with} not undone: {subject} changed {since}{cause}",
                note.action,
                task_names(domain, &note.subjects)
            )
        }
        _ => format!("{subject} changed {since}{cause}; nothing undone"),
    }
}

/// A **mutating board action**: an intent this reducer may report as persisting domain
/// state, and so the one predicate every rule about mutating actions reads.
///
/// The board loop uses it to decide whether a save baseline has to be loaded before the
/// intent is applied, and [`apply_intent`] uses it to time the delete recovery notice
///. Both read this list rather than keeping one of their own, so the pinned
/// definition and the classification cannot drift apart.
pub fn board_intent_may_persist(model: &BoardModel, intent: &BoardIntent) -> bool {
    // The mouse route recurses into ConfirmListPicker inside the reducer, so the outer
    // intent is the one the save baseline is decided on.
    // A click in the task picker only pins or unpins; shift+Enter is its one save.
    let picker_click_pins = matches!(intent, BoardIntent::SelectListOption(_))
        && model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::After);
    let picker_assignment = matches!(
        intent,
        BoardIntent::ConfirmListPicker | BoardIntent::SelectListOption(_)
    ) && !picker_click_pins
        && model.input_mode == BoardInputMode::ListPicker
        && matches!(
            model.list_picker_kind(),
            Some(
                crate::ui::board::ListPickerKind::Assignee
                    | crate::ui::board::ListPickerKind::WaitingOn
                    | crate::ui::board::ListPickerKind::Base
                    | crate::ui::board::ListPickerKind::After
            )
        );
    picker_assignment
        || matches!(
            intent,
            BoardIntent::ConfirmEdit
                | BoardIntent::ConfirmEditNext
                | BoardIntent::SetStatus(_)
                | BoardIntent::Complete
                | BoardIntent::Reopen
                | BoardIntent::SoftDelete
                | BoardIntent::Undo
                | BoardIntent::File
                | BoardIntent::LaunchUnarchive
                | BoardIntent::PrimaryVerb
                | BoardIntent::DispatchAgain
                | BoardIntent::ConfirmDispatch
                | BoardIntent::StartWithoutRelaunch
                | BoardIntent::ConfirmCleanup
                | BoardIntent::KeepCleanup
                | BoardIntent::ToggleBlock
                | BoardIntent::BlockCardConfirm
                | BoardIntent::ReplySave
                | BoardIntent::ReplySaveUnblock
                | BoardIntent::PageReplyUnblock
                | BoardIntent::ReplySaveStart
                | BoardIntent::ReplySaveBeforeLaunch
                | BoardIntent::ToggleReview
                | BoardIntent::CycleCheck
                | BoardIntent::ClickCheck(_)
                | BoardIntent::ReplyApprove
                | BoardIntent::ApproveReview(_)
                | BoardIntent::ToggleStep
                | BoardIntent::MoveStepUp
                | BoardIntent::MoveStepDown
                | BoardIntent::QuickAddSave
                | BoardIntent::QuickAddSaveNext
                | BoardIntent::ChainAfter
                | BoardIntent::RemoveAssigneeName
                | BoardIntent::ListPickerUndo
                | BoardIntent::ClearParent
                | BoardIntent::ConfirmParentPicker
                | BoardIntent::SelectParentOption(_)
        )
}

/// The chrome row's lifetime rule, in one place.
///
/// The row carries two different things with two different lifetimes: the **message**, which
/// is feedback about the action the user just took, and the **delete recovery notice**, which
/// outlives its action on purpose so the way back stays on screen. Every event:
///
/// - **an edit opens** — the message is cleared (a refusal left by an earlier action would
///   read as a refusal of an edit nobody has attempted yet); the notice is kept, because
///   opening an edit is not a mutating action and says the notice survives one.
///   Applied by the two `BeginEdit…` arms, which know whether a field actually opened.
/// - **an edit is cancelled** — the message is cleared, for the same reason in reverse: the
///   refusal explained a field that is now gone; the notice is kept. Applied by the
///   `CancelEdit` arm.
/// - **a mutating board action** — *both* are cleared, and only here. The notice because
///   says so; the message because it was feedback about an earlier action, and a
///   message left standing would be read as this action's answer. Whatever the action then
///   reports (a confirmation, a refusal, the stale-undo message) owns the row on its own,
///   because this clearing happens before the intent is applied. The exception is an action
///   that **refused**: clears the notice for a change, and a refusal is not one, so
///   [`apply_intent`] puts the notice back whenever the intent it ran reported nothing to
///   persist: a refused Undo or Done keeps the way back it never spent. That is one rule at one
///   place rather than a per-arm exception, and it reads off the same outcome the caller
///   persists on.
/// - **the cursor or the lens moves** (a different selected row or project, another
///   destination or View, the done drawer opening or closing) — the notice is cleared: it
///   reported the delete, and the user has moved on. The message is left alone. Applied by
///   [`apply_intent`] from the state before and after a non-mutating intent, so a mouse
///   click and a key move are the same event. The footer's undo seat
///   ([`BoardModel::undo_hint`]) is not the notice and stays while its entry is live.
/// - **a few seconds pass** — the notice is cleared ([`BoardModel::expire_delete_notice`],
///   on the idle tick), so a delete with nothing after it does not hold the row forever.
/// - **any other action** — both are left exactly as they are.
///
/// Two events outside this reducer touch the notice. A failed save
/// ([`BoardModel::begin_save_recovery`]) *suspends* it, because a deletion that did not
/// reach disk has nothing to undo, and a successful Retry
/// ([`BoardModel::end_save_recovery`]) puts it back. An open command surface *covers* the
/// notice for as long as it is open
/// ([`BoardModel::visible_delete_notice`]): the notice is not taken down, but for that
/// duration it is genuinely not painted, which is the cost of the row never advertising a
/// click the modal surface would swallow.
///
/// When both channels are set the row carries **both**, notice first, message second, legend
/// last (see [`fit_chrome_row`]). Nothing on this row wins by taking another thing off it.
fn apply_chrome_row_lifetime(model: &mut BoardModel, intent: &BoardIntent) {
    if board_intent_may_persist(model, intent) {
        model.clear_delete_notice();
        model.clear_message();
    }
}

/// Apply a board intent to domain + model.
///
/// Mutating intents call Task Domain only. Caller persists with Task Store when outcome is
/// [`IntentOutcome::Persist`]. The capture snapshot is retained by the form.
/// The refusal a read-only task page paints: a relation open reached a task inside an
/// archived project.
fn read_only_page_refusal(model: &BoardModel) -> Option<String> {
    let path = model.page_archived_project()?;
    let name = crate::ui::render::short_project(&path).to_string();
    Some(format!("project {name} is archived \u{b7} read only"))
}

/// Intents a read-only page refuses: everything that would mutate a task or open a
/// capture/edit surface. `Undo` is excluded here; the caller refuses it separately.
fn read_only_focus_refuses(model: &BoardModel, intent: &BoardIntent) -> bool {
    if matches!(intent, BoardIntent::Undo) {
        return false;
    }
    if board_intent_may_persist(model, intent) {
        return true;
    }
    // On a task page in view mode, Tab, shift+Tab, and a step click only move the page's
    // selection (steps, `+ step`, relations): reading, not editing. `Enter` on `+ step`
    // and every edit entry stay refused below and in the `OpenTaskPage` arm.
    if matches!(
        intent,
        BoardIntent::FormFocusNext | BoardIntent::FormFocusPrev | BoardIntent::SelectStep(_)
    ) && model.input_mode == BoardInputMode::TaskPage
        && model.focused_page_task().is_some()
        && !model.task_editing()
    {
        return false;
    }
    matches!(
        intent,
        BoardIntent::OpenCapture
            | BoardIntent::ExpandQuickAdd
            | BoardIntent::BeginEditTitle
            | BoardIntent::BeginEditNotes
            | BoardIntent::BeginEditScope
            | BoardIntent::OpenAssigneePicker
            | BoardIntent::OpenBasePicker
            | BoardIntent::OpenAfterPicker
            | BoardIntent::OpenOnPicker
            | BoardIntent::BeginAddStep
            | BoardIntent::BeginAppendStep
            | BoardIntent::FormCycleScope
            // AC-44: Tab, shift+Tab, a field click, and the step cursor are edit
            // entries on the task page, so the read-only page refuses them too.
            | BoardIntent::FormFocusNext
            | BoardIntent::FormFocusPrev
            | BoardIntent::FocusFormField(_)
            | BoardIntent::FocusFormCursor(_, _, _)
            | BoardIntent::SelectStep(_)
            | BoardIntent::OpenParentPicker
    )
}

pub fn apply_intent(
    domain: &mut DomainState,
    model: &mut BoardModel,
    intent: BoardIntent,
    snapshot: Option<&InvocationSnapshot>,
) -> Result<IntentOutcome, DomainError> {
    // A successful quick add remains emphasized only until the next input intent.
    model.clear_saved_task();
    // Mark-then-confirm (AC-11): any intent other than the delete verb's own
    // confirmation routes clears an armed steps delete mark. Command confirmations
    // are excluded here because they recurse below as the intent they resolved to, which
    // then faces this same rule as itself.
    if model.pending_delete.is_some()
        && !matches!(
            intent,
            BoardIntent::SoftDelete | BoardIntent::ConfirmCommand | BoardIntent::SelectCommand(_)
        )
    {
        model.pending_delete = None;
        model.pending_delete_bulk = false;
    }
    if model.empty_add_step_editor()
        && !matches!(
            intent,
            BoardIntent::EditInsert(_)
                | BoardIntent::EditInsertText(_)
                | BoardIntent::EditBackspace
                | BoardIntent::EditDeleteForward
                | BoardIntent::EditMoveLeft
                | BoardIntent::EditMoveRight
                | BoardIntent::EditMoveLineStart
                | BoardIntent::EditMoveLineEnd
                | BoardIntent::EditMoveWordLeft
                | BoardIntent::EditMoveWordRight
                | BoardIntent::EditDeleteWordLeft
                | BoardIntent::EditDeleteWordRight
                | BoardIntent::EditDeleteToLineStart
                | BoardIntent::EditDeleteToLineEnd
                | BoardIntent::ConfirmEdit
                | BoardIntent::ConfirmEditNext
                | BoardIntent::CancelEdit
                | BoardIntent::ShiftEnterAsCtrlJ
                | BoardIntent::BeginAddStep
                | BoardIntent::BeginAppendStep
                // The delete verb discards the row itself; parked first, it would fall
                // through to the step above or the task.
                | BoardIntent::SoftDelete
        )
    {
        let _ = model.park_rename_step_draft();
    }
    if !model.task_editing()
        && model
            .form
            .as_ref()
            .is_some_and(|form| form.steps.delete_mark.is_some())
        && !matches!(
            intent,
            BoardIntent::SoftDelete | BoardIntent::ConfirmCommand | BoardIntent::SelectCommand(_)
        )
    {
        if let Some(form) = model.form.as_mut() {
            form.steps.delete_mark = None;
        }
        // The press-again hint lives exactly as long as the mark it explains
        // (AC-23): the intervening intent that disarms the mark takes the footer
        // message down with it, before whatever the intent itself has to report.
        model.clear_message();
    }
    // A relation open can reach a task of an archived project from a working lens. Its page
    // is read-only, and `ctrl+u` refuses too: unarchiving a project is
    // `tsk project unarchive`'s route, not a task page's.
    if model.page_archived_project().is_some()
        && model.popup() == BoardPopup::None
        && model.surface == CommandSurface::None
        && (matches!(intent, BoardIntent::Undo) || read_only_focus_refuses(model, &intent))
    {
        if let Some(refusal) = read_only_page_refusal(model) {
            model.set_message(refusal);
        }
        return Ok(IntentOutcome::None);
    }
    let notice_before = (
        model.delete_notice.clone(),
        model.delete_notice_count,
        model.delete_notice_label.clone(),
        model.delete_notice_expires_at,
    );
    let mutating = board_intent_may_persist(model, &intent);
    if mutating {
        model.undo_hint_before_save = model.undo_hint.clone();
    }
    let place_before = BoardPlace::of(model);
    let result = apply_board_intent(domain, model, intent, snapshot);
    // One page selection at a time: a step or `+ step` selected by any route takes it over
    // from a relation entry.
    if let Some(form) = model.form.as_mut() {
        if form.steps.cursor.is_some() || form.steps.add_selected {
            form.steps.relation_cursor = None;
        }
    }
    // A command confirmation carries no lifetime of its own: it recurses with the command it
    // resolved to, and that intent is classified on the way through, so it is the recursion
    // that restores.
    if mutating && !matches!(result, Ok(IntentOutcome::Persist)) {
        model.delete_notice = notice_before.0;
        model.delete_notice_count = notice_before.1;
        model.delete_notice_label = notice_before.2;
        model.delete_notice_expires_at = notice_before.3;
    }
    if !mutating && BoardPlace::of(model) != place_before {
        model.clear_delete_notice();
    }
    result
}

/// Where the user is looking: the cursor and the lens it moves in. A change between two
/// intents is "moving on" for the chrome-row lifetime rule ([`apply_chrome_row_lifetime`]).
#[derive(PartialEq)]
struct BoardPlace {
    selection: Option<Uuid>,
    location: BoardLocation,
    drawer_open: bool,
}

impl BoardPlace {
    fn of(model: &BoardModel) -> Self {
        Self {
            selection: model.selection_id,
            location: model.board_location.clone(),
            drawer_open: model.drawer_open,
        }
    }
}

fn apply_board_intent(
    domain: &mut DomainState,
    model: &mut BoardModel,
    intent: BoardIntent,
    snapshot: Option<&InvocationSnapshot>,
) -> Result<IntentOutcome, DomainError> {
    // While the task page owns input, its verbs act on the page's own task: keep the
    // selection pinned to the bound id, whatever deck visibility did to it meanwhile
    // (a completed task leaves the list, but the page and its verbs stay on it).
    //
    // Closing the page ENDS that contract, so the closing intents are excluded. Pinning
    // through the close left `selection_id` on a task that had just left the deck (complete
    // the page's task with `d`, then Esc): the board then painted no highlighted row while
    // the pin still named the hidden task, and the next `space` or `d` mutated something the
    // user could not see. Excluding them lets `reanchor_selection` move the pin to a visible
    // row, which is what it already does for every other way a task leaves the deck.
    if model.focused_surface() == FocusedSurface::Task
        && model.input_mode == BoardInputMode::TaskPage
        && matches!(
            intent,
            BoardIntent::SelectNext | BoardIntent::SelectPrev | BoardIntent::SelectIndex(_)
        )
    {
        return Ok(IntentOutcome::None);
    }
    let closes_the_page = matches!(
        intent,
        BoardIntent::CloseLayer | BoardIntent::OpenTaskPage | BoardIntent::FocusList
    );
    // A focused task page acts on its own task: bind the selection to it before the intent,
    // so verbs and pickers reach the page's task even when the board does not paint it (a
    // `before` link to a done task with the drawer shut opens without moving the board).
    if model.focused_surface() == FocusedSurface::Task
        && !closes_the_page
        && matches!(
            model.input_mode,
            BoardInputMode::TaskPage
                | BoardInputMode::EditTitle
                | BoardInputMode::EditNotes
                | BoardInputMode::EditScope
                | BoardInputMode::EditAssignee
                | BoardInputMode::SelectBase
                | BoardInputMode::SelectAfter
                | BoardInputMode::FormDropdown
        )
    {
        // Only onto a row the list paints: a page reached through a relation, or a task that
        // left the lens, keeps the list cursor where it is. Verbs follow the page through
        // `BoardModel::action_target_id`, not through this pin.
        if let Some(bound) = model
            .form
            .as_ref()
            .filter(|form| form.is_task())
            .and_then(BoardForm::task_id)
            .filter(|bound| model.visible_ids().contains(bound))
        {
            model.selection_id = Some(bound);
        }
    }
    // What this intent does to the chrome row, decided once, before it is applied.
    apply_chrome_row_lifetime(model, &intent);

    // A command surface is a dispatch surface: any real intent closes it first, so the
    // reducer below runs exactly as it does for the direct keyboard or chip route.
    // CloseLayer owns the progressive dismiss order, including the command
    // surface as its first layer, so it must not be pre-cleared here.
    if !matches!(
        intent,
        BoardIntent::OpenCommandPalette
            | BoardIntent::CommandNext
            | BoardIntent::CommandPrev
            | BoardIntent::CommandQueryInsert(_)
            | BoardIntent::CommandQueryInsertText(_)
            | BoardIntent::CommandQueryBackspace
            | BoardIntent::CommandQueryDeleteWord
            | BoardIntent::ConfirmCommand
            | BoardIntent::SelectCommand(_)
            | BoardIntent::CloseLayer
    ) {
        model.close_command_surface();
    }

    match intent {
        // The app boundary owns question actions (store, durability, Herdr hints); the pure
        // reducer has nothing to apply for them.
        BoardIntent::Question(_) => return Ok(IntentOutcome::None),
        // Likewise report actions (store reads, `mark_read`) and the task-page report open,
        // which the app boundary resolves through `command_target_id`.
        BoardIntent::Report(_) | BoardIntent::OpenTaskReport => return Ok(IntentOutcome::None),
        BoardIntent::OpenCommandPalette => {
            model.open_command_surface(CommandSurface::Palette);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandNext => {
            model.move_command_selection(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandPrev => {
            model.move_command_selection(false);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandQueryInsert(character) => {
            if model.surface == CommandSurface::Palette {
                model.command_query.push(character);
                model.command_selected = 0;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandQueryInsertText(text) => {
            if model.surface == CommandSurface::Palette {
                // The query is a single search line, so each pasted break becomes one space
                // rather than gluing the words on either side of it together (as Title does).
                model.command_query.push_str(&flatten_line_breaks(&text));
                model.command_selected = 0;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CommandQueryBackspace | BoardIntent::CommandQueryDeleteWord => {
            if model.surface == CommandSurface::Palette {
                if intent == BoardIntent::CommandQueryDeleteWord {
                    delete_last_word(&mut model.command_query);
                } else {
                    model.command_query.pop();
                }
                model.command_selected = 0;
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CloseCommandSurface => return Ok(IntentOutcome::None),
        // The app loop performs the OSC 52 write so pure reducer tests stay terminal-free.
        BoardIntent::CopyTaskNumber(_) => return Ok(IntentOutcome::None),
        // Stall checks prompt Herdr agents at the app boundary; nothing to reduce.
        BoardIntent::RefreshTask(_) | BoardIntent::RefreshAll => return Ok(IntentOutcome::None),
        BoardIntent::ConfirmCommand | BoardIntent::SelectCommand(_) => {
            // Resolve to the existing intent, then run that exact route. `SelectCommand`
            // reaches here only defensively: every real caller -- the key, paste and
            // mouse routes, and `apply_board_intent_with_save_recovery` -- already runs it
            // through `resolve_board_command` before an intent gets this far, exactly as
            // they already do for `ConfirmCommand`; this arm just keeps a direct
            // `apply_intent` call (as tests make) from skipping that resolution.
            let Some(resolved) = resolve_board_command(model, intent) else {
                return Ok(IntentOutcome::None);
            };
            return apply_intent(domain, model, resolved, snapshot);
        }
        BoardIntent::Quit => return Ok(IntentOutcome::Quit),
        BoardIntent::OpenCapture => {
            model.close_popup();
            // The palette reaches this from a task page too: never drop a dirty session.
            if model.form.is_some() && model.has_unsaved_task_work() {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
            model.form = None;
            // The board scope picks the destination: every scope and the desk add to the
            // desk, a project scope adds to its project.
            let scope = model
                .quick_add_scope()
                .or_else(|| snapshot.map(crate::ui::capture::CaptureModel::default_scope))
                .unwrap_or(TaskScope::Global);
            // The default never resolves to an archived project, however the archive
            // arrived (launch card kept, picker verb, or a sibling process): fall back to
            // the desk.
            let scope = match scope {
                TaskScope::Project { ref path } if domain.is_project_archived(path) => {
                    TaskScope::Global
                }
                other => other,
            };
            model.quick_add = Some(super::model::QuickAddState::new(snapshot.cloned(), scope));
            model.quick_add_save = None;
            model.input_mode = BoardInputMode::QuickAdd;
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddInsert(character) => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.insert_char(character);
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddInsertText(text) => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.insert_text(&flatten_line_breaks(&text));
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddBackspace => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.backspace();
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddDeleteWordLeft
        | BoardIntent::QuickAddDeleteWordRight
        | BoardIntent::QuickAddDeleteToLineStart
        | BoardIntent::QuickAddDeleteToLineEnd => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                match intent {
                    BoardIntent::QuickAddDeleteWordLeft => quick_add.title.delete_word_left(),
                    BoardIntent::QuickAddDeleteWordRight => quick_add.title.delete_word_right(),
                    BoardIntent::QuickAddDeleteToLineStart => {
                        quick_add.title.delete_to_line_start()
                    }
                    _ => quick_add.title.delete_to_line_end(),
                }
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddDeleteForward => {
            model.invalidate_quick_add_stash();
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.delete_forward();
                refresh_quick_add_scope(model, domain);
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveLeft => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_left();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveRight => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_right();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveLineStart => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_line_start();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveLineEnd => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_line_end();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveWordLeft => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_word_left();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddMoveWordRight => {
            if let Some(quick_add) = model.quick_add.as_mut() {
                quick_add.title.move_word_right();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ExpandQuickAdd => {
            let Some(quick_add) = model.quick_add.as_ref() else {
                return Ok(IntentOutcome::None);
            };
            if model.form.as_ref().is_some_and(|form| !form.is_task()) {
                // The task page is view-first for persisted tasks. A capture draft has nothing
                // to view, so quick-add deliberately opens its page in Notes edit mode.
                model.focus_form_field(CaptureField::Notes);
                return Ok(IntentOutcome::None);
            }
            let lifted = match crate::capture::lift_quick_add_tokens(
                quick_add.title.value(),
                domain,
                quick_add.snapshot.as_ref().as_ref(),
                &quick_add.default,
                &model.agent_names,
            ) {
                Ok(lifted) => lifted,
                Err(message) => {
                    model.set_message(message);
                    return Ok(IntentOutcome::None);
                }
            };
            let scope = lifted.scope.unwrap_or_else(|| quick_add.scope.clone());
            let snapshot = quick_add.snapshot.as_ref().clone();
            let mut form = BoardForm::capture(
                snapshot,
                model.this_repo.as_deref(),
                &model.tasks,
                &model.archived_projects,
            );
            form.title = crate::ui::edit::seeded_draft(&lifted.title);
            form.scope = scope;
            form.assignee = lifted.assignee;
            form.base = lifted.base;
            form.after = lifted.after;
            form.set_agent_names(&model.agent_names);
            form.focus = CaptureField::Notes;
            form.select_current_scope();
            model.form = Some(form);
            model.input_mode = BoardInputMode::EditNotes;
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelQuickAdd => {
            model.discard_quick_add();
            // A save+next acknowledgement has no free status row while the line is open.
            // Keep it for the restored board row when the user finally closes quick-add.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::QuickAddSelectIndex(index) => {
            model.discard_quick_add();
            return apply_board_intent(domain, model, BoardIntent::SelectIndex(index), snapshot);
        }
        BoardIntent::QuickAddSave | BoardIntent::QuickAddSaveNext => {
            return quick_add_save(
                domain,
                model,
                matches!(intent, BoardIntent::QuickAddSaveNext),
            );
        }
        BoardIntent::FormFocusNext => {
            if model.input_mode == BoardInputMode::EditStep {
                if model.park_rename_step_draft() {
                    model.input_mode = BoardInputMode::TaskPage;
                    if !move_step_within_edit_group(model, true) {
                        select_add_step(model);
                    }
                }
            } else if matches!(
                model.input_mode,
                BoardInputMode::TaskPage | BoardInputMode::CapturePage
            ) {
                if model.capture_draft_open() {
                    if model
                        .form
                        .as_ref()
                        .is_some_and(|form| form.steps.add_selected)
                    {
                        model.focus_form_field(CaptureField::Assignee);
                    } else if !move_capture_step_with_tab(model, true) {
                        select_add_step(model);
                    }
                } else if model.task_editing() {
                    if model
                        .form
                        .as_ref()
                        .is_some_and(|form| form.steps.add_selected)
                    {
                        model.focus_form_field(CaptureField::Assignee);
                    } else if !move_step_within_edit_group(model, true) {
                        select_add_step(model);
                    }
                } else if let Some(trail) = trail_tab(model, true) {
                    // The PAPER TRAIL closes the whole page ring, including its relations.
                    if matches!(trail, super::block::PageTab::LeaveBlock)
                        && !super::block::enter_block_ring(model, true)
                    {
                        select_first_page_stop(model);
                    }
                } else {
                    match super::block::move_block_tab(model, true) {
                        super::block::PageTab::Moved => {}
                        super::block::PageTab::LeaveBlock => {
                            if !select_first_page_stop(model) {
                                model.enter_page_field_focus();
                            }
                        }
                        super::block::PageTab::NotHandled => {
                            let at_end = at_page_ring_edge(model, true);
                            let nothing_selected = model.form.as_ref().is_some_and(|form| {
                                form.steps.cursor.is_none()
                                    && !form.steps.add_selected
                                    && form.steps.relation_cursor.is_none()
                            });
                            // Relations stay in the ring before the PAPER TRAIL. An unselected
                            // page enters its BLOCKED section first when one exists.
                            if !(at_end && super::block::enter_trail(model, true)
                                || nothing_selected && super::block::enter_block_ring(model, true)
                                || move_page_ring(model, true)
                                || select_first_step_from_page(model)
                                || read_only_page_refusal(model).is_some())
                            {
                                model.enter_page_field_focus();
                            }
                        }
                    }
                }
            } else if model.input_mode == BoardInputMode::EditNotes
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                select_step_from_tab(model, true);
            } else if model.input_mode == BoardInputMode::EditNotes && model.capture_draft_open() {
                select_first_capture_step_or_add(model);
            } else if model.input_mode == BoardInputMode::EditScope
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                model.focus_form_field(CaptureField::Title);
            } else if model.input_mode == BoardInputMode::EditAssignee && model.form.is_some() {
                model.focus_form_field(CaptureField::Base);
            } else if model.input_mode == BoardInputMode::SelectBase && model.form.is_some() {
                model.focus_form_field(CaptureField::After);
            } else if model.input_mode == BoardInputMode::SelectAfter && model.form.is_some() {
                model.focus_form_field(CaptureField::Scope);
            } else if model.form.is_some() && model.input_mode != BoardInputMode::FormDropdown {
                model.move_form_focus(true);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormFocusPrev => {
            if model.input_mode == BoardInputMode::EditStep {
                if model.park_rename_step_draft() {
                    model.input_mode = BoardInputMode::TaskPage;
                    if !move_step_within_edit_group(model, false) {
                        model.focus_form_field(CaptureField::Notes);
                    }
                }
                return Ok(IntentOutcome::None);
            } else if matches!(
                model.input_mode,
                BoardInputMode::TaskPage | BoardInputMode::CapturePage
            ) {
                if model.capture_draft_open() {
                    if model
                        .form
                        .as_ref()
                        .is_some_and(|form| form.steps.add_selected)
                    {
                        if !select_last_capture_step(model) {
                            model.focus_form_field(CaptureField::Notes);
                        }
                    } else if !move_capture_step_with_tab(model, false) {
                        model.focus_form_field(CaptureField::Notes);
                    }
                } else if model.task_editing() {
                    if model
                        .form
                        .as_ref()
                        .is_some_and(|form| form.steps.add_selected)
                    {
                        select_last_step_for_edit(model);
                    } else if !move_step_within_edit_group(model, false) {
                        model.focus_form_field(CaptureField::Notes);
                    }
                } else if let Some(trail) = trail_tab(model, false) {
                    // Shift+Tab from the trail returns to the page ring's last relation.
                    if matches!(trail, super::block::PageTab::LeaveBlock) {
                        select_ring_end(model);
                    }
                } else {
                    match super::block::move_block_tab(model, false) {
                        super::block::PageTab::Moved => {}
                        // Shift+Tab before the top line wraps to the ring's last stop.
                        super::block::PageTab::LeaveBlock => select_last_page_stop(model),
                        super::block::PageTab::NotHandled
                            if model.page_section_open()
                                && model.form.as_ref().is_some_and(|form| {
                                    form.steps.cursor.is_none()
                                        && !form.steps.add_selected
                                        && form.steps.relation_cursor.is_none()
                                }) =>
                        {
                            // Nothing selected yet (a blocked page opens so): the last stop.
                            select_last_page_stop(model);
                        }
                        super::block::PageTab::NotHandled => {
                            let at_start = at_page_ring_edge(model, false);
                            let climbed = at_start && super::block::enter_block_ring(model, false);
                            let wrapped =
                                at_start && !climbed && super::block::enter_trail(model, false);
                            if !climbed && !wrapped && !move_page_ring(model, false) {
                                // A read-only page has no edit session to enter: shift+Tab
                                // starts the ring from its far end instead.
                                if read_only_page_refusal(model).is_some() {
                                    select_ring_end(model);
                                } else {
                                    model.enter_page_field_focus();
                                }
                            }
                        }
                    }
                }
                return Ok(IntentOutcome::None);
            }
            if model.input_mode == BoardInputMode::EditTitle
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                model.focus_form_field(CaptureField::Scope);
            } else if model.input_mode == BoardInputMode::SelectAfter && model.form.is_some() {
                model.focus_form_field(CaptureField::Base);
            } else if model.input_mode == BoardInputMode::SelectBase && model.form.is_some() {
                model.focus_form_field(CaptureField::Assignee);
            } else if model.input_mode == BoardInputMode::EditScope
                && model.form.as_ref().is_some_and(|form| form.is_task())
            {
                model.focus_form_field(CaptureField::After);
            } else if model.input_mode == BoardInputMode::EditAssignee && model.form.is_some() {
                select_add_step(model);
            } else if model.form.is_some() && model.input_mode != BoardInputMode::FormDropdown {
                model.move_form_focus(false);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusFormCursor(field, row, column) => {
            let cursor = model.form.as_ref().and_then(|form| {
                let (draft, width) = match field {
                    CaptureField::Title => (&form.title, form.title_wrap_width.get()),
                    CaptureField::Notes => (&form.notes, form.notes_width.get()),
                    _ => return None,
                };
                crate::ui::edit::wrap_text(draft.value(), width.max(1))
                    .get(row)
                    .map(|row| row.cursor_at(column))
            });
            model.focus_form_field(field);
            let focused = matches!(
                (field, model.input_mode),
                (CaptureField::Title, BoardInputMode::EditTitle)
                    | (CaptureField::Notes, BoardInputMode::EditNotes)
            );
            if let Some(cursor) = cursor.filter(|_| focused) {
                edit_draft(model, |draft| draft.set_cursor(cursor));
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusFormField(field) => {
            model.focus_form_field(field);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormAssigneeNext | BoardIntent::FormAssigneePrev => {
            model.cycle_form_assignee(matches!(intent, BoardIntent::FormAssigneeNext));
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormCycleScope => {
            if model
                .form
                .as_ref()
                .is_some_and(|form| form.focus == CaptureField::Scope)
                && model.input_mode != BoardInputMode::FormDropdown
            {
                model.cycle_form_scope();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenFormDropdown(field) => {
            model.open_form_dropdown(field);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormDropdownNext => {
            model.move_form_dropdown(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FormDropdownPrev => {
            model.move_form_dropdown(false);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmFormDropdown => {
            if model.input_mode == BoardInputMode::FormDropdown {
                model.close_form_dropdown(true);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelFormDropdown => {
            if model.input_mode == BoardInputMode::FormDropdown {
                model.close_form_dropdown(false);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectFormDropdownOption(index) => {
            model.select_form_dropdown_option(index);
            return Ok(IntentOutcome::None);
        }

        BoardIntent::SelectNext => {
            model.select_next();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectPrev => {
            model.select_prev();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::NextTask | BoardIntent::PrevTask => {
            step_task_page(domain, model, intent == BoardIntent::NextTask);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleMarkMode => {
            if !model.mark_mode_active() && !model.task_list_owns_input() {
                return Ok(IntentOutcome::None);
            }
            model.toggle_mark_mode();
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MarkToggle => {
            if !model.task_list_owns_input() || !model.mark_mode_active() {
                return Ok(IntentOutcome::None);
            }
            model.toggle_selected_mark();
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MarkToggleAt(idx) => {
            if !model.task_list_owns_input() || !model.mark_mode_active() {
                return Ok(IntentOutcome::None);
            }
            if model.select_index(idx) {
                model.toggle_selected_mark();
                model.last_row_click = None;
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MarkExtend(direction) => {
            if !model.task_list_owns_input() || !model.mark_mode_active() {
                return Ok(IntentOutcome::None);
            }
            model.mark_selected();
            match direction {
                MarkDirection::Up => model.select_prev(),
                MarkDirection::Down => model.select_next(),
            };
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MarkClear => {
            model.clear_marks();
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectIndex(idx) => {
            if !model.select_index(idx) {
                return Ok(IntentOutcome::None);
            }
            // A row click selects; a second click on the same row inside the double-click
            // window opens the task page.
            if let Some(id) = model.selected_id() {
                let now = Instant::now();
                let is_double = model.last_row_click.is_some_and(|(at, last)| {
                    last == id && now.duration_since(at) <= ROW_DOUBLE_CLICK_WINDOW
                });
                if is_double {
                    model.last_row_click = None;
                    open_focused_task_page(domain, model, id);
                } else {
                    model.last_row_click = Some((now, id));
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusBoardAndSelectIndex(idx) => {
            if !model.select_index(idx) {
                return Ok(IntentOutcome::None);
            }
            // A row click retargets the detail column and returns focus to the list. A second
            // click on the same row inside the double-click window focuses the detail.
            model.wide_stage = WideStage::Board;
            if let Some(id) = model.selected_id() {
                let now = Instant::now();
                let is_double = model.last_row_click.is_some_and(|(at, last)| {
                    last == id && now.duration_since(at) <= ROW_DOUBLE_CLICK_WINDOW
                });
                if is_double {
                    model.last_row_click = None;
                    open_focused_task_page(domain, model, id);
                } else {
                    model.last_row_click = Some((now, id));
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListScrollTo(offset) => {
            if model.focused_surface() != FocusedSurface::Board {
                return Ok(IntentOutcome::None);
            }
            model.set_list_scroll(offset);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BlockCardConfirm => {
            return super::block::confirm_block_card(domain, model);
        }
        BoardIntent::BlockCardCancel => {
            // Esc keeps the marks: the card returns to the same marked set.
            super::block::cancel_block_card(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BlockCardNextField | BoardIntent::BlockCardPrevField => {
            super::block::block_card_field(model, intent == BoardIntent::BlockCardNextField);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BlockCardLeft | BoardIntent::BlockCardRight => {
            super::block::block_card_arrow(model, intent == BoardIntent::BlockCardRight);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BlockCardNewline => {
            super::block::block_card_newline(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CycleCheck => {
            return super::block::cycle_selected_check(domain, model);
        }
        BoardIntent::TogglePassedChecks => {
            super::block::toggle_passed_checks(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ClickCheck(index) => {
            if !super::block::select_stop(model, super::block::BlockTarget::Check(index)) {
                return Ok(IntentOutcome::None);
            }
            return super::block::cycle_selected_check(domain, model);
        }
        BoardIntent::ClickPassedFold => {
            if super::block::select_stop(model, super::block::BlockTarget::PassedFold) {
                super::block::toggle_passed_checks(model);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleTrail => {
            super::block::toggle_trail(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleTrailRecord(index) => {
            super::block::toggle_trail_record(model, index);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ApproveReview(id) => {
            model.close_popup();
            if !domain
                .get(id)
                .is_some_and(|task| task.status == HumanStatus::Review)
            {
                return Ok(IntentOutcome::None);
            }
            domain.complete(id)?;
        }
        BoardIntent::ReplyApprove => {
            // The application boundary approves a review; anything else lands here.
            if model.input_mode == BoardInputMode::EditReply {
                model.set_message("only a task in review can be approved");
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BeginReply => {
            if model.input_mode() == BoardInputMode::Normal {
                // `r` on a board row answers the cursor task inline, whatever is marked.
                if !super::block::begin_row_reply(model) {
                    model.set_message("not blocked or in review");
                }
            } else if model.input_mode == BoardInputMode::TaskPage
                && !super::block::begin_reply(model, "", None)
            {
                model.set_message("only a blocked or review task takes replies");
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PickOption(index) => {
            // A number picks a block option only on a blocked page; anywhere else it is inert.
            if model.input_mode == BoardInputMode::TaskPage {
                if let Some(text) = super::block::option_text(model, index) {
                    super::block::begin_reply(model, &text, None);
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageReplyUnblock => {
            // The application boundary opens the box and saves it; a bare reducer only opens it.
            if model.input_mode == BoardInputMode::TaskPage {
                super::block::begin_reply(model, "", None);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ReplyWithOption => {
            if let Some(text) = super::block::selected_option_text(model) {
                super::block::begin_reply(model, &text, None);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ReplySave
        | BoardIntent::ReplySaveUnblock
        | BoardIntent::ReplySaveStart
        | BoardIntent::ReplySaveBeforeLaunch => {
            if model.input_mode != BoardInputMode::EditReply {
                return Ok(IntentOutcome::None);
            }
            let unblock = match intent {
                BoardIntent::ReplySaveUnblock => Some(HumanStatus::Ready),
                BoardIntent::ReplySaveStart => Some(HumanStatus::Started),
                _ => None,
            };
            return super::block::save_reply(
                domain,
                model,
                unblock,
                intent == BoardIntent::ReplySaveBeforeLaunch,
            );
        }
        BoardIntent::BeginAddStep | BoardIntent::BeginAppendStep => {
            model.close_popup();
            // Ctrl+A and the trailing target both open the independent add row from a task
            // page or an expanded capture. Park a rename first, so Ctrl+A never drops an
            // existing staged rename while replacing its cursor with the add row.
            // A selected stored step anchors the row: the new step lands directly below it.
            // A click on the trailing `+ step` row means the end of the list.
            let anchor = (intent == BoardIntent::BeginAddStep)
                .then(|| selected_step(domain, model).map(|(_, step_id)| step_id))
                .flatten();
            if model.form.is_some() && model.park_rename_step_draft() {
                open_step_editor(model, "", None);
                if let Some(editor) = model
                    .form
                    .as_mut()
                    .filter(|form| form.is_task())
                    .and_then(|form| form.steps.editor.as_mut())
                    .filter(|editor| editor.rename.is_none())
                {
                    editor.insert_after = anchor;
                }
                // Keep the new row, painted directly below its anchor, inside the viewport.
                let anchor_index = anchor.and_then(|anchor| {
                    let task = domain.get(model.form.as_ref()?.task_id()?)?;
                    task.steps.iter().position(|step| step.id == anchor)
                });
                if let (Some(index), Some(form)) = (anchor_index, model.form.as_mut()) {
                    let rows_through_anchor =
                        form.steps.row_counts.borrow().iter().take(index + 1).sum();
                    steps_scroll_to_row(form, rows_through_anchor, 1);
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::BeginEditTitle | BoardIntent::BeginEditNotes | BoardIntent::BeginEditScope => {
            model.close_popup();
            model.clear_marks();
            // Ctrl+E on the BLOCKED section: the heading edits why, on and needs in the
            // block card; one of your replies opens the reply box on it.
            if intent == BoardIntent::BeginEditTitle && model.input_mode == BoardInputMode::TaskPage
            {
                if let Some(task) = super::block::heading_selected_task(model) {
                    super::block::open_block_edit_card(model, &task);
                    return Ok(IntentOutcome::None);
                }
                if let Some((index, text)) = super::block::selected_own_reply(model) {
                    super::block::begin_reply(model, &text, Some(index));
                    return Ok(IntentOutcome::None);
                }
                if super::block::agent_reply_selected(model) {
                    model.set_message("agent replies cannot be edited");
                    return Ok(IntentOutcome::None);
                }
            }
            // Ctrl+E on an already-open inline row keeps that row focused. Field traversal is
            // explicit through Tab or clicks, so this never discards or redirects its draft.
            if intent == BoardIntent::BeginEditTitle && model.input_mode == BoardInputMode::EditStep
            {
                return Ok(IntentOutcome::None);
            }
            let focus = match intent {
                BoardIntent::BeginEditTitle => CaptureField::Title,
                BoardIntent::BeginEditNotes => CaptureField::Notes,
                BoardIntent::BeginEditScope => CaptureField::Scope,
                _ => unreachable!("matched task-form entry intent"),
            };
            // Ctrl+E on a selected step begins the whole task edit session and opens that
            // row's in-place editor, including when the selection was made in task view.
            if intent == BoardIntent::BeginEditTitle {
                if let Some((task_id, step_id)) = selected_step(domain, model) {
                    let text = domain.get(task_id).and_then(|task| {
                        task.steps
                            .iter()
                            .find(|step| step.id == step_id)
                            .map(|step| step.text.clone())
                    });
                    if let Some(text) = text {
                        if let Some(form) = model.form.as_mut().filter(|form| form.is_task()) {
                            form.editing = true;
                        }
                        open_step_editor(model, &text, Some(step_id));
                        return Ok(IntentOutcome::None);
                    }
                }
            }
            // The page already open: move focus into the asked field, keep every draft, and
            // transfer input ownership before the editor can accept a key.
            if model.form.as_ref().is_some_and(BoardForm::is_task) {
                enter_task_stage(model);
                model.focus_form_field(focus);
                return Ok(IntentOutcome::None);
            }
            if let Some(id) = model.selected_id() {
                if let Some(task) = domain.get(id) {
                    // One immutable id and three independent drafts are captured at open.
                    // `sync_from_domain` deliberately never writes this form, so background
                    // refresh can reanchor selection without redirecting its later save.
                    let mut form = BoardForm::task(
                        task,
                        model.this_repo.as_deref(),
                        &model.tasks,
                        focus,
                        &model.archived_projects,
                        &model.agent_names,
                    );
                    // A direct board edit is a real edit session too, so its confirmed task
                    // page keeps step interaction available after the field saves.
                    form.editing = true;
                    model.input_mode = form.parent_mode();
                    model.form = Some(form);
                    enter_task_stage(model);
                    model.clear_message();
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditInsert(c) => {
            edit_draft(model, |draft| draft.insert_char(c));
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditInsertText(text) => {
            // Title stays one line in either form; Notes preserves pasted line
            // breaks. The step editor is one line by construction, so it flattens like Title.
            let single_line = model.input_mode == BoardInputMode::EditStep
                || model.input_mode() == BoardInputMode::BlockCard
                || (model.input_mode != BoardInputMode::EditReply
                    && model
                        .form
                        .as_ref()
                        .is_some_and(|form| matches!(form.focus, CaptureField::Title)));
            edit_draft(model, |draft| {
                if single_line {
                    draft.insert_text(&flatten_line_breaks(&text));
                } else {
                    draft.insert_text(&text);
                }
            });
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditInsertLineBreak => {
            if model.input_mode == BoardInputMode::EditReply
                || model
                    .form
                    .as_ref()
                    .is_some_and(|form| form.focus == CaptureField::Notes)
            {
                edit_draft(model, |draft| draft.insert_char('\n'));
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditBackspace => {
            edit_draft(model, EditBuffer::backspace);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteForward => {
            edit_draft(model, EditBuffer::delete_forward);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveLeft => {
            edit_draft(model, EditBuffer::move_left);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveRight => {
            edit_draft(model, EditBuffer::move_right);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveUp | BoardIntent::EditMoveDown => {
            // Vertical movement wraps at the painted notes width the renderer
            // recorded; until a frame has been drawn (width 0) it stays inert.
            let delta: isize = if matches!(intent, BoardIntent::EditMoveDown) {
                1
            } else {
                -1
            };
            let reply = super::block::active_reply(model)
                .filter(|_| model.input_mode == BoardInputMode::EditReply)
                .and_then(|(_, editor)| {
                    crate::ui::edit::wrapped_vertical_move(
                        &editor.buffer,
                        editor.width.get(),
                        delta,
                    )
                });
            let target = reply.or_else(|| {
                model
                    .form
                    .as_ref()
                    .filter(|form| {
                        form.focus == CaptureField::Notes
                            && model.input_mode == BoardInputMode::EditNotes
                    })
                    .and_then(|form| {
                        crate::ui::edit::wrapped_vertical_move(
                            &form.notes,
                            form.notes_width.get(),
                            delta,
                        )
                    })
            });
            if let Some(target) = target {
                edit_draft(model, |draft| draft.set_cursor(target));
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveLineStart => {
            edit_draft(model, EditBuffer::move_line_start);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveLineEnd => {
            edit_draft(model, EditBuffer::move_line_end);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveWordLeft => {
            edit_draft(model, EditBuffer::move_word_left);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditMoveWordRight => {
            edit_draft(model, EditBuffer::move_word_right);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteWordLeft => {
            edit_draft(model, EditBuffer::delete_word_left);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteWordRight => {
            edit_draft(model, EditBuffer::delete_word_right);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteToLineStart => {
            edit_draft(model, EditBuffer::delete_to_line_start);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::EditDeleteToLineEnd => {
            edit_draft(model, EditBuffer::delete_to_line_end);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ShiftEnterAsCtrlJ => {
            model.set_ephemeral_message(SHIFT_ENTER_AS_CTRL_J_HINT, Duration::from_secs(5));
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelEdit => {
            // The reply box cancels to page view (or the board), its draft discarded.
            if model.input_mode == BoardInputMode::EditReply {
                if super::block::active_reply(model)
                    .is_some_and(|(_, editor)| editor.pending.is_none())
                {
                    super::block::cancel_reply(model);
                }
                return Ok(IntentOutcome::None);
            }
            // The inline step editor cancels to page view: draft discarded, no mutation,
            // and the page's step cursor state stays intact.
            if model.input_mode == BoardInputMode::EditStep && model.form.is_some() {
                let capture = model.form.as_ref().is_some_and(|form| !form.is_task());
                close_step_editor(model);
                if capture {
                    if let Some(form) = model.form.as_ref() {
                        model.input_mode = form.parent_mode();
                    }
                }
                return Ok(IntentOutcome::None);
            }
            // Field edit on the task page: Esc cancels the field being edited (its draft
            // resets to the saved value) and steps back to view mode. Drafts on other
            // fields survive; the second Esc closes the page.
            if matches!(
                model.input_mode,
                BoardInputMode::EditTitle
                    | BoardInputMode::EditNotes
                    | BoardInputMode::EditScope
                    | BoardInputMode::EditAssignee
                    | BoardInputMode::SelectBase
                    | BoardInputMode::SelectAfter
            ) && model.form.as_ref().is_some_and(BoardForm::is_task)
            {
                let field = model
                    .form
                    .as_ref()
                    .map(|form| form.focus)
                    .unwrap_or(CaptureField::Title);
                let saved = model
                    .form
                    .as_ref()
                    .and_then(|form| form.task_id())
                    .and_then(|id| model.tasks.iter().find(|task| task.id == id))
                    .cloned();
                if let (Some(form), Some(task)) = (model.form.as_mut(), saved) {
                    form.reset_field_to_saved(field, &task);
                }
                model.input_mode = BoardInputMode::TaskPage;
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            // Esc from an expanded capture returns to its retained quick-add line. Keep the
            // complete form as a stash so Tab can restore its notes and scope.
            if model.quick_add.is_some() && model.form.as_ref().is_some_and(|form| !form.is_task())
            {
                model.input_mode = BoardInputMode::QuickAdd;
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            // All other complete forms discard as before. Dropdown Esc has its own intent.
            if model.form.take().is_some() {
                model.input_mode = if model.quick_add.is_some() {
                    BoardInputMode::QuickAdd
                } else {
                    BoardInputMode::Normal
                };
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmEditNext => {
            // Shift+Enter on an existing step, or on a typed new step, commits the complete
            // task edit session. Keep the active row allocated until persistence confirms.
            if model.input_mode == BoardInputMode::EditStep {
                if model.stage_active_rename_draft() {
                    let outcome = confirm_edit(domain, model, None)?;
                    model.sync_from_domain(domain);
                    return Ok(outcome);
                }
                return confirm_add_step(domain, model, snapshot, true);
            }
            return apply_intent(domain, model, BoardIntent::ConfirmEdit, snapshot);
        }
        BoardIntent::ConfirmEdit => {
            if model.input_mode == BoardInputMode::EditStep {
                // Plain Enter parks an existing-step rename. New-step adds save and open the
                // next empty row, including an empty draft which stays on the line as a refusal.
                if model.has_active_step_rename() {
                    if model.park_rename_step_draft() {
                        model.input_mode = BoardInputMode::TaskPage;
                    }
                    return Ok(IntentOutcome::None);
                }
                return confirm_add_step(domain, model, snapshot, false);
            }
            if model.form.as_ref().is_some_and(|form| !form.is_task()) {
                // Capture keeps its immutable invocation snapshot in the shared form. Without
                // it there is nothing to save against, so the draft remains visible and intact.
                let form = model.form.as_ref().expect("capture form checked above");
                let Some(snap) = form.snapshot().cloned() else {
                    model.set_message("capture context unavailable; press Esc and try again");
                    return Ok(IntentOutcome::None);
                };
                let title = form.title.value().to_string();
                let notes =
                    (!form.notes.value().trim().is_empty()).then(|| form.notes.value().to_string());
                let scope_override = Some(form.scope.clone());
                let assignee = form.assignee.clone();
                let base = form.base.clone();
                let after = form.after.clone();
                if let Err(error) = domain.check_new_after(&after) {
                    model.set_message(error.to_string());
                    return Ok(IntentOutcome::None);
                }
                let pending_adds = form.steps.pending_adds.clone();
                return match crate::capture::capture_save_configured(
                    domain,
                    None,
                    &snap,
                    title,
                    notes,
                    scope_override,
                    None,
                    assignee,
                    base,
                ) {
                    Ok(id) => {
                        domain.set_after_on_create(id, &after)?;
                        for text in &pending_adds {
                            domain.add_step(id, text)?;
                        }
                        let expanded_quick_add = model.quick_add.is_some();
                        if expanded_quick_add {
                            // The app save boundary still owns this create. Retain the expanded
                            // form until it succeeds so recovery Cancel can return to a complete
                            // quick-add stash instead of an edit mode with no form.
                            model.begin_quick_add_save(id, false);
                        } else {
                            model.input_mode = BoardInputMode::Normal;
                            model.clear_message();
                        }
                        Ok(IntentOutcome::Persist)
                    }
                    Err(crate::capture::CaptureError::Domain(DomainError::EmptyTitle)) => {
                        model.set_message(TITLE_REQUIRED_MESSAGE);
                        Ok(IntentOutcome::None)
                    }
                    Err(error) => {
                        model.set_message(error.to_string());
                        Ok(IntentOutcome::None)
                    }
                };
            }
            let outcome = confirm_edit(domain, model, None)?;
            model.sync_from_domain(domain);
            return Ok(outcome);
        }

        BoardIntent::SelectNavTab(tab) => {
            // The board-tab route ends the relation trail, like a scope change.
            model.page_back.clear();
            if model.select_nav_tab(tab) {
                model.clear_marks();
            }
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenAssigneePicker => {
            if model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
            model.close_help();
            // An open edit session owns its assignee draft: `@` opens that field's dropdown
            // rather than writing behind the draft.
            if model.input_mode == BoardInputMode::TaskPage && model.task_editing() {
                return apply_board_intent(
                    domain,
                    model,
                    BoardIntent::OpenFormDropdown(CaptureField::Assignee),
                    snapshot,
                );
            }
            let task_page = model.input_mode == BoardInputMode::TaskPage
                && model.form.as_ref().is_some_and(BoardForm::is_task);
            if !(task_page || model.input_mode == BoardInputMode::Normal) {
                return Ok(IntentOutcome::None);
            }
            let ids: Vec<Uuid> = model
                .verb_target_ids()
                .into_iter()
                .filter(|id| domain.get(*id).is_some_and(|task| !task.is_notice()))
                .collect();
            if ids.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let current = model
                .selected_id()
                .filter(|id| ids.contains(id))
                .or_else(|| ids.first().copied())
                .and_then(|id| domain.get(id))
                .and_then(Task::assigned);
            model.clear_message();
            model.open_assignee_picker(ids, current);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenBasePicker => {
            if model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
            model.close_help();
            let task_page = model.input_mode == BoardInputMode::TaskPage
                && model.form.as_ref().is_some_and(BoardForm::is_task);
            let edit_draft =
                (model.task_editing() || model.capture_draft_open()) && model.form.is_some();
            if !(task_page || edit_draft || model.input_mode == BoardInputMode::Normal) {
                return Ok(IntentOutcome::None);
            }
            let ids: Vec<Uuid> = if edit_draft {
                model
                    .form
                    .as_ref()
                    .and_then(BoardForm::task_id)
                    .into_iter()
                    .collect()
            } else {
                model
                    .verb_target_ids()
                    .into_iter()
                    .filter(|id| domain.get(*id).is_some_and(|task| !task.is_notice()))
                    .collect()
            };
            if ids.is_empty() && !edit_draft {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let project = if edit_draft {
                match model.form.as_ref().map(|form| &form.scope) {
                    Some(TaskScope::Project { path }) => PathBuf::from(path),
                    _ => {
                        model.set_message("base requires a project task");
                        return Ok(IntentOutcome::None);
                    }
                }
            } else {
                match base_target_project(domain, &ids) {
                    Ok(project) => project,
                    Err(message) => {
                        model.set_message(message);
                        return Ok(IntentOutcome::None);
                    }
                }
            };
            let current = if edit_draft {
                model.form.as_ref().and_then(|form| form.base.clone())
            } else {
                ids.first()
                    .and_then(|id| domain.get(*id))
                    .and_then(|task| task.base.clone())
            };
            model.clear_message();
            model.open_base_picker(ids, current, &project, edit_draft);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenOnPicker => {
            if model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
            model.close_help();
            open_on_picker(domain, model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenAfterPicker => {
            if model.popup == BoardPopup::SaveRecovery {
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
            model.close_help();
            let task_page = model.input_mode == BoardInputMode::TaskPage
                && model.form.as_ref().is_some_and(BoardForm::is_task);
            let edit_draft =
                (model.task_editing() || model.capture_draft_open()) && model.form.is_some();
            if !(task_page || edit_draft || model.input_mode == BoardInputMode::Normal) {
                return Ok(IntentOutcome::None);
            }
            let ids: Vec<Uuid> = if edit_draft {
                model
                    .form
                    .as_ref()
                    .and_then(BoardForm::task_id)
                    .into_iter()
                    .collect()
            } else {
                model
                    .verb_target_ids()
                    .into_iter()
                    .filter(|id| domain.get(*id).is_some_and(|task| !task.is_notice()))
                    .collect()
            };
            if ids.is_empty() && !edit_draft {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let first = ids.first().and_then(|id| domain.get(*id));
            let (current, scope) = if edit_draft {
                let form = model.form.as_ref().expect("edit draft has a form");
                (form.after.clone(), form.scope.clone())
            } else {
                (
                    first
                        .filter(|_| ids.len() == 1)
                        .map(|task| task.after.clone())
                        .unwrap_or_default(),
                    first.map_or(TaskScope::Global, |task| task.scope.clone()),
                )
            };
            let own = first
                .and_then(|task| task.number)
                .filter(|_| ids.len() == 1);
            model.clear_message();
            model.open_after_picker(ids, current, &scope, edit_draft, own);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenBeforeTask(number) => {
            let Some(id) = domain.task_by_number(number).map(|task| task.id) else {
                return Ok(IntentOutcome::None);
            };
            if model.form.is_some() && model.has_unsaved_work() {
                model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
                return Ok(IntentOutcome::None);
            }
            // The board paints every live scope, so the task opens like a relation: without
            // moving the list's search or cursor, `Esc` returning to this page.
            open_related_task(domain, model, id);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::AfterPickerPin => {
            model.toggle_after_pin();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::AfterPickerClear => {
            model.clear_after_pins();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectListOption(index)
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::After) =>
        {
            if model.select_list_picker_option(index) {
                model.toggle_after_pin();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmListPicker
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::After) =>
        {
            let Some(after) = model.after_picker_choice() else {
                return Ok(IntentOutcome::None);
            };
            let Some(target) = model
                .list_picker
                .as_ref()
                .and_then(|picker| picker.after_target.clone())
            else {
                return Ok(IntentOutcome::None);
            };
            // A refusal (a loop, or a task done meanwhile) keeps the picker open.
            let checked = match target.ids.as_slice() {
                [] => domain.check_new_after(&after),
                ids if target.edit_draft => domain.check_after(ids[0], &after),
                ids => {
                    let mut work = domain.clone();
                    work.set_after_batch(ids, &after).map(|_| ())
                }
            };
            if let Err(error) = checked {
                model.set_message(error.to_string());
                return Ok(IntentOutcome::None);
            }
            model.close_after_picker();
            if target.edit_draft {
                if let Some(form) = model.form.as_mut() {
                    form.after = after;
                    form.focus = CaptureField::After;
                    form.editing = true;
                    model.input_mode = BoardInputMode::SelectAfter;
                }
                return Ok(IntentOutcome::None);
            }
            let changed = domain.set_after_batch(&target.ids, &after)?;
            model.clear_marks();
            if !changed {
                return Ok(IntentOutcome::None);
            }
            model.pending_form_after_sync = Some(target.ids.clone());
            let subject = match target.ids.as_slice() {
                [id] => domain
                    .get(*id)
                    .and_then(|task| task.number)
                    .map_or_else(|| "task".to_string(), |number| format!("T{number}")),
                ids => format!("{} tasks", ids.len()),
            };
            model.set_message(if after.is_empty() {
                format!("{subject} runs after nothing")
            } else {
                format!("{subject} runs after {}", after_list(&after))
            });
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::ChainAfter => {
            model.close_popup();
            let ids = if model.bulk_verb_active() {
                model.marked_in_order()
            } else {
                Vec::new()
            };
            if ids.len() < 2 {
                model.set_message(CHAIN_NEEDS_MARKS);
                return Ok(IntentOutcome::None);
            }
            if let Err(error) = domain.chain_after(&ids) {
                model.set_message(error.to_string());
                return Ok(IntentOutcome::None);
            }
            model.clear_marks();
            let chain = ids
                .iter()
                .filter_map(|id| domain.get(*id).and_then(|task| task.number))
                .map(|number| format!("T{number}"))
                .collect::<Vec<_>>()
                .join(" → ");
            model.sync_from_domain(domain);
            model.set_message(format!("chained {chain}"));
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::RemoveAssigneeName => return Ok(remove_assignee_name(domain, model)),
        BoardIntent::ListPickerUndo => return undo_assignee_name(domain, model),
        BoardIntent::ListPickerNext => {
            model.move_list_picker(true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerPrev => {
            model.move_list_picker(false);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerTabNext | BoardIntent::ListPickerTabPrev => {
            if !model.switch_list_picker_tab(None, intent == BoardIntent::ListPickerTabNext) {
                model.move_list_picker(intent == BoardIntent::ListPickerTabNext);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectListPickerTab(tab) => {
            model.switch_list_picker_tab(Some(tab), true);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerQueryInsert(character) => {
            model.list_picker_query_insert(character);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerQueryInsertText(text) => {
            model.list_picker_query_insert_text(&flatten_line_breaks(&text));
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerQueryBackspace => {
            model.list_picker_query_backspace();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelListPicker => {
            // Every picker returns to the mode it opened over (the parent chooser: its page).
            model.cancel_list_picker();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerQueryDeleteWord => {
            model.list_picker_query_delete_word();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ListPickerScrollUp | BoardIntent::ListPickerScrollDown => {
            if model.input_mode == BoardInputMode::ParentPicker {
                model.scroll_list_picker_rows(intent == BoardIntent::ListPickerScrollDown);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenRelatedTask(id) => {
            open_related_task(domain, model, id);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenParentPicker => {
            let Some(task) = parent_verb_target(model) else {
                return Ok(IntentOutcome::None);
            };
            if model.open_parent_picker(task) {
                model.input_mode = BoardInputMode::ParentPicker;
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ClearParent => {
            let Some(task) = parent_verb_target(model) else {
                return Ok(IntentOutcome::None);
            };
            if domain
                .get(task)
                .and_then(|task| task.parent_task_id)
                .is_none()
            {
                model.set_message(NO_PARENT);
                return Ok(IntentOutcome::None);
            }
            domain.set_parent(task, None)?;
            model.set_message(PARENT_CLEARED);
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::SelectParentOption(index) => {
            if let Some(picker) = model.list_picker.as_mut() {
                picker.selected = index;
            }
            return apply_board_intent(domain, model, BoardIntent::ConfirmParentPicker, snapshot);
        }
        BoardIntent::ConfirmParentPicker => {
            let Some(task) = model.parent_picker_task() else {
                return Ok(IntentOutcome::None);
            };
            let Some((_, option)) = model.selected_list_picker_option() else {
                return Ok(IntentOutcome::None);
            };
            let parent = match option.value {
                ListPickerValue::Clear => None,
                ListPickerValue::Task(id) => Some(id),
                ListPickerValue::Cycle(id) => {
                    // Listed so the user can see why it is missing from the choices; the
                    // chooser stays open for another pick.
                    let name = domain
                        .get(id)
                        .and_then(|task| task.board_identifier())
                        .unwrap_or_else(|| "that task".to_string());
                    model.set_message(format!(
                        "{name} is under this task · choosing it would form a cycle"
                    ));
                    return Ok(IntentOutcome::None);
                }
                _ => return Ok(IntentOutcome::None),
            };
            model.cancel_list_picker();
            model.input_mode = BoardInputMode::TaskPage;
            if !domain.set_parent(task, parent)? {
                model.set_message(PARENT_UNCHANGED);
                return Ok(IntentOutcome::None);
            }
            model.set_message(match parent.and_then(|id| domain.get(id)) {
                Some(parent) => format!(
                    "parent set to {}",
                    parent
                        .board_identifier()
                        .unwrap_or_else(|| parent.title.clone())
                ),
                None => PARENT_CLEARED.to_string(),
            });
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::SelectListOption(index) => {
            // Mouse-only jump onto a visible picker row, same discipline as
            // `SelectCommand`/`SelectProjectOption`: name the row directly.
            if model.select_list_picker_option(index) {
                return apply_board_intent(domain, model, BoardIntent::ConfirmListPicker, snapshot);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmListPicker
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::Base) =>
        {
            let Some((_, option)) = model.selected_list_picker_option() else {
                return Ok(IntentOutcome::None);
            };
            let ListPickerValue::Base(base) = option.value else {
                return Ok(IntentOutcome::None);
            };
            let Some(target) = model.close_base_picker() else {
                return Ok(IntentOutcome::None);
            };
            if target.edit_draft {
                if let Some(form) = model.form.as_mut() {
                    form.base = base;
                    form.focus = CaptureField::Base;
                    form.editing = true;
                    model.input_mode = BoardInputMode::SelectBase;
                }
                return Ok(IntentOutcome::None);
            }
            if let Some(branch) = base.as_deref() {
                for id in &target.ids {
                    let Some(task) = domain.get(*id) else {
                        model.set_message("base requires a project task");
                        return Ok(IntentOutcome::None);
                    };
                    if let Err(message) =
                        crate::git_base::validate_task_base(&task.scope, branch, false)
                    {
                        model.set_message(message);
                        return Ok(IntentOutcome::None);
                    }
                }
            }
            let changed = domain.set_base_batch(&target.ids, base.clone())?;
            model.clear_marks();
            if !changed {
                return Ok(IntentOutcome::None);
            }
            model.pending_form_base_sync = Some(target.ids.clone());
            let subject = match target.ids.as_slice() {
                [id] => domain
                    .get(*id)
                    .and_then(|task| task.number)
                    .map_or_else(|| "task".to_string(), |number| format!("T{number}")),
                ids => format!("{} tasks", ids.len()),
            };
            model.set_message(match base {
                Some(branch) => format!("set {subject} base to {branch}"),
                None => format!("set {subject} base to default"),
            });
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::ConfirmListPicker
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::Assignee) =>
        {
            let Some((_, option)) = model.selected_list_picker_option() else {
                return Ok(IntentOutcome::None);
            };
            let assignee = match option.value {
                ListPickerValue::Assignee(assignee) => assignee,
                ListPickerValue::TypedName => match model.typed_picker_value() {
                    Some(Ok(ListPickerValue::Assignee(assignee))) => assignee,
                    Some(Err(refusal)) => {
                        model.set_message(refusal);
                        return Ok(IntentOutcome::None);
                    }
                    _ => {
                        model.set_message(TYPE_A_NAME_FIRST);
                        return Ok(IntentOutcome::None);
                    }
                },
                _ => return Ok(IntentOutcome::None),
            };
            let Some(target) = model.close_assignee_picker() else {
                return Ok(IntentOutcome::None);
            };
            let changed = domain.assign_batch(&target.ids, assignee.clone())?;
            model.clear_marks();
            if !changed {
                return Ok(IntentOutcome::None);
            }
            model.pending_form_assignee_sync = Some(target.ids.clone());
            // Every task already had the name: the assignment only put it back on the list.
            if let Some(UndoEntry::ShowAssignee { name }) = domain.last_undo() {
                model.set_message(format!(
                    "{} back on the list",
                    crate::domain::display_name(name)
                ));
                return Ok(IntentOutcome::Persist);
            }
            let subject = match target.ids.as_slice() {
                [id] => domain
                    .get(*id)
                    .and_then(|task| task.number)
                    .map_or_else(|| "task".to_string(), |number| format!("T{number}")),
                ids => format!("{} tasks", ids.len()),
            };
            model.set_message(match assignee {
                Some(assignee) => format!(
                    "assigned {subject} to {}",
                    crate::ui::render::assignee_handle(&assignee.name)
                ),
                None => format!("unassigned {subject}"),
            });
            return Ok(IntentOutcome::Persist);
        }
        BoardIntent::ConfirmListPicker
            if model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::WaitingOn) =>
        {
            return confirm_on_picker(domain, model);
        }
        BoardIntent::ConfirmListPicker => {
            // The parent chooser confirms through its own save path.
            if model.parent_picker_task().is_some() {
                return apply_board_intent(
                    domain,
                    model,
                    BoardIntent::ConfirmParentPicker,
                    snapshot,
                );
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusSearch => {
            if model.input_mode != BoardInputMode::Search {
                model.search_return_mode = model.input_mode;
                model.input_mode = BoardInputMode::Search;
            }
            model.search_pinned = false;
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PinSearch => {
            if model.input_mode == BoardInputMode::Search {
                model.input_mode = model.search_return_mode;
                if model.search_query.trim().is_empty() {
                    model.search_query.clear();
                    model.search_pinned = false;
                } else {
                    model.search_pinned = true;
                }
                model.clear_message();
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SearchQueryInsert(character) => {
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            model.search_query.push(character);
            model.search_pinned = false;
            model.clear_message();
            model.reanchor_selection(previous, &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SearchQueryInsertText(text) => {
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            model.search_query.push_str(&text);
            model.search_pinned = false;
            model.clear_message();
            model.reanchor_selection(previous, &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SearchQueryBackspace | BoardIntent::SearchQueryDeleteWord => {
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            if intent == BoardIntent::SearchQueryDeleteWord {
                delete_last_word(&mut model.search_query);
            } else {
                model.search_query.pop();
            }
            model.search_pinned = false;
            model.clear_message();
            model.reanchor_selection(previous, &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleStatusLines => {
            model.show_status_lines = !model.show_status_lines;
            model.follow_list.set(true);
            model.text_selection = None;
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleMessages => {
            if !model.task_list_owns_input() {
                return Ok(IntentOutcome::None);
            }
            // Rows keep their place and marks: only the lines under them come and go.
            if model.toggle_messages_hidden() {
                model.set_message("agent messages hidden");
            } else {
                model.set_message("agent messages shown");
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SwitchListPickerTab(forward) => {
            // The waiting-on picker has no tabs: `←`/`→` cycle its choices.
            if !model.switch_list_picker_tab(None, forward)
                && model.list_picker_kind() == Some(crate::ui::board::ListPickerKind::WaitingOn)
            {
                model.move_list_picker(forward);
            }
            return Ok(IntentOutcome::None);
        }
        // The app-level save-recovery boundary handles these while unresolved.
        BoardIntent::RetrySave | BoardIntent::CancelSave => return Ok(IntentOutcome::None),
        BoardIntent::ToggleStep => {
            model.close_popup();
            let Some((task_id, step_id)) = selected_step(domain, model) else {
                return Ok(IntentOutcome::None);
            };
            domain.with_steps_undo(task_id, |domain| domain.toggle_step(task_id, step_id))?;
        }
        BoardIntent::DispatchAgain => {
            // Host work and its one durable save are owned by the application boundary.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmCleanup | BoardIntent::KeepCleanup | BoardIntent::MergeCleanup => {
            // Cleanup host work and completion persistence are owned by the app boundary.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelCleanup => {
            model.cancel_cleanup_card();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ConfirmDispatch | BoardIntent::StartWithoutRelaunch => {
            // Launches, starts, and their saves are owned by the application boundary.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CancelDispatch => {
            // The card keeps the marks: Esc returns to the same marked set.
            model.close_popup();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CleanupScrollUp | BoardIntent::CleanupScrollDown => {
            model.scroll_cleanup(intent == BoardIntent::CleanupScrollDown);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CleanupSelectUp | BoardIntent::CleanupSelectDown => {
            model.step_cleanup_selection(intent == BoardIntent::CleanupSelectDown);
            return Ok(IntentOutcome::None);
        }
        // The application boundary resolves these to the option they run.
        BoardIntent::CleanupRunSelected | BoardIntent::CleanupClickOption(_) => {
            return Ok(IntentOutcome::None);
        }
        BoardIntent::MoveStepUp | BoardIntent::MoveStepDown => {
            model.close_popup();
            let Some((task_id, step_id)) = selected_step(domain, model) else {
                return Ok(IntentOutcome::None);
            };
            let Some(form) = model.form.as_mut().filter(|form| form.is_task()) else {
                return Ok(IntentOutcome::None);
            };
            let steps = &domain.get(task_id).expect("selected_step checked").steps;
            let from = steps
                .iter()
                .position(|step| step.id == step_id)
                .expect("selected_step checked");
            // Swap with the nearest painted neighbour; staged removals are already gone from
            // the page, so a move never seems to stall behind an invisible row.
            let visible = |index: &usize| !form.steps.removals.contains(&steps[*index].id);
            let target = if intent == BoardIntent::MoveStepUp {
                (0..from).rev().find(visible)
            } else {
                (from + 1..steps.len()).find(visible)
            };
            let Some(target) = target else {
                return Ok(IntentOutcome::None);
            };
            // One undo step per move, like every other step change (T241).
            domain.with_steps_undo(task_id, |domain| domain.move_step(task_id, step_id, target))?;
            // The renderer-recorded heights are source-aligned; reorder them with the step so
            // the scroll below measures the new layout, not the one painted before the move.
            {
                let mut counts = form.steps.row_counts.borrow_mut();
                if from < counts.len() && target < counts.len() {
                    let moved = counts.remove(from);
                    counts.insert(target, moved);
                }
            }
            form.steps.cursor = Some(target);
            form.steps.add_selected = false;
            steps_scroll_to_cursor(form, target);
        }
        BoardIntent::PrimaryVerb => {
            model.close_popup();
            // Status verbs always act on tasks, even with a step selected: Enter owns steps.
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let mut moving = Vec::with_capacity(targets.len());
            let mut skipped_done = 0usize;
            let mut refusal = None;
            for id in targets {
                let Some(task) = domain.get(id) else {
                    model.set_message("that task is no longer here");
                    return Ok(IntentOutcome::None);
                };
                if ctrl_s_starts(task.status) {
                    moving.push(id);
                } else {
                    if task.status == HumanStatus::Done {
                        skipped_done += 1;
                    }
                    // A notice's review or started status is its own; no hint applies.
                    if task.is_notice() {
                        continue;
                    }
                    let label = task
                        .board_identifier()
                        .unwrap_or_else(|| "this task".to_string());
                    refusal = Some(ctrl_s_refusal(&label, task.status));
                }
            }
            // The one cursor task that cannot start says why rather than doing nothing.
            if !bulk && moving.is_empty() {
                if let Some(refusal) = refusal {
                    model.set_message(refusal);
                }
                return Ok(IntentOutcome::None);
            }
            // A marked set that held done tasks says so: start never reopens them.
            if bulk && skipped_done > 0 {
                model.set_message(format!(
                    "doing {} · skipped {skipped_done} done (ctrl+o reopens them)",
                    moving.len()
                ));
            }
            if !domain.change_status(&moving, HumanStatus::Started, bulk)? {
                return Ok(IntentOutcome::None);
            }
        }
        BoardIntent::ToggleBlock => {
            model.close_popup();
            let bulk = model.task_list_owns_input()
                && model.mark_mode_active()
                && model.marked_count() > 0;
            let targets = model.verb_target_ids();
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            // A done task alone refuses; a marked set skips its done tasks, as start does.
            let (targets, skipped_done) = without_done(domain, targets);
            if targets.is_empty() {
                model.set_message("completed tasks cannot be blocked");
                return Ok(IntentOutcome::None);
            }
            let all_blocked = targets.iter().all(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Blocked)
            });
            if !all_blocked {
                // Blocking asks why first. The marks stay until the card confirms.
                super::block::open_block_card(model, targets);
                note_skipped_done(model, skipped_done);
                return Ok(IntentOutcome::None);
            }
            model.clear_marks();
            note_skipped_done(model, skipped_done);
            domain.change_status(&targets, HumanStatus::Ready, bulk)?;
        }
        BoardIntent::ToggleReview => {
            model.close_popup();
            let bulk = model.task_list_owns_input()
                && model.mark_mode_active()
                && model.marked_count() > 0;
            let targets = model.verb_target_ids();
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let (targets, skipped_done) = without_done(domain, targets);
            if targets.is_empty() {
                model.set_message("completed tasks cannot go to review");
                return Ok(IntentOutcome::None);
            }
            let all_review = targets.iter().all(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Review)
            });
            if !all_review {
                // Review asks what was done first. The marks stay until the card confirms.
                super::block::open_review_card(model, targets);
                note_skipped_done(model, skipped_done);
                return Ok(IntentOutcome::None);
            }
            model.clear_marks();
            note_skipped_done(model, skipped_done);
            domain.change_status(&targets, HumanStatus::Ready, bulk)?;
        }
        BoardIntent::ToggleAllGroups => {
            model.clear_marks();
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            if model.toggle_all_groups() {
                model.reanchor_selection(previous, &previous_visible);
            }
            model.clear_message();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusDetail => {
            focus_detail(domain, model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::FocusList => {
            focus_list(model);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenTaskPage => {
            // Enter on a group header toggles that group instead of opening a page:
            // the header is chrome, never a task.
            if model.archived_header_selected() {
                model.clear_marks();
                let previous_visible = model.visible_ids();
                model.toggle_archived_collapsed();
                model.reanchor_selection(Some(ARCHIVED_HEADER_ROW_ID), &previous_visible);
                return Ok(IntentOutcome::None);
            }
            // A capture's staged rows have no domain task identity. Their page owns Enter, so
            // it must never fall through to the board selection hidden behind the draft.
            if model.capture_draft_open() && model.input_mode == BoardInputMode::CapturePage {
                if model
                    .form
                    .as_ref()
                    .is_some_and(|form| form.steps.add_selected)
                {
                    open_step_editor(model, "", None);
                } else if let Some(index) = model.form.as_ref().and_then(|form| form.steps.cursor) {
                    open_capture_pending_step_editor(model, index);
                }
                return Ok(IntentOutcome::None);
            }
            // Enter never opens inline step editing. A selected step remains selected in either
            // page state; Ctrl+E is the deliberate route into its editor.
            if selected_step(domain, model).is_some() {
                return Ok(IntentOutcome::None);
            }
            if model
                .form
                .as_ref()
                .is_some_and(|form| form.steps.add_selected)
            {
                // Selecting `+ step` is reading; opening its editor is an edit entry, which a
                // read-only page refuses like every other.
                if let Some(refusal) = read_only_page_refusal(model) {
                    model.set_message(refusal);
                    return Ok(IntentOutcome::None);
                }
                open_step_editor(model, "", None);
                return Ok(IntentOutcome::None);
            }
            if model.form.as_ref().is_some_and(BoardForm::is_task) {
                // Enter from the list focuses the selected task's detail; on the focused page
                // it closes the page again, as it always has.
                if model.focused_surface() == FocusedSurface::Board {
                    let Some(id) = model.selected_id() else {
                        return Ok(IntentOutcome::None);
                    };
                    open_focused_task_page(domain, model, id);
                } else {
                    leave_task_page(model);
                }
                return Ok(IntentOutcome::None);
            }
            let Some(id) = model.selected_id() else {
                return Ok(IntentOutcome::None);
            };
            open_focused_task_page(domain, model, id);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SelectStep(index) => {
            // Existing steps belong to the enclosing task draft. Parking a rename before
            // selecting another row permits several step edits before one final Shift+Enter.
            // New-step add remains a focused save-and-next line and cannot be abandoned by a
            // row click.
            let editing = model.task_editing();
            if editing && !model.park_rename_step_draft() {
                return Ok(IntentOutcome::None);
            }
            let selected = model
                .form
                .as_ref()
                .filter(|form| form.is_task())
                .and_then(|form| {
                    form.task_id()
                        .map(|task_id| (task_id, &form.steps.removals))
                })
                .and_then(|(task_id, removals)| {
                    domain.get(task_id).and_then(|task| {
                        task.steps
                            .iter()
                            .enumerate()
                            .filter(|(_, step)| !removals.contains(&step.id))
                            .nth(index)
                            .map(|(source_index, step)| (source_index, step.id, step.text.clone()))
                    })
                });
            if let Some((source_index, step_id, text)) = selected {
                if let Some(form) = model.form.as_mut() {
                    form.steps.cursor = Some(source_index);
                    form.steps.add_selected = false;
                    steps_scroll_to_cursor(form, source_index);
                }
                if editing {
                    open_step_editor(model, &text, Some(step_id));
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageScrollTo(offset) => {
            if model.focused_surface() != FocusedSurface::Task && !model.expanded_capture_open() {
                return Ok(IntentOutcome::None);
            }
            if let Some(form) = model.form.as_mut() {
                if matches!(
                    model.input_mode,
                    BoardInputMode::TaskPage
                        | BoardInputMode::EditStep
                        | BoardInputMode::EditTitle
                        | BoardInputMode::EditNotes
                        | BoardInputMode::EditScope
                ) {
                    form.manual_page_scroll = true;
                    let horizon = form.notes_max_scroll.get();
                    form.notes_scroll = offset.min(horizon);
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageWheelScrollUp | BoardIntent::PageWheelScrollDown => {
            if model.focused_surface() != FocusedSurface::Task && !model.expanded_capture_open() {
                return Ok(IntentOutcome::None);
            }
            if let Some(form) = model.form.as_mut() {
                if matches!(
                    model.input_mode,
                    BoardInputMode::TaskPage
                        | BoardInputMode::EditStep
                        | BoardInputMode::EditTitle
                        | BoardInputMode::EditNotes
                        | BoardInputMode::EditScope
                ) {
                    form.manual_page_scroll = true;
                    let horizon = form.notes_max_scroll.get();
                    form.notes_scroll = match intent {
                        BoardIntent::PageWheelScrollUp => form.notes_scroll.saturating_sub(1),
                        BoardIntent::PageWheelScrollDown => {
                            form.notes_scroll.saturating_add(1).min(horizon)
                        }
                        _ => unreachable!("wheel intents matched above"),
                    };
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageScrollUp => {
            if model.focused_surface() != FocusedSurface::Task {
                return Ok(IntentOutcome::None);
            }
            // A fresh add is independent of the surrounding task edit session, so it may be
            // open directly from task view. It has no rename draft to park: keep its cursor
            // alive and scroll the same shared body that task view uses.
            if model.input_mode == BoardInputMode::EditStep && !model.park_rename_step_draft() {
                if let Some(form) = model.form.as_mut() {
                    form.notes_scroll = form.notes_scroll.saturating_sub(1);
                }
                return Ok(IntentOutcome::None);
            }
            if model.task_editing() {
                model.input_mode = BoardInputMode::TaskPage;
                if model
                    .form
                    .as_ref()
                    .is_some_and(|form| form.steps.add_selected)
                {
                    select_last_step_for_edit(model);
                } else if !move_step_within_edit_group(model, false) {
                    model.focus_form_field(CaptureField::Notes);
                }
                return Ok(IntentOutcome::None);
            }
            // ↑ from the first step reaches the parent line; ↑ from a child walks back
            // through the steps. ↑ on the parent line deactivates, as on the first step.
            if model.input_mode == BoardInputMode::TaskPage && move_page_arrow(model, false) {
                return Ok(IntentOutcome::None);
            }
            if let Some(form) = model.form.as_mut().filter(|form| form.is_task()) {
                if model.input_mode == BoardInputMode::TaskPage
                    && form.steps.relation_cursor.is_some()
                {
                    form.steps.relation_cursor = None;
                    return Ok(IntentOutcome::None);
                }
                if model.input_mode == BoardInputMode::TaskPage {
                    // Keyboard arrows own an active step cursor before they move the
                    // shared stream. Wheel intents above remain the explicit reading
                    // route, so an active cursor does not become inert just because
                    // notes overflow (AC-18, AC-26).
                    match form.steps.cursor {
                        Some(0) => form.steps.cursor = None,
                        Some(index) => {
                            let cursor = index - 1;
                            form.steps.cursor = Some(cursor);
                            steps_scroll_to_cursor(form, cursor);
                        }
                        None => form.notes_scroll = form.notes_scroll.saturating_sub(1),
                    }
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::PageScrollDown => {
            if model.focused_surface() != FocusedSurface::Task {
                return Ok(IntentOutcome::None);
            }
            // See Up above. This must precede `task_editing()`, because Ctrl+A and the trailing
            // add target also open a fresh editor directly from task view.
            if model.input_mode == BoardInputMode::EditStep && !model.park_rename_step_draft() {
                if let Some(form) = model.form.as_mut() {
                    let horizon = form.notes_max_scroll.get();
                    form.notes_scroll = form.notes_scroll.saturating_add(1).min(horizon);
                }
                return Ok(IntentOutcome::None);
            }
            if model.task_editing() {
                model.input_mode = BoardInputMode::TaskPage;
                if !model
                    .form
                    .as_ref()
                    .is_some_and(|form| form.steps.add_selected)
                {
                    let _ = move_step_within_edit_group(model, true);
                }
                return Ok(IntentOutcome::None);
            }
            // ↓ from the parent line reaches the first step, ↓ from the last step the first
            // child. At the last entry the selection stays where it is.
            if model.input_mode == BoardInputMode::TaskPage
                && model.form.as_ref().is_some_and(|form| {
                    form.is_task()
                        && (form.steps.relation_cursor.is_some()
                            || form.steps.cursor.is_some()
                            || form.steps.add_selected)
                })
                && move_page_arrow(model, true)
            {
                return Ok(IntentOutcome::None);
            }
            if let Some(form) = model.form.as_mut().filter(|form| form.is_task()) {
                if model.input_mode == BoardInputMode::TaskPage
                    && form.steps.relation_cursor.is_some()
                {
                    return Ok(IntentOutcome::None);
                }
                if model.input_mode == BoardInputMode::TaskPage {
                    let steps = form
                        .task_id()
                        .and_then(|id| domain.get(id))
                        .map(|task| task.steps.len())
                        .unwrap_or(0);
                    match form.steps.cursor {
                        Some(index) if steps > 0 => {
                            let cursor = (index + 1).min(steps - 1);
                            form.steps.cursor = Some(cursor);
                            steps_scroll_to_cursor(form, cursor);
                        }
                        None if steps > 0 && !form.steps.add_selected => {
                            form.steps.cursor = Some(0);
                            steps_scroll_to_cursor(form, 0);
                        }
                        _ => {
                            let horizon = form.notes_max_scroll.get();
                            form.notes_scroll = form.notes_scroll.saturating_add(1).min(horizon);
                        }
                    }
                }
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleDoneDrawer => {
            model.clear_marks();
            let previous_visible = model.visible_ids();
            let previous = model.selection_id;
            model.drawer_open = !model.drawer_open;
            model.reanchor_selection(previous, &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleArchivedGroup => {
            // Flip that one group and select its header. The drawer is already open,
            // because the header only paints inside it.
            model.clear_marks();
            let previous_visible = model.visible_ids();
            model.toggle_archived_collapsed();
            model.select_archived_header();
            model.reanchor_selection(Some(ARCHIVED_HEADER_ROW_ID), &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::ToggleInboxGroup => {
            model.clear_marks();
            let previous_visible = model.visible_ids();
            model.toggle_inbox_collapsed();
            model.select_inbox_header();
            model.reanchor_selection(Some(INBOX_HEADER_ROW_ID), &previous_visible);
            return Ok(IntentOutcome::None);
        }
        BoardIntent::OpenHelp => {
            let active_mode = model.input_mode();
            let return_mode = model.input_mode;
            model.close_command_surface();
            if active_mode != BoardInputMode::LaunchCard {
                model.close_popup();
            }
            model.help_return_mode = return_mode;
            model.help_query.clear();
            model.help_scroll = 0;
            model.help_max_scroll.set(usize::MAX);
            model.input_mode = BoardInputMode::Help;
            return Ok(IntentOutcome::None);
        }
        BoardIntent::HelpQueryInsert(character) => {
            if model.input_mode == BoardInputMode::Help {
                model.help_query.push(character);
                model.help_scroll = 0;
                model.help_max_scroll.set(usize::MAX);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::HelpQueryInsertText(text) => {
            if model.input_mode == BoardInputMode::Help {
                model.help_query.push_str(&flatten_line_breaks(&text));
                model.help_scroll = 0;
                model.help_max_scroll.set(usize::MAX);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::HelpQueryBackspace | BoardIntent::HelpQueryDeleteWord => {
            if model.input_mode == BoardInputMode::Help {
                if intent == BoardIntent::HelpQueryDeleteWord {
                    delete_last_word(&mut model.help_query);
                } else {
                    model.help_query.pop();
                }
                model.help_scroll = 0;
                model.help_max_scroll.set(usize::MAX);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CloseHelp => {
            model.close_help();
            return Ok(IntentOutcome::None);
        }
        BoardIntent::HelpScrollUp | BoardIntent::HelpScrollDown => {
            if model.input_mode != BoardInputMode::Help {
                return Ok(IntentOutcome::None);
            }
            // The painter records the wrapped-row horizon for the last frame. Source catalog
            // rows are not a valid second bound because one description may occupy several
            // screen rows at compact widths.
            let horizon = model.help_max_scroll.get();
            model.help_scroll = match intent {
                BoardIntent::HelpScrollUp => model.help_scroll.saturating_sub(1),
                _ => model.help_scroll.saturating_add(1).min(horizon),
            };
            return Ok(IntentOutcome::None);
        }
        BoardIntent::CloseLayer => {
            if model.clear_marks() {
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            // Progressive close: transient surface (palette/help/dropdown) →
            // open detail → quit. SaveRecovery is not dismissible here; the save-recovery
            // gate owns Retry/Cancel.
            // A clean parked edit is not a visible layer. Check root before resetting it;
            // the application boundary has already refused any dirty root quit.
            if model.root_escape_requests_quit() {
                return Ok(IntentOutcome::Quit);
            }
            if model.surface != CommandSurface::None {
                model.close_command_surface();
                return Ok(IntentOutcome::None);
            }
            if model.input_mode == BoardInputMode::Help {
                if model.help_query.is_empty() {
                    model.close_help();
                } else {
                    model.help_query.clear();
                    model.help_scroll = 0;
                    model.help_max_scroll.set(usize::MAX);
                }
                return Ok(IntentOutcome::None);
            }
            if model.input_mode == BoardInputMode::Search {
                let previous_visible = model.visible_ids();
                let previous = model.selection_id;
                model.search_query.clear();
                model.search_pinned = false;
                model.input_mode = model.search_return_mode;
                model.clear_message();
                model.reanchor_selection(previous, &previous_visible);
                return Ok(IntentOutcome::None);
            }
            // Back after a relation open comes before the pinned search: the search belongs
            // to the list, the trail to the page the user is reading. A dirty draft never
            // gets here as Back; it keeps its own cancel route below.
            if model.input_mode == BoardInputMode::TaskPage
                && model.focused_page_task().is_some()
                && !model.task_editing()
                && !model.task_session_dirty()
                && !model.page_back.is_empty()
                && restore_previous_page(domain, model)
            {
                return Ok(IntentOutcome::None);
            }
            if model.search_pinned {
                let previous_visible = model.visible_ids();
                let previous = model.selection_id;
                model.search_query.clear();
                model.search_pinned = false;
                model.clear_message();
                model.reanchor_selection(previous, &previous_visible);
                return Ok(IntentOutcome::None);
            }
            if model.input_mode == BoardInputMode::FormDropdown {
                model.close_form_dropdown(false);
                return Ok(IntentOutcome::None);
            }
            // A session parked behind list focus is not the active editor: Esc from the list
            // keeps that draft, just like the left arrow does.
            if model.task_editing() && model.focused_surface() == FocusedSurface::Task {
                let saved = model
                    .form
                    .as_ref()
                    .and_then(BoardForm::task_id)
                    .and_then(|id| model.tasks.iter().find(|task| task.id == id))
                    .cloned();
                if let (Some(form), Some(task)) = (model.form.as_mut(), saved) {
                    // Still the same task: the PAPER TRAIL keeps how it was expanded.
                    let trail_expanded = form.block.trail_expanded;
                    let trail_open = std::mem::take(&mut form.block.trail_open);
                    *form = BoardForm::task(
                        &task,
                        model.this_repo.as_deref(),
                        &model.tasks,
                        CaptureField::Title,
                        &model.archived_projects,
                        &model.agent_names,
                    );
                    form.block.trail_expanded = trail_expanded;
                    form.block.trail_open = trail_open;
                    model.input_mode = BoardInputMode::TaskPage;
                    model.clear_message();
                    return Ok(IntentOutcome::None);
                }
            }
            if model.form.as_ref().is_some_and(BoardForm::is_task)
                && model.focused_surface() == FocusedSurface::Task
            {
                leave_task_page(model);
                return Ok(IntentOutcome::None);
            }
            if model.focused_surface() == FocusedSurface::Board
                && model.form.as_ref().is_some_and(BoardForm::is_task)
            {
                // A parked page is not a visible layer on the board: Esc falls through to
                // the board's own close order instead of silently dropping the session.
            } else if model.form.take().is_some() {
                model.input_mode = if model.quick_add.is_some() {
                    BoardInputMode::QuickAdd
                } else {
                    BoardInputMode::Normal
                };
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            if model.quick_add.take().is_some() {
                model.input_mode = BoardInputMode::Normal;
                model.clear_message();
                return Ok(IntentOutcome::None);
            }
            let before = model.popup;
            model.close_popup();
            if model.popup != before {
                return Ok(IntentOutcome::None);
            }
            return Ok(IntentOutcome::None);
        }
        // The five intents below aim at the selection, and an empty board has none. Each
        // says so rather than returning to a row that has just been cleared for an action
        // that then did nothing: a silent no-op is the failure the row exists to prevent.
        // Palette **set status: blocked** asks why like `ctrl+b`, over the targets not yet
        // blocked; it never unblocks.
        BoardIntent::SetStatus(HumanStatus::Blocked) => {
            model.close_popup();
            let targets = model.verb_target_ids();
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let targets: Vec<Uuid> = targets
                .into_iter()
                .filter(|id| {
                    domain
                        .get(*id)
                        .is_some_and(|task| task.status != HumanStatus::Blocked)
                })
                .collect();
            if !targets.is_empty() {
                super::block::open_block_card(model, targets);
            }
            return Ok(IntentOutcome::None);
        }
        // Palette **set status: review** asks what was done like `ctrl+r`, over the targets not
        // yet in review; it never returns a review to ready.
        BoardIntent::SetStatus(HumanStatus::Review) => {
            model.close_popup();
            let targets = model.verb_target_ids();
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            let targets: Vec<Uuid> = targets
                .into_iter()
                .filter(|id| {
                    domain
                        .get(*id)
                        .is_some_and(|task| task.status != HumanStatus::Review)
                })
                .collect();
            if !targets.is_empty() {
                super::block::open_review_card(model, targets);
            }
            return Ok(IntentOutcome::None);
        }
        BoardIntent::SetStatus(status) => {
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !domain.change_status(&targets, status, bulk)? {
                model.close_popup();
                return Ok(IntentOutcome::None);
            }
            model.close_popup();
        }
        BoardIntent::Complete => {
            model.close_popup();
            // Task-level, even with a step selected (Enter owns the step).
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if targets.iter().all(|id| {
                domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Done)
            }) {
                return Ok(IntentOutcome::None);
            }
            if bulk {
                domain.complete_batch(&targets)?;
            } else {
                domain.complete(targets[0])?;
            }
        }
        BoardIntent::Reopen => {
            model.close_popup();
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !domain.change_status(&targets, HumanStatus::Open, bulk)? {
                return Ok(IntentOutcome::None);
            }
        }
        BoardIntent::SoftDelete => {
            model.close_popup();
            // On the BLOCKED section a selected reply is the target, never the task: yours
            // soft-deletes to a stub, an agent's refuses.
            if model.input_mode == BoardInputMode::TaskPage {
                if super::block::selected_own_reply(model).is_some() {
                    model.pending_delete = None;
                    model.clear_marks();
                    return super::block::delete_selected_reply(domain, model);
                }
                if super::block::agent_reply_selected(model) {
                    model.pending_delete = None;
                    model.set_message("agent replies cannot be deleted");
                    return Ok(IntentOutcome::None);
                }
            }
            // In a new-step row the delete verb discards that draft row, as Esc does. It
            // never reaches the step the row was opened below, nor the task.
            if model.input_mode == BoardInputMode::EditStep
                && model
                    .form
                    .as_ref()
                    .and_then(|form| form.steps.editor.as_ref())
                    .is_some_and(|editor| editor.rename.is_none())
            {
                let capture = model.form.as_ref().is_some_and(|form| !form.is_task());
                close_step_editor(model);
                if capture {
                    if let Some(form) = model.form.as_ref() {
                        model.input_mode = form.parent_mode();
                    }
                }
                return Ok(IntentOutcome::None);
            }
            // On the page with the step cursor active, the delete verb is the
            // steps's mark-then-confirm: the first press visibly marks the
            // highlighted step, a second press removes it, and the task-level soft
            // delete below never runs.
            match page_step_delete(domain, model)? {
                PageStepDelete::Marked | PageStepDelete::Staged => {
                    model.clear_marks();
                    return Ok(IntentOutcome::None);
                }
                PageStepDelete::Removed => {
                    model.clear_marks();
                }
                PageStepDelete::NotApplicable => {
                    let pending = model.pending_delete.clone();
                    let bulk = if pending.is_some() {
                        model.pending_delete_bulk
                    } else {
                        model.task_list_owns_input()
                            && model.mark_mode_active()
                            && model.marked_count() > 0
                    };
                    let targets = pending
                        .as_ref()
                        .map(|targets| targets.iter().copied().collect())
                        .unwrap_or_else(|| model.verb_target_ids());
                    if targets.is_empty() {
                        model.set_message(NO_SELECTION);
                        return Ok(IntentOutcome::None);
                    }
                    if pending.is_none() {
                        model.pending_delete = Some(targets.iter().copied().collect());
                        model.pending_delete_bulk = bulk;
                        model.clear_marks();
                        if bulk {
                            let noun = if targets.len() == 1 { "task" } else { "tasks" };
                            model.set_message(format!(
                                "press ctrl+x again to delete {} {noun}",
                                targets.len()
                            ));
                        } else {
                            model.set_message("press ctrl+x again to delete");
                        }
                        return Ok(IntentOutcome::None);
                    }
                    model.pending_delete = None;
                    model.pending_delete_bulk = false;
                    model.clear_marks();
                    // Read the title before the delete, and only arm the notice once the delete
                    // itself succeeded: a refused delete has nothing to recover from.
                    // A numbered task is named by its `T` number too; a notice task (`N…`)
                    // keeps its title alone.
                    let named = targets.first().and_then(|id| domain.get(*id)).map(|task| {
                        let label = task
                            .notice
                            .is_none()
                            .then(|| task.board_identifier())
                            .flatten();
                        (label, task.title.clone())
                    });
                    // A ready task the delete leaves waiting on nothing stays ready: say so.
                    let unwaited = domain.unwaited_by_delete(&targets);
                    if bulk {
                        domain.soft_delete_batch(&targets)?;
                        model.arm_bulk_delete_notice(targets.len());
                    } else {
                        let id = targets[0];
                        domain.soft_delete(id)?;
                        if let Some((label, title)) = named {
                            model.arm_delete_notice(label.as_deref(), &title);
                        }
                    }
                    if !unwaited.is_empty() {
                        let waiting = unwaited
                            .iter()
                            .map(|(waiting, _)| format!("T{waiting}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let mut deleted: Vec<u64> =
                            unwaited.iter().map(|(_, deleted)| *deleted).collect();
                        deleted.sort_unstable();
                        deleted.dedup();
                        let deleted = deleted
                            .iter()
                            .map(|number| format!("T{number}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        model.set_message(format!("{waiting} no longer waits ({deleted} deleted)"));
                    }
                    model.undo_hint = domain.last_undo().cloned();
                    // Deleting from the page deletes the page's own task: the surface closes and
                    // the undo route back to it lives on the board row, same as the notice says.
                    // Focus returns to the list the same way `Esc` would.
                    if !bulk
                        && model
                            .form
                            .as_ref()
                            .filter(|form| form.is_task())
                            .and_then(BoardForm::task_id)
                            == Some(targets[0])
                    {
                        model.form = None;
                        model.input_mode = BoardInputMode::Normal;
                        model.wide_stage = WideStage::Board;
                    }
                }
            }
        }
        BoardIntent::LaunchUnarchive => {
            let Some(path) = model.launch_card.as_ref().map(|card| card.path.clone()) else {
                return Ok(IntentOutcome::None);
            };
            let scope_path = path.to_string_lossy().into_owned();
            domain.unarchive_project(&scope_path)?;
            // A failed save lands in Save Recovery via the existing boundary. The restored
            // project opens the way a launch inside its repository does, so quick-add
            // targets it.
            model.launch_card = None;
            model.popup = BoardPopup::None;
            model.sync_from_domain(domain);
            model.focus_project_group(&path);
            model.clear_message();
        }
        BoardIntent::LaunchKeepArchived => {
            let Some(path) = model.launch_card.take().map(|card| card.path) else {
                return Ok(IntentOutcome::None);
            };
            model.popup = BoardPopup::None;
            model.session_default_scope = Some(TaskScope::Global);
            let lossy = path.to_string_lossy().into_owned();
            let name = crate::ui::render::short_project(&lossy);
            model.set_message(format!(
                "project {name} is archived · quick-add goes to your desk this session"
            ));
            // Session-only change: nothing durable to write.
            return Ok(IntentOutcome::None);
        }
        BoardIntent::File => {
            model.close_popup();
            let (targets, bulk) = take_verb_targets(model);
            if targets.is_empty() {
                model.set_message(NO_SELECTION);
                return Ok(IntentOutcome::None);
            }
            if !domain.file_tasks(&targets, bulk)? {
                return Ok(IntentOutcome::None);
            }
            // Success has no message: disappearing rows are the feedback.
        }
        BoardIntent::Undo => {
            model.clear_marks();
            model.close_popup();
            // ctrl+u right after an archive undoes it. Otherwise ctrl+u on an archived
            // selection is the unarchive route, never an undo: the stack is not popped (AC-3).
            // On a focused page that is the page's own task, which a relation open may have
            // reached outside the list.
            let archive_on_top = domain.top_undo_is_live()
                && domain.last_undo().is_some_and(|entry| {
                    entry
                        .leaf_entries()
                        .iter()
                        .all(|leaf| matches!(leaf, UndoEntry::Archive { .. }))
                });
            if let Some(id) = model.action_target_id().filter(|_| !archive_on_top) {
                if domain.get(id).is_some_and(|task| task.archived) {
                    domain.unarchive_task(id)?;
                    let name = task_names(domain, &[id]);
                    model.sync_from_domain(domain);
                    model.set_message(format!("{name} unarchived"));
                    return Ok(IntentOutcome::Persist);
                }
            }
            let Some(entry) = domain.last_undo().cloned() else {
                model.set_message(NOTHING_TO_UNDO);
                return Ok(IntentOutcome::None);
            };
            let note = UndoNote::read(domain, &entry);
            if let Err(error) = domain.undo() {
                if let DomainError::StaleUndo(id) = error {
                    model.sync_from_domain(domain);
                    // Nothing moved, so this reports nothing to persist -- which is what
                    // puts the way back on the row beside the refusal that explains why
                    // this attempt did not take.
                    model.set_message(stale_undo_message(domain, id));
                    return Ok(IntentOutcome::None);
                }
                return Err(error);
            }
            model.sync_from_domain(domain);
            if let Some(note) = note {
                model.set_message(note.message(domain));
            }
            return Ok(IntentOutcome::Persist);
        }
    }

    model.sync_from_domain(domain);
    Ok(IntentOutcome::Persist)
}

/// Apply one draft operation, but only while a field edit is actually open.
///
/// Every editing intent is inert in every other mode, exactly as the insert and backspace
/// intents already were before the cursor arrived.
fn edit_draft(model: &mut BoardModel, operation: impl FnOnce(&mut EditBuffer)) {
    if model.input_mode == BoardInputMode::FormDropdown {
        return;
    }
    // The steps step editor owns the keyboard in its mode: its draft is the page
    // form's steps editor buffer, not the task form's title/notes fields. Any
    // edit-draft intent takes the line's refusal down (AC-13) — including a cursor
    // move that changes no text — the same lifetime quick-add's message follows.
    if model.input_mode == BoardInputMode::EditStep {
        if let Some(editor) = model
            .form
            .as_mut()
            .and_then(|form| form.steps.editor.as_mut())
        {
            editor.refusal = None;
            operation(&mut editor.buffer);
        }
        return;
    }
    if model.input_mode() == BoardInputMode::BlockCard {
        if let Some(buffer) = model
            .block_card
            .as_mut()
            .and_then(|card| card.focused_buffer_mut())
        {
            operation(buffer);
        }
        return;
    }
    if model.input_mode == BoardInputMode::EditReply {
        if let Some(buffer) = super::block::reply_buffer_mut(model) {
            operation(buffer);
        }
        return;
    }
    let Some(form) = model.form.as_mut() else {
        return;
    };
    form.manual_page_scroll = false;
    match form.focus {
        CaptureField::Title => operation(&mut form.title),
        CaptureField::Notes => operation(&mut form.notes),
        CaptureField::Scope | CaptureField::Assignee | CaptureField::Base | CaptureField::After => {
        }
    }
}

/// Move input focus from the list to the detail column (wide frames only). The detail binds
/// (or rebinds) its page to the selected task on the way in.
fn focus_detail(domain: &DomainState, model: &mut BoardModel) {
    if model.wide_stage == WideStage::Task {
        return;
    }
    // A disk merge can remove the selected task; ask the domain, not the pin.
    if model
        .selected_id()
        .is_some_and(|id| domain.get(id).is_some())
    {
        bind_selected_task_page(domain, model);
        model.wide_stage = WideStage::Task;
    }
}

/// Move input focus from the detail column back to the list. The page session is parked,
/// never dropped: a dirty draft stays bound to its task and keeps refusing a retarget.
fn focus_list(model: &mut BoardModel) {
    if model.wide_stage == WideStage::Board {
        return;
    }
    model.wide_stage = WideStage::Board;
    model.page_back.clear();
    model.clear_message();
}

/// Give the task side input ownership when a task-side action (a field edit from the list)
/// needs it.
fn enter_task_stage(model: &mut BoardModel) {
    model.wide_stage = WideStage::Task;
}

/// Close the focused task page and return focus to the list. A wide frame keeps the page
/// parked so the detail column still shows it; a narrow frame closes a clean page.
fn leave_task_page(model: &mut BoardModel) {
    model.wide_stage = WideStage::Board;
    model.page_back.clear();
    if !model.frame_wide() && !model.task_session_dirty() {
        model.form = None;
        model.input_mode = if model.quick_add.is_some() {
            BoardInputMode::QuickAdd
        } else {
            BoardInputMode::Normal
        };
    }
    model.clear_message();
}

/// The task `P` / `U` act on: the focused task page's own task, in view mode only. A dirty
/// edit session refuses on the status row and keeps its draft; every other surface is inert.
fn parent_verb_target(model: &mut BoardModel) -> Option<Uuid> {
    if model.focused_surface() != FocusedSurface::Task
        || model.input_mode != BoardInputMode::TaskPage
    {
        return None;
    }
    let task = model
        .form
        .as_ref()
        .filter(|form| form.is_task())
        .and_then(BoardForm::task_id)?;
    if model.task_session_dirty() {
        model.set_message(PARENT_NEEDS_CLEAN_PAGE);
        return None;
    }
    Some(task)
}

/// Make sure the task page is bound to the selected task before the task side owns input.
fn bind_selected_task_page(domain: &DomainState, model: &mut BoardModel) {
    // `BoardModel::input_mode()` reports Normal while a task form is parked, so every
    // refocus route must restore the raw TaskPage mode before task input can dispatch.
    let Some(id) = model.selected_id() else {
        return;
    };
    if model.edit_target() != Some(id) || model.input_mode != BoardInputMode::TaskPage {
        open_task_page_on(domain, model, id);
    }
}

/// Open the task page on `id` and give it input focus. Shared by the keyboard route
/// (`Enter`) and the mouse route (a row double-click).
fn open_focused_task_page(domain: &DomainState, model: &mut BoardModel, id: Uuid) {
    if domain.get(id).is_none() {
        return;
    }
    if model.edit_target() != Some(id) || model.input_mode != BoardInputMode::TaskPage {
        open_task_page_on(domain, model, id);
    }
    model.wide_stage = WideStage::Task;
}

/// `J` / `K` on the focused task page: open the next or previous task in the list's own
/// order, skipping group headers. A dirty draft refuses, like every other retarget.
fn step_task_page(domain: &DomainState, model: &mut BoardModel, forward: bool) {
    if model.focused_surface() != FocusedSurface::Task {
        return;
    }
    let Some(current) = model
        .form
        .as_ref()
        .filter(|form| form.is_task())
        .and_then(BoardForm::task_id)
    else {
        return;
    };
    if model.task_session_dirty() {
        model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
        return;
    }
    let rows: Vec<Uuid> = model
        .visible_ids()
        .into_iter()
        .filter(|id| *id != ARCHIVED_HEADER_ROW_ID && *id != INBOX_HEADER_ROW_ID)
        .collect();
    let Some(index) = rows.iter().position(|id| *id == current) else {
        model.set_message(TASK_NOT_IN_LIST);
        return;
    };
    let next = if forward {
        rows.get(index + 1)
    } else {
        index.checked_sub(1).and_then(|i| rows.get(i))
    };
    let Some(&next) = next else {
        model.set_message(if forward { LAST_TASK } else { FIRST_TASK });
        return;
    };
    model.selection_id = Some(next);
    open_task_page_on(domain, model, next);
    // A successful list step ends the relation trail; the refusals above keep it.
    model.page_back.clear();
    model.follow_list.set(true);
}

/// Open a related task (a parent or a direct child) on the focused page. The list keeps its
/// lens, search, sort, and cursor: the selection follows only when the list paints the
/// target. The page left behind is retained for `Esc`.
fn open_related_task(domain: &DomainState, model: &mut BoardModel, id: Uuid) {
    let Some(current) = model.focused_page_task() else {
        return;
    };
    if model.task_session_dirty() {
        model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
        return;
    }
    if model.input_mode != BoardInputMode::TaskPage || model.task_editing() {
        return;
    }
    if domain.get(id).is_none_or(|task| task.soft_deleted) {
        model.set_message(related_not_found(id));
        return;
    }
    model.page_back.push(current);
    open_task_page_on(domain, model, id);
    if model.visible_ids().contains(&id) {
        model.selection_id = Some(id);
        model.follow_list.set(true);
    }
}

/// `Esc` on a clean page after a relation open: restore the newest retained page that still
/// exists, skipping missing or deleted ones. Returns false when none is left.
fn restore_previous_page(domain: &DomainState, model: &mut BoardModel) -> bool {
    while let Some(id) = model.page_back.pop() {
        if domain.get(id).is_some_and(|task| !task.soft_deleted) {
            open_task_page_on(domain, model, id);
            if model.visible_ids().contains(&id) {
                model.selection_id = Some(id);
                model.follow_list.set(true);
            }
            return true;
        }
    }
    false
}

/// A relation whose task is missing or deleted: the link stays as stored.
fn related_not_found(id: Uuid) -> String {
    format!("{id} not found \u{b7} link kept")
}

fn open_task_page_on(domain: &DomainState, model: &mut BoardModel, id: Uuid) {
    let Some(task) = domain.get(id) else {
        return;
    };
    model.close_popup();
    model.close_command_surface();
    model.close_help();
    let form = BoardForm::task(
        task,
        model.this_repo.as_deref(),
        &model.tasks,
        CaptureField::Title,
        &model.archived_projects,
        &model.agent_names,
    );
    model.form = Some(form);
    model.input_mode = BoardInputMode::TaskPage;
    model.clear_message();
}

/// Leave the questions tab for one task's page (`t` on a question). A clean `Esc` on that
/// page comes back to `question`. A dirty board edit refuses and keeps the questions tab.
pub(crate) fn open_task_from_questions(
    domain: &DomainState,
    model: &mut BoardModel,
    task: Uuid,
    question: Option<Uuid>,
) {
    if model.task_session_dirty() {
        model.set_message(DIRTY_TASK_SWITCH_REFUSAL);
        return;
    }
    if domain.get(task).is_none() {
        if let Some(moved) = model.questions.moved.get(&task) {
            let label = moved.board_identifier().unwrap_or_default();
            model.set_message(format!(
                "{label} moved to done history \u{b7} read it with tsk list {label}"
            ));
            return;
        }
    }
    if domain.get(task).is_none_or(|found| found.soft_deleted) {
        model.set_message(related_not_found(task));
        return;
    }
    model.questions.active = false;
    model.page_back.clear();
    if model.visible_ids().contains(&task) {
        model.selection_id = Some(task);
        model.follow_list.set(true);
    }
    open_focused_task_page(domain, model, task);
    model.questions.return_question = question;
}

/// A question line on a task page: open it on the questions tab. `Esc` on the question
/// returns to this task page, which stays parked meanwhile.
pub(crate) fn open_question_from_task(model: &mut BoardModel, question: Uuid) {
    if model.questions.feed.get(question).is_none() {
        model.set_message("that question is no longer readable");
        return;
    }
    if refuse_answer_elsewhere(model, question) {
        return;
    }
    let task = model.focused_page_task();
    model.questions.active = true;
    model.questions.return_question = None;
    model.questions.open(question);
    model.questions.return_task = task;
    model.clear_message();
}

/// Opening another question would discard the answer being typed: refuse and say where it is.
pub(crate) fn refuse_answer_elsewhere(model: &mut BoardModel, question: Uuid) -> bool {
    let Some(other) = model.questions.answer_elsewhere(question) else {
        return false;
    };
    let name = model
        .questions
        .feed
        .get(other)
        .map(|record| format!("Q{}", record.reference.number))
        .unwrap_or_else(|| "another question".to_string());
    model.set_message(format!(
        "finish the answer on {name} first · enter saves · esc cancels"
    ));
    true
}

/// A clean `Esc` on a task page opened from a question: back to that question. Returns
/// whether it handled the `Esc`.
pub(crate) fn return_to_question(model: &mut BoardModel) -> bool {
    let Some(question) = model.questions.return_question else {
        return false;
    };
    if model.input_mode() != BoardInputMode::TaskPage
        || model.task_session_dirty()
        || model.task_editing()
        || !model.page_back.is_empty()
    {
        return false;
    }
    model.questions.return_question = None;
    leave_task_page(model);
    model.questions.active = true;
    if model.questions.feed.get(question).is_some() {
        model.questions.open(question);
    }
    true
}

/// The row double-click window: a second click on the same row within it opens the page.
pub const ROW_DOUBLE_CLICK_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);

/// Keep the quick-add line's visible destination truthful while `!p` tokens are typed.
///
/// The row names where Enter will save (`Add to desk` / `Add to <project>`), so every
/// buffer change re-lifts the tokens: an override applies the moment it is typed and
/// reverts the moment it is deleted. An invalid `!p` leaves the last good destination
/// painted; the save itself refuses with the same words.
fn refresh_quick_add_scope(model: &mut BoardModel, domain: &DomainState) {
    let Some(quick_add) = model.quick_add.as_ref() else {
        return;
    };
    let default = quick_add.default.clone();
    let lifted = crate::capture::lift_quick_add_tokens(
        quick_add.title.value(),
        domain,
        quick_add.snapshot.as_ref().as_ref(),
        &quick_add.default,
        &model.agent_names,
    );
    let Ok(lifted) = lifted else {
        return;
    };
    if let Some(quick_add) = model.quick_add.as_mut() {
        quick_add.scope = lifted.scope.unwrap_or(default);
    }
}

/// Save the line through the same capture pipeline the expanded form uses.
fn quick_add_save(
    domain: &mut DomainState,
    model: &mut BoardModel,
    keep_open: bool,
) -> Result<IntentOutcome, DomainError> {
    let Some(quick_add) = model.quick_add.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let Some(snapshot) = quick_add.snapshot.as_ref().clone() else {
        model.set_message("capture context unavailable; press Esc and try again");
        return Ok(IntentOutcome::None);
    };
    let lifted = match crate::capture::lift_quick_add_tokens(
        quick_add.title.value(),
        domain,
        quick_add.snapshot.as_ref().as_ref(),
        &quick_add.default,
        &model.agent_names,
    ) {
        Ok(lifted) => lifted,
        Err(message) => {
            model.set_message(message);
            return Ok(IntentOutcome::None);
        }
    };
    let scope = lifted.scope.unwrap_or_else(|| quick_add.scope.clone());
    match crate::capture::capture_save_configured(
        domain,
        None,
        &snapshot,
        lifted.title,
        None,
        Some(scope.clone()),
        None,
        lifted.assignee,
        lifted.base,
    ) {
        Ok(id) => {
            domain.set_after_on_create(id, &lifted.after)?;
            // Do not discard the draft until the app save boundary confirms persistence. A
            // failed save keeps this exact state behind SaveRecovery for retry or cancel.
            model.form = None;
            model.begin_quick_add_save(id, keep_open);
            Ok(IntentOutcome::Persist)
        }
        Err(crate::capture::CaptureError::Domain(DomainError::EmptyTitle)) => {
            model.set_message(TITLE_REQUIRED_MESSAGE);
            Ok(IntentOutcome::None)
        }
        Err(error) => {
            model.set_message(error.to_string());
            Ok(IntentOutcome::None)
        }
    }
}

/// `Enter` on the task page's BLOCKED or REVIEW top line opens the waiting-on picker for that
/// task; palette **set waiting on…** opens it for the selected task or the marked set. Every
/// target needs an open block or review: one without refuses the whole set.
fn open_on_picker(domain: &DomainState, model: &mut BoardModel) {
    let task_page = model.input_mode == BoardInputMode::TaskPage
        && model.form.as_ref().is_some_and(BoardForm::is_task);
    if !(task_page || model.input_mode == BoardInputMode::Normal) {
        return;
    }
    // The page acts on its own task; the board on the selection or the marked set.
    let ids: Vec<Uuid> = if task_page {
        model
            .form
            .as_ref()
            .and_then(BoardForm::task_id)
            .into_iter()
            .collect()
    } else {
        model.verb_target_ids()
    }
    .into_iter()
    .filter(|id| domain.get(*id).is_some_and(|task| !task.is_notice()))
    .collect();
    if ids.is_empty() {
        model.set_message(NO_SELECTION);
        return;
    }
    let mut records = Vec::with_capacity(ids.len());
    for id in &ids {
        let Some(task) = domain.get(*id) else {
            continue;
        };
        match task.block.as_ref() {
            Some(block) => records.push((*id, block.key(), block.kind)),
            None => {
                model.set_message(no_open_record(task));
                return;
            }
        }
    }
    let current = match records.as_slice() {
        [(id, _, _)] => domain
            .get(*id)
            .and_then(|task| task.block.as_ref())
            .map(|block| block.on.clone()),
        _ => None,
    };
    model.clear_message();
    model.open_on_picker(OnPickerTarget { records }, current);
}

/// `ctrl+x` in the assignee picker: take the person under the cursor off the list. Tasks keep
/// the name; assigning it again puts it back. Profiles, `@you`, and the fixed rows refuse.
fn remove_assignee_name(domain: &mut DomainState, model: &mut BoardModel) -> IntentOutcome {
    if model.list_picker_kind() != Some(crate::ui::board::ListPickerKind::Assignee) {
        return IntentOutcome::None;
    }
    let Some(name) = model.assignee_picker_person() else {
        let agent = matches!(
            model.selected_list_picker_option(),
            Some((_, option)) if matches!(
                &option.value,
                ListPickerValue::Assignee(Some(assignee)) if !assignee.is_person()
            )
        );
        model.set_message(if agent {
            "agents are removed in config.toml"
        } else {
            "only a person's name can be removed"
        });
        return IntentOutcome::None;
    };
    if !domain.hide_assignee(&name) {
        return IntentOutcome::None;
    }
    model.sync_from_domain(domain);
    model.refresh_assignee_picker();
    model.set_message(format!(
        "{} removed from the list",
        crate::domain::display_name(&name)
    ));
    IntentOutcome::Persist
}

/// `ctrl+u` in the assignee picker: undo a name removal on top of the undo stack and show the
/// name again. Any other undo waits until the picker closes.
fn undo_assignee_name(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<IntentOutcome, DomainError> {
    if model.list_picker_kind() != Some(crate::ui::board::ListPickerKind::Assignee) {
        return Ok(IntentOutcome::None);
    }
    let Some(entry @ UndoEntry::HideAssignee { .. }) = domain.last_undo().cloned() else {
        model.set_message("nothing to undo here");
        return Ok(IntentOutcome::None);
    };
    let note = UndoNote::read(domain, &entry);
    domain.undo()?;
    model.sync_from_domain(domain);
    model.refresh_assignee_picker();
    if let Some(note) = note {
        model.set_message(note.message(domain));
    }
    Ok(IntentOutcome::Persist)
}

/// The refusal for a task with no open block or review to change.
fn no_open_record(task: &Task) -> String {
    match task.number {
        Some(number) => format!("T{number} has no open block or review"),
        None => "no open block or review".to_string(),
    }
}

/// Enter on the waiting-on picker: change who every target's open record waits on, in place
/// and as one undo entry. A task must be live, and a block never waits on itself. A record
/// closed or replaced since the picker opened refuses the whole set.
fn confirm_on_picker(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<IntentOutcome, DomainError> {
    let Some((_, option)) = model.selected_list_picker_option() else {
        return Ok(IntentOutcome::None);
    };
    let on = match option.value {
        ListPickerValue::WaitingOn(on) => on,
        ListPickerValue::TypedName => match model.typed_picker_value() {
            Some(Ok(ListPickerValue::WaitingOn(on))) => on,
            _ => {
                model.set_message(TYPE_A_NAME_FIRST);
                return Ok(IntentOutcome::None);
            }
        },
        _ => return Ok(IntentOutcome::None),
    };
    let on = match on {
        crate::domain::BlockOn::Other(text) => {
            match crate::domain::block_text(Some(&text), crate::domain::BlockField::On) {
                Ok(Some(text)) => crate::domain::BlockOn::Other(text),
                Ok(None) => crate::domain::BlockOn::You,
                Err(field) => {
                    model.set_message(DomainError::TextTooLong(field).to_string());
                    return Ok(IntentOutcome::None);
                }
            }
        }
        on => on,
    };
    let Some(target) = model
        .list_picker
        .as_ref()
        .and_then(|picker| picker.on_target.clone())
    else {
        return Ok(IntentOutcome::None);
    };
    if let crate::domain::BlockOn::Task(number) = on {
        let found = domain
            .tasks()
            .iter()
            .find(|task| task.number == Some(number) && !task.soft_deleted && !task.is_notice());
        match found {
            Some(task) if target.records.iter().any(|(id, _, _)| *id == task.id) => {
                model.set_message(format!("T{number} cannot wait on itself"));
                return Ok(IntentOutcome::None);
            }
            Some(_) => {}
            None => {
                model.set_message(format!("T{number} is not on the board"));
                return Ok(IntentOutcome::None);
            }
        }
    }
    let changed_elsewhere = target.records.iter().any(|(id, key, _)| {
        domain
            .get(*id)
            .and_then(|task| task.block.as_ref())
            .map(crate::domain::Block::key)
            .as_ref()
            != Some(key)
    });
    model.close_on_picker();
    if changed_elsewhere {
        model.set_message("that block or review changed elsewhere");
        return Ok(IntentOutcome::None);
    }
    let changes: Vec<(Uuid, crate::domain::BlockOn)> = target
        .records
        .iter()
        .map(|(id, _, _)| (*id, on.clone()))
        .collect();
    let changed = domain.set_on_batch(&changes)?;
    model.clear_marks();
    if !changed {
        return Ok(IntentOutcome::None);
    }
    let subject = match target.records.as_slice() {
        [(id, _, _)] => domain
            .get(*id)
            .and_then(|task| task.number)
            .map_or_else(|| "task".to_string(), |number| format!("T{number}")),
        records => format!("{} tasks", records.len()),
    };
    model.set_message(format!(
        "set {subject} waiting {}",
        crate::activity::on_text(&on)
    ));
    Ok(IntentOutcome::Persist)
}

fn confirm_edit(
    domain: &mut DomainState,
    model: &mut BoardModel,
    extra_step: Option<(&str, Option<Uuid>)>,
) -> Result<IntentOutcome, DomainError> {
    // The task bound at open, not `model.selected_id()`: refresh may move the visible pin, but
    // it never changes the form's immutable id or any of its three drafts.
    let Some(form) = model.form.as_ref().filter(|form| form.is_task()) else {
        return Ok(IntentOutcome::None);
    };
    let id = form.task_id().expect("task form has immutable id");
    let title = form.title.value().trim().to_string();
    let notes = (!form.notes.value().trim().is_empty()).then(|| form.notes.value().to_string());
    let scope = form.scope.clone();
    let assignee = form.assignee.clone();
    let base = form.base.clone();
    let after = form.after.clone();
    let step_renames = form
        .steps
        .drafts
        .iter()
        .filter(|(step_id, _)| !form.steps.removals.contains(step_id))
        .map(|(step_id, draft)| (*step_id, draft.value().trim().to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let step_removals = form.steps.removals.clone();
    let task = domain.get(id).ok_or(DomainError::UnknownId(id))?.clone();
    // An existing step emptied in this session is refused the way a new empty step is: on
    // its own row, with its editor open, never by a status-row message beside a blank row.
    let emptied = task.steps.iter().enumerate().find(|(_, step)| {
        step_renames
            .get(&step.id)
            .is_some_and(|text| text.is_empty())
    });
    if let Some((index, step)) = emptied {
        refuse_empty_step_rename(model, index, step.id);
        return Ok(IntentOutcome::None);
    }
    let selected_step = form
        .steps
        .cursor
        .and_then(|source_index| task.steps.get(source_index))
        .map(|step| step.id);
    // `DomainState::edit` does not reject a soft-deleted task itself. Refuse before touching
    // the form so's bound-task and draft-recovery guarantees remain intact.
    if task.soft_deleted {
        return Err(DomainError::SoftDeleted(id));
    }
    if let Some(branch) = base
        .as_deref()
        .filter(|_| base != task.base || scope != task.scope)
    {
        if let Err(message) = crate::git_base::validate_task_base(&scope, branch, false) {
            model.set_message(message);
            return Ok(IntentOutcome::None);
        }
    }

    // Refused before anything changes: the draft stays for another choice.
    if after != task.after {
        if let Err(error) = domain.check_after(id, &after) {
            model.set_message(error.to_string());
            return Ok(IntentOutcome::None);
        }
    }

    // The full page session changes under one revision, then the app persists exactly once.
    // A chained `edit` plus `rename_step` sequence would advance the merge base after each
    // draft and make the store correctly refuse the later revision as a conflicting writer.
    let step_rename_list = step_renames
        .iter()
        .map(|(step_id, text)| (*step_id, text.clone()))
        .collect::<Vec<_>>();
    let extra_steps = extra_step
        .map(|(text, _)| vec![text.trim().to_string()])
        .unwrap_or_default();
    let extra_after = extra_step.and_then(|(_, insert_after)| insert_after);
    // Only the steps part of the save is undoable. Tasks carry no thread label on the board;
    // the stored value passes through unchanged.
    let thread = domain.get(id).and_then(|task| task.thread.clone());
    domain.with_steps_undo(id, |domain| {
        domain.edit_with_step_changes_and_base(
            id,
            &title,
            notes.clone(),
            scope.clone(),
            thread,
            assignee.clone(),
            base.clone(),
            &step_rename_list,
            &step_removals.iter().copied().collect::<Vec<_>>(),
            &extra_steps,
            extra_after,
        )?;
        domain.stage_after_in_edit(id, &after)
    })?;

    // Retain the complete form and mode until the persistence boundary confirms this exact
    // task-session save. A failed save can then Retry or Cancel without orphaning drafts.
    model.task_edit_save = Some(TaskEditSave {
        id,
        title,
        notes,
        scope,
        assignee,
        base,
        after,
        step_renames,
        step_removals,
        selected_step,
    });
    Ok(IntentOutcome::Persist)
}

// ---------------------------------------------------------------------------
// Steps step cursor, verbs, and one-line editor (T-3)
// ---------------------------------------------------------------------------

/// Move from the form's end fields into its existing-step group or trailing add target.
fn select_step_from_tab(model: &mut BoardModel, forward: bool) {
    let target = model.form.as_ref().and_then(|form| {
        if !form.is_task()
            || !form.editing
            || (forward && form.focus != CaptureField::Notes)
            || (!forward && form.focus != CaptureField::Scope)
        {
            None
        } else {
            let task_id = form.task_id()?;
            let visible: Vec<usize> = model
                .tasks
                .iter()
                .find(|task| task.id == task_id)?
                .steps
                .iter()
                .enumerate()
                .filter(|(_, step)| !form.steps.removals.contains(&step.id))
                .map(|(source_index, _)| source_index)
                .collect();
            Some(if forward {
                visible.first().copied()
            } else {
                visible.last().copied()
            })
        }
    });
    let Some(target) = target else {
        return;
    };
    let Some(index) = target else {
        select_add_step(model);
        return;
    };
    let text = model.form.as_ref().and_then(|form| {
        form.task_id().and_then(|task_id| {
            model
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .and_then(|task| {
                    task.steps
                        .get(index)
                        .map(|step| (step.id, step.text.clone()))
                })
        })
    });
    if let Some((step_id, text)) = text {
        if let Some(form) = model.form.as_mut() {
            form.steps.cursor = Some(index);
            form.steps.add_selected = false;
            steps_scroll_to_cursor(form, index);
        }
        open_step_editor(model, &text, Some(step_id));
    }
}

/// Start a task-page Tab cycle on its first stored step, or its trailing add target when
/// the checklist is empty, without entering the task edit session.
fn select_first_step_from_page(model: &mut BoardModel) -> bool {
    let count = model.form.as_ref().and_then(|form| {
        let task_id = form.task_id()?;
        model
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| task.steps.len())
    });
    let Some(count) = count else {
        return false;
    };
    if count == 0 {
        select_add_step(model);
    } else if let Some(form) = model.form.as_mut().filter(|form| form.is_task()) {
        form.steps.cursor = Some(0);
        form.steps.add_selected = false;
        steps_scroll_to_cursor(form, 0);
    }
    true
}

/// Shift+Tab wraps to the PAPER TRAIL's last record or the last relation when no trail exists.
fn select_last_page_stop(model: &mut BoardModel) {
    if !super::block::enter_trail(model, false) {
        select_ring_end(model);
    }
}

/// Move Tab within the PAPER TRAIL when one of its records is selected; `None` otherwise.
fn trail_tab(model: &mut BoardModel, forward: bool) -> Option<super::block::PageTab> {
    match super::block::move_trail_tab(model, forward) {
        super::block::PageTab::NotHandled => None,
        moved => Some(moved),
    }
}

/// One view-mode page selection: the parent line, a stored step, `+ step`, or a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageRingSlot {
    Relation(usize),
    Step(usize),
    Add,
}

/// The view-mode ring in page order: parent, steps, `+ step`, children. Empty without a
/// focused task page.
fn page_ring(model: &BoardModel) -> Vec<PageRingSlot> {
    let Some(task) = model
        .focused_page_task()
        .and_then(|id| model.tasks.iter().find(|task| task.id == id))
    else {
        return Vec::new();
    };
    let relations = model.relation_targets().len();
    let mut ring = Vec::with_capacity(relations + task.steps.len() + 1);
    if relations > 0 {
        ring.push(PageRingSlot::Relation(0));
    }
    ring.extend((0..task.steps.len()).map(PageRingSlot::Step));
    ring.push(PageRingSlot::Add);
    ring.extend((1..relations).map(PageRingSlot::Relation));
    ring
}

fn at_page_ring_edge(model: &BoardModel, forward: bool) -> bool {
    let Some(form) = model.form.as_ref().filter(|form| form.is_task()) else {
        return false;
    };
    let ring = page_ring(model);
    let edge = if forward { ring.last() } else { ring.first() };
    match edge {
        Some(PageRingSlot::Relation(index)) => form.steps.relation_cursor == Some(*index),
        Some(PageRingSlot::Step(index)) => form.steps.cursor == Some(*index),
        Some(PageRingSlot::Add) => form.steps.add_selected,
        None => false,
    }
}

fn select_first_page_stop(model: &mut BoardModel) -> bool {
    let Some(&slot) = page_ring(model).first() else {
        return false;
    };
    select_ring_slot(model, slot);
    true
}

fn select_ring_slot(model: &mut BoardModel, slot: PageRingSlot) {
    let Some(form) = model.form.as_mut().filter(|form| form.is_task()) else {
        return;
    };
    match slot {
        PageRingSlot::Relation(index) => {
            form.steps.cursor = None;
            form.steps.add_selected = false;
            form.steps.relation_cursor = Some(index);
            relation_scroll_to(form, index);
        }
        PageRingSlot::Step(index) => {
            form.steps.relation_cursor = None;
            form.steps.cursor = Some(index);
            form.steps.add_selected = false;
            steps_scroll_to_cursor(form, index);
        }
        PageRingSlot::Add => {
            form.steps.relation_cursor = None;
            select_add_step(model);
        }
    }
}

/// Move the view-mode page selection one slot around the ring (Tab / shift+Tab). Returns
/// false when nothing on the page is selected yet, so the caller keeps its entry route.
fn move_page_ring(model: &mut BoardModel, forward: bool) -> bool {
    let current = model
        .form
        .as_ref()
        .filter(|form| form.is_task())
        .and_then(|form| {
            if form.steps.add_selected {
                Some(PageRingSlot::Add)
            } else if let Some(index) = form.steps.cursor {
                Some(PageRingSlot::Step(index))
            } else {
                form.steps.relation_cursor.map(PageRingSlot::Relation)
            }
        });
    let Some(current) = current else {
        return false;
    };
    let ring = page_ring(model);
    let Some(position) = ring.iter().position(|slot| *slot == current) else {
        return false;
    };
    let next = if forward {
        (position + 1) % ring.len()
    } else {
        position.checked_sub(1).unwrap_or(ring.len() - 1)
    };
    select_ring_slot(model, ring[next]);
    true
}

/// Select the ring's last slot: shift+Tab's entry on a read-only page, where entering the
/// edit session is refused.
fn select_ring_end(model: &mut BoardModel) {
    if let Some(&slot) = page_ring(model).last() {
        select_ring_slot(model, slot);
    }
}

/// Move the view-mode selection one slot by arrow, without wrapping: ↑ from the first step
/// reaches the parent line, ↓ from the last step or `+ step` reaches the first child.
/// Returns false at either end, so the caller keeps its own end behaviour.
fn move_page_arrow(model: &mut BoardModel, forward: bool) -> bool {
    let current = model
        .form
        .as_ref()
        .filter(|form| form.is_task())
        .and_then(|form| {
            if form.steps.add_selected {
                Some(PageRingSlot::Add)
            } else if let Some(index) = form.steps.cursor {
                Some(PageRingSlot::Step(index))
            } else {
                form.steps.relation_cursor.map(PageRingSlot::Relation)
            }
        });
    let Some(current) = current else {
        return false;
    };
    // Arrows walk the stored rows: `+ step` is a Tab/click target, not an arrow stop.
    let ring: Vec<PageRingSlot> = page_ring(model)
        .into_iter()
        .filter(|slot| *slot != PageRingSlot::Add || current == PageRingSlot::Add)
        .collect();
    let Some(position) = ring.iter().position(|slot| *slot == current) else {
        return false;
    };
    let next = if forward {
        ring.get(position + 1)
    } else {
        position.checked_sub(1).and_then(|index| ring.get(index))
    };
    let Some(&next) = next else {
        return false;
    };
    select_ring_slot(model, next);
    true
}

/// Move within stored task-edit steps without wrapping, opening the reached row inline.
fn move_step_within_edit_group(model: &mut BoardModel, forward: bool) -> bool {
    let target = model.form.as_ref().and_then(|form| {
        if !form.is_task() || !form.editing {
            return None;
        }
        let index = form.steps.cursor?;
        let task_id = form.task_id()?;
        let task = model.tasks.iter().find(|task| task.id == task_id)?;
        let visible: Vec<usize> = task
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| !form.steps.removals.contains(&step.id))
            .map(|(source_index, _)| source_index)
            .collect();
        let position = visible
            .iter()
            .position(|source_index| *source_index == index)?;
        let target = if forward {
            *visible.get(position + 1)?
        } else {
            *visible.get(position.checked_sub(1)?)?
        };
        task.steps
            .get(target)
            .map(|step| (target, step.id, step.text.clone()))
    });
    let Some((target, step_id, text)) = target else {
        return false;
    };
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(target);
        form.steps.add_selected = false;
        steps_scroll_to_cursor(form, target);
    }
    open_step_editor(model, &text, Some(step_id));
    true
}

fn select_add_step(model: &mut BoardModel) {
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = None;
        form.steps.add_selected = true;
        model.input_mode = if form.is_task() {
            BoardInputMode::TaskPage
        } else {
            BoardInputMode::CapturePage
        };
    }
}

/// Expanded capture stages new steps locally, so its ring selects those rows without trying to
/// resolve task-backed step identities that do not exist until the capture is saved.
fn select_first_capture_step_or_add(model: &mut BoardModel) {
    let count = model
        .form
        .as_ref()
        .filter(|form| !form.is_task())
        .map(|form| form.steps.pending_adds.len())
        .unwrap_or(0);
    if count == 0 {
        select_add_step(model);
    } else if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(0);
        form.steps.add_selected = false;
        model.input_mode = BoardInputMode::CapturePage;
    }
}

fn select_last_capture_step(model: &mut BoardModel) -> bool {
    let count = model
        .form
        .as_ref()
        .filter(|form| !form.is_task())
        .map(|form| form.steps.pending_adds.len())
        .unwrap_or(0);
    let Some(index) = count.checked_sub(1) else {
        return false;
    };
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(index);
        form.steps.add_selected = false;
        model.input_mode = BoardInputMode::CapturePage;
    }
    true
}

/// Move among locally staged capture steps without wrapping. The caller handles the field at
/// either end of the group, matching task-page step traversal.
fn move_capture_step_with_tab(model: &mut BoardModel, forward: bool) -> bool {
    let state = model.form.as_ref().and_then(|form| {
        (!form.is_task()).then_some((form.steps.cursor, form.steps.pending_adds.len()))
    });
    let Some((Some(index), count)) = state else {
        return false;
    };
    if forward && index + 1 == count {
        select_add_step(model);
        return true;
    }
    if !forward && index == 0 {
        return false;
    }
    let next = if forward { index + 1 } else { index - 1 };
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(next);
        form.steps.add_selected = false;
        model.input_mode = BoardInputMode::CapturePage;
    }
    true
}

fn select_last_step_for_edit(model: &mut BoardModel) {
    let target = model.form.as_ref().and_then(|form| {
        let task = model
            .tasks
            .iter()
            .find(|task| Some(task.id) == form.task_id())?;
        task.steps
            .iter()
            .enumerate()
            .rev()
            .find(|(_, step)| !form.steps.removals.contains(&step.id))
            .map(|(source_index, step)| (source_index, step.id, step.text.clone()))
    });
    let Some((index, step_id, text)) = target else {
        model.focus_form_field(CaptureField::Notes);
        return;
    };
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(index);
        form.steps.add_selected = false;
        steps_scroll_to_cursor(form, index);
    }
    open_step_editor(model, &text, Some(step_id));
}

/// Resolve the page's selected step to live task and step ids, declining a stale index after
/// a domain change. Task view and task edit sessions share this selection.
fn selected_step(domain: &DomainState, model: &BoardModel) -> Option<(Uuid, Uuid)> {
    if model.focused_surface() != FocusedSurface::Task
        || !matches!(
            model.input_mode,
            BoardInputMode::TaskPage | BoardInputMode::EditStep
        )
    {
        return None;
    }
    let form = model.form.as_ref().filter(|form| form.is_task())?;
    if form.steps.add_selected {
        return None;
    }
    let task_id = form.task_id()?;
    let index = form.steps.cursor?;
    let step_id = domain.get(task_id)?.steps.get(index).map(|step| step.id)?;
    (!form.steps.removals.contains(&step_id)).then_some((task_id, step_id))
}

/// Outcome of routing the delete verb through the page's step cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageStepDelete {
    /// The cursor is not active on the page: the verb is the task soft delete.
    NotApplicable,
    /// The edit session owns this removal until its final save.
    Staged,
    /// First view-mode press: the cursor's step is visibly marked; nothing was removed.
    Marked,
    /// Second view-mode press: the marked step was removed through the domain command.
    Removed,
}

/// Mark-then-confirm delete for the step under the page's cursor (AC-11). Any
/// intervening intent already cleared the mark in [`apply_intent`], so a mark found
/// here equal to the cursor's step can only be this verb's own first press.
fn page_step_delete(
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<PageStepDelete, DomainError> {
    let Some((task_id, step_id)) = selected_step(domain, model) else {
        return Ok(PageStepDelete::NotApplicable);
    };
    let form = model
        .form
        .as_ref()
        .filter(|form| form.is_task())
        .expect("selected_step checked a task form");
    let index = form
        .steps
        .cursor
        .expect("selected_step checked an active cursor");
    if model.task_editing() {
        let task = domain
            .get(task_id)
            .expect("selected_step checked a live task");
        let form = model.form.as_mut().expect("task form checked above");
        form.steps.removals.insert(step_id);
        form.steps.drafts.remove(&step_id);
        form.steps.editor = None;
        form.steps.delete_mark = None;
        // Keep the selector on the nearest remaining stored row. Its source index remains
        // stable while the renderer filters staged removals, so it cannot point at the row
        // that just vanished or silently select a different draft.
        let remaining: Vec<usize> = task
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| !form.steps.removals.contains(&step.id))
            .map(|(source_index, _)| source_index)
            .collect();
        form.steps.cursor = remaining
            .iter()
            .copied()
            .find(|source_index| *source_index >= index)
            .or_else(|| remaining.last().copied());
        form.steps.add_selected = false;
        if let Some(cursor) = form.steps.cursor {
            steps_scroll_to_cursor(form, cursor);
        }
        model.input_mode = BoardInputMode::TaskPage;
        return Ok(PageStepDelete::Staged);
    }
    if form.steps.delete_mark != Some(index) {
        model
            .form
            .as_mut()
            .expect("task form checked above")
            .steps
            .delete_mark = Some(index);
        // AC-23: the footer's message slot carries the press-again hint for exactly
        // as long as the mark is armed — the removal press and every intervening
        // intent clear it with the mark. The verb's own modifier names the key, so
        model.set_message("press ctrl+x again to remove");
        return Ok(PageStepDelete::Marked);
    }
    domain.with_steps_undo(task_id, |domain| domain.remove_step(task_id, step_id))?;
    let len = domain
        .get(task_id)
        .map(|task| task.steps.len())
        .unwrap_or(0);
    let form = model
        .form
        .as_mut()
        .filter(|form| form.is_task())
        .expect("task form checked above");
    form.steps.delete_mark = None;
    if len == 0 {
        form.steps.cursor = None;
    } else {
        let cursor = form.steps.cursor.unwrap_or(0).min(len - 1);
        form.steps.cursor = Some(cursor);
        steps_scroll_to_cursor(form, cursor);
    }
    Ok(PageStepDelete::Removed)
}

/// Open an in-place step row seeded with `text`, renaming `step` when given and adding
/// a transient row when `None`.
fn open_step_editor(model: &mut BoardModel, text: &str, rename: Option<Uuid>) {
    if let Some(form) = model.form.as_mut() {
        form.steps.add_selected = false;
        if form
            .steps
            .editor
            .as_ref()
            .is_some_and(|editor| editor.rename == rename)
        {
            model.input_mode = BoardInputMode::EditStep;
            model.clear_message();
            return;
        }
        let buffer = rename
            .and_then(|step_id| form.steps.drafts.remove(&step_id))
            .unwrap_or_else(|| crate::ui::edit::seeded_draft(text));
        form.steps.editor = Some(StepEditor {
            buffer,
            rename,
            pending_index: None,
            insert_after: None,
            refusal: None,
        });
        model.input_mode = BoardInputMode::EditStep;
        model.clear_message();
    }
}

/// Reopen the emptied existing step at `index` with the inline `text required` refusal.
///
/// A typed new-step row that is open when the session save is refused would lose its text if
/// the emptied step took its place, so it stays open and the status row says what is wrong.
fn refuse_empty_step_rename(model: &mut BoardModel, index: usize, step_id: Uuid) {
    let typed_add_open = model.form.as_ref().is_some_and(|form| {
        form.steps.editor.as_ref().is_some_and(|editor| {
            editor.rename.is_none() && !editor.buffer.value().trim().is_empty()
        })
    });
    if typed_add_open {
        model.set_message(EMPTIED_STEP_REFUSAL);
        return;
    }
    if let Some(form) = model.form.as_mut() {
        form.steps.cursor = Some(index);
        steps_scroll_to_cursor(form, index);
    }
    open_step_editor(model, "", Some(step_id));
    if let Some(editor) = model
        .form
        .as_mut()
        .and_then(|form| form.steps.editor.as_mut())
    {
        editor.refusal = Some(STEP_TEXT_REQUIRED.to_string());
    }
}

/// Close the inline step editor back to page view, discarding its draft. Any pending editor
/// save goes with it: a closed row has nothing left for the save boundary to release (the only
/// path that can be here with one pending is a defensive direct
/// intent, never the keyboard).
fn open_capture_pending_step_editor(model: &mut BoardModel, index: usize) {
    let text = model
        .form
        .as_ref()
        .filter(|form| !form.is_task())
        .and_then(|form| form.steps.pending_adds.get(index))
        .cloned();
    let Some(text) = text else {
        return;
    };
    if let Some(form) = model.form.as_mut() {
        form.steps.add_selected = false;
        form.steps.editor = Some(StepEditor {
            buffer: crate::ui::edit::seeded_draft(&text),
            rename: None,
            pending_index: Some(index),
            insert_after: None,
            refusal: None,
        });
        model.input_mode = BoardInputMode::EditStep;
        model.clear_message();
    }
}

fn close_step_editor(model: &mut BoardModel) {
    if let Some(form) = model.form.as_mut() {
        form.steps.pending_save = None;
        form.steps.editor = None;
    }
    model.input_mode = BoardInputMode::TaskPage;
    model.clear_message();
}

/// What the inline step row paints when its draft is empty after trim: a short dim refusal
/// on the row itself, never the board status row.
const STEP_TEXT_REQUIRED: &str = "text required";

/// The status-row form of the same refusal, for when an open new-step row keeps the editor.
const EMPTIED_STEP_REFUSAL: &str = "a cleared step needs text";

/// Enter saves a new step and opens the next empty row. Shift+Enter saves it and
/// then the enclosing form (task session or expanded capture).
fn confirm_add_step(
    domain: &mut DomainState,
    model: &mut BoardModel,
    snapshot: Option<&InvocationSnapshot>,
    save_session: bool,
) -> Result<IntentOutcome, DomainError> {
    if !save_session {
        return confirm_step_editor(domain, model, true, false);
    }
    if model.form.as_ref().is_some_and(|form| !form.is_task()) {
        let outcome = confirm_step_editor(domain, model, false, false)?;
        if model
            .form
            .as_ref()
            .is_some_and(|form| form.steps.editor.is_some())
        {
            return Ok(outcome);
        }
        return apply_intent(domain, model, BoardIntent::ConfirmEdit, snapshot);
    }
    let extra_step = model.form.as_ref().and_then(|form| {
        form.steps
            .editor
            .as_ref()
            .filter(|editor| editor.rename.is_none())
            .map(|editor| {
                (
                    editor.buffer.value().trim().to_string(),
                    editor.insert_after,
                )
            })
    });
    let Some((text, insert_after)) = extra_step.filter(|(text, _)| !text.is_empty()) else {
        return confirm_step_editor(domain, model, false, true);
    };
    confirm_edit(domain, model, Some((&text, insert_after)))
}

fn stage_capture_step(
    model: &mut BoardModel,
    keep_open: bool,
) -> Result<IntentOutcome, DomainError> {
    let Some(form) = model.form.as_mut().filter(|form| !form.is_task()) else {
        return Ok(IntentOutcome::None);
    };
    let Some(editor) = form.steps.editor.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let text = editor.buffer.value().trim().to_string();
    let pending_index = editor.pending_index;
    if text.is_empty() {
        if let Some(editor) = form.steps.editor.as_mut() {
            editor.refusal = Some(STEP_TEXT_REQUIRED.to_string());
        }
        return Ok(IntentOutcome::None);
    }
    if let Some(index) = pending_index {
        if let Some(existing) = form.steps.pending_adds.get_mut(index) {
            *existing = text;
        }
    } else {
        form.steps.pending_adds.push(text);
    }
    if keep_open {
        form.steps.editor = Some(StepEditor {
            buffer: crate::ui::edit::seeded_draft(""),
            rename: None,
            pending_index: None,
            insert_after: None,
            refusal: None,
        });
        model.input_mode = BoardInputMode::EditStep;
    } else {
        form.steps.editor = None;
        model.input_mode = form.parent_mode();
    }
    Ok(IntentOutcome::None)
}

/// Persist a new-step editor draft through the domain command.
///
/// Existing-step editors are parked into the enclosing task session before this function is
/// reached. A successful add records the touched step on the page's pending-save slot and
/// leaves the line exactly as the user left it. Only the persistence boundary's confirmed
/// sync releases it, closing for Enter or reopening empty for Shift+Enter. A failed save
/// therefore holds the line behind SaveRecovery until Retry/Cancel resolve it.
///
/// An empty-after-trim draft is the line's own refusal (AC-13), painted on the line
/// and cleared when it closes or its buffer changes; it never reaches the board
/// message. Every other domain refusal — an step another actor removed — propagates
/// before anything is cleared, exactly as the task form's edit does.
fn confirm_step_editor(
    domain: &mut DomainState,
    model: &mut BoardModel,
    keep_open: bool,
    exit_editing: bool,
) -> Result<IntentOutcome, DomainError> {
    if model.form.as_ref().is_some_and(|form| !form.is_task()) {
        return stage_capture_step(model, keep_open);
    }
    let Some(form) = model.form.as_ref().filter(|form| form.is_task()) else {
        return Ok(IntentOutcome::None);
    };
    let Some(task_id) = form.task_id() else {
        return Ok(IntentOutcome::None);
    };
    let Some(editor) = form.steps.editor.as_ref() else {
        return Ok(IntentOutcome::None);
    };
    let text = editor.buffer.value().to_string();
    debug_assert!(
        editor.rename.is_none(),
        "existing-step drafts are staged by the task session"
    );
    let index = editor
        .insert_after
        .and_then(|anchor| {
            domain
                .get(task_id)?
                .steps
                .iter()
                .position(|step| step.id == anchor)
        })
        .map_or(usize::MAX, |anchor| anchor + 1);
    let touched =
        match domain.with_steps_undo(task_id, |domain| domain.insert_step(task_id, &text, index)) {
            Ok(step) => step,
            Err(DomainError::EmptyStepText) => {
                if let Some(editor) = model
                    .form
                    .as_mut()
                    .and_then(|form| form.steps.editor.as_mut())
                {
                    editor.refusal = Some(STEP_TEXT_REQUIRED.to_string());
                }
                return Ok(IntentOutcome::None);
            }
            Err(other) => return Err(other),
        };
    let form = model.form.as_mut().expect("task form checked above");
    form.steps.pending_save = Some(StepEditorSave {
        step: touched,
        text: text.trim().to_string(),
        reopen: keep_open,
        exit_editing,
    });
    Ok(IntentOutcome::Persist)
}
