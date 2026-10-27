//! Session-only board state, forms, selection, and recovery presentation.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use ratatui::layout::Position;
use uuid::Uuid;

use crate::context::InvocationSnapshot;
use crate::domain::{
    Assignee, BlockKey, BlockKind, BlockOn, DomainState, HumanStatus, Task, TaskScope, UndoEntry,
};
use crate::scope::{archived_path_contains, paths_equivalent, PathIdentityCache};
use crate::ui::capture::CaptureField;
use crate::ui::edit::{seeded_draft, EditBuffer};
use crate::ui::input::{
    BoardIntent, COMMAND_SURFACE_HELP_LINE, HELP_SURFACE_HELP_LINE, LAUNCH_CARD_HELP_LINE,
    SAVE_RECOVERY_HELP_LINE,
};
use crate::ui::mouse::BoardPopup;
use crate::ui::queue::{
    self, visible_task_ids, BoardLens, BoardSort, NavTab, QueueView, SectionKind,
};
use crate::ui::selection;
use crate::ui::terminal_text;
use crate::ui::text_select::TextSelection;
use crate::ui::tier::{FocusedSurface, WideStage};

use super::cleanup_card::CleanupChoice;
use super::commands::CommandSurface;
use super::dispatch_card::DispatchChoice;
use super::selector::{CardCursor, OptionCard, OptionSelector};

/// Visible board title. Also the herdr pane title.
pub const BOARD_TITLE: &str = "Tasks";
/// How long a delete recovery notice stays on the status row when nothing else takes it down.
pub(super) const DELETE_NOTICE_TTL: std::time::Duration = std::time::Duration::from_secs(4);

pub(super) const DIRTY_TASK_SWITCH_REFUSAL: &str = "save or cancel edits before switching tasks";
const QUESTION_QUIT_REFUSAL: &str =
    "a question answer or save is unresolved · tab 2: enter saves or confirms · esc cancels";

fn is_header_row(id: Uuid) -> bool {
    queue::is_fold_header_id(id)
}

fn nearest_surviving_task(
    previous: Uuid,
    previous_visible: &[Uuid],
    new_visible: &[Uuid],
) -> Option<Uuid> {
    let index = previous_visible.iter().position(|&id| id == previous)?;
    (1..=index.max(previous_visible.len().saturating_sub(index + 1))).find_map(|distance| {
        let before = index
            .checked_sub(distance)
            .and_then(|i| previous_visible.get(i))
            .copied()
            .filter(|id| !is_header_row(*id))
            .filter(|id| new_visible.contains(id));
        before.or_else(|| {
            previous_visible
                .get(index + distance)
                .copied()
                .filter(|id| !is_header_row(*id))
                .filter(|id| new_visible.contains(id))
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionRetarget {
    Explicit,
    Reanchor,
}

/// Board chrome input mode (normal list vs field edit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoardInputMode {
    #[default]
    Normal,
    EditTitle,
    EditNotes,
    /// The task page footer's dispatch base is selected. Enter opens the branch picker.
    SelectBase,
    /// The task page footer's `after` list is selected. Enter opens the task picker.
    SelectAfter,
    /// The scope row of an open task form owns focus. The renderer adaptation remains
    /// deliberately thin until, but its keyboard state is a first-class form field.
    EditScope,
    /// The assignee field cycles defined agent profiles plus unassigned.
    EditAssignee,
    /// A task/capture form footer chooser. The retained form focus identifies Scope or
    /// Assignee, and Esc returns to that parent field without applying the highlight.
    FormDropdown,
    /// The launch card owns input: a two-choice modal raised at most once per session
    /// when the invocation default resolved to an archived project.
    LaunchCard,
    /// A cursor-pinned clean worktree is awaiting y/n/Esc cleanup choice.
    CleanupConfirm,
    /// A dirty worktree can only be kept and completed, or cancelled.
    CleanupDirtyConfirm,
    /// The bulk dispatch card over a marked set is awaiting y/Esc.
    DispatchConfirm,
    /// The task page is open in view mode: the full-page surface shows the bound task and
    /// no field owns the cursor. Verbs act on the task; `e`/`n`/Tab enter field edits. A
    /// click does NOT: field regions are inert in this state, and only move focus once one
    /// of the edit states is already open (see the mouse mapper's form-field arms).
    TaskPage,
    /// Expanded quick-add has selected a staged step or its trailing `+ step` control. This
    /// must never resolve to the board's Normal mode: the full-frame capture owns every key.
    CapturePage,
    /// A one-line add/rename step draft owns input in its own row in the steps section. It is
    /// a Title-like [`crate::ui::edit::EditBuffer`] carried on the page form's steps state,
    /// not one of the three task-form fields: Enter applies the domain command, Shift+Enter
    /// adds and reopens an empty next row, and Esc cancels. The draft and this mode outlive
    /// the save call until confirmed sync closes or reopens it; every close returns to
    /// [`BoardInputMode::TaskPage`].
    EditStep,
    /// Board persistence failed; only navigation plus Retry/Cancel are available.
    SaveRecovery,
    /// Searchable command palette is open.
    Palette,
    /// The help card is open.
    Help,
    /// The task page's parent chooser (`P`): the searchable list picker over candidate
    /// parents. Enter applies and saves; Esc returns to the task page.
    ParentPicker,
    /// Any other searchable list picker (assignee, waiting on, base, after). Enter applies
    /// the highlighted option; Esc returns to the picker's `return_mode`.
    ListPicker,
    /// The active board lens's search field owns text input until Enter or Esc.
    Search,
    /// Single-line status-row capture from board `+`.
    QuickAdd,
    /// The block card (`ctrl+b`) owns input: why, on and needs.
    BlockCard,
    /// The task page's reply box under the last reply owns input.
    EditReply,
}

/// Result of applying a [`BoardIntent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentOutcome {
    /// No durable domain change.
    None,
    /// Domain mutated; caller should persist via Task Store and may refresh.
    Persist,
    /// Persistence already succeeded through the save-recovery boundary.
    Persisted,
    /// Leave the board UI.
    Quit,
}

/// Session-only board scope. The board itself always shows every live project grouped
/// (`All`); `Desk` and `Project` remain for embedders and tests that pin a scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BoardLocation {
    /// Every live scope: the desk and every live project.
    All,
    /// Tasks that belong to no project.
    Desk,
    /// One project's tasks.
    Project(PathBuf),
}

impl BoardLocation {
    pub(super) fn lens(&self) -> BoardLens<'_> {
        match self {
            Self::All => BoardLens::All,
            Self::Desk => BoardLens::Desk,
            Self::Project(path) => BoardLens::Project(path.as_path()),
        }
    }

    pub(super) fn at_home(&self) -> bool {
        matches!(self, Self::All | Self::Desk)
    }
}

/// The tabs of a tabbed list picker. Kept for the picker state shape; the board's single
/// lists open on the first tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterTab {
    #[default]
    Threads,
    Assignees,
    Projects,
}

/// Which searchable list picker is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPickerKind {
    /// The task page's parent chooser (`P`) for this task.
    Parent { task: Uuid },
    /// The quick assignee picker (bare `@`, the task-page footer, palette **set assignee**).
    Assignee,
    /// Who an open block or review waits on (`Enter` on the task page's top line, palette
    /// **set waiting on…**).
    WaitingOn,
    /// Explicit dispatch base, including the leading remote-default choice.
    Base,
    /// The tasks a task runs after: a search box over the tasks, the chosen ones pinned on top.
    After,
}

/// One choice inside a searchable list picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPickerOption {
    pub label: String,
    pub count: Option<usize>,
    pub value: ListPickerValue,
}

/// What `Enter` or a click does on one task-page relation entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationTarget {
    /// The parent-none line: opens the parent picker.
    SetParent,
    /// An existing parent or child: opens its task page.
    Open(Uuid),
    /// A parent link whose task is missing or deleted: refuses and keeps the link.
    Missing(Uuid),
    /// A question or finding of this task: opens it on the questions tab.
    Question(Uuid),
    /// A reference line in this task's notes (index into its links).
    Link(Uuid, usize),
}

/// What confirming a list-picker option applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListPickerValue {
    /// Clear the task's parent.
    Clear,
    /// Make this task the parent.
    Task(Uuid),
    /// A task under this one: choosing it would form a cycle, so it is listed but refused.
    Cycle(Uuid),
    /// Assign this agent profile or person, or clear the assignee with `None`.
    Assignee(Option<Assignee>),
    /// Wait on this.
    WaitingOn(BlockOn),
    /// The trailing `type a name…` row of the assignee and waiting-on pickers: confirming it
    /// reads the query by the name rule.
    TypedName,
    /// Set this exact branch name, or clear the explicit base with `None`.
    Base(Option<String>),
    /// A task-picker match the target may run after, by number.
    After(u64),
    /// A task the target runs after, pinned on top of the task picker.
    AfterPinned(u64),
    /// Informational row, never a selectable or confirmable value.
    Unavailable,
}

/// Whether a task-picker query term names `number` (`202`, `T202`, a prefix of either) or
/// appears in the task's `text` (its title and project).
fn after_term_matches(term: &str, number: u64, text: &str) -> bool {
    let digits = term.strip_prefix('t').unwrap_or(term);
    (!digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && number.to_string().starts_with(digits))
        || text.contains(term)
}

/// A waiting-on picker row: you as your handle, as in the assignee picker, else the record's
/// own words (`@pi`, `T202`, `coordinator`).
fn on_choice_label(on: &BlockOn) -> String {
    match on {
        BlockOn::You => crate::ui::render::assignee_handle(crate::domain::OWNER),
        other => terminal_text(&other.label()),
    }
}

/// What the assignee picker applies to, captured when it opens so a refresh that moves the
/// cursor underneath it can never redirect the assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AssigneePickerTarget {
    pub ids: Vec<Uuid>,
}

/// What the waiting-on picker applies to: each task's open record, captured when it opened, so
/// a record closed or replaced meanwhile refuses instead of moving the new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OnPickerTarget {
    pub records: Vec<(Uuid, BlockKey, BlockKind)>,
}

impl OnPickerTarget {
    /// Task addresses read as tasks only when every target is a block.
    pub(super) fn kind(&self) -> BlockKind {
        if self
            .records
            .iter()
            .all(|(_, _, kind)| *kind == BlockKind::Blocked)
        {
            BlockKind::Blocked
        } else {
            BlockKind::Review
        }
    }
}

/// The empty typed row's label in the assignee and waiting-on pickers.
pub(super) const TYPE_A_NAME: &str = "type a name…";

/// What the branch picker applies to. An edit-ring picker updates only its retained draft;
/// every other picker target is one atomic domain batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BasePickerTarget {
    pub ids: Vec<Uuid>,
    pub edit_draft: bool,
    request: Uuid,
    project: PathBuf,
    current: Option<String>,
    selection_pending: bool,
}

/// What the task picker applies to, captured when it opens. An edit-ring picker updates only
/// its form's draft; every other target is one atomic, undoable domain batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AfterPickerTarget {
    pub ids: Vec<Uuid>,
    pub edit_draft: bool,
    /// The card title: `T5 runs after`, `3 tasks run after`, `new task runs after`.
    pub title: String,
    /// The chosen numbers, in display order: Enter saves exactly these.
    pub pinned: Vec<u64>,
    /// Labels for current links the search never offers (a done or archived prerequisite),
    /// so they still show pinned and can be removed.
    pub unlisted: Vec<(u64, String)>,
}

/// An open searchable list picker (parent / assignee / waiting on / base / after).
/// Session-only.
#[derive(Debug, Clone)]
pub(super) struct ListPickerState {
    pub kind: ListPickerKind,
    pub options: Vec<ListPickerOption>,
    pub selected: usize,
    pub query: String,
    /// Row scroll of the wrapping parent chooser; unused by the one-row-per-option pickers.
    pub rows: crate::ui::render::PickerRowState,
    /// The mode confirm and cancel restore. The thread and View pickers open from the board
    /// only; the assignee picker also opens over the task page, which must survive it.
    pub return_mode: BoardInputMode,
    pub assignee_target: Option<AssigneePickerTarget>,
    pub on_target: Option<OnPickerTarget>,
    pub base_target: Option<BasePickerTarget>,
    pub after_target: Option<AfterPickerTarget>,
    /// Source index of the option the board currently applies (painted `✓`).
    pub active: Option<usize>,
    /// The tabbed filter pickers keep the inactive tab here; `None` for single lists.
    pub tabs: Option<ListPickerTabs>,
}

/// The inactive tab of a tabbed list picker, swapped in by `Tab`.
#[derive(Debug, Clone)]
pub(super) struct ListPickerTabs {
    pub active_tab: FilterTab,
    pub other_options: Vec<ListPickerOption>,
    pub other_active: Option<usize>,
}
/// The immutable value a board form carries for its whole lifetime.
///
/// Keeping task identity and capture provenance mutually exclusive by construction means a
/// capture can never acquire a mutable selected-task target, and a task form can never reload
/// an invocation snapshot underneath its drafts.
#[derive(Debug, Clone)]
pub(super) enum BoardFormBinding {
    Task(Uuid),
    Capture(Box<Option<InvocationSnapshot>>),
}

/// One transient board form, shared by quick-add expansion and task editing.
///
/// Both variants own two independent [`EditBuffer`]s, the selected task scope, field focus,
/// and a selection within the scope choices. A task form adds exactly one immutable task id;
/// a quick-add form instead retains exactly one immutable invocation snapshot. Keeping those
/// two mutually exclusive values inside the same form prevents title, Notes, and scope from
/// drifting into separately-bound edits.
#[derive(Debug, Clone)]
pub(super) struct QuickAddState {
    pub(super) title: EditBuffer,
    /// The immutable invocation snapshot, retained for title prefill, token resolution, and
    /// capture provenance.
    pub(super) snapshot: Box<Option<InvocationSnapshot>>,
    /// Current visible destination: the default below, overridden while `!p` tokens are
    /// typed. The input row paints it (`Add to …`).
    pub(super) scope: TaskScope,
    /// The pre-token destination this draft saves to when no `!p` override applies.
    pub(super) default: TaskScope,
}

impl QuickAddState {
    pub(super) fn new(snapshot: Option<InvocationSnapshot>, scope: TaskScope) -> Self {
        let title = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.title_prefill.as_deref())
            .unwrap_or_default();
        Self {
            title: seeded_draft(title),
            snapshot: Box::new(snapshot),
            default: scope.clone(),
            scope,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct QuickAddSave {
    id: Uuid,
    keep_open: bool,
}

/// A task form mutation staged in the domain but not yet confirmed at the persistence boundary.
#[derive(Debug, Clone)]
pub(super) struct TaskEditSave {
    pub(super) id: Uuid,
    pub(super) title: String,
    pub(super) notes: Option<String>,
    pub(super) scope: TaskScope,
    pub(super) assignee: Option<Assignee>,
    pub(super) base: Option<String>,
    pub(super) after: Vec<u64>,
    /// Existing-step names staged alongside the ordinary task fields. They reach the
    /// domain only when the task session is confirmed with Shift+Enter.
    pub(super) step_renames: BTreeMap<Uuid, String>,
    /// Existing steps staged for removal by the task edit session.
    pub(super) step_removals: BTreeSet<Uuid>,
    /// Stable identity of the selected step when the save began. Source indices shift after
    /// removals, so the retained page cursor must reanchor through this id after sync.
    pub(super) selected_step: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub(super) struct BoardForm {
    pub(super) title: EditBuffer,
    pub(super) notes: EditBuffer,
    pub(super) scope: TaskScope,
    /// Optional assignee draft and the defined choices available this session.
    pub(super) assignee: Option<Assignee>,
    /// Optional explicit dispatch branch. `None` means the repository's remote default.
    pub(super) base: Option<String>,
    /// The tasks this one runs after, by number, as the task picker last set them.
    pub(super) after: Vec<u64>,
    pub(super) assignee_options: Vec<Option<Assignee>>,
    pub(super) assignee_selected: usize,
    /// A task page starts view-only. Entering any field makes its steps selectable and editable
    /// for the rest of that page session; closing the page drops the state with the form.
    pub(super) editing: bool,
    pub(super) focus: CaptureField,
    pub(super) scope_options: Vec<TaskScope>,
    pub(super) scope_selected: usize,
    pub(super) binding: BoardFormBinding,
    /// Editable values as they stood when this task session bound or last saved successfully.
    /// Dirty checks use this immutable baseline rather than the live domain snapshot, which can
    /// advance independently while the user is editing.
    pub(super) task_snapshot: Option<Box<Task>>,
    /// View-mode scroll of the task page's shared notes-and-steps body, never used by capture.
    pub(super) title_wrap_width: std::cell::Cell<usize>,
    pub(super) notes_scroll: usize,
    pub(super) manual_page_scroll: bool,
    /// Page-session steps state (step cursor, window scroll, delete mark, step
    /// editor). Carried by the form so it lives exactly as long as the page does.
    pub(super) steps: StepsPageState,
    /// The BLOCKED section's ring stop and reply box. Lives as long as the page does.
    pub(super) block: super::block::BlockPageState,
    /// The furthest shared-content scroll offset the last painted frame can show.
    ///
    /// The scroll bound depends on the wrap width, which only the renderer knows: the model
    /// is deliberately geometry-free and the render path takes `&BoardModel`. Bounding the
    /// intent by `notes.value().lines().count()` instead was wrong in the reachable
    /// direction, because wrapping only ever ADDS rows, so the tail of any wrapping note
    /// was unreachable while the divider still advertised it. A `Cell` lets the immutable
    /// renderer record what it actually laid out, without making the whole render path `&mut`
    /// or pushing geometry into every scroll intent.
    pub(super) notes_max_scroll: std::cell::Cell<usize>,
    /// The notes wrap width the last painted frame used, recorded by the immutable
    /// renderer for the same reason as `notes_max_scroll`: vertical arrow movement
    /// wraps at the painted width, which only the renderer knows.
    pub(super) notes_width: std::cell::Cell<usize>,
}

impl BoardForm {
    pub(super) fn task(
        task: &Task,
        this_repo: Option<&Path>,
        tasks: &[Task],
        focus: CaptureField,
        archived: &BTreeSet<String>,
        agent_names: &[String],
    ) -> Self {
        let mut form = Self::new(
            &task.title,
            task.notes.as_deref().unwrap_or_default(),
            task.scope.clone(),
            focus,
            BoardFormBinding::Task(task.id),
            this_repo,
            tasks,
            archived,
        );
        form.assignee = task.assigned();
        form.base = task.base.clone();
        form.after = task.after.clone();
        form.set_agent_names(agent_names);
        form.block.fold = super::block::CheckFold::fresh(task);
        form.block.target = super::block::initial_target(task, form.block.fold.as_ref());
        form.task_snapshot = Some(Box::new(task.clone()));
        form
    }

    pub(super) fn capture(
        snapshot: Option<InvocationSnapshot>,
        this_repo: Option<&Path>,
        tasks: &[Task],
        archived: &BTreeSet<String>,
    ) -> Self {
        let initial_scope = snapshot
            .as_ref()
            .map(|snapshot| snapshot.default_scope.clone())
            .unwrap_or(TaskScope::Global);
        let title = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.title_prefill.as_deref())
            .unwrap_or_default()
            .to_string();
        Self::new(
            &title,
            "",
            initial_scope,
            CaptureField::Title,
            BoardFormBinding::Capture(Box::new(snapshot)),
            this_repo,
            tasks,
            archived,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        title: &str,
        notes: &str,
        scope: TaskScope,
        focus: CaptureField,
        binding: BoardFormBinding,
        this_repo: Option<&Path>,
        tasks: &[Task],
        archived: &BTreeSet<String>,
    ) -> Self {
        let scope_options = board_form_scope_options(&scope, this_repo, tasks, archived);
        let scope_selected = scope_options
            .iter()
            .position(|option| option == &scope)
            .unwrap_or(0);
        Self {
            title: seeded_draft(title),
            notes: seeded_draft(notes),
            scope,
            assignee: None,
            base: None,
            after: Vec::new(),
            assignee_options: vec![None],
            assignee_selected: 0,
            editing: false,
            focus,
            scope_options,
            scope_selected,
            binding,
            task_snapshot: None,
            title_wrap_width: std::cell::Cell::new(0),
            notes_scroll: 0,
            manual_page_scroll: false,
            steps: StepsPageState::default(),
            block: super::block::BlockPageState::default(),
            notes_max_scroll: std::cell::Cell::new(0),
            notes_width: std::cell::Cell::new(0),
        }
    }

    pub(super) fn is_task(&self) -> bool {
        matches!(&self.binding, BoardFormBinding::Task(_))
    }

    pub(super) fn task_id(&self) -> Option<Uuid> {
        match &self.binding {
            BoardFormBinding::Task(id) => Some(*id),
            BoardFormBinding::Capture(_) => None,
        }
    }

    pub(super) fn snapshot(&self) -> Option<&InvocationSnapshot> {
        match &self.binding {
            BoardFormBinding::Task(_) => None,
            BoardFormBinding::Capture(snapshot) => snapshot.as_ref().as_ref(),
        }
    }

    pub(super) fn parent_mode(&self) -> BoardInputMode {
        match self.focus {
            CaptureField::Title => BoardInputMode::EditTitle,
            CaptureField::Notes => BoardInputMode::EditNotes,
            CaptureField::Scope => BoardInputMode::EditScope,
            CaptureField::Assignee => BoardInputMode::EditAssignee,
            CaptureField::Base => BoardInputMode::SelectBase,
            CaptureField::After => BoardInputMode::SelectAfter,
        }
    }

    /// Restore one field's draft to its bound task's saved value (Esc on the task page).
    pub(super) fn reset_field_to_saved(&mut self, field: CaptureField, task: &Task) {
        match field {
            CaptureField::Title => self.title = seeded_draft(&task.title),
            CaptureField::Notes => {
                self.notes = seeded_draft(task.notes.as_deref().unwrap_or_default());
            }
            CaptureField::Scope => {
                self.scope = task.scope.clone();
                self.select_current_scope();
            }
            CaptureField::Assignee => {
                self.assignee = task.assigned();
                self.select_current_assignee();
            }
            CaptureField::Base => self.base = task.base.clone(),
            CaptureField::After => self.after = task.after.clone(),
        }
        if let Some(snapshot) = self.task_snapshot.as_mut() {
            match field {
                CaptureField::Title => snapshot.title = task.title.clone(),
                CaptureField::Notes => snapshot.notes = task.notes.clone(),
                CaptureField::Scope => snapshot.scope = task.scope.clone(),
                CaptureField::Assignee => {
                    snapshot.assignee = task.assignee.clone();
                    snapshot.assignee_kind = task.assignee_kind;
                }
                CaptureField::Base => snapshot.base = task.base.clone(),
                CaptureField::After => snapshot.after = task.after.clone(),
            }
        }
    }

    pub(super) fn focus_next(&mut self) {
        self.focus = match self.focus {
            CaptureField::Title => CaptureField::Notes,
            CaptureField::Notes => CaptureField::Assignee,
            CaptureField::Assignee => CaptureField::Base,
            CaptureField::Base => CaptureField::After,
            CaptureField::After => CaptureField::Scope,
            CaptureField::Scope => CaptureField::Title,
        };
    }

    pub(super) fn focus_prev(&mut self) {
        self.focus = match self.focus {
            CaptureField::Title => CaptureField::Scope,
            CaptureField::Notes => CaptureField::Title,
            CaptureField::Assignee => CaptureField::Notes,
            CaptureField::Base => CaptureField::Assignee,
            CaptureField::After => CaptureField::Base,
            CaptureField::Scope => CaptureField::After,
        };
    }

    /// The Assignee field's choices: none, every profile, then you. A person or removed
    /// profile the task already has joins at the end, so cycling never drops it unasked.
    pub(super) fn set_agent_names(&mut self, names: &[String]) {
        self.assignee_options = std::iter::once(None)
            .chain(names.iter().map(|name| Some(Assignee::agent(name.clone()))))
            .chain(std::iter::once(Some(Assignee::you())))
            .collect();
        self.select_current_assignee();
    }

    pub(super) fn cycle_assignee(&mut self, forward: bool) {
        if self.assignee_options.is_empty() {
            return;
        }
        self.assignee_selected = if forward {
            (self.assignee_selected + 1) % self.assignee_options.len()
        } else {
            self.assignee_selected
                .checked_sub(1)
                .unwrap_or(self.assignee_options.len() - 1)
        };
        self.assignee = self.assignee_options[self.assignee_selected].clone();
    }

    fn select_current_assignee(&mut self) {
        if !self.assignee_options.contains(&self.assignee) {
            self.assignee_options.push(self.assignee.clone());
        }
        self.assignee_selected = self
            .assignee_options
            .iter()
            .position(|option| option == &self.assignee)
            .unwrap_or(0);
    }

    fn move_assignee_selection(&mut self, forward: bool) {
        if self.assignee_options.is_empty() {
            return;
        }
        self.assignee_selected = if forward {
            (self.assignee_selected + 1) % self.assignee_options.len()
        } else {
            self.assignee_selected
                .checked_sub(1)
                .unwrap_or(self.assignee_options.len() - 1)
        };
    }

    fn apply_assignee_selection(&mut self) {
        if let Some(assignee) = self.assignee_options.get(self.assignee_selected) {
            self.assignee = assignee.clone();
        }
    }

    pub(super) fn cycle_scope(&mut self) {
        if self.scope_options.is_empty() {
            return;
        }
        let current = self
            .scope_options
            .iter()
            .position(|option| option == &self.scope)
            .unwrap_or(0);
        self.scope_selected = (current + 1) % self.scope_options.len();
        self.scope = self.scope_options[self.scope_selected].clone();
    }

    pub(super) fn move_scope_selection(&mut self, forward: bool) {
        if self.scope_options.is_empty() {
            return;
        }
        self.scope_selected = if forward {
            (self.scope_selected + 1) % self.scope_options.len()
        } else {
            self.scope_selected
                .checked_sub(1)
                .unwrap_or(self.scope_options.len() - 1)
        };
    }

    pub(super) fn select_current_scope(&mut self) {
        self.scope_selected = self
            .scope_options
            .iter()
            .position(|option| option == &self.scope)
            .unwrap_or(0);
    }

    pub(super) fn apply_scope_selection(&mut self) {
        if let Some(scope) = self.scope_options.get(self.scope_selected) {
            self.scope = scope.clone();
        }
    }
}

/// The in-place add/rename step input in the task page's steps section (page-session only).
///
/// `rename` names the step being edited; `None` is an add (the buffer starts empty).
/// `refusal` is the line's own empty-text refusal (AC-13): painted on the line,
/// never the board status row, and cleared when the line closes or its buffer
/// changes.
#[derive(Debug, Clone)]
pub(super) struct StepEditor {
    pub(super) buffer: EditBuffer,
    pub(super) rename: Option<Uuid>,
    /// Expanded capture has no durable step ids yet, so an existing staged row is addressed by
    /// its stable draft index until the capture saves.
    pub(super) pending_index: Option<usize>,
    /// A task-page add row lands directly below this stored step (`ctrl+a` with a step
    /// selected); `None` appends. An id, not an index, so other writers cannot shift it.
    pub(super) insert_after: Option<Uuid>,
    pub(super) refusal: Option<String>,
}

/// An step-editor apply waiting for the app save boundary to confirm persistence
/// (AC-14). `step` + `text` name the mutation that must land before the line may
/// close — or, for Shift+Enter in add mode, reopen empty.
#[derive(Debug, Clone)]
pub(super) struct StepEditorSave {
    pub(super) step: Uuid,
    pub(super) text: String,
    pub(super) reopen: bool,
    /// Leave the task-edit session after this add lands (Shift+Enter on a new step).
    pub(super) exit_editing: bool,
}

/// Page-session steps state for the task page, never persisted.
///
/// The step cursor lifecycle (spec: resolved decisions, amended 2026-08-22):
/// inactive when the page opens; a bare ↓ (re-)activates it on the first step —
/// including after an earlier ↑-deactivation, so activation is never one-shot;
/// ↑ from the first step deactivates it. While inactive ↑ scrolls the notes and
/// ↓ re-activates; a click on an step row moves the cursor onto that step
/// (AC-21). A task with no steps never activates a cursor.
#[derive(Debug, Clone, Default)]
pub(super) struct StepsPageState {
    /// Highlighted stored step index; `None` = inactive or the trailing add target.
    pub(super) cursor: Option<usize>,
    /// The trailing `+ step` target is selected instead of a stored step.
    pub(super) add_selected: bool,
    /// Absolute content row of the steps label, recorded by the renderer so cursor
    /// movement can keep its selected step inside the shared viewport.
    pub(super) content_start: std::cell::Cell<usize>,
    /// Step index visibly marked by the first press of the delete verb. Any intervening
    /// intent clears it; only the verb's second press removes.
    pub(super) delete_mark: Option<usize>,
    /// The open in-place add/rename editor, if any.
    pub(super) editor: Option<StepEditor>,
    /// Existing-step text staged by the task edit session. Leaving an inline rename parks its
    /// buffer here, so another field or step can receive the one terminal cursor without
    /// committing either change.
    pub(super) drafts: BTreeMap<Uuid, EditBuffer>,
    /// Existing steps removed only when the enclosing task form saves. Esc drops this set.
    pub(super) removals: BTreeSet<Uuid>,
    /// Capture-form steps staged until the create saves. Task pages persist adds immediately.
    pub(super) pending_adds: Vec<String>,
    /// An editor apply the save boundary has not confirmed yet (AC-14). While it is
    /// set, the editor and its input mode are held exactly as the user left them.
    pub(super) pending_save: Option<StepEditorSave>,
    /// Shared-content rows the last painted viewport showed (renderer-recorded).
    pub(super) window_rows: std::cell::Cell<usize>,
    /// Wrapped row count for each stored step at the last painted width. The renderer records
    /// it so keyboard movement scrolls to the selected step's first wrapped row.
    pub(super) row_counts: std::cell::RefCell<Vec<usize>>,
    /// Step text wrap width at the last paint, so a step added between paints can be
    /// measured before the renderer records its row count.
    pub(super) text_width: std::cell::Cell<usize>,
    /// View-mode relation selection: 0 is the parent line, `1 + k` the k-th direct child
    /// ([`BoardModel::relation_targets`]). Mutually exclusive with `cursor` and
    /// `add_selected`: [`super::apply::apply_intent`] drops it whenever either is set.
    pub(super) relation_cursor: Option<usize>,
    /// Absolute content row and row count of each relation entry at the last paint
    /// (renderer-recorded), so keyboard movement can keep the selected entry in view.
    pub(super) relation_geometry: std::cell::RefCell<Vec<(usize, usize)>>,
}

/// Keep relation entry `target` inside the renderer-recorded shared content viewport.
pub(super) fn relation_scroll_to(form: &mut BoardForm, target: usize) {
    let Some((start, rows)) = form.steps.relation_geometry.borrow().get(target).copied() else {
        return;
    };
    let window = form.steps.window_rows.get().max(1);
    let end = start + rows.max(1);
    if start < form.notes_scroll {
        form.notes_scroll = start;
    } else if end > form.notes_scroll.saturating_add(window) {
        form.notes_scroll = end.saturating_sub(window).min(start);
    }
    form.notes_scroll = form.notes_scroll.min(form.notes_max_scroll.get());
}

/// Keep the selected step in the renderer-recorded shared content viewport.
pub(super) fn steps_scroll_to_cursor(form: &mut BoardForm, cursor: usize) {
    let counts = form.steps.row_counts.borrow();
    let wrapped_before: usize = if counts.len() >= cursor {
        counts.iter().take(cursor).copied().sum()
    } else {
        cursor
    };
    drop(counts);
    steps_scroll_to_row(form, wrapped_before, 0);
}

/// Keep the steps row `wrapped_before` wrapped rows below the first step inside the
/// viewport. `added_rows` is content the last paint did not see yet (a new add row or
/// step), which raises the recorded scroll horizon by as much.
pub(super) fn steps_scroll_to_row(form: &mut BoardForm, wrapped_before: usize, added_rows: usize) {
    let target = form
        .steps
        .content_start
        .get()
        .saturating_add(1)
        .saturating_add(wrapped_before);
    let rows = form.steps.window_rows.get().max(1);
    if target < form.notes_scroll {
        form.notes_scroll = target;
    } else if target >= form.notes_scroll.saturating_add(rows) {
        form.notes_scroll = target + 1 - rows;
    }
    form.notes_scroll = form
        .notes_scroll
        .min(form.notes_max_scroll.get().saturating_add(added_rows));
}

/// Scope choices shared by capture and task forms.
///
/// The order is intentional: the form's initial scope, the current repository, project scopes
/// represented by non-soft-deleted tasks, then Global. `TaskScope` has no all-projects value,
/// so an editor can never accidentally adopt the board selector's session-only `All` choice.
fn board_form_scope_options(
    initial_scope: &TaskScope,
    this_repo: Option<&Path>,
    tasks: &[Task],
    archived: &BTreeSet<String>,
) -> Vec<TaskScope> {
    let mut options = Vec::new();
    // Aliases of one directory (`/tmp/x` and `/private/tmp/x`) are one choice; the first
    // spelling pushed wins, so the form's own scope keeps its exact value.
    let identities = PathIdentityCache::default();
    let push = |scope: TaskScope, options: &mut Vec<TaskScope>| {
        let known = options.iter().any(|option| match (option, &scope) {
            (TaskScope::Project { path: known }, TaskScope::Project { path }) => {
                identities.equivalent(known, path)
            }
            (known, scope) => known == scope,
        });
        if !known {
            options.push(scope);
        }
    };
    // AC-39: no dropdown offers an archived project. The form's own initial scope is
    // exempt: a task already sitting in an archived project keeps it as its value.
    let filed = |scope: &TaskScope| match scope {
        TaskScope::Project { path } => archived_path_contains(archived, path),
        TaskScope::Global => false,
    };
    push(initial_scope.clone(), &mut options);
    if let Some(repo) = this_repo {
        let scope = TaskScope::Project {
            path: repo.to_string_lossy().into_owned(),
        };
        if !filed(&scope) {
            push(scope, &mut options);
        }
    }
    for task in tasks {
        if !task.soft_deleted {
            if let TaskScope::Project { path } = &task.scope {
                let scope = TaskScope::Project { path: path.clone() };
                if !filed(&scope) {
                    push(scope, &mut options);
                }
            }
        }
    }
    push(TaskScope::Global, &mut options);
    options
}

/// Shared, session-local remote-default cache. Metadata refreshes off the render
/// thread, after a dispatch changes origin/HEAD or a short TTL catches external changes.
#[derive(Debug, Clone, Default)]
struct DefaultBranchCache(Arc<Mutex<BTreeMap<PathBuf, CachedDefaultBranch>>>);

#[derive(Debug, Clone)]
struct CachedDefaultBranch {
    name: Option<String>,
    checked_at: Instant,
    loading: bool,
    request: Uuid,
}

impl DefaultBranchCache {
    fn invalidate(&self, project: &Path) {
        if let Ok(mut cache) = self.0.try_lock() {
            cache.remove(project);
        }
    }

    fn get_or_request(&self, project: &Path) -> String {
        const TTL: Duration = Duration::from_secs(2);
        let path = project.to_path_buf();
        let mut request = None;
        let cached = self.0.try_lock().ok().and_then(|mut cache| {
            let entry = cache.entry(path.clone()).or_insert_with(|| {
                let id = Uuid::new_v4();
                request = Some(id);
                CachedDefaultBranch {
                    name: None,
                    checked_at: Instant::now(),
                    loading: true,
                    request: id,
                }
            });
            if !entry.loading && entry.checked_at.elapsed() >= TTL {
                let id = Uuid::new_v4();
                entry.request = id;
                entry.loading = true;
                request = Some(id);
            }
            entry.name.clone()
        });
        if let Some(request) = request {
            let shared = Arc::clone(&self.0);
            std::thread::spawn(move || {
                let name = crate::git_base::default_branch_name(&path)
                    .unwrap_or_else(|| "default".to_string());
                if let Ok(mut cache) = shared.lock() {
                    if let Some(entry) = cache
                        .get_mut(&path)
                        .filter(|entry| entry.request == request)
                    {
                        entry.name = Some(name);
                        entry.checked_at = Instant::now();
                        entry.loading = false;
                    }
                }
            });
        }
        cached.unwrap_or_else(|| "default".to_string())
    }
}

/// The base picker's disabled row while its background fetch runs.
pub const REFRESHING_BRANCHES: &str = "refreshing…";

/// Workers publish here, only the event-loop poll updates the active picker.
#[derive(Debug)]
struct BasePickerWorker {
    job: Uuid,
    request: Uuid,
}

#[derive(Debug)]
struct BasePickerResult {
    request: Uuid,
    project: PathBuf,
    branches: Result<(Vec<String>, Option<String>), String>,
    default: String,
}

/// Pure board presentation state for one open session.
///
/// Session-only UI state lives here and is never written under the task store or
/// plugin config dirs. Visible rows come from [`queue::query`]; selection is
/// id-pinned and reanchored via [`selection::reanchor`].
#[derive(Debug, Clone)]
pub struct BoardModel {
    pub(super) tasks: Vec<Task>,
    /// Scope paths of archived projects, carried from domain state so the lens
    /// query can filter hidden tasks without re-deriving from records.
    pub(super) archived_projects: BTreeSet<String>,
    /// Person names removed from the assignee picker, from the store.
    pub(super) hidden_assignees: BTreeSet<String>,
    /// Defined profile names loaded once for this session.
    pub(super) agent_names: Vec<String>,
    pub(super) this_repo: Option<PathBuf>,
    default_branches: DefaultBranchCache,
    /// Session board location (active surface). Not durable.
    pub(super) board_location: BoardLocation,
    /// The last project scope picked, kept for quick-add and the picker highlight. Not durable.
    pub(super) selected_project: Option<PathBuf>,
    /// Focus owner: the list or the task detail. Single-pane presentation derives from it
    /// below 110 columns. Session-only.
    pub(super) wide_stage: WideStage,
    /// Session-only row order of every section (`S`).
    pub(super) sort: BoardSort,
    /// `[board]` from `config.toml`: the name the board calls you and the message default.
    pub(super) layout_defaults: crate::agents::BoardDefaults,
    /// `z`: board rows paint without their `└─` message lines. Session-only; it starts from
    /// `[board] messages` and is never written back.
    pub(super) messages_hidden: bool,
    /// The active lens's content-search query. Session-only.
    pub(super) search_query: String,
    /// Whether Enter pinned the query and returned input to the board. Session-only.
    pub(super) search_pinned: bool,
    /// Open searchable list picker (the parent chooser). Session-only.
    pub(super) list_picker: Option<ListPickerState>,
    base_picker_results: Arc<Mutex<Vec<BasePickerResult>>>,
    base_picker_workers: Arc<Mutex<HashMap<PathBuf, BasePickerWorker>>>,
    /// Whether the done drawer lists completed tasks. Session-only.
    pub(super) drawer_open: bool,
    /// Notes status lines start hidden; the board-list `v` key toggles them for this session.
    pub(super) show_status_lines: bool,
    /// The done drawer's archived group starts collapsed on every launch. Session-only.
    pub(super) archived_collapsed: bool,
    /// The ON DECK inbox group starts expanded. Session-only.
    pub(super) inbox_collapsed: bool,
    /// The launch card naming an archived project, while it is up.
    pub(super) launch_card: Option<super::launch_card::LaunchPrompt>,
    /// The launch card is shown at most once per session, whichever choice was made.
    pub(super) launch_card_shown: bool,
    /// Session quick-add default override (desk after "keep archived"). Session-only.
    pub(super) session_default_scope: Option<TaskScope>,
    /// Id-pinned selection into the queue-visible row set.
    pub(super) selection_id: Option<Uuid>,
    /// Whether explicit task marking controls own Space, shifted arrows, and row clicks.
    mark_mode: bool,
    /// Session-only tasks included in the next bulk verb. Every id remains visible in this lens.
    pub(super) marked_ids: BTreeSet<Uuid>,
    /// Session-only task pages a relation open left behind, newest last. A clean `Esc` on
    /// the task page restores the newest one that still exists; closing the page, a list
    /// retarget, a scope change, or a board-tab route clears it.
    pub(super) page_back: Vec<Uuid>,
    /// The last task-row click (time + id), kept only to detect a double-click that opens
    /// the task page. Presentation-only, never persisted.
    pub(super) last_row_click: Option<(Instant, Uuid)>,
    /// Focused help-card search query. Session-only, reset on open and close.
    pub(super) help_query: String,
    /// First visible row of the filtered help-card key list. Session-only, reset on query.
    pub(super) help_scroll: usize,
    /// Furthest help scroll the last painted card could show (renderer-recorded).
    pub(super) help_max_scroll: Cell<usize>,
    /// This board's SaveRecovery popup only mirrors a failed save another board owns.
    pub(super) save_recovery_proxy: bool,
    /// Furthest cleanup-card scroll the last painted card could show (renderer-recorded).
    pub(super) cleanup_max_scroll: Cell<usize>,
    /// The painted cleanup card's option rows, so `↑`/`↓` scroll the selection into view.
    pub(super) cleanup_layout: std::cell::RefCell<crate::ui::render::CleanupLayout>,
    /// Underlying surface to restore after Help closes.
    pub(super) help_return_mode: BoardInputMode,
    /// Underlying board mode to restore after the search input closes or pins.
    pub(super) search_return_mode: BoardInputMode,
    pub(super) input_mode: BoardInputMode,
    /// The one active board form. It is present for expanded quick-add and task editing alike;
    /// task identity or invocation context are held inside it and never rebound after open.
    pub(super) form: Option<BoardForm>,
    /// The status-row quick-add draft. It remains while its expanded form is open so Esc can
    /// return to the line without reconstructing its snapshot or cursor state.
    pub(super) quick_add: Option<QuickAddState>,
    /// Newly-created task briefly emphasized until the next input intent.
    pub(super) saved_task: Option<Uuid>,
    /// A quick-add create waiting for the app save boundary to confirm persistence.
    pub(super) quick_add_save: Option<QuickAddSave>,
    /// A task-form edit waiting for the app save boundary to confirm persistence.
    pub(super) task_edit_save: Option<TaskEditSave>,
    /// Tasks a quick-picker assignment changed, waiting for its save to land before an open
    /// task form bound to one of them adopts the new assignee. Cancel drops it unapplied.
    pub(super) pending_form_assignee_sync: Option<Vec<Uuid>>,
    /// Same stale-draft guard for an immediately-applied Base picker.
    pub(super) pending_form_base_sync: Option<Vec<Uuid>>,
    /// The app save boundary holds task-form release across its inner reducer sync.
    pub(super) hold_task_edit_save: bool,
    /// Last board action feedback or empty-selection chrome message.
    pub(super) message: Option<String>,
    /// Dim status-row notice when a cached release tag is newer than this binary.
    pub(super) update_notice: Option<String>,
    /// Title of the task the last soft-delete removed, captured at delete time.
    ///
    /// Transient presentation only: it is never persisted, and it is the whole of the delete
    /// recovery notice's state. The title is captured rather than re-read,
    /// because by the time the row is painted the selection may have moved and the task
    /// itself may have been restored by the very Undo this notice offers.
    pub(super) delete_notice: Option<String>,
    /// Number of tasks represented by a bulk delete notice. `None` keeps the single-title form.
    pub(super) delete_notice_count: Option<usize>,
    /// The deleted task's `T` number, painted before its title. `None` for a notice task
    /// (`N…`), which keeps the title alone, and for a bulk notice.
    pub(super) delete_notice_label: Option<String>,
    /// When the delete recovery notice leaves the status row on its own. A clean outcome
    /// clears after a few seconds; the footer's undo seat ([`Self::undo_hint`]) outlives it.
    pub(super) delete_notice_expires_at: Option<Instant>,
    /// The undo entry a delete pushed, held so the footer can keep offering `ctrl+u undo`
    /// after the status-row notice clears. [`Self::sync_from_domain`] drops it the moment
    /// that entry is no longer the live top of the stack (undone, superseded, or stale), so
    /// the footer never advertises an undo that would do something else or refuse.
    pub(super) undo_hint: Option<UndoEntry>,
    /// [`Self::undo_hint`] as it stood before the last mutating intent. That intent's sync
    /// may drop the hint against a domain that never reaches disk; a cancelled failed save
    /// puts it back ([`Self::restore_undo_hint_before_save`]) for the baseline to revalidate.
    pub(super) undo_hint_before_save: Option<UndoEntry>,
    /// A delete recovery notice a failed save took off the row, held until the failure is
    /// resolved.
    ///
    /// A failed save means the deletion is not durable, so the notice must not offer to undo
    /// it; but Retry can still make it durable, and then the way back has to come with it.
    /// See [`BoardModel::begin_save_recovery`] and [`BoardModel::end_save_recovery`].
    pub(super) suspended_delete_notice: Option<String>,
    pub(super) suspended_delete_notice_count: Option<usize>,
    pub(super) suspended_delete_notice_label: Option<String>,
    /// First ctrl+x arms this exact target set; a second press on the same set deletes it.
    pub(super) pending_delete: Option<BTreeSet<Uuid>>,
    /// Cursor-pinned dispatch cleanup details while the confirmation modal owns input.
    pub(super) cleanup_prompt: Option<CleanupPrompt>,
    /// A confirmed cleanup whose host work runs off the event loop. Outlives its card: Esc
    /// hides the card and the status row reports progress until the run finishes.
    pub(super) cleanup_run: SharedCleanupRun,
    /// Quit waits here while a cleanup run finishes its git and Herdr steps, up to this bound.
    pub(super) quit_after_cleanup: Option<Instant>,
    /// The status row while a hidden cleanup runs (`cleaning 2 of 5…`, `finishing
    /// cleanup…`). Its own slot, under any action message: another action never erases it
    /// and it never outlives the run.
    pub(super) cleanup_status: Option<String>,
    /// Bulk dispatch card over the marked set while it owns input.
    pub(super) dispatch_prompt: Option<DispatchPrompt>,
    /// A reply-box `ctrl+s` whose reply save failed: the task to start once Retry saves it.
    /// Cancel drops it. Session-only.
    pub pending_reply_start: Option<Uuid>,
    /// A reply-box `ctrl+s` to a running agent whose save failed: the task whose reply goes
    /// to its agent once Retry saves it. Cancel drops it. Session-only.
    pub pending_reply_delivery: Option<Uuid>,
    /// A status change outside the reply box (`ctrl+b`, palette set status, a marked start)
    /// whose save failed: each task whose open record it closes, with how many records that
    /// task had closed before, so Retry can send what the record owes its agent. Cancel drops
    /// it. Session-only.
    pub pending_closing: Vec<(Uuid, usize)>,
    /// `y` on the start-anyway card: the replayed start skips the after check once.
    pub start_anyway: bool,
    /// What a completion's save released (`T203 started · T202 done`), joined onto the cleanup
    /// summary that owns the status row once the cleanup lands.
    pub released_note: Option<String>,
    /// Marked tasks in the order they were marked, for **chain in order**.
    pub(super) mark_order: Vec<Uuid>,
    /// Tasks a quick task-picker change saved: an open page form bound to one adopts the new
    /// list once the save lands, unless its own After draft changed.
    pub(super) pending_form_after_sync: Option<Vec<Uuid>>,
    /// The block card while it owns input.
    pub(super) block_card: Option<super::block::BlockCard>,
    /// The reply box open inline under a blocked board row.
    pub(crate) row_reply: Option<super::block::RowReply>,
    /// A bulk dispatch whose launches are still landing. One slot shared by the outer board and
    /// its project preview, so dropping or rebinding the preview never loses a running batch.
    pub(super) bulk_dispatch: SharedBulkDispatch,
    /// Whether the armed delete originated from a non-empty marked set.
    pub(super) pending_delete_bulk: bool,
    /// Open launch-card or save-recovery presentation.
    pub(super) popup: BoardPopup,
    /// Open palette (presentation only).
    pub(super) surface: CommandSurface,
    /// The last left-button press cell, held until release so a drag can grow a text
    /// selection out of it. Presentation-only; a plain click never reads it.
    pub(super) mouse_press: Option<Position>,
    /// `list_scroll` / notes scroll at the press, so the anchor stays on that content
    /// row while edge auto-scroll moves the viewport.
    pub(super) mouse_press_scroll: Option<usize>,
    /// List viewport offset. Scrollbar click/drag writes it; row click leaves it.
    pub(super) list_scroll: Cell<usize>,
    /// When true, the next paint nudges `list_scroll` so the selection is on screen
    /// (keyboard / reanchor). Row click and scrollbar leave it false.
    pub(super) follow_list: Cell<bool>,
    /// The live drag-selected screen region, cleared on the next press.
    pub(super) text_selection: Option<TextSelection>,
    /// Furthest list scroll the last painted frame could show (renderer-recorded).
    pub(super) list_max_scroll: Cell<usize>,
    /// When set, [`Self::message`] clears itself on the next idle tick after this instant.
    /// Sticky messages (errors, delete notices) leave this `None`.
    pub(super) message_expires_at: Option<Instant>,
    /// Sticky status restored when an ephemeral toast expires (e.g. save-recovery banner
    /// under a brief `copied` notice). Cleared by [`Self::set_message`] / [`Self::clear_message`].
    pub(super) message_restore: Option<String>,
    /// Palette query.
    pub(super) command_query: String,
    /// Selection into the currently visible command set.
    pub(super) command_selected: usize,
    /// Whether the last app-boundary presentation was wide enough to paint list and detail.
    pub(super) frame_wide: Cell<bool>,
    /// The questions tab: its session snapshot and its own cursor and page. Session-only.
    pub(crate) questions: crate::ui::questions::QuestionsState,
    /// The reports tab: its session snapshot and its own cursor and page. Session-only.
    pub(crate) reports: crate::ui::reports::ReportsState,
    /// Stall checks: the last Herdr agent snapshot and when each task was asked about.
    /// Session-only.
    pub(crate) stall: crate::stall::StallState,
    /// A fixed clock for every age the board paints; `None` reads the wall clock. Tests and
    /// goldens pin it so no frame depends on today's date.
    clock: Option<SystemTime>,
}

/// One dispatched task on a cleanup card, captured before the modal opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupRow {
    pub task_id: Uuid,
    pub number: u64,
    pub worktree: String,
    pub branch: String,
    pub base: String,
    pub dirty: bool,
    /// The uncommitted paths, for the card to name the first few.
    pub dirty_files: Vec<String>,
    /// Another worktree has the branch checked out: cleanup keeps it, merged or not.
    pub checked_out_elsewhere: bool,
    pub branch_merged: bool,
    pub workspace_exists: bool,
    pub warning: Option<String>,
    pub base_available: bool,
    /// The background fetch and ancestry recheck still running; the card shows `checking…`
    /// in place of the cached merged status until it lands.
    pub merge_check: Option<crate::dispatch::MergeCheck>,
    /// The dispatch record the card inspected. Cleanup touches the task only while this is
    /// still its record; `None` (a card built without one) never cleans.
    pub inspected: Option<crate::domain::Dispatch>,
    /// The background check landed without confirming anything (its ancestry query errored
    /// or timed out): the branch stays, whatever the refs on disk say later.
    pub check_failed: bool,
    /// The background check could not fetch this remote: the refs on disk confirm nothing,
    /// so the card says `not confirmed (offline)` and the branch stays.
    pub unreachable_remote: Option<String>,
    /// The row's pull request, through `gh`, and what `m` did with it.
    pub pr: CleanupPr,
}

/// A cleanup row's pull request: the card's lookup, then `m`'s merge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupPr {
    /// The lookup still running off the event loop.
    pub probe: Option<crate::dispatch::PrProbe>,
    /// What the lookup found.
    pub landed: Option<crate::dispatch::pr::PrProbeResult>,
    /// `m` merged this PR: the row's cleanup fetches the base itself rather than trusting the
    /// card's pre-merge check, and gh's answer confirms a squash or rebase.
    pub merged: Option<crate::dispatch::MergeOutcome>,
    /// `m`'s merge failed: gh's reason, on one line. Nothing about the task changed.
    pub merge_failed: Option<String>,
}

impl CleanupRow {
    /// Copy a landed PR lookup onto the row. True when one landed.
    pub fn poll_pr(&mut self) -> bool {
        let Some(result) = self.pr.probe.as_ref().and_then(|probe| probe.take()) else {
            return false;
        };
        self.pr.probe = None;
        self.pr.landed = Some(result);
        true
    }

    /// `m` would merge this row: clean, not merged yet, with an open PR into the recorded base,
    /// and no other worktree holding the branch (its cleanup would keep it).
    pub fn mergeable(&self) -> bool {
        !self.dirty
            && !self.checked_out_elsewhere
            && self.pr.merged.is_none()
            && match (&self.pr.landed, &self.inspected) {
                (Some(landed), Some(record)) => {
                    crate::dispatch::pr::mergeable(&landed.lookup, record).is_ok()
                }
                _ => false,
            }
    }

    /// GitHub merged this exact branch tip, though ancestry reads unmerged (a squash or rebase).
    pub fn pr_merged_at_tip(&self) -> bool {
        let base = self
            .inspected
            .as_ref()
            .and_then(crate::dispatch::pr::record_base_branch);
        match (&self.pr.landed, base) {
            (Some(landed), Some(base)) => landed.merged_at_tip(&base),
            _ => false,
        }
    }

    /// Why `m` did not merge this row: its failure, uncommitted changes, or why its PR is no
    /// candidate.
    pub fn merge_skip_reason(&self) -> String {
        if let Some(reason) = &self.pr.merge_failed {
            return reason.clone();
        }
        if self.dirty {
            return "uncommitted changes".into();
        }
        match (&self.pr.landed, &self.inspected) {
            (Some(landed), Some(record)) => crate::dispatch::pr::mergeable(&landed.lookup, record)
                .err()
                .map_or_else(|| "not merged".into(), |refusal| refusal.message()),
            _ => "pull request not checked yet".into(),
        }
    }

    /// The card's line about the PR: `None` without `gh`, so a card without it reads as before.
    pub fn pr_line(&self) -> Option<String> {
        use crate::dispatch::pr::{MergeRefusal, PrLookupError};
        if let Some(reason) = &self.pr.merge_failed {
            return Some(format!("merge failed: {reason}"));
        }
        if self.pr.probe.is_some() {
            return Some("checking pull request…".into());
        }
        let (landed, record) = (self.pr.landed.as_ref()?, self.inspected.as_ref()?);
        let number = match &landed.lookup {
            Ok(Some(pr)) => format!("PR #{}", pr.number),
            _ => String::new(),
        };
        Some(
            match crate::dispatch::pr::mergeable(&landed.lookup, record) {
                Ok(_) if self.dirty => {
                    format!("{number} open · commit or discard changes to merge")
                }
                Ok(_) => format!("{number} open"),
                Err(MergeRefusal::GhUnavailable(PrLookupError::Missing)) => return None,
                Err(MergeRefusal::GhUnavailable(error)) => error.message(),
                Err(MergeRefusal::NoPr) => "no pull request".into(),
                Err(MergeRefusal::NotOpen(crate::dispatch::pr::PrState::Merged))
                    if matches!(&landed.lookup, Ok(Some(pr))
                    if Some(&pr.base) != crate::dispatch::pr::record_base_branch(record).as_ref()) =>
                {
                    let into = match &landed.lookup {
                        Ok(Some(pr)) => pr.base.clone(),
                        _ => String::new(),
                    };
                    let base = crate::dispatch::pr::record_base_branch(record).unwrap_or_default();
                    format!("{number} merged into {into}, not {base}")
                }
                Err(MergeRefusal::NotOpen(state)) => format!("{number} {}", state.name()),
                Err(MergeRefusal::BaseMismatch { pr_base, base }) => {
                    format!("{number} targets {pr_base}, not {base}")
                }
            },
        )
    }

    /// The inspected record has no exact base ref, so no ancestry target is known and the
    /// branch stays whatever the refs say.
    pub fn no_recorded_base(&self) -> bool {
        self.inspected
            .as_ref()
            .is_some_and(|record| record.base_ref.is_none())
    }

    /// Copy a landed merged check onto the row. True when one landed.
    fn apply_landed_check(&mut self) -> bool {
        let Some(verdict) = self.merge_check.as_ref().and_then(|check| check.take()) else {
            return false;
        };
        self.merge_check = None;
        self.check_failed = !verdict.confirmed;
        self.unreachable_remote = verdict.unreachable_remote;
        self.branch_merged = verdict.branch_merged;
        self.base_available = verdict.base_available;
        self.warning = verdict.warning;
        true
    }

    pub fn checking(&self) -> bool {
        self.merge_check.is_some()
    }

    /// No completed check vouches for the merged status: cleanup keeps the branch.
    pub fn merge_unconfirmed(&self) -> bool {
        self.checking() || self.check_failed
    }

    /// `y` deletes the branch: a completed check confirmed the merge into an available base,
    /// or GitHub merged the branch's exact tip (a squash or rebase).
    pub fn branch_deletable(&self) -> bool {
        !self.merge_unconfirmed()
            && self.unreachable_remote.is_none()
            && self.base_available
            && (self.branch_merged || self.pr_merged_at_tip())
    }

    /// The ref policy `y` cleans this row with: the card already refreshed the base off the
    /// event loop, so cleanup never fetches; an unfinished, failed or offline check keeps
    /// the branch.
    pub fn cleanup_refs(&self) -> crate::dispatch::CleanupRefs {
        if self.pr.merged.is_some() {
            // The base moved on GitHub after the card checked it: fetch it again.
            crate::dispatch::CleanupRefs::Fetch
        } else if self.merge_unconfirmed() {
            crate::dispatch::CleanupRefs::Unconfirmed
        } else if self.unreachable_remote.is_some() {
            crate::dispatch::CleanupRefs::Offline
        } else {
            crate::dispatch::CleanupRefs::Cached
        }
    }

    /// `y` removes this worktree. A dirty worktree is never cleaned.
    pub fn cleanable(&self) -> bool {
        !self.dirty
    }
}

/// The marked set behind a bulk cleanup card: every target completes in one save.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BulkCleanup {
    /// The whole marked set, in mark order, including tasks without a live dispatch.
    pub targets: Vec<Uuid>,
    /// Board identifiers (`T12`) of targets with no live dispatch: marked done, nothing to clean.
    pub plain: Vec<String>,
    /// Targets whose recorded worktree was already gone, with the dispatch record inspected:
    /// on `y` or `n` they converge to cleaned only if that same record is still current and
    /// its worktree is still gone.
    pub missing: Vec<(Uuid, String, crate::domain::Dispatch)>,
    /// The `missing` targets whose agent pane (Herdr workspace) was still open: `y` and `m`
    /// close it.
    pub missing_panes: Vec<String>,
    /// Dispatched targets whose worktree could not be inspected, with the reason: kept, still
    /// marked done by `y` and `n`; `m` leaves them unchanged.
    pub refused: Vec<(Uuid, String, String)>,
}

/// Session-only start confirmation: the bulk card over a marked set where a start would
/// launch an agent, checked when `ctrl+s` opened it, or the relaunch card for one cursor task
/// whose agent is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchPrompt {
    /// The tasks `y` launches, in board order.
    pub launch: Vec<crate::dispatch::EligibleDispatch>,
    /// Marked tasks the card skips: the board identifier and the single-task refusal. They
    /// stay unstarted.
    pub skipped: Vec<(String, String)>,
    /// Marked tasks `y` only starts, in board order: unassigned, or already dispatched
    /// (relaunching stays cursor-only).
    pub start_only: Vec<(String, Uuid)>,
    /// Start-only rows that were done or archived when the card opened: the palette's absolute
    /// start corrects them to started. Only these may start while done or archived.
    pub corrections: Vec<Uuid>,
    /// Start-only rows assigned and never dispatched that cannot launch here, with why (not
    /// in Herdr, a desk task): `y` starts them plainly.
    pub no_launch: Vec<(Uuid, &'static str)>,
    /// Opened by the palette's absolute **set status: started** (any status but started
    /// moves) rather than `ctrl+s` (open and ready only); `y` rechecks the rows by it.
    pub any_status: bool,
    /// The relaunch card: one cursor task whose agent is gone. `y` relaunches it, `n` only
    /// starts it. `launch`, `skipped` and `start_only` are empty.
    pub relaunch: Option<RelaunchPrompt>,
    /// The start-anyway card: the start's targets still wait on tasks that are not done. `y`
    /// starts them anyway, through the same start route. Everything else is empty.
    pub start_anyway: Option<StartAnywayPrompt>,
    /// The git-repository check of the listed tasks, running off the event loop; rows show
    /// `checking…` until it lands.
    pub git_checks: Option<crate::dispatch::GitChecks>,
    /// First visible card row; the painter clamps it to the rows that fit.
    pub scroll: usize,
    /// The painter scrolls the `▸` option into view until a page key scrolls the card by hand.
    pub follow: bool,
    /// The `▸` cursor, the early-`Enter` guard and the double click.
    pub selector: OptionSelector<DispatchChoice>,
}

impl DispatchPrompt {
    pub fn checking(&self) -> bool {
        self.git_checks.is_some()
    }

    /// The relaunch card for one cursor task whose agent is gone.
    pub fn relaunch(prompt: RelaunchPrompt) -> Self {
        Self {
            launch: Vec::new(),
            skipped: Vec::new(),
            start_only: Vec::new(),
            corrections: Vec::new(),
            no_launch: Vec::new(),
            any_status: prompt.any_status,
            relaunch: Some(prompt),
            start_anyway: None,
            git_checks: None,
            scroll: 0,
            follow: true,
            selector: OptionSelector::default(),
        }
    }

    /// The start-anyway card over a start whose targets still wait.
    pub fn start_anyway(prompt: StartAnywayPrompt) -> Self {
        Self {
            launch: Vec::new(),
            skipped: Vec::new(),
            start_only: Vec::new(),
            corrections: Vec::new(),
            no_launch: Vec::new(),
            any_status: prompt.intent != crate::ui::input::BoardIntent::PrimaryVerb,
            relaunch: None,
            start_anyway: Some(prompt),
            git_checks: None,
            scroll: 0,
            follow: true,
            selector: OptionSelector::default(),
        }
    }
}

/// A start (`ctrl+s`, palette **set status: started**) whose targets still wait on tasks that
/// are not done, asking first. The marks stay, so `y` starts the same set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartAnywayPrompt {
    /// One line per waiting target: `T203 runs after T202 (started)`.
    pub waiting: Vec<String>,
    /// The start that asked, replayed on `y`.
    pub intent: crate::ui::input::BoardIntent,
}

/// A cursor task whose dispatched agent is gone, asking before a relaunch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaunchPrompt {
    pub task_id: Uuid,
    pub number: u64,
    /// The start's rule: any status but started moves (palette, reply box), or only open and
    /// ready (`ctrl+s`). `n` rechecks the task by it.
    pub any_status: bool,
    pub assignee: String,
    pub worktree: String,
}

/// A launch recorded on its task whose save has not been confirmed yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLaunch {
    pub task_id: Uuid,
    pub identifier: String,
    pub assignee: String,
    pub record: crate::domain::Dispatch,
    pub naming: Option<crate::dispatch::AgentNaming>,
    /// The human status kept instead of `started`, when it changed after `y`.
    pub kept_status: Option<String>,
}

/// A bulk dispatch in flight. Launches run off the event loop; each outcome is recorded and
/// saved as it lands, like a single dispatch.
#[derive(Debug, Clone)]
pub struct BulkDispatchRun {
    pub batch: crate::dispatch::LaunchBatch,
    /// Every task `y` confirmed, including any a recheck refused before launching.
    pub total: usize,
    /// Board identifiers of launched tasks whose record is saved, with their assignee.
    pub launched: Vec<(String, String)>,
    /// Saved launches whose task kept a status the human set after `y`.
    pub kept_status: Vec<(String, String)>,
    /// Recorded launches waiting on a save (a failed one is in save recovery).
    pub pending: Vec<PendingLaunch>,
    /// Launched agents whose record was discarded (a cancelled save): identifier, workspace.
    pub unrecorded: Vec<(String, String)>,
    /// Board identifiers of tasks whose launch or record failed, with the reason.
    pub failed: Vec<(String, String)>,
    /// Fetch fallbacks reported by launches (cached refs used).
    pub warnings: Vec<String>,
}

impl BulkDispatchRun {
    pub fn new(batch: crate::dispatch::LaunchBatch, total: usize) -> Self {
        Self {
            batch,
            total,
            launched: Vec::new(),
            kept_status: Vec::new(),
            pending: Vec::new(),
            unrecorded: Vec::new(),
            failed: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Every launch has landed and its record is saved or reported.
    pub fn settled(&self) -> bool {
        self.batch.finished() && self.pending.is_empty()
    }

    /// The status row while launches land, and the outcome once all have.
    pub fn message(&self) -> String {
        let total = self.total;
        let settled =
            self.launched.len() + self.failed.len() + self.unrecorded.len() + self.pending.len();
        let mut parts = Vec::new();
        if settled < total {
            parts.push(format!("dispatching {}/{total}…", settled + 1));
        }
        if !self.launched.is_empty() {
            let launched = self
                .launched
                .iter()
                .map(|(identifier, assignee)| format!("{identifier} to @{assignee}"))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("dispatched {launched}"));
        }
        for (identifier, status) in &self.kept_status {
            parts.push(format!("{identifier} kept {status} (changed meanwhile)"));
        }
        if !self.unrecorded.is_empty() {
            let unrecorded = self
                .unrecorded
                .iter()
                .map(|(identifier, workspace)| {
                    format!("{identifier} (agent running in workspace {workspace})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("launched but not recorded: {unrecorded}"));
        }
        for (identifier, reason) in &self.failed {
            parts.push(format!("{identifier} failed: {reason}"));
        }
        let mut warnings = self.warnings.clone();
        warnings.dedup();
        parts.extend(warnings);
        parts.join(" · ")
    }
}

/// The refusal when no marked task can be dispatched: one shared reason, or each task's own.
pub fn nothing_to_dispatch(skipped: &[(String, String)]) -> String {
    let shared = skipped
        .first()
        .filter(|(_, first)| skipped.iter().all(|(_, reason)| reason == first));
    match shared {
        Some((_, reason)) => format!("nothing to dispatch: {reason}"),
        None => format!(
            "nothing to dispatch: {}",
            skipped
                .iter()
                .map(|(identifier, reason)| format!("{identifier} {reason}"))
                .collect::<Vec<_>>()
                .join(" · ")
        ),
    }
}

/// The running bulk dispatch slot, shared between a board and its project preview.
#[derive(Debug, Clone, Default)]
pub struct SharedBulkDispatch(std::rc::Rc<std::cell::RefCell<Option<BulkDispatchRun>>>);

/// Session-only cleanup confirmation: one row for the cursor task, or one per live dispatch
/// in a marked set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupPrompt {
    pub rows: Vec<CleanupRow>,
    /// `Some` for a marked set; `None` for the single cursor card.
    pub bulk: Option<BulkCleanup>,
    /// First visible card row; the painter clamps it to the rows that fit.
    pub scroll: usize,
    /// The painter scrolls the `▸` option into view: from the card opening, after an arrow
    /// and after a resize, until a page key scrolls the card by hand.
    pub follow: bool,
    /// `m`'s merges running off the event loop, with the task each job slot merges.
    pub merging: Option<(crate::dispatch::pr::MergeJob, Vec<Uuid>)>,
    /// Why `m` merged nothing before it started, such as an invalid `[merge]` setting.
    pub merge_note: Option<String>,
    /// `[merge] method`, for the `m` line's verb (`squash-merge PR #7`).
    pub method: crate::agents::MergeMethod,
    /// The `▸` cursor, the early-`Enter` guard and the double click.
    pub selector: OptionSelector<CleanupChoice>,
    /// `m` went nothing on: one line per task saying why it is unchanged. The card then only
    /// closes.
    pub settled: Option<Vec<(String, String)>>,
}

impl CleanupPrompt {
    pub fn single(row: CleanupRow) -> Self {
        Self::new(vec![row], None)
    }

    /// The card over a marked set.
    pub fn bulk(rows: Vec<CleanupRow>, bulk: BulkCleanup) -> Self {
        Self::new(rows, Some(bulk))
    }

    fn new(rows: Vec<CleanupRow>, bulk: Option<BulkCleanup>) -> Self {
        Self {
            rows,
            bulk,
            scroll: 0,
            follow: true,
            merging: None,
            merge_note: None,
            method: crate::agents::MergeMethod::default(),
            selector: OptionSelector::default(),
            settled: None,
        }
    }

    /// Whether `m` is offered: at least one listed PR can be merged.
    pub fn can_merge_any(&self) -> bool {
        self.merging.is_none() && self.rows.iter().any(CleanupRow::mergeable)
    }

    /// `m` is merging: the card waits for gh and takes no choice until it lands.
    pub fn merging(&self) -> bool {
        self.merging.is_some()
    }

    pub fn checking(&self) -> bool {
        self.rows.iter().any(CleanupRow::checking)
    }

    /// Whether `y` is offered: at least one listed worktree can be removed.
    pub fn can_clean_any(&self) -> bool {
        self.rows.iter().any(CleanupRow::cleanable)
    }
}

/// Longest a quit waits for a running cleanup's git and Herdr steps before exiting anyway. A
/// row cut off here is never marked cleaned, and its dispatch converges on the next cleanup.
pub const QUIT_CLEANUP_BOUND: Duration = Duration::from_secs(30);

/// `ctrl+d` while a confirmed cleanup still runs.
pub const CLEANUP_BUSY: &str = "cleanup still running; try again when it finishes";

/// The status row while quit waits on a running cleanup.
pub const FINISHING_CLEANUP: &str = "finishing cleanup…";

/// How long a clean cleanup outcome stays on the status row.
pub const CLEANUP_SUMMARY_TTL: Duration = Duration::from_secs(4);

/// Longest status-row summary a cleanup names tasks in; past it, only counts.
const CLEANUP_SUMMARY_WIDTH: usize = 72;

/// The running cleanup, shared between a board and its project preview: dropping or rebinding
/// the preview never loses the worker's handle, its cleaned markers, or the busy and quit
/// guards.
#[derive(Debug, Clone, Default)]
pub struct SharedCleanupRun(std::rc::Rc<std::cell::RefCell<Option<CleanupRun>>>);

/// What one row of a confirmed cleanup shows on its card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupRowState {
    /// Its merged check is still running: the worker starts once every check lands.
    Checking,
    Queued,
    Removing,
    /// Removed. `branch_kept` holds the short and full reason the branch stayed, if it did.
    Cleaned {
        branch_kept: Option<(String, String)>,
        /// The worktree was already gone when the worker reached it.
        worktree_gone: bool,
        /// That gone worktree's workspace, still open: closed (`Ok`), or why Herdr kept it.
        workspace: Option<Result<(), String>>,
        /// The cleanup's fetch warning.
        warning: Option<String>,
    },
    /// Left in place, with a short reason for the status row and a full one for the card.
    Kept {
        short: String,
        full: String,
    },
}

impl CleanupRowState {
    /// Removed, with nothing else to say but the branch.
    pub fn cleaned(branch_kept: Option<(String, String)>) -> Self {
        Self::Cleaned {
            branch_kept,
            worktree_gone: false,
            workspace: None,
            warning: None,
        }
    }

    /// A worktree the cleanup left in place. The card leads with `kept:`, so the full reason
    /// is the bare one, never the CLI line that already carries it.
    pub fn kept(error: &crate::dispatch::CleanupError) -> Self {
        Self::Kept {
            short: error.short().into(),
            full: error.reason(),
        }
    }

    pub fn landed(&self) -> bool {
        matches!(self, Self::Cleaned { .. } | Self::Kept { .. })
    }

    /// Anything the user should not miss: a kept worktree, branch, or workspace, or a warning.
    fn exception(&self) -> bool {
        matches!(
            self,
            Self::Kept { .. }
                | Self::Cleaned {
                    branch_kept: Some(_),
                    ..
                }
                | Self::Cleaned {
                    workspace: Some(Err(_)),
                    ..
                }
                | Self::Cleaned {
                    warning: Some(_),
                    ..
                }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupRunRow {
    pub task_id: Uuid,
    pub number: u64,
    /// This row's slot in the job; `None` for a row kept before any host work (dirty, or
    /// changed since the card opened).
    pub slot: Option<usize>,
    /// The dispatch the card inspected: the cleaned marker lands only while it is current.
    pub inspected: Option<crate::domain::Dispatch>,
    pub state: CleanupRowState,
}

/// A confirmed cleanup: completion is already saved, and the host work runs off the event
/// loop while the board keeps painting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupRun {
    pub job: crate::dispatch::CleanupJob,
    pub rows: Vec<CleanupRunRow>,
    /// A marked set's card: how many tasks it completed and how many worktrees were
    /// already gone. `None` for the single cursor card.
    pub bulk: Option<(usize, usize)>,
    /// A marked set's dispatched targets the card could not inspect: identifier and reason.
    pub refused: Vec<(String, String)>,
    /// The job reported every row's git and Herdr work done.
    pub settled: bool,
    /// The card rows the worker will clean, in job-slot order, with their merged checks.
    /// Completion is saved at `y`; the worker starts once every check lands or at
    /// `start_deadline`, cleaning each row with the refs its check confirmed.
    pub waiting: Vec<CleanupRow>,
    /// One plan row per job slot; refs are filled in when the worker starts.
    pub plan: Vec<crate::dispatch::CleanupPlanRow>,
    pub start_deadline: Instant,
    pub started: bool,
    /// `m`: each merged task, its PR, and what became of the remote branch.
    pub merged: Vec<MergedPr>,
    /// Tasks the card left exactly as they were, with why (gh's reason, uncommitted files,
    /// not merged under `m`).
    pub unmerged: Vec<(String, String)>,
    /// A marked set's tasks whose worktree was already gone and whose workspace was still
    /// open: closed once the completion saved, or why it stayed open.
    pub workspaces: Vec<(String, Result<(), String>)>,
}

/// One pull request a cleanup card's `m` merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedPr {
    /// `T12`.
    pub identifier: String,
    /// `PR #7 (squash)`.
    pub pr: String,
    /// Whether the PR's branch was still on the remote after the merge; `None`: unknown.
    pub remote_left: Option<bool>,
}

impl MergedPr {
    /// What became of the remote branch, when known. tsk never deletes it.
    pub fn remote_note(&self) -> Option<&'static str> {
        match self.remote_left {
            Some(true) => Some("remote branch kept"),
            Some(false) => Some("remote branch deleted by GitHub"),
            None => None,
        }
    }
}

impl CleanupRun {
    pub fn finished(&self) -> bool {
        self.started && self.settled && self.rows.iter().all(|row| row.state.landed())
    }

    /// Apply landed merged checks to the rows still waiting. True when the worker may start:
    /// every check landed, or the bound passed (unconfirmed rows then keep their branch).
    pub fn poll_checks(&mut self) -> bool {
        for (slot, row) in self.waiting.iter_mut().enumerate() {
            if !row.apply_landed_check() {
                continue;
            }
            if let Some(run_row) = self
                .rows
                .iter_mut()
                .find(|run_row| run_row.slot == Some(slot))
            {
                if run_row.state == CleanupRowState::Checking {
                    run_row.state = CleanupRowState::Queued;
                }
            }
        }
        !self.waiting.iter().any(CleanupRow::checking) || Instant::now() >= self.start_deadline
    }

    /// Rows the worker handles, and how many of them have landed.
    pub fn progress(&self) -> (usize, usize) {
        let worked = self.rows.iter().filter(|row| row.slot.is_some());
        let total = worked.clone().count();
        (worked.filter(|row| row.state.landed()).count(), total)
    }

    pub fn progress_message(&self) -> String {
        if !self.started {
            return "cleanup waits for merge checks…".into();
        }
        let (landed, total) = self.progress();
        format!("cleaning {} of {total}…", (landed + 1).min(total.max(1)))
    }

    /// The merge `m` made for a task, if any.
    pub fn merged_for(&self, identifier: &str) -> Option<&MergedPr> {
        self.merged
            .iter()
            .find(|merged| merged.identifier == identifier)
    }

    /// One task's outcome after `Done T7 · ` or `T7  `: what merged, what was cleaned, what
    /// stayed. `None` while its row still works.
    pub fn row_outcome(&self, row: &CleanupRunRow) -> Option<String> {
        let identifier = format!("T{}", row.number);
        let merged = self.merged_for(&identifier);
        let mut parts = Vec::new();
        if let Some(merged) = merged {
            parts.push(format!("merged {}", merged.pr));
        }
        match &row.state {
            CleanupRowState::Cleaned {
                branch_kept,
                worktree_gone,
                workspace,
                warning,
            } => {
                if self.bulk.is_some() {
                    parts.push("done".into());
                }
                parts.push(if *worktree_gone {
                    "worktree already gone".into()
                } else {
                    "cleaned".into()
                });
                match workspace {
                    Some(Ok(())) => parts.push("workspace closed".into()),
                    Some(Err(why)) => parts.push(why.clone()),
                    None => {}
                }
                match branch_kept {
                    Some(_) if *worktree_gone => parts.push("branch kept".into()),
                    Some((short, _)) => parts.push(format!("branch kept ({short})")),
                    None => {}
                }
                parts.extend(warning.clone());
            }
            CleanupRowState::Kept { full, .. } => {
                if self.bulk.is_some() {
                    parts.push("done".into());
                }
                parts.push(format!("cleanup refused: {full}"));
            }
            CleanupRowState::Checking | CleanupRowState::Queued | CleanupRowState::Removing => {
                return None
            }
        }
        if let Some(note) = merged.and_then(MergedPr::remote_note) {
            parts.push(note.into());
        }
        Some(parts.join(" · "))
    }

    /// The status-row outcome: counts, naming only the tasks that kept something. The bool
    /// is true when nothing was kept, so the line may expire on its own.
    pub fn summary(&self) -> (String, bool) {
        let (text, clean) = self.cleanup_summary();
        let clean = clean && self.unmerged.is_empty();
        if self.bulk.is_none() {
            return (text, clean);
        }
        let mut parts = Vec::new();
        if !self.merged.is_empty() {
            parts.push(format!("merged {}", self.merged.len()));
        }
        parts.push(text);
        match self
            .merged
            .iter()
            .filter(|merged| merged.remote_left == Some(true))
            .count()
        {
            0 => {}
            1 => parts.push("1 remote branch kept".into()),
            kept => parts.push(format!("{kept} remote branches kept")),
        }
        if !self.unmerged.is_empty() {
            parts.push(format!("unchanged {}", self.unmerged.len()));
        }
        (parts.join(" · "), clean)
    }

    fn cleanup_summary(&self) -> (String, bool) {
        let clean = self.refused.is_empty()
            && !self.rows.iter().any(|row| row.state.exception())
            && self.workspaces.iter().all(|(_, closed)| closed.is_ok());
        let Some((done, missing)) = self.bulk else {
            let Some(row) = self.rows.first() else {
                return ("done".into(), true);
            };
            let text = match &row.state {
                CleanupRowState::Kept { short, .. } => {
                    format!("done T{} · kept ({short})", row.number)
                }
                _ => match self.row_outcome(row) {
                    Some(outcome) => format!("done T{} · {outcome}", row.number),
                    None => format!("done T{}", row.number),
                },
            };
            return (text, clean);
        };
        let cleaned = self
            .rows
            .iter()
            .filter(|row| matches!(row.state, CleanupRowState::Cleaned { .. }))
            .count();
        let kept = self
            .rows
            .iter()
            .filter_map(|row| match &row.state {
                CleanupRowState::Kept { short, .. } => Some(format!("T{} ({short})", row.number)),
                _ => None,
            })
            .chain(
                self.refused
                    .iter()
                    .map(|(identifier, _)| format!("{identifier} (can't inspect)")),
            )
            .collect::<Vec<_>>();
        let branches = self
            .rows
            .iter()
            .filter_map(|row| match &row.state {
                CleanupRowState::Cleaned {
                    branch_kept: Some((short, _)),
                    ..
                } => Some(format!("T{} ({short})", row.number)),
                _ => None,
            })
            .collect::<Vec<_>>();
        // Gone worktrees' workspaces: the card's missing rows, then rows the worker found gone.
        let gone = self
            .workspaces
            .iter()
            .map(|(identifier, closed)| (identifier.clone(), closed.is_ok()))
            .chain(self.rows.iter().filter_map(|row| match &row.state {
                CleanupRowState::Cleaned {
                    workspace: Some(closed),
                    ..
                } => Some((format!("T{}", row.number), closed.is_ok())),
                _ => None,
            }))
            .collect::<Vec<_>>();
        let closed = gone.iter().filter(|(_, closed)| *closed).count();
        let open = gone
            .iter()
            .filter(|(_, closed)| !closed)
            .map(|(identifier, _)| identifier.as_str())
            .collect::<Vec<_>>();
        let build = |named: bool| {
            let mut parts = vec![format!("done {done}")];
            if cleaned > 0 {
                parts.push(format!("cleaned {cleaned}"));
            }
            if !kept.is_empty() {
                parts.push(if named {
                    format!("kept {}", kept.join(", "))
                } else {
                    format!("kept {}", kept.len())
                });
            }
            if !branches.is_empty() {
                parts.push(if named {
                    format!("branch kept {}", branches.join(", "))
                } else {
                    format!("branch kept {}", branches.len())
                });
            }
            if missing > 0 {
                parts.push(format!("{missing} already gone"));
            }
            if closed > 0 {
                parts.push(format!(
                    "{closed} {} closed",
                    if closed == 1 {
                        "workspace"
                    } else {
                        "workspaces"
                    }
                ));
            }
            if !open.is_empty() {
                parts.push(if named {
                    format!("workspace kept {}", open.join(", "))
                } else {
                    format!("workspace kept {}", open.len())
                });
            }
            parts.join(" · ")
        };
        let named = build(true);
        let text = if named.chars().count() <= CLEANUP_SUMMARY_WIDTH {
            named
        } else {
            build(false)
        };
        (text, clean)
    }
}

/// How an unresolved failed save ended.
///
/// The two outcomes differ in exactly one way the chrome row cares about: Retry makes the
/// working state durable, Cancel throws it away. See [`BoardModel::end_save_recovery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveResolution {
    /// Retry succeeded: what the failed save carried is now on disk.
    Retried,
    /// Cancel restored the pre-failure baseline: none of it happened.
    Cancelled,
}

impl BoardModel {
    /// The time every painted age is measured against.
    pub fn now(&self) -> SystemTime {
        self.clock.unwrap_or_else(SystemTime::now)
    }

    /// Pin the board's clock (tests and goldens); `None` returns to the wall clock.
    pub fn set_clock(&mut self, clock: Option<SystemTime>) {
        self.clock = clock;
    }

    /// Build a board model with queue-local session state only. Every scope is the
    /// default; [`Self::from_domain`] applies directory-aware startup.
    pub fn from_tasks(tasks: Vec<Task>, this_repo: Option<PathBuf>) -> Self {
        let mut model = Self {
            clock: None,
            tasks,
            archived_projects: BTreeSet::new(),
            hidden_assignees: BTreeSet::new(),
            agent_names: Vec::new(),
            this_repo: this_repo.clone(),
            default_branches: DefaultBranchCache::default(),
            board_location: BoardLocation::All,
            selected_project: this_repo,
            wide_stage: WideStage::Board,
            sort: BoardSort::Project,
            layout_defaults: crate::agents::BoardDefaults::default(),
            messages_hidden: false,
            search_query: String::new(),
            search_pinned: false,
            list_picker: None,
            base_picker_results: Arc::default(),
            base_picker_workers: Arc::default(),
            drawer_open: false,
            show_status_lines: false,
            archived_collapsed: true,
            inbox_collapsed: false,
            launch_card: None,
            launch_card_shown: false,
            session_default_scope: None,
            selection_id: None,
            mark_mode: false,
            marked_ids: BTreeSet::new(),
            page_back: Vec::new(),
            last_row_click: None,
            help_query: String::new(),
            help_scroll: 0,
            help_max_scroll: Cell::new(usize::MAX),
            cleanup_max_scroll: Cell::new(usize::MAX),
            cleanup_layout: std::cell::RefCell::default(),
            save_recovery_proxy: false,
            help_return_mode: BoardInputMode::Normal,
            search_return_mode: BoardInputMode::Normal,
            input_mode: BoardInputMode::Normal,
            form: None,
            quick_add: None,
            saved_task: None,
            quick_add_save: None,
            task_edit_save: None,
            pending_form_assignee_sync: None,
            pending_form_base_sync: None,
            hold_task_edit_save: false,
            message: None,
            update_notice: None,
            delete_notice: None,
            delete_notice_count: None,
            delete_notice_label: None,
            delete_notice_expires_at: None,
            undo_hint: None,
            undo_hint_before_save: None,
            suspended_delete_notice: None,
            suspended_delete_notice_count: None,
            suspended_delete_notice_label: None,
            pending_delete: None,
            cleanup_prompt: None,
            cleanup_run: SharedCleanupRun::default(),
            quit_after_cleanup: None,
            cleanup_status: None,
            dispatch_prompt: None,
            pending_reply_start: None,
            pending_reply_delivery: None,
            pending_closing: Vec::new(),
            start_anyway: false,
            released_note: None,
            mark_order: Vec::new(),
            pending_form_after_sync: None,
            block_card: None,
            row_reply: None,
            bulk_dispatch: SharedBulkDispatch::default(),
            pending_delete_bulk: false,
            popup: BoardPopup::None,
            surface: CommandSurface::None,
            command_query: String::new(),
            command_selected: 0,
            frame_wide: Cell::new(true),
            mouse_press: None,
            mouse_press_scroll: None,
            list_scroll: Cell::new(0),
            follow_list: Cell::new(true),
            text_selection: None,
            list_max_scroll: Cell::new(0),
            message_expires_at: None,
            message_restore: None,
            questions: crate::ui::questions::QuestionsState::default(),
            reports: crate::ui::reports::ReportsState::default(),
            stall: crate::stall::StallState::default(),
        };
        model.seed_selection();
        model
    }

    /// Current detail popup (status picker or more menu).
    pub fn popup(&self) -> BoardPopup {
        self.popup
    }

    pub fn set_popup(&mut self, popup: BoardPopup) {
        self.close_help();
        self.popup = popup;
    }

    /// Dismiss help and restore the non-input surface it covered.
    pub fn close_help(&mut self) {
        if self.input_mode == BoardInputMode::Help {
            self.input_mode = self.help_return_mode;
            self.help_query.clear();
            self.help_scroll = 0;
        }
    }

    pub fn close_popup(&mut self) {
        if self.popup != BoardPopup::SaveRecovery {
            let cleanup = matches!(
                self.popup,
                BoardPopup::CleanupConfirm | BoardPopup::DispatchConfirm
            );
            self.popup = BoardPopup::None;
            self.cleanup_prompt = None;
            self.dispatch_prompt = None;
            self.block_card = None;
            if cleanup {
                self.clear_message();
            }
        }
    }

    /// Open a cleanup card. The single card is cursor-only and clears marks; a bulk card keeps
    /// them, so `Esc` returns to the same marked set.
    pub fn begin_cleanup_prompt(&mut self, mut prompt: CleanupPrompt) {
        // The early-Enter guard counts from the card appearing, not from its inspection.
        prompt.selector.opened_at = Instant::now();
        if prompt.bulk.is_none() {
            self.clear_marks();
        }
        self.cleanup_max_scroll.set(usize::MAX);
        self.cleanup_layout.take();
        self.close_help();
        self.close_command_surface();
        self.cleanup_prompt = Some(prompt);
        self.popup = BoardPopup::CleanupConfirm;
        self.clear_message();
    }

    pub fn cleanup_prompt(&self) -> Option<&CleanupPrompt> {
        self.cleanup_prompt.as_ref()
    }

    pub fn cleanup_run(&self) -> Option<std::cell::Ref<'_, CleanupRun>> {
        std::cell::Ref::filter_map(self.cleanup_run.0.borrow(), Option::as_ref).ok()
    }

    pub fn cleanup_run_mut(&self) -> Option<std::cell::RefMut<'_, CleanupRun>> {
        std::cell::RefMut::filter_map(self.cleanup_run.0.borrow_mut(), Option::as_mut).ok()
    }

    /// Show a confirmed cleanup's progress on the card that confirmed it.
    pub fn begin_cleanup_run(&mut self, run: CleanupRun) {
        if let Some(prompt) = self.cleanup_prompt.as_mut() {
            prompt.scroll = 0;
        }
        self.cleanup_max_scroll.set(usize::MAX);
        *self.cleanup_run.0.borrow_mut() = Some(run);
    }

    /// Whether this board shows the running cleanup's card.
    pub fn cleanup_card_open(&self) -> bool {
        self.popup == BoardPopup::CleanupConfirm
            && self.cleanup_prompt.is_some()
            && self.cleanup_running()
    }

    /// A confirmed cleanup is running on this board or its project preview (one shared slot).
    pub fn cleanup_running(&self) -> bool {
        self.cleanup_run.0.borrow().is_some()
    }

    /// Drop a finished run and its card. Returns the status-row summary to post.
    pub fn finish_cleanup_run(&mut self) -> Option<(String, bool)> {
        let run = self.cleanup_run.0.borrow_mut().take()?;
        if self.popup == BoardPopup::CleanupConfirm {
            self.close_popup();
        }
        self.cleanup_status = None;
        Some(run.summary())
    }

    /// Esc or `[x]` on the cleanup card. Before `y` it cancels (never while `m` merges); while
    /// the run works it hides
    /// the card (completion stays saved, cleanup continues, the status row reports it); once
    /// the run finished it closes with the summary.
    pub fn cancel_cleanup_card(&mut self) {
        // `m` is merging: the card stays until gh answers, so the merge lands on it.
        if self
            .cleanup_prompt
            .as_ref()
            .is_some_and(CleanupPrompt::merging)
        {
            return;
        }
        // A marked set's `m` that went on with nothing keeps the marks for a retry, and says
        // on the status row why nothing merged.
        let settled = self
            .cleanup_prompt
            .as_ref()
            .filter(|prompt| prompt.bulk.is_some())
            .and_then(|prompt| prompt.settled.as_ref())
            .map(|settled| {
                std::iter::once("nothing merged · marks kept".to_string())
                    .chain(
                        settled
                            .iter()
                            .map(|(identifier, reason)| format!("{identifier} {reason}")),
                    )
                    .collect::<Vec<_>>()
                    .join(" · ")
            });
        let finished = self.cleanup_run().is_some_and(|run| run.finished());
        if let Some(summary) = settled {
            self.close_popup();
            self.post_cleanup_summary(summary, false);
        } else if !finished {
            self.close_popup();
        } else if let Some((summary, clean)) = self.finish_cleanup_run() {
            self.post_cleanup_summary(summary, clean);
        }
    }

    /// A cleanup outcome on the status row: a clean one expires on its own, one that kept
    /// anything stays until the next action so it is not missed.
    pub fn post_cleanup_summary(&mut self, summary: String, clean: bool) {
        // A refusal that only said the cleanup was still running is obsolete now: it must
        // neither stay up nor come back when an expiring summary restores what it covered.
        self.drop_cleanup_refusals();
        let summary = match self.released_note.take() {
            Some(note) => format!("{summary} · {note}"),
            None => summary,
        };
        if clean {
            self.set_ephemeral_message(summary, CLEANUP_SUMMARY_TTL);
        } else {
            self.set_message(summary);
        }
    }

    /// No cleanup runs: a "still running" refusal on this board is obsolete.
    pub fn drop_cleanup_refusals(&mut self) {
        self.drop_cleanup_refusal();
    }

    fn drop_cleanup_refusal(&mut self) {
        if self.message.as_deref() == Some(CLEANUP_BUSY) {
            self.message = None;
            self.message_expires_at = None;
            self.message_restore = None;
        }
        if self.message_restore.as_deref() == Some(CLEANUP_BUSY) {
            self.message_restore = None;
        }
    }

    /// Point the hidden-cleanup status slot at `status` (`None` clears it).
    pub fn set_cleanup_status(&mut self, status: Option<String>) {
        self.cleanup_status = status;
    }

    /// Rows a cleanup card's `m` is still merging on this board.
    fn merging_rows(&self) -> usize {
        self.cleanup_prompt
            .as_ref()
            .and_then(|prompt| prompt.merging.as_ref())
            .map_or(0, |(_, ids)| ids.len())
    }

    /// A cleanup card's `m` is merging: quit waits for it.
    pub fn merge_running(&self) -> bool {
        self.merging_rows() > 0
    }

    /// Quit with a cleanup still running, or a card's `m` still merging: hide the card and
    /// wait, visibly and bounded, for its git and Herdr steps to land. A merging card stays
    /// (its merges land on it), and the bound grows by the gh bound of each merging row, so
    /// the merge lands, done saves, and its cleanup runs before the board exits.
    pub fn begin_quit_after_cleanup(&mut self) {
        if self.quit_after_cleanup.is_none() {
            let merges = crate::dispatch::pr::MERGE_ROW_BOUND
                * u32::try_from(self.merging_rows()).unwrap_or(u32::MAX);
            self.quit_after_cleanup = Some(Instant::now() + QUIT_CLEANUP_BOUND + merges);
        }
        let merging = |board: &BoardModel| {
            board
                .cleanup_prompt
                .as_ref()
                .is_some_and(CleanupPrompt::merging)
        };
        if self.popup == BoardPopup::CleanupConfirm && !merging(self) {
            self.close_popup();
        }
        self.clear_message();
        self.cleanup_status = Some(FINISHING_CLEANUP.into());
    }

    pub fn quitting_after_cleanup(&self) -> bool {
        self.quit_after_cleanup.is_some()
    }

    /// A quit that waited on cleanup may exit: the merges landed and the run finished, or its
    /// bound passed.
    pub fn quit_after_cleanup_due(&self) -> bool {
        self.quit_after_cleanup.is_some_and(|deadline| {
            (!self.cleanup_running() && !self.merge_running()) || Instant::now() >= deadline
        })
    }

    /// Open the bulk dispatch card. It keeps the marks, so `Esc` returns to the same set.
    pub fn begin_dispatch_prompt(&mut self, mut prompt: DispatchPrompt) {
        // The early-Enter guard counts from the card appearing.
        prompt.selector.opened_at = Instant::now();
        self.cleanup_max_scroll.set(usize::MAX);
        self.cleanup_layout.take();
        self.close_help();
        self.close_command_surface();
        self.dispatch_prompt = Some(prompt);
        self.popup = BoardPopup::DispatchConfirm;
        self.clear_message();
    }

    /// The block card while it owns input.
    pub fn block_card(&self) -> Option<&super::block::BlockCard> {
        self.block_card.as_ref()
    }

    /// The task page's selected BLOCKED stop, if any. A selected step outranks it.
    pub fn block_target(&self) -> Option<super::block::BlockTarget> {
        super::block::selected_block_target(self)
    }

    /// Whether `Enter` on the task page lands on a block option.
    pub fn block_option_selected(&self) -> bool {
        matches!(
            self.block_target(),
            Some(super::block::BlockTarget::Option(_))
        )
    }

    /// Whether `Enter` on the task page lands on the BLOCKED or REVIEW top line.
    pub fn block_heading_selected(&self) -> bool {
        self.block_target() == Some(super::block::BlockTarget::Heading)
    }

    /// Whether `Enter` on the task page lands on a review check.
    pub fn review_check_selected(&self) -> bool {
        matches!(
            self.block_target(),
            Some(super::block::BlockTarget::Check(_))
        )
    }

    /// Whether `Enter` on the task page lands on a closed record on the PAPER TRAIL.
    pub fn trail_record_selected(&self) -> bool {
        matches!(
            self.block_target(),
            Some(super::block::BlockTarget::Trail(_))
        )
    }

    /// Whether `Enter` on the task page lands on the PAPER TRAIL heading.
    pub fn trail_heading_selected(&self) -> bool {
        self.block_target() == Some(super::block::BlockTarget::TrailHeading)
    }

    /// Whether the task page shows its BLOCKED or REVIEW section (view mode, no edit session).
    pub fn page_section_open(&self) -> bool {
        super::block::page_section_open(self)
    }

    /// Open the empty reply (feedback) box on the page's BLOCKED or REVIEW section.
    pub fn open_page_reply_box(&mut self) -> bool {
        super::block::begin_reply(self, "", None)
    }

    /// Whether `Enter` on the task page lands on the `N passed` line.
    pub fn passed_checks_selected(&self) -> bool {
        self.block_target() == Some(super::block::BlockTarget::PassedFold)
    }

    /// Whether the open reply box still answers the task's open record; otherwise it says so
    /// and keeps the draft.
    pub fn reply_box_current(&mut self, domain: &crate::domain::DomainState) -> bool {
        super::block::reply_box_current(domain, self)
    }

    /// Close the reply box without saving, back to the page or the board.
    pub fn close_reply_box(&mut self) {
        super::block::cancel_reply(self);
    }

    /// Whether the open reply box gives feedback on a review round.
    pub fn reply_is_feedback(&self) -> bool {
        super::block::active_reply(self).is_some_and(|(_, editor)| editor.review)
    }

    /// The task whose reply box is open, while it is open.
    pub fn reply_task_id(&self) -> Option<Uuid> {
        super::block::active_reply(self).map(|(id, _)| id)
    }

    /// The reply box's draft while it is open.
    pub fn reply_draft(&self) -> Option<&str> {
        super::block::active_reply(self).map(|(_, editor)| editor.buffer.value())
    }

    /// The task whose reply box is open inline under its board row.
    pub fn row_reply_task(&self) -> Option<Uuid> {
        self.row_reply.as_ref().map(|row| row.task)
    }

    pub fn dispatch_prompt(&self) -> Option<&DispatchPrompt> {
        self.dispatch_prompt.as_ref()
    }

    /// Close the card and take what it would launch.
    pub fn take_dispatch_prompt(&mut self) -> Option<DispatchPrompt> {
        let prompt = self.dispatch_prompt.take()?;
        self.close_popup();
        Some(prompt)
    }

    /// Hand the running batch to the board and its project preview (one shared slot).
    pub fn begin_bulk_dispatch(&mut self, run: BulkDispatchRun) {
        *self.bulk_dispatch.0.borrow_mut() = Some(run);
    }

    /// Take the running batch out to land outcomes; put it back unless it has settled.
    pub fn take_bulk_dispatch(&mut self) -> Option<BulkDispatchRun> {
        self.bulk_dispatch.0.borrow_mut().take()
    }

    /// A bulk dispatch started on this board or its project preview is still landing.
    pub fn bulk_dispatch_running(&self) -> bool {
        self.bulk_dispatch.0.borrow().is_some()
    }

    /// Apply a landed git check to the open bulk dispatch card (this board's or its project
    /// preview's): tasks outside a git repository move to the skipped rows. A card left with
    /// nothing to launch closes with the refusal, keeping the marks. True when anything changed.
    pub fn poll_dispatch_checks(&mut self) -> bool {
        let Some(prompt) = self.dispatch_prompt.as_mut() else {
            return false;
        };
        let Some(results) = prompt.git_checks.as_ref().and_then(|checks| checks.take()) else {
            return false;
        };
        prompt.git_checks = None;
        let (launch, outside): (Vec<_>, Vec<_>) = std::mem::take(&mut prompt.launch)
            .into_iter()
            .partition(|eligible| results.get(eligible.project()).copied().unwrap_or(false));
        prompt.launch = launch;
        for eligible in outside {
            prompt.skipped.push((
                format!("T{}", eligible.number),
                "not a project in a git repo".into(),
            ));
        }
        prompt.skipped.sort_by_key(|(identifier, _)| {
            identifier
                .strip_prefix('T')
                .and_then(|number| number.parse::<u64>().ok())
        });
        // With nothing left to launch, the card stays only for what `y` still starts.
        if prompt.launch.is_empty() && prompt.start_only.is_empty() {
            let message = nothing_to_dispatch(&prompt.skipped);
            self.close_popup();
            self.set_message(message);
        }
        true
    }

    pub fn cleanup_prompt_mut(&mut self) -> Option<&mut CleanupPrompt> {
        self.cleanup_prompt.as_mut()
    }

    /// Apply a finished background merged check to the open cleanup card (this board's or its
    /// project preview's). A check whose card was closed is dropped with it, so a late result
    /// never reaches another prompt.
    pub fn poll_cleanup_check(&mut self) -> bool {
        let Some(prompt) = self.cleanup_prompt.as_mut() else {
            return false;
        };
        let mut landed = false;
        for row in &mut prompt.rows {
            landed |= row.apply_landed_check();
            landed |= row.poll_pr();
        }
        landed
    }

    /// Make an unresolved failed save visible and reachable on whichever board owns input, and
    /// take a resolved one down from both boards. A queued cleanup can finish (and fail its
    /// save) inside a project preview that a narrowing frame has parked, and the frame can
    /// widen or narrow again before the user answers Retry or Cancel.
    ///
    /// The board that failed keeps owning the recovery (its held form, its drafts); a board
    /// that only shows it for input is a proxy, and Retry or Cancel resolve the owner.
    pub fn present_save_recovery(&mut self, pending: Option<&str>) {
        match pending {
            Some(error) => {
                if self.popup != BoardPopup::SaveRecovery {
                    self.begin_proxy_save_recovery(error);
                }
            }
            None => self.drop_stale_save_recovery(),
        }
    }

    fn drop_stale_save_recovery(&mut self) {
        if self.popup == BoardPopup::SaveRecovery {
            self.end_proxy_save_recovery();
            self.clear_message();
        }
    }

    /// Whether this board holds the failed save itself, not just a proxy of its banner.
    pub fn owns_save_recovery(&self) -> bool {
        self.popup == BoardPopup::SaveRecovery && !self.save_recovery_proxy
    }

    pub fn shows_save_recovery_proxy(&self) -> bool {
        self.popup == BoardPopup::SaveRecovery && self.save_recovery_proxy
    }

    /// Take down a proxied recovery banner. Nothing failed on this board, so its own state
    /// (drafts, a durable delete notice) is restored as it was.
    pub fn end_proxy_save_recovery(&mut self) {
        self.save_recovery_proxy = false;
        self.end_save_recovery(SaveResolution::Retried);
    }

    /// The open decision card's cursor: the cleanup card before its run starts, else a start
    /// card. A running cleanup's card only reports, so it has none.
    fn open_card_cursor(&mut self) -> Option<&mut dyn CardCursor> {
        let running = self.cleanup_running();
        match (self.cleanup_prompt.as_mut(), self.dispatch_prompt.as_mut()) {
            (Some(_), _) if running => None,
            (Some(prompt), _) => Some(prompt),
            (None, Some(prompt)) => Some(prompt),
            (None, None) => self
                .launch_card
                .as_mut()
                .map(|prompt| prompt as &mut dyn CardCursor),
        }
    }

    /// A decision card's `↑`/`↓`: move the option cursor (stopping at the ends) and scroll its
    /// line into view. A card without options (a run, a check still running) scrolls instead.
    pub fn step_cleanup_selection(&mut self, down: bool) {
        let layout = self.cleanup_layout.borrow().clone();
        let max_scroll = self.cleanup_max_scroll.get();
        let Some(card) = self.open_card_cursor() else {
            self.scroll_cleanup(down);
            return;
        };
        let Some(index) = card.step_option(down) else {
            self.scroll_cleanup(down);
            return;
        };
        let Some(&(start, end)) = layout.options.get(index) else {
            return;
        };
        let (scroll, follow) = card.scroll_follow();
        let shown = if *follow { layout.scroll } else { *scroll }.min(max_scroll);
        *follow = true;
        *scroll = if start < shown {
            start
        } else if layout.capacity > 0 && end > shown + layout.capacity {
            end.saturating_sub(layout.capacity)
        } else {
            shown
        };
    }

    /// Scroll the cleanup or start card one row; the painter records how far it may go.
    pub fn scroll_cleanup(&mut self, down: bool) {
        let horizon = self.cleanup_max_scroll.get();
        let painted = self.cleanup_layout.borrow().scroll;
        let running = self.cleanup_running();
        let (scroll, follow) = match (self.cleanup_prompt.as_mut(), self.dispatch_prompt.as_mut()) {
            (Some(prompt), _) => (&mut prompt.scroll, &mut prompt.follow),
            (None, Some(prompt)) => (&mut prompt.scroll, &mut prompt.follow),
            (None, None) => match self.launch_card.as_mut() {
                Some(prompt) => (&mut prompt.scroll, &mut prompt.follow),
                None => return,
            },
        };
        // A page key takes over from the painter's scroll to the `▸` option.
        if std::mem::take(follow) && !running {
            *scroll = painted;
        }
        *scroll = if down {
            scroll.saturating_add(1).min(horizon)
        } else {
            scroll.saturating_sub(1)
        };
    }

    /// `Enter` on a card whose `▸` option a page key scrolled out of sight: scroll it back into
    /// view instead of running it, so nothing runs unseen. True when it did.
    pub fn reveal_hidden_cleanup_selection(&mut self) -> bool {
        let max_scroll = self.cleanup_max_scroll.get();
        let layout = self.cleanup_layout.borrow().clone();
        let Some(card) = self.open_card_cursor() else {
            return false;
        };
        let selected = card.selected_option();
        let (scroll, follow) = card.scroll_follow();
        if *follow || layout.capacity == 0 {
            return false;
        }
        let Some(&(start, end)) = selected.and_then(|index| layout.options.get(index)) else {
            return false;
        };
        let scroll = (*scroll).min(max_scroll);
        // In view: all of it, or its first rows when it is taller than the frame.
        let shown = start >= scroll
            && (end <= scroll + layout.capacity
                || (start == scroll && end - start > layout.capacity));
        if shown {
            return false;
        }
        *follow = true;
        true
    }

    /// The frame changed size: scroll the open card's `▸` option back into view, here and in
    /// the project preview.
    pub fn follow_cleanup_selection(&mut self) {
        if let Some(prompt) = self.cleanup_prompt.as_mut() {
            prompt.follow = true;
        }
        if let Some(prompt) = self.dispatch_prompt.as_mut() {
            prompt.follow = true;
        }
        if let Some(prompt) = self.launch_card.as_mut() {
            prompt.follow = true;
        }
    }

    /// The open decision card's `Enter`, option click or choice key, resolved to the intent it
    /// runs through the shared selector; a key the card does not offer does nothing. `None`:
    /// nothing runs. Other intents (the launch card's own keys among them) pass through.
    pub fn resolve_card_choice(
        &mut self,
        intent: BoardIntent,
        now: Instant,
    ) -> Option<BoardIntent> {
        let cleanup_key = match intent {
            BoardIntent::ConfirmCleanup => Some(CleanupChoice::Clean),
            BoardIntent::KeepCleanup => Some(CleanupChoice::Keep),
            BoardIntent::MergeCleanup => Some(CleanupChoice::Merge),
            _ => None,
        };
        let dispatch_key = match intent {
            BoardIntent::ConfirmDispatch => Some(DispatchChoice::Launch),
            BoardIntent::StartWithoutRelaunch => Some(DispatchChoice::StartOnly),
            _ => None,
        };
        let selector = matches!(
            intent,
            BoardIntent::CleanupRunSelected | BoardIntent::CleanupClickOption(_)
        );
        if !selector && cleanup_key.is_none() && dispatch_key.is_none() {
            return Some(intent);
        }
        // A confirmed cleanup's card only reports its run.
        if self.cleanup_prompt.is_some() && self.cleanup_running() {
            return None;
        }
        if intent == BoardIntent::CleanupRunSelected
            && self.reveal_ready(now)
            && self.reveal_hidden_cleanup_selection()
        {
            // The first `Enter` only shows what the second would run.
            return None;
        }
        // A key reaches only the card it belongs to.
        match (
            self.cleanup_prompt.as_mut(),
            self.dispatch_prompt.as_mut(),
            self.launch_card.as_mut(),
        ) {
            (Some(prompt), _, _) if dispatch_key.is_none() => {
                prompt.resolve(intent, cleanup_key, now)
            }
            (None, Some(prompt), _) if cleanup_key.is_none() => {
                prompt.resolve(intent, dispatch_key, now)
            }
            (None, None, Some(prompt)) if selector => prompt.resolve(intent, None, now),
            _ => None,
        }
    }

    /// Whether the open card's `Enter` would run something now (past the guard, an option).
    fn reveal_ready(&self, now: Instant) -> bool {
        match (&self.cleanup_prompt, &self.dispatch_prompt) {
            (Some(prompt), _) => prompt.selector.enter(&prompt.options(), now).is_some(),
            (None, Some(prompt)) => prompt.selector.enter(&prompt.options(), now).is_some(),
            (None, None) => self
                .launch_card
                .as_ref()
                .is_some_and(|prompt| prompt.selector.enter(&prompt.options(), now).is_some()),
        }
    }

    /// Present a failed board save without replacing its visible working state.
    ///
    /// A failed save *suspends* any delete recovery notice rather than taking it down for
    /// good: the deletion it offered to undo is not durable yet, so offering Undo would
    /// offer to undo nothing. Retry can still make it durable, and
    /// [`Self::end_save_recovery`] decides which of the two happened. Repeated failures
    /// keep the first suspended notice, so a second failed Retry cannot lose it.
    pub fn begin_save_recovery(&mut self, error: &str) {
        self.close_help();
        self.popup = BoardPopup::SaveRecovery;
        if let Some(notice) = self.delete_notice.take() {
            self.suspended_delete_notice = Some(notice);
            self.suspended_delete_notice_count = self.delete_notice_count.take();
            self.suspended_delete_notice_label = self.delete_notice_label.take();
            self.delete_notice_expires_at = None;
        }
        self.set_message(format!("save failed: {error} · Retry or Cancel"));
    }

    /// Show a failed background save (a bulk dispatch landing) on this board without making it
    /// the owner: Retry or Cancel end it as a proxy, never closing this board's form.
    pub fn begin_proxy_save_recovery(&mut self, error: &str) {
        self.begin_save_recovery(error);
        self.save_recovery_proxy = true;
    }

    /// End save recovery only after Retry succeeds or Cancel restores the baseline.
    ///
    /// A successful Retry makes the suspended deletion durable, so its recovery notice comes
    /// back: otherwise the user is left with a deleted task and no visible way back.
    /// Cancel restores the pre-failure baseline, so the deletion never happened and its
    /// notice is discarded.
    ///
    /// Nothing can be armed while a save failure is unresolved: the recovery gate routes
    /// every mutating intent back into [`Self::begin_save_recovery`], and only a mutating
    /// intent (a delete) arms a notice. That invariant is stated here by a resolution that is
    /// *total* rather than by an assertion, because the alternative to holding it is a panic
    /// inside a raw-mode TUI — a wedged terminal and a lost session, to report bookkeeping
    /// the user cannot see. Retry therefore prefers the suspended notice (the deletion it
    /// just made durable) and falls back to whatever was armed instead of wiping it, and both
    /// fields are emptied either way, so no path can lose the notice or leave a second copy
    /// behind to resurface at the next failure.
    pub fn end_save_recovery(&mut self, resolution: SaveResolution) -> bool {
        if self.popup == BoardPopup::SaveRecovery {
            self.popup = BoardPopup::None;
        }
        self.save_recovery_proxy = false;
        let armed = (
            self.delete_notice.take(),
            self.delete_notice_count.take(),
            self.delete_notice_label.take(),
        );
        let suspended = (
            self.suspended_delete_notice.take(),
            self.suspended_delete_notice_count.take(),
            self.suspended_delete_notice_label.take(),
        );
        let restored = match resolution {
            SaveResolution::Retried if suspended.0.is_some() => suspended,
            SaveResolution::Retried => armed,
            SaveResolution::Cancelled => (None, None, None),
        };
        // A restored notice gets a fresh few seconds: the deletion only just became durable.
        self.delete_notice_expires_at = restored
            .0
            .as_ref()
            .map(|_| Instant::now() + DELETE_NOTICE_TTL);
        self.delete_notice = restored.0;
        self.delete_notice_count = restored.1;
        self.delete_notice_label = restored.2;
        let cancelled_quick_add = resolution == SaveResolution::Cancelled
            && self.quick_add_save.take().is_some()
            && self.quick_add.is_some();
        if cancelled_quick_add {
            // The retained capture form is a stash, not an active page. Return its line to
            // normal keyboard routing and remove the failure chrome that line would cover.
            self.input_mode = BoardInputMode::QuickAdd;
            self.clear_message();
        }
        // The restored baseline discards the staged task edit, but its task-page form remains
        // open in view mode with every draft reset to the durable task.
        if resolution == SaveResolution::Cancelled {
            if let Some(pending) = self.task_edit_save.take() {
                let task = self
                    .tasks
                    .iter()
                    .find(|task| task.id == pending.id)
                    .cloned();
                if let (Some(form), Some(task)) = (self.form.as_mut(), task) {
                    form.title = seeded_draft(&task.title);
                    form.notes = seeded_draft(task.notes.as_deref().unwrap_or_default());
                    form.assignee = task.assigned();
                    form.base = task.base.clone();
                    form.scope = task.scope.clone();
                    form.select_current_scope();
                    form.editing = false;
                    form.steps.editor = None;
                    form.steps.pending_save = None;
                    form.steps.drafts.clear();
                    form.steps.removals.clear();
                    form.steps.add_selected = false;
                    form.steps.delete_mark = None;
                    form.task_snapshot = Some(Box::new(task));
                }
                self.hold_task_edit_save = false;
                self.input_mode = BoardInputMode::TaskPage;
                self.clear_message();
            }
        }
        // A held inline step editor unwinds to page view on Cancel: the restored baseline
        // rolled its mutation back, so nothing is left to hold the row for, and an edit mode
        // whose editor is gone is the orphan no key can escape. The Cancel path's
        // `sync_from_domain` ran first and left
        // the pending save unresolved precisely because the mutation is not in the
        // baseline; Retried resolves it there instead and never reaches this branch.
        if resolution == SaveResolution::Cancelled {
            super::block::release_cancelled_reply(self);
            super::block::release_cancelled_block_card(self);
        }
        if resolution == SaveResolution::Cancelled
            && self
                .form
                .as_ref()
                .is_some_and(|form| form.steps.pending_save.is_some())
        {
            if let Some(form) = self.form.as_mut() {
                form.steps.pending_save = None;
                form.steps.editor = None;
            }
            self.input_mode = BoardInputMode::TaskPage;
            self.clear_message();
        }
        cancelled_quick_add
    }

    /// Snapshot tasks from domain state and open a supplied project when it is live.
    pub fn from_domain(state: &DomainState, this_repo: Option<PathBuf>) -> Self {
        Self::from_domain_with_start(state, this_repo, true)
    }

    /// Snapshot tasks using the invocation's project candidate and default scope.
    ///
    /// A Git invocation opens its project. Outside Git, Desk opens while the current
    /// directory remains selected in project slot 2 and Desk remains quick-add's default.
    pub fn from_domain_for_snapshot(state: &DomainState, snapshot: &InvocationSnapshot) -> Self {
        let start_in_project = matches!(snapshot.default_scope, TaskScope::Project { .. });
        // The board navigates and offers the stored identity of an aliased launch repo
        // (`/tmp/x` for `/private/tmp/x`); the snapshot keeps the launch spelling for `!p`.
        let this_repo = snapshot.this_repo.clone().map(|repo| {
            crate::scope::stored_spelling(state, &repo.to_string_lossy())
                .map_or(repo, PathBuf::from)
        });
        let mut model = Self::from_domain_with_start(state, this_repo, start_in_project);
        model.session_default_scope = Some(snapshot.default_scope.clone());
        model
    }

    fn from_domain_with_start(
        state: &DomainState,
        this_repo: Option<PathBuf>,
        start_in_project: bool,
    ) -> Self {
        let mut model = Self::from_tasks(state.tasks().to_vec(), this_repo);
        model.archived_projects = state.archived_projects();
        model.hidden_assignees = state.hidden_assignees().clone();
        if let Some(repo) = model.this_repo.clone() {
            if model.is_archived_project_path(&repo) {
                model.selected_project = None;
            } else if start_in_project {
                // The board always shows every project, grouped; launching inside a
                // project starts the cursor on that project's first row.
                model.focus_project_group(&repo);
            }
        }
        model
    }

    /// Raise the two-choice launch card when the invocation project candidate is
    /// archived, at most once per session. Returns whether the card was raised.
    pub fn offer_launch_card(
        &mut self,
        state: &DomainState,
        snapshot: &InvocationSnapshot,
    ) -> bool {
        if self.launch_card_shown {
            return false;
        }
        let Some(path) = snapshot.this_repo.as_deref() else {
            return false;
        };
        if !state.is_project_archived(&path.to_string_lossy()) {
            return false;
        }
        self.launch_card_shown = true;
        self.launch_card = Some(super::launch_card::LaunchPrompt::new(path.to_path_buf()));
        self.popup = BoardPopup::LaunchCard;
        true
    }

    /// The launch card while it is up.
    pub fn launch_card(&self) -> Option<&super::launch_card::LaunchPrompt> {
        self.launch_card.as_ref()
    }

    /// Navigation is never yanked by saves or background merges: a save reanchors the
    /// pin only when the current destination already renders the saved task, and no
    /// path switches the board to another project merely to reveal a row.
    ///
    /// Replace task snapshot from domain (after mutation) and reanchor selection by id.
    pub fn sync_from_domain(&mut self, state: &DomainState) {
        // Capture the prior visible order before the snapshot is replaced, so reanchoring
        // can still see where the selection used to live. Project rows are derived from task
        // order, so the cursor needs a path anchor as well: an arriving project above it must
        // not silently change which preview seat the user is looking at.
        let previous = self.selection_id;
        let previous_visible = self.visible_ids();
        let previous_id_set: HashSet<Uuid> = self.tasks.iter().map(|task| task.id).collect();
        // Dispatch refreshes origin/HEAD in the task repository. Do not keep a footer
        // cached before that fetch, and do not invalidate on ordinary unchanged reloads.
        let prior_dispatches = self
            .tasks
            .iter()
            .filter_map(|task| task.dispatch.as_ref().map(|record| (task.id, record)))
            .collect::<HashMap<_, _>>();
        for task in state.tasks() {
            if prior_dispatches.get(&task.id).copied() != task.dispatch.as_ref() {
                if let TaskScope::Project { path } = &task.scope {
                    self.default_branches.invalidate(Path::new(path));
                }
            }
        }
        self.tasks = state.tasks().to_vec();
        self.archived_projects = state.archived_projects();
        self.hidden_assignees = state.hidden_assignees().clone();
        if self
            .undo_hint
            .as_ref()
            .is_some_and(|entry| !state.undo_entry_is_live_top(entry))
        {
            self.undo_hint = None;
        }
        let new_ids: Vec<Uuid> = self
            .tasks
            .iter()
            .filter(|task| !previous_id_set.contains(&task.id))
            .map(|task| task.id)
            .collect();
        // A merge (or this board's own picker verb) may archive the project the board is
        // showing: reset the scope to every project and name the project on the status
        // row. The quick-add default is guarded at `OpenCapture`, which never resolves to
        // an archived project, so the session default is left alone here.
        let focus_archived = match &self.board_location {
            BoardLocation::Project(path) => self.is_archived_project_path(path),
            BoardLocation::All | BoardLocation::Desk => false,
        };
        if focus_archived {
            let name = match &self.board_location {
                BoardLocation::Project(path) => {
                    crate::ui::render::short_project(&path.to_string_lossy()).to_string()
                }
                _ => String::new(),
            };
            self.board_location = BoardLocation::All;
            self.selected_project = None;
            self.set_message(format!("project {name} is archived"));
        }
        let pinned_edit = self.task_edit_save.as_ref().map(|pending| pending.id);
        let pinned_quick_add = self.quick_add_save.as_ref().map(|pending| pending.id);
        self.finish_quick_add_save();
        self.finish_task_edit_save();
        self.finish_step_editor_save();
        super::block::finish_reply_save(self);
        super::block::flag_stale_row_reply(self);
        super::block::finish_block_card_save(self);
        super::block::freeze_check_fold(self);
        if let Some(id) = pinned_edit.or(pinned_quick_add) {
            // A save this surface just made owns the selection, but navigation never
            // follows it: the pin moves only when the current destination already
            // renders the saved task, otherwise it anchors on the saved id's old
            // position (or the prior pin) so nothing jumps to the first row.
            if self.visible_ids().contains(&id) {
                self.retarget_selection(Some(id), SelectionRetarget::Reanchor);
                self.follow_list.set(true);
            } else {
                let anchor = if previous_visible.contains(&id) {
                    Some(id)
                } else {
                    previous
                };
                self.reanchor_selection(anchor, &previous_visible);
            }
        } else {
            // Tasks merged in from disk are somebody else's work: never move the
            // board's destination or selection toward them. An otherwise-empty view
            // may surface the first one, but only when the current destination already
            // renders it — the desk's global lanes usually do.
            if previous_visible.is_empty() {
                if let Some(id) = new_ids.first() {
                    if self.visible_ids().contains(id) {
                        self.retarget_selection(Some(*id), SelectionRetarget::Reanchor);
                        self.follow_list.set(true);
                    }
                }
            }
            self.reanchor_selection(previous, &previous_visible);
        }
        let visible: BTreeSet<Uuid> = self.visible_ids().into_iter().collect();
        self.marked_ids.retain(|id| visible.contains(id));
        let marked = &self.marked_ids;
        self.mark_order.retain(|id| marked.contains(id));
    }

    /// Presenter / pane title string.
    pub fn title(&self) -> &'static str {
        BOARD_TITLE
    }

    /// Resolved invocation repository used for scope-changing task edits.
    pub fn this_repo(&self) -> Option<&Path> {
        self.this_repo.as_deref()
    }

    /// Whether the persistent tabs read as "home" surfaces (quick-add scope defaults).
    pub fn at_home(&self) -> bool {
        self.board_location.at_home()
    }

    /// Which navigation tab is active right now.
    pub fn nav_tab(&self) -> NavTab {
        if self.questions.active {
            NavTab::Questions
        } else if self.reports.active {
            NavTab::Reports
        } else {
            NavTab::Board
        }
    }

    /// Replace the questions snapshot (board open, store change, after a question action).
    pub fn set_question_feed(&mut self, feed: crate::ui::questions::QuestionFeed) {
        self.questions.set_feed(feed);
    }

    /// The questions tab's session state.
    pub fn questions(&self) -> &crate::ui::questions::QuestionsState {
        &self.questions
    }

    pub(crate) fn questions_mut(&mut self) -> &mut crate::ui::questions::QuestionsState {
        &mut self.questions
    }

    /// Replace the reports snapshot (board open, store change, after `mark_read`).
    pub fn set_report_feed(&mut self, feed: crate::ui::reports::ReportFeed) {
        self.reports.set_feed(feed);
    }

    /// The reports tab's session state.
    pub fn reports(&self) -> &crate::ui::reports::ReportsState {
        &self.reports
    }

    pub(crate) fn reports_mut(&mut self) -> &mut crate::ui::reports::ReportsState {
        &mut self.reports
    }

    /// The session-only stall-check state.
    pub fn stall(&self) -> &crate::stall::StallState {
        &self.stall
    }

    pub fn stall_mut(&mut self) -> &mut crate::stall::StallState {
        &mut self.stall
    }

    /// Doing tasks that look stuck right now (see [`crate::stall`]).
    pub fn stale_ids(&self) -> BTreeSet<Uuid> {
        let working: Vec<Task> = self
            .tasks
            .iter()
            .filter(|task| self.on_working_board(task))
            .cloned()
            .collect();
        self.stall.stale_ids(
            &working,
            std::time::SystemTime::now(),
            crate::stall::threshold(),
        )
    }

    /// Numbered Todo tasks on the working board (no notices, nothing archived or deleted),
    /// in store order: the ones check-all offers the lead to start.
    pub fn todo_check_ids(&self) -> Vec<Uuid> {
        self.tasks
            .iter()
            .filter(|task| {
                task.status == crate::domain::HumanStatus::Todo
                    && task.number.is_some()
                    && task.notice.is_none()
                    && self.on_working_board(task)
            })
            .map(|task| task.id)
            .collect()
    }

    /// Not deleted, not archived, and not in an archived project.
    fn on_working_board(&self, task: &Task) -> bool {
        !task.soft_deleted
            && !task.archived
            && match &task.scope {
                TaskScope::Project { path } => {
                    !archived_path_contains(&self.archived_projects, path)
                }
                TaskScope::Global => true,
            }
    }

    /// The board's loaded tasks, in store order.
    pub fn loaded_tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// The task a cursor command acts on: the focused page's task, else the selection.
    pub fn command_target_id(&self) -> Option<Uuid> {
        self.action_target_id()
    }

    /// Switch to a navigation tab. The board keeps a parked task page; the questions and
    /// reports tabs refuse while a board edit is unsaved. Returns whether anything moved.
    pub(super) fn select_nav_tab(&mut self, tab: NavTab) -> bool {
        match tab {
            NavTab::Board => {
                let moved = self.questions.active || self.reports.active;
                self.questions.active = false;
                self.questions.return_question = None;
                self.reports.active = false;
                moved
            }
            NavTab::Questions => {
                if self.questions.active {
                    return false;
                }
                if self.has_unsaved_task_work() || self.task_editing() {
                    self.set_message("save or cancel the edit before opening questions");
                    return false;
                }
                self.reports.active = false;
                self.questions.active = true;
                self.questions.return_question = None;
                self.questions.reanchor();
                true
            }
            NavTab::Reports => {
                if self.reports.active {
                    return false;
                }
                if self.has_unsaved_task_work() || self.task_editing() {
                    self.set_message("save or cancel the edit before opening reports");
                    return false;
                }
                self.questions.active = false;
                self.reports.active = true;
                self.reports.reanchor();
                true
            }
        }
    }

    /// The tab change `3` makes (see `BoardIntent::SelectNavTab` in `apply`), for routes that
    /// open tab 3 directly (`r`): the same edit guard and refusal message, and on a move the
    /// same relation-trail and mark clearing. Returns whether tab 3 is active afterwards.
    pub(crate) fn enter_reports_tab(&mut self) -> bool {
        if self.reports.active {
            return true;
        }
        if self.has_unsaved_task_work() || self.task_editing() {
            // Refused before anything moves, exactly like `3`.
            self.select_nav_tab(NavTab::Reports);
            return false;
        }
        self.page_back.clear();
        if self.select_nav_tab(NavTab::Reports) {
            self.clear_marks();
        }
        self.reports.active
    }

    /// Session-only row order of every section.
    pub fn sort(&self) -> BoardSort {
        self.sort
    }

    /// Record whether the app's current frame paints a wide split presentation.
    /// The stage and right-seat session remain parked when this is false.
    pub(crate) fn set_frame_wide(&self, wide: bool) {
        self.frame_wide.set(wide);
    }

    pub(crate) fn frame_wide(&self) -> bool {
        self.frame_wide.get()
    }

    /// Move to `target` through the one shared guarded transition: reanchor by the
    /// previous visible order, and never touch open forms or save-recovery state.
    pub(super) fn switch_location(&mut self, target: BoardLocation) {
        if self.board_location == target {
            return;
        }
        let previous_visible = self.visible_ids();
        let previous = self.selection_id;
        self.search_query.clear();
        self.search_pinned = false;
        if self.input_mode == BoardInputMode::Search {
            self.input_mode = BoardInputMode::Normal;
        }
        let same_project = matches!(
            (&self.board_location, &target),
            (BoardLocation::Project(a), BoardLocation::Project(b))
                if paths_equivalent(&a.to_string_lossy(), &b.to_string_lossy())
        );
        if same_project {
            // Equivalent spellings are the same stored project identity. Do not rewrite the
            // selected path merely because a host supplied an alias.
            return;
        }
        self.board_location = target;
        self.reanchor_selection(previous, &previous_visible);
    }

    /// The last project scope picked, independent of the active scope.
    pub fn selected_project(&self) -> Option<&Path> {
        self.selected_project.as_deref()
    }

    pub fn active_project(&self) -> Option<&Path> {
        match &self.board_location {
            BoardLocation::Project(path) => Some(path.as_path()),
            BoardLocation::All | BoardLocation::Desk => None,
        }
    }

    fn is_archived_project_path(&self, path: &Path) -> bool {
        let path = path.to_string_lossy();
        archived_path_contains(&self.archived_projects, &path)
    }

    /// Whether a layered Escape has reached the visible main board root.
    ///
    /// A task form parked behind `FullBoard` does not stop this from being a root Escape: the
    /// application quit guard sees that retained draft and refuses before the reducer can leave.
    pub(crate) fn root_escape_requests_quit(&self) -> bool {
        self.input_mode_local() == BoardInputMode::Normal
            && self.wide_stage == WideStage::Board
            && self.surface == CommandSurface::None
            && self.popup == BoardPopup::None
            && self.list_picker.is_none()
            && self.search_query.is_empty()
            && !self.inbox_header_selected()
            && !self.archived_header_selected()
    }

    /// Present the existing draft-preservation refusal when quitting would lose work.
    pub(crate) fn refuse_quit_with_unsaved_work(&mut self) -> bool {
        if self.has_unsaved_task_work() {
            self.set_message(DIRTY_TASK_SWITCH_REFUSAL);
            return true;
        }
        if self.questions.has_unsaved_work() {
            self.set_message(QUESTION_QUIT_REFUSAL);
            return true;
        }
        false
    }

    /// Whether quitting would discard or hide an unsaved draft: a task draft, or a typed
    /// answer or unresolved save on the questions tab, wherever the board now is.
    pub fn has_unsaved_work(&self) -> bool {
        self.has_unsaved_task_work() || self.questions.has_unsaved_work()
    }

    /// Whether leaving the current task surface would discard or hide an unsaved draft.
    pub(crate) fn has_unsaved_task_work(&self) -> bool {
        if self.task_session_dirty()
            || self.row_reply.is_some()
            || self.quick_add_save.is_some()
            || self.task_edit_save.is_some()
        {
            return true;
        }
        let underlying_mode = match self.input_mode {
            BoardInputMode::Help => self.help_return_mode,
            BoardInputMode::Search => self.search_return_mode,
            // The parent chooser opens over a clean task page and holds no draft of its own.
            BoardInputMode::ParentPicker => BoardInputMode::TaskPage,
            mode => mode,
        };
        if self.form.is_some() && underlying_mode != BoardInputMode::TaskPage {
            return true;
        }
        self.quick_add
            .as_ref()
            .is_some_and(|quick_add| !quick_add.title.value().is_empty())
    }

    /// The every-project board adds to the project the board was launched in (when it is
    /// live), otherwise to the desk; a project scope adds to its project.
    pub(super) fn quick_add_scope(&self) -> Option<TaskScope> {
        match &self.board_location {
            BoardLocation::All | BoardLocation::Desk => match &self.session_default_scope {
                Some(TaskScope::Project { path })
                    if !self.is_archived_project_path(Path::new(path)) =>
                {
                    Some(TaskScope::Project { path: path.clone() })
                }
                _ => Some(TaskScope::Global),
            },
            BoardLocation::Project(path) => Some(TaskScope::Project {
                path: path.to_string_lossy().into_owned(),
            }),
        }
    }

    /// Whether board rows paint without their `└─` message lines (`z`).
    pub fn messages_hidden(&self) -> bool {
        self.messages_hidden
    }

    /// `z`: hide or show every board row's message line. Returns the new state.
    pub(super) fn toggle_messages_hidden(&mut self) -> bool {
        self.messages_hidden = !self.messages_hidden;
        self.messages_hidden
    }

    /// Whether the peek's closing line names the assignee. The board has no assignee
    /// filter or view, so it always does.
    pub fn assignee_label_shown(&self) -> bool {
        true
    }

    /// The searchable list picker is open (the parent chooser).
    pub fn list_picker_open(&self) -> bool {
        self.list_picker.is_some()
    }

    /// The open list picker's kind, when one is open.
    pub fn list_picker_kind(&self) -> Option<ListPickerKind> {
        self.list_picker.as_ref().map(|picker| picker.kind)
    }

    pub fn list_picker_query(&self) -> Option<&str> {
        self.list_picker
            .as_ref()
            .map(|picker| picker.query.as_str())
    }

    /// Cancel an armed card-option double-click when another pointer target intervenes.
    /// Mouse-boundary state only, never persisted.
    pub(crate) fn cancel_card_option_clicks(&mut self) {
        if let Some(prompt) = self.cleanup_prompt.as_mut() {
            prompt.selector.option_click = None;
        }
        if let Some(prompt) = self.dispatch_prompt.as_mut() {
            prompt.selector.option_click = None;
        }
        if let Some(prompt) = self.launch_card.as_mut() {
            prompt.selector.option_click = None;
        }
    }

    /// Record a left-button press cell and drop any finished selection's highlight.
    ///
    /// The press cell is what a following drag grows the selection from; a plain
    /// click never reads it. Called for every left press, whatever the input mode.
    pub fn begin_mouse_press(&mut self, position: Position) {
        self.mouse_press = Some(position);
        self.mouse_press_scroll = Some(self.content_scroll());
        self.text_selection = None;
    }

    fn content_scroll(&self) -> usize {
        if self.focused_surface() == FocusedSurface::Task {
            self.form
                .as_ref()
                .filter(|form| form.is_task())
                .map(|form| form.notes_scroll)
                .unwrap_or(0)
        } else {
            self.list_scroll.get()
        }
    }

    fn content_relative_anchor(&self, press: Position) -> Position {
        let Some(press_scroll) = self.mouse_press_scroll else {
            return press;
        };
        let dy = self.content_scroll() as i32 - press_scroll as i32;
        let y = if dy >= 0 {
            press.y.saturating_sub(dy as u16)
        } else {
            press.y.saturating_add(dy.unsigned_abs() as u16)
        };
        Position::new(press.x, y)
    }

    /// Extend the drag selection to `position`, anchored at the press cell.
    ///
    /// Inert without a live press (a drag that starts mid-gesture, e.g. before tsk
    /// saw the press), so it can never invent an anchor.
    pub fn drag_text_selection(&mut self, position: Position) {
        if let Some(press) = self.mouse_press {
            let anchor = self.content_relative_anchor(press);
            self.text_selection = Some(TextSelection::new(anchor, position));
        }
    }

    /// Recompute the live highlight after the viewport scrolled under a held drag.
    pub fn recompute_text_selection_head(&mut self, head: Position) {
        self.drag_text_selection(head);
    }

    /// Clear the press on release; the selection itself stays until copy clears it
    /// or the next press replaces it.
    pub fn end_mouse_press(&mut self) {
        self.mouse_press = None;
        self.mouse_press_scroll = None;
    }

    /// Drop a finished text selection highlight (after copy, Esc, or cancel).
    pub fn clear_text_selection(&mut self) {
        self.text_selection = None;
    }

    /// The live drag selection, if a drag is (or was) in progress.
    pub fn text_selection(&self) -> Option<TextSelection> {
        self.text_selection
    }

    /// Scroll the board list by `rows` without moving the pinned selection.
    ///
    /// Used by drag edge auto-scroll so a text drag near the viewport edge can
    /// reveal more of the deck. Clamped to the last painted max scroll.
    pub fn nudge_list_scroll(
        &self,
        direction: crate::ui::text_select::AutoScrollDirection,
        rows: u16,
    ) -> usize {
        use crate::ui::text_select::AutoScrollDirection;
        self.follow_list.set(false);
        let max = self.list_max_scroll.get();
        let cur = self.list_scroll.get();
        let next = match direction {
            AutoScrollDirection::Up => cur.saturating_sub(rows as usize),
            AutoScrollDirection::Down => cur.saturating_add(rows as usize).min(max),
        };
        self.list_scroll.set(next);
        cur.abs_diff(next)
    }

    /// Scroll the open task page's shared notes body by `rows` (view mode only).
    pub fn nudge_notes_scroll(
        &mut self,
        direction: crate::ui::text_select::AutoScrollDirection,
        rows: u16,
    ) -> usize {
        use crate::ui::text_select::AutoScrollDirection;
        let Some(form) = self.form.as_mut() else {
            return 0;
        };
        let horizon = form.notes_max_scroll.get();
        let cur = form.notes_scroll;
        form.notes_scroll = match direction {
            AutoScrollDirection::Up => form.notes_scroll.saturating_sub(rows as usize),
            AutoScrollDirection::Down => {
                form.notes_scroll.saturating_add(rows as usize).min(horizon)
            }
        };
        cur.abs_diff(form.notes_scroll)
    }

    /// Queue sections + counts for the current session destination.
    pub fn queue_view(&self) -> QueueView {
        queue::query_board_search(
            &self.tasks,
            &self.archived_projects,
            self.effective_lens(),
            self.drawer_open,
            self.sort,
            &self.search_query,
        )
    }

    /// The lens the current scope resolves to.
    pub(super) fn effective_lens(&self) -> BoardLens<'_> {
        self.board_location.lens()
    }

    /// The help card's focused search query.
    pub fn help_query(&self) -> &str {
        &self.help_query
    }

    /// The help card's scroll offset.
    pub fn help_scroll(&self) -> usize {
        self.help_scroll
    }

    /// The active lens's search query.
    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// Whether the current query is pinned while normal board keys own input.
    pub fn search_pinned(&self) -> bool {
        self.search_pinned
    }

    /// The open list picker's tab. `None` for an untabbed picker (every board picker today).
    pub fn list_picker_tab(&self) -> Option<FilterTab> {
        let picker = self.list_picker.as_ref()?;
        picker.tabs.as_ref().map(|tabs| tabs.active_tab)
    }

    /// Source index of the option the board currently applies in the open picker's tab.
    pub fn list_picker_active(&self) -> Option<usize> {
        self.list_picker.as_ref().and_then(|picker| picker.active)
    }

    /// Show `target` in a tabbed picker, or flip to its other tab when `None`. The query
    /// clears and the cursor lands on that tab's active choice. Returns false for an
    /// untabbed picker.
    pub(super) fn switch_list_picker_tab(
        &mut self,
        target: Option<FilterTab>,
        _forward: bool,
    ) -> bool {
        let Some(picker) = self.list_picker.as_mut() else {
            return false;
        };
        let Some(tabs) = picker.tabs.as_mut() else {
            return false;
        };
        if target.is_some_and(|tab| tab == tabs.active_tab) {
            return true;
        }
        tabs.active_tab = match tabs.active_tab {
            FilterTab::Threads => FilterTab::Assignees,
            _ => FilterTab::Threads,
        };
        std::mem::swap(&mut picker.options, &mut tabs.other_options);
        std::mem::swap(&mut picker.active, &mut tabs.other_active);
        picker.query.clear();
        picker.selected = picker.active.unwrap_or(0);
        true
    }

    /// Open the assignee picker over `ids`: every defined profile, `@you`, the people already
    /// assigned on the board, `type a name…`, then **none** last. `current` (the cursor task's
    /// assignee) is preselected, else the first row. Nothing changes until Enter.
    pub(super) fn open_assignee_picker(&mut self, ids: Vec<Uuid>, current: Option<Assignee>) {
        let options = self.assignee_picker_options();
        // An unassigned task starts on the first row, so Enter right away assigns.
        let selected = current
            .and_then(|current| {
                options.iter().position(|option| {
                    option.value == ListPickerValue::Assignee(Some(current.clone()))
                })
            })
            .unwrap_or(0);
        self.list_picker = Some(ListPickerState {
            kind: ListPickerKind::Assignee,
            options,
            selected,
            query: String::new(),
            return_mode: self.input_mode,
            assignee_target: Some(AssigneePickerTarget { ids }),
            on_target: None,
            base_target: None,
            after_target: None,
            active: None,
            tabs: None,
            rows: Default::default(),
        });
        self.input_mode = BoardInputMode::ListPicker;
    }

    /// The assignee picker's rows: every profile, `@you`, each person named on a live task
    /// and not removed from the list, `type a name…`, and `none`.
    fn assignee_picker_options(&self) -> Vec<ListPickerOption> {
        let mut choices: Vec<Assignee> = self
            .agent_names
            .iter()
            .map(|name| Assignee::agent(name.clone()))
            .collect();
        choices.push(Assignee::you());
        let people: BTreeSet<String> = self
            .tasks
            .iter()
            .filter(|task| !task.soft_deleted && !task.is_notice())
            .filter_map(Task::assigned)
            .filter(|assignee| assignee.is_person() && assignee.name != crate::domain::OWNER)
            .map(|assignee| assignee.name)
            .filter(|name| !self.hidden_assignees.contains(name))
            .collect();
        choices.extend(people.into_iter().map(Assignee::person));
        let mut options: Vec<ListPickerOption> = choices
            .into_iter()
            .map(|assignee| ListPickerOption {
                label: crate::ui::render::assignee_handle(&assignee.name),
                count: None,
                value: ListPickerValue::Assignee(Some(assignee)),
            })
            .collect();
        options.push(ListPickerOption {
            label: TYPE_A_NAME.to_string(),
            count: None,
            value: ListPickerValue::TypedName,
        });
        options.push(ListPickerOption {
            label: "none".to_string(),
            count: None,
            value: ListPickerValue::Assignee(None),
        });
        options
    }

    /// The person name under the assignee picker's cursor, the one row `ctrl+x` removes.
    /// `None` on a profile, `@you`, `type a name…`, `none`, or any other picker.
    pub fn assignee_picker_person(&self) -> Option<String> {
        if self.list_picker_kind() != Some(ListPickerKind::Assignee) {
            return None;
        }
        match self.selected_list_picker_option()?.1.value {
            ListPickerValue::Assignee(Some(assignee))
                if assignee.is_person() && assignee.name != crate::domain::OWNER =>
            {
                Some(assignee.name)
            }
            _ => None,
        }
    }

    /// Rebuild the open assignee picker's rows after a name left or rejoined the list. The
    /// cursor stays on the same row number, clamped to the rows left.
    pub(super) fn refresh_assignee_picker(&mut self) {
        if self.list_picker_kind() != Some(ListPickerKind::Assignee) {
            return;
        }
        let options = self.assignee_picker_options();
        if let Some(picker) = self.list_picker.as_mut() {
            picker.options = options;
        }
        let last = self.visible_list_picker_options().len().saturating_sub(1);
        if let Some(picker) = self.list_picker.as_mut() {
            picker.selected = picker.selected.min(last);
        }
    }

    /// Open on the branches already on disk (bounded local queries, never the network). Unless
    /// origin was fetched inside the fetch window, a background worker fetches and refreshes
    /// the list in place behind a disabled `refreshing…` row.
    pub(super) fn open_base_picker(
        &mut self,
        ids: Vec<Uuid>,
        current: Option<String>,
        project: &Path,
        edit_draft: bool,
    ) {
        let project = project
            .canonicalize()
            .unwrap_or_else(|_| project.to_path_buf());
        let default = crate::git_base::default_branch_name(&project)
            .unwrap_or_else(|| self.default_branch_name(&project));
        self.open_base_picker_from(
            ids,
            current,
            project,
            edit_draft,
            default,
            |project| crate::git_base::fetch_is_fresh(project, "origin"),
            crate::git_base::list_cached_branches,
        );
    }

    /// Freshness is read before the snapshot: a fetch that lands between the two then leaves
    /// a not-fresh answer and a refresh subscription, never a fresh answer over pre-fetch refs.
    #[allow(clippy::too_many_arguments)]
    fn open_base_picker_from(
        &mut self,
        ids: Vec<Uuid>,
        current: Option<String>,
        project: PathBuf,
        edit_draft: bool,
        default: String,
        is_fresh: impl FnOnce(&Path) -> bool,
        cached_branches: impl FnOnce(&Path) -> Result<Vec<String>, String>,
    ) {
        let request = Uuid::new_v4();
        let fresh = is_fresh(&project);
        let cached = cached_branches(&project);
        let mut options = vec![Self::default_base_option(default)];
        let status = match &cached {
            Ok(branches) => {
                options.extend(branches.iter().map(|branch| ListPickerOption {
                    label: branch.clone(),
                    count: None,
                    value: ListPickerValue::Base(Some(branch.clone())),
                }));
                (!fresh).then(|| REFRESHING_BRANCHES.to_string())
            }
            Err(_) if !fresh => Some(REFRESHING_BRANCHES.to_string()),
            Err(message) => Some(message.clone()),
        };
        if let Some(label) = status {
            options.push(ListPickerOption {
                label,
                count: None,
                value: ListPickerValue::Unavailable,
            });
        }
        let preferred = ListPickerValue::Base(current.clone());
        let selected = options
            .iter()
            .position(|option| option.value == preferred)
            .unwrap_or(0);
        self.list_picker = Some(ListPickerState {
            kind: ListPickerKind::Base,
            options,
            selected,
            query: String::new(),
            return_mode: self.input_mode,
            assignee_target: None,
            on_target: None,
            base_target: Some(BasePickerTarget {
                ids,
                edit_draft,
                request,
                project: project.clone(),
                current,
                // A cached list already placed the cursor; the refresh keeps whatever the
                // user moves it to. Without one, the refresh selects the current base.
                selection_pending: cached.is_err(),
            }),
            after_target: None,
            active: None,
            tabs: None,
            rows: Default::default(),
        });
        self.input_mode = BoardInputMode::ListPicker;
        if fresh {
            return;
        }
        // A cancelled picker leaves its bounded lookup running. Reopening subscribes
        // the new request to that project's existing job instead of starting another fetch.
        {
            let mut workers = self
                .base_picker_workers
                .lock()
                .expect("base worker registry");
            if let Some(worker) = workers.get_mut(&project) {
                worker.request = request;
                return;
            }
            workers.insert(
                project.clone(),
                BasePickerWorker {
                    job: request,
                    request,
                },
            );
        }
        let mailbox = Arc::clone(&self.base_picker_results);
        let workers = Arc::clone(&self.base_picker_workers);
        std::thread::spawn(move || {
            let branches = crate::git_base::list_branches_with_warning(&project);
            let default =
                crate::git_base::default_branch_name(&project).unwrap_or_else(|| "default".into());
            if let Ok(mut workers) = workers.lock() {
                if workers
                    .get(&project)
                    .is_some_and(|worker| worker.job == request)
                {
                    let subscriber = workers
                        .remove(&project)
                        .expect("completed base worker")
                        .request;
                    if let Ok(mut results) = mailbox.lock() {
                        results.push(BasePickerResult {
                            request: subscriber,
                            project,
                            branches,
                            default,
                        });
                    }
                }
            }
        });
    }

    fn default_base_option(default: String) -> ListPickerOption {
        ListPickerOption {
            label: if default == "default" {
                default
            } else {
                format!("default ({default})")
            },
            count: None,
            value: ListPickerValue::Base(None),
        }
    }

    /// Whether a base picker's branch lookup is still running, cancelled or not. Public only
    /// so tests can wait for its Git child before removing the repository (Windows refuses to
    /// delete a process's working directory).
    #[doc(hidden)]
    pub fn base_picker_lookup_running(&self) -> bool {
        self.base_picker_workers
            .lock()
            .is_ok_and(|workers| !workers.is_empty())
    }

    /// Apply completed branch lookup on the event loop, including idle redraw ticks.
    /// Cancelled and superseded generations cannot overwrite a newly opened picker.
    pub fn poll_base_picker_results(&mut self) -> bool {
        let results = match self.base_picker_results.try_lock() {
            Ok(mut mailbox) => std::mem::take(&mut *mailbox),
            Err(_) => return false,
        };
        let mut changed = false;
        for result in results {
            let Some(target) = self
                .list_picker
                .as_ref()
                .and_then(|picker| picker.base_target.as_ref())
                .filter(|target| {
                    target.request == result.request && target.project == result.project
                })
            else {
                continue;
            };
            let preferred = if target.selection_pending {
                Some(ListPickerValue::Base(target.current.clone()))
            } else {
                self.selected_list_picker_option()
                    .map(|(_, option)| option.value)
            };
            let picker = self.list_picker.as_mut().expect("active base picker");
            picker.options = vec![Self::default_base_option(result.default)];
            match result.branches {
                Ok((branches, warning)) => {
                    picker
                        .options
                        .extend(branches.into_iter().map(|branch| ListPickerOption {
                            label: branch.clone(),
                            count: None,
                            value: ListPickerValue::Base(Some(branch)),
                        }));
                    if let Some(warning) = warning {
                        picker.options.push(ListPickerOption {
                            label: warning,
                            count: None,
                            value: ListPickerValue::Unavailable,
                        });
                    }
                }
                Err(message) => picker.options.push(ListPickerOption {
                    label: message,
                    count: None,
                    value: ListPickerValue::Unavailable,
                }),
            }
            picker
                .base_target
                .as_mut()
                .expect("base target")
                .selection_pending = false;
            let visible = self.visible_list_picker_options();
            let selected = visible
                .iter()
                .position(|(_, option)| Some(&option.value) == preferred.as_ref())
                .or_else(|| {
                    visible
                        .iter()
                        .position(|(_, option)| option.value != ListPickerValue::Unavailable)
                })
                .unwrap_or(0);
            self.list_picker.as_mut().expect("active picker").selected = selected;
            changed = true;
        }
        changed
    }

    pub(super) fn close_base_picker(&mut self) -> Option<BasePickerTarget> {
        let picker = self.list_picker.take()?;
        self.input_mode = picker.return_mode;
        picker.base_target
    }

    /// Open the task picker over `ids` (or the open form's draft): a search box over the tasks
    /// not done, the target's project first. `current` starts pinned, a done or archived
    /// prerequisite too, so every link can be removed. `Enter` pins a match, `shift+Enter` saves.
    pub(super) fn open_after_picker(
        &mut self,
        ids: Vec<Uuid>,
        current: Vec<u64>,
        scope: &TaskScope,
        edit_draft: bool,
        own: Option<u64>,
    ) {
        let mut candidates: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|task| {
                !task.soft_deleted
                    && !task.is_notice()
                    && !task.archived
                    && task.status != HumanStatus::Done
                    && task.number.is_some()
                    && task.number != own
                    && !ids.contains(&task.id)
            })
            .collect();
        candidates.sort_by_key(|task| (task.scope != *scope, task.number));
        let label = |task: &Task, number: u64| {
            let place = if task.scope == *scope {
                String::new()
            } else {
                match &task.scope {
                    TaskScope::Project { path } => {
                        format!(" · {}", crate::ui::render::short_project(path))
                    }
                    TaskScope::Global => " · desk".to_string(),
                }
            };
            format!("T{number} {}{place}", terminal_text(&task.title))
        };
        let options: Vec<ListPickerOption> = candidates
            .iter()
            .map(|task| {
                let number = task.number.expect("listed tasks are numbered");
                ListPickerOption {
                    label: label(task, number),
                    count: None,
                    value: ListPickerValue::After(number),
                }
            })
            .collect();
        let listed = |number: u64| {
            options
                .iter()
                .any(|option| option.value == ListPickerValue::After(number))
        };
        let unlisted: Vec<(u64, String)> = current
            .iter()
            .copied()
            .filter(|number| !listed(*number))
            .map(|number| {
                let text = match self.tasks.iter().find(|task| task.number == Some(number)) {
                    Some(task) => {
                        let state = if task.soft_deleted {
                            " · deleted"
                        } else if task.status == HumanStatus::Done {
                            " · done"
                        } else if task.archived {
                            " · archived"
                        } else {
                            ""
                        };
                        format!("{}{state}", label(task, number))
                    }
                    None => format!("T{number} · deleted"),
                };
                (number, text)
            })
            .collect();
        let title = match (ids.len(), own) {
            (1, Some(number)) => format!("T{number} runs after"),
            (0 | 1, _) => "new task runs after".to_string(),
            (count, _) => format!("{count} tasks run after"),
        };
        self.list_picker = Some(ListPickerState {
            kind: ListPickerKind::After,
            options,
            selected: 0,
            query: String::new(),
            return_mode: self.input_mode,
            assignee_target: None,
            on_target: None,
            base_target: None,
            after_target: Some(AfterPickerTarget {
                ids,
                edit_draft,
                title,
                pinned: current,
                unlisted,
            }),
            active: None,
            tabs: None,
            rows: Default::default(),
        });
        self.input_mode = BoardInputMode::ListPicker;
    }

    /// The task picker's rows: the pinned tasks (`✓ T202 …`), then the tasks the query
    /// matches. An empty query matches nothing.
    fn after_picker_rows(&self) -> Vec<(usize, ListPickerOption)> {
        let Some(picker) = self.list_picker.as_ref() else {
            return Vec::new();
        };
        let Some(target) = picker.after_target.as_ref() else {
            return Vec::new();
        };
        let mut rows: Vec<ListPickerOption> = target
            .pinned
            .iter()
            .map(|number| {
                let text = picker
                    .options
                    .iter()
                    .find(|option| option.value == ListPickerValue::After(*number))
                    .map(|option| option.label.clone())
                    .or_else(|| {
                        target
                            .unlisted
                            .iter()
                            .find(|(n, _)| n == number)
                            .map(|(_, text)| text.clone())
                    })
                    .unwrap_or_else(|| format!("T{number}"));
                ListPickerOption {
                    label: format!("✓ {text}"),
                    count: None,
                    value: ListPickerValue::AfterPinned(*number),
                }
            })
            .collect();
        let query = picker.query.to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        if !terms.is_empty() {
            rows.extend(
                picker
                    .options
                    .iter()
                    .filter(|option| match option.value {
                        ListPickerValue::After(number) => {
                            !target.pinned.contains(&number) && {
                                // The title and project, without the leading `T202 `.
                                let text = option
                                    .label
                                    .split_once(' ')
                                    .map_or("", |(_, rest)| rest)
                                    .to_lowercase();
                                terms
                                    .iter()
                                    .all(|term| after_term_matches(term, number, &text))
                            }
                        }
                        _ => false,
                    })
                    .cloned(),
            );
        }
        rows.into_iter().enumerate().collect()
    }

    /// The task picker's card title, while it is open.
    pub fn after_picker_title(&self) -> Option<&str> {
        self.list_picker
            .as_ref()?
            .after_target
            .as_ref()
            .map(|target| target.title.as_str())
    }

    /// How many leading task-picker rows are pinned tasks; the matches follow.
    pub fn after_picker_pinned_rows(&self) -> usize {
        self.list_picker
            .as_ref()
            .and_then(|picker| picker.after_target.as_ref())
            .map_or(0, |target| target.pinned.len())
    }

    /// Put the task-picker cursor on the first match after a query change, or on the first
    /// row when nothing matches.
    fn reseat_after_cursor(&mut self) {
        let pinned_rows = self.after_picker_pinned_rows();
        let rows = self.after_picker_rows().len();
        if let Some(picker) = self.list_picker.as_mut() {
            picker.selected = if rows > pinned_rows { pinned_rows } else { 0 };
        }
    }

    /// `Enter` (or a click) in the task picker: pin the highlighted match and clear the query,
    /// or unpin the highlighted pinned task.
    pub(super) fn toggle_after_pin(&mut self) {
        let Some((index, option)) = self.selected_list_picker_option() else {
            return;
        };
        let Some(picker) = self.list_picker.as_mut() else {
            return;
        };
        let Some(target) = picker.after_target.as_mut() else {
            return;
        };
        match option.value {
            ListPickerValue::After(number) => {
                target.pinned.push(number);
                // The query found it; the next search starts empty, cursor on the new pin.
                picker.selected = target.pinned.len() - 1;
                picker.query.clear();
            }
            ListPickerValue::AfterPinned(number) => {
                target.pinned.retain(|n| *n != number);
                let pinned = target.pinned.len();
                let rows = self.after_picker_rows().len();
                if let Some(picker) = self.list_picker.as_mut() {
                    // The next pinned task, else the first match, else the last pinned task.
                    picker.selected = if index < pinned || rows > pinned {
                        index.min(rows.saturating_sub(1))
                    } else {
                        pinned.saturating_sub(1)
                    };
                }
            }
            _ => {}
        }
    }

    /// `ctrl+x` in the task picker: unpin every task.
    pub(super) fn clear_after_pins(&mut self) {
        if let Some(target) = self
            .list_picker
            .as_mut()
            .and_then(|picker| picker.after_target.as_mut())
        {
            target.pinned.clear();
        }
        self.reseat_after_cursor();
    }

    /// What `shift+Enter` in the task picker saves: exactly the pinned tasks. Nothing pinned
    /// runs after nothing.
    pub fn after_picker_choice(&self) -> Option<Vec<u64>> {
        let picker = self.list_picker.as_ref()?;
        Some(picker.after_target.as_ref()?.pinned.clone())
    }

    pub(super) fn close_after_picker(&mut self) -> Option<AfterPickerTarget> {
        let picker = self.list_picker.take()?;
        self.input_mode = picker.return_mode;
        picker.after_target
    }

    /// Cached remote-default label for the task footer. Cache misses schedule metadata lookup and
    /// return `default` for this frame.
    pub fn default_branch_name(&self, project: &Path) -> String {
        self.default_branches.get_or_request(project)
    }

    /// Close the assignee picker after its choice was applied (or refused), restoring the
    /// surface it opened over.
    pub(super) fn close_assignee_picker(&mut self) -> Option<AssigneePickerTarget> {
        let picker = self.list_picker.take()?;
        self.input_mode = picker.return_mode;
        picker.assignee_target
    }

    /// Open the waiting-on picker over `target`: you, every defined profile, the record's
    /// current value when it is none of those, then `type a name…` (a task number on a block,
    /// or any text). The current value is preselected and marked `· current`. Nothing changes
    /// until Enter.
    pub(super) fn open_on_picker(&mut self, target: OnPickerTarget, current: Option<BlockOn>) {
        let mut choices: Vec<BlockOn> = std::iter::once(BlockOn::You)
            .chain(self.agent_names.iter().cloned().map(BlockOn::Agent))
            .collect();
        if let Some(current) = current.as_ref().filter(|on| !choices.contains(on)) {
            choices.push(current.clone());
        }
        let selected = current
            .as_ref()
            .and_then(|current| choices.iter().position(|on| on == current))
            .unwrap_or(0);
        let mut options: Vec<ListPickerOption> = choices
            .into_iter()
            .map(|on| {
                let label = on_choice_label(&on);
                ListPickerOption {
                    label: if current.as_ref() == Some(&on) {
                        format!("{label} · current")
                    } else {
                        label
                    },
                    count: None,
                    value: ListPickerValue::WaitingOn(on),
                }
            })
            .collect();
        options.push(ListPickerOption {
            label: TYPE_A_NAME.to_string(),
            count: None,
            value: ListPickerValue::TypedName,
        });
        self.list_picker = Some(ListPickerState {
            kind: ListPickerKind::WaitingOn,
            options,
            selected,
            query: String::new(),
            return_mode: self.input_mode,
            assignee_target: None,
            on_target: Some(target),
            base_target: None,
            after_target: None,
            active: None,
            tabs: None,
            rows: Default::default(),
        });
        self.input_mode = BoardInputMode::ListPicker;
    }

    /// Close the waiting-on picker after its choice was applied, restoring the surface it
    /// opened over.
    pub(super) fn close_on_picker(&mut self) -> Option<OnPickerTarget> {
        let picker = self.list_picker.take()?;
        self.input_mode = picker.return_mode;
        picker.on_target
    }

    /// What the typed row of the open assignee or waiting-on picker would apply: the query
    /// read by the name rule. `None` with an empty query; an invalid name is the refusal.
    pub(super) fn typed_picker_value(&self) -> Option<Result<ListPickerValue, String>> {
        let picker = self.list_picker.as_ref()?;
        let query = picker.query.trim();
        if query.is_empty() {
            return None;
        }
        let is_profile = |name: &str| self.agent_names.iter().any(|agent| agent == name);
        match picker.kind {
            ListPickerKind::Assignee => Some(
                crate::domain::resolve_assignee(query, is_profile)
                    .map(|assignee| ListPickerValue::Assignee(Some(assignee)))
                    .map_err(|error| {
                        crate::domain::thread_refusal_message(error)
                            .replacen("thread", "assignee", 1)
                    }),
            ),
            ListPickerKind::WaitingOn => {
                let kind = picker.on_target.as_ref()?.kind();
                Some(Ok(ListPickerValue::WaitingOn(crate::domain::resolve_on(
                    query, kind, is_profile,
                ))))
            }
            _ => None,
        }
    }

    /// Resolve a quick-picker assignment at the persistence boundary. Once it is on disk
    /// (`saved`), an open task form bound to one of its tasks adopts the new assignee, unless
    /// its own assignee draft was already changed; otherwise a later save of that form would
    /// write its stale draft over the assignment. A cancelled save leaves the form alone.
    /// Call after `sync_from_domain`, so the bound task is current.
    pub fn finish_form_assignee_sync(&mut self, saved: bool) {
        let Some(ids) = self.pending_form_assignee_sync.take() else {
            return;
        };
        if !saved {
            return;
        }
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let tasks = &self.tasks;
        let Some(id) = form.task_id().filter(|id| ids.contains(id)) else {
            return;
        };
        let untouched = form
            .task_snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.assigned() == form.assignee);
        if let (true, Some(task)) = (untouched, tasks.iter().find(|task| task.id == id)) {
            form.reset_field_to_saved(CaptureField::Assignee, task);
        }
    }

    /// Resolve a persisted quick Base choice into an open page form without overwriting a base
    /// draft the user changed while the save was in flight.
    pub fn finish_form_base_sync(&mut self, saved: bool) {
        let Some(ids) = self.pending_form_base_sync.take() else {
            return;
        };
        if !saved {
            return;
        }
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let Some(id) = form.task_id().filter(|id| ids.contains(id)) else {
            return;
        };
        let untouched = form
            .task_snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.base == form.base);
        if let (true, Some(task)) = (untouched, self.tasks.iter().find(|task| task.id == id)) {
            form.reset_field_to_saved(CaptureField::Base, task);
        }
    }

    /// [`Self::finish_form_base_sync`] for a quick task-picker change.
    pub fn finish_form_after_sync(&mut self, saved: bool) {
        let Some(ids) = self.pending_form_after_sync.take() else {
            return;
        };
        if !saved {
            return;
        }
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let Some(id) = form.task_id().filter(|id| ids.contains(id)) else {
            return;
        };
        let untouched = form
            .task_snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.after == form.after);
        if let (true, Some(task)) = (untouched, self.tasks.iter().find(|task| task.id == id)) {
            form.reset_field_to_saved(CaptureField::After, task);
        }
    }

    /// Options the open picker shows after its search query, as (source index, option).
    ///
    /// The typed row of the assignee and waiting-on pickers stays under any query and paints
    /// what Enter would apply (`@dana`, `T202`), unless a listed row already offers it.
    pub fn visible_list_picker_options(&self) -> Vec<(usize, ListPickerOption)> {
        let Some(picker) = self.list_picker.as_ref() else {
            return Vec::new();
        };
        if picker.kind == ListPickerKind::After {
            return self.after_picker_rows();
        }
        let query = picker.query.trim().to_ascii_lowercase();
        let typed = self.typed_picker_value();
        let mut visible: Vec<_> = picker
            .options
            .iter()
            .enumerate()
            .filter(|(_, option)| {
                option.value == ListPickerValue::Unavailable
                    || option.value == ListPickerValue::TypedName
                    || query.is_empty()
                    || option.label.to_ascii_lowercase().contains(&query)
            })
            .map(|(index, option)| (index, option.clone()))
            .collect();
        let listed = |value: &ListPickerValue| {
            visible
                .iter()
                .any(|(_, option): &(usize, ListPickerOption)| option.value == *value)
        };
        let typed_label = match &typed {
            Some(Ok(value)) if listed(value) => None,
            Some(Ok(ListPickerValue::Assignee(Some(assignee)))) => {
                Some(Some(crate::ui::render::assignee_handle(&assignee.name)))
            }
            Some(Ok(ListPickerValue::WaitingOn(on))) => Some(Some(on_choice_label(on))),
            Some(_) => Some(Some(terminal_text(picker.query.trim()))),
            None => Some(None),
        };
        match typed_label {
            None => visible.retain(|(_, option)| option.value != ListPickerValue::TypedName),
            Some(Some(label)) => {
                for (_, option) in &mut visible {
                    if option.value == ListPickerValue::TypedName {
                        option.label = label.clone();
                    }
                }
            }
            Some(None) => {}
        }
        // With only a loading/error row matching, retain the real default choice rather than
        // putting a highlight on an informational row.
        if picker.kind == ListPickerKind::Base
            && !visible
                .iter()
                .any(|(_, option)| option.value != ListPickerValue::Unavailable)
        {
            if let Some(option) = picker.options.first() {
                visible.insert(0, (0, option.clone()));
            }
        }
        visible
    }

    /// The picker row the cursor rests on, in the visible (filtered) order.
    pub fn list_picker_selected(&self) -> usize {
        self.list_picker
            .as_ref()
            .map(|picker| picker.selected)
            .unwrap_or(0)
    }

    pub fn selected_list_picker_option(&self) -> Option<(usize, ListPickerOption)> {
        self.visible_list_picker_options()
            .into_iter()
            .nth(self.list_picker_selected())
            .filter(|(_, option)| option.value != ListPickerValue::Unavailable)
    }

    pub(super) fn select_list_picker_option(&mut self, index: usize) -> bool {
        if self
            .visible_list_picker_options()
            .get(index)
            .is_none_or(|(_, option)| option.value == ListPickerValue::Unavailable)
        {
            return false;
        }
        if let Some(picker) = self.list_picker.as_mut() {
            picker.selected = index;
            if let Some(target) = picker.base_target.as_mut() {
                target.selection_pending = false;
            }
        }
        true
    }

    pub(super) fn move_list_picker(&mut self, forward: bool) {
        let visible = self.visible_list_picker_options();
        let len = visible.len();
        if let Some(picker) = self.list_picker.as_mut() {
            if let Some(target) = picker.base_target.as_mut() {
                target.selection_pending = false;
            }
            // The parent chooser wraps tall options: ↓/↑ first reveal the rest of the selected
            // option inside the painted window, then move to the next/previous option.
            if matches!(picker.kind, ListPickerKind::Parent { .. }) && len > 0 {
                let rows = &picker.rows;
                let window = rows.window.get();
                let scroll = rows.scroll.get();
                if let (Some((start, height)), true) =
                    (rows.option_span(picker.selected), window > 0)
                {
                    if forward && start + height > scroll + window {
                        rows.scroll.set(scroll + 1);
                        return;
                    }
                    if !forward && start < scroll {
                        rows.scroll.set(scroll - 1);
                        return;
                    }
                }
                rows.follow.set(Some(!forward));
            }
            if len == 0 {
                picker.selected = 0;
            }
            // Informational rows are never a resting place: step over them.
            for _ in 0..len {
                picker.selected = if forward {
                    (picker.selected + 1) % len
                } else {
                    picker.selected.checked_sub(1).unwrap_or(len - 1)
                };
                if visible[picker.selected].1.value != ListPickerValue::Unavailable {
                    break;
                }
            }
        }
    }

    /// Wheel over the parent chooser: scroll its wrapped rows, selection unchanged.
    pub(super) fn scroll_list_picker_rows(&mut self, down: bool) {
        let Some(picker) = self.list_picker.as_mut() else {
            return;
        };
        let rows = &picker.rows;
        rows.follow.set(None);
        let max = rows.total_rows().saturating_sub(rows.window.get());
        let scroll = rows.scroll.get();
        rows.scroll.set(if down {
            (scroll + 1).min(max)
        } else {
            scroll.saturating_sub(1)
        });
    }

    /// The parent chooser's row-scroll state, for the painter.
    pub fn list_picker_rows(&self) -> Option<&crate::ui::render::PickerRowState> {
        self.list_picker.as_ref().map(|picker| &picker.rows)
    }

    /// A new query starts the filtered list from its top.
    fn reset_list_picker_rows(picker: &mut ListPickerState) {
        picker.selected = 0;
        picker.rows.scroll.set(0);
        picker.rows.follow.set(Some(false));
    }

    pub(super) fn list_picker_query_insert(&mut self, character: char) {
        self.list_picker_query_insert_text(&character.to_string());
    }

    pub(super) fn list_picker_query_backspace(&mut self) {
        if let Some(picker) = self.list_picker.as_mut() {
            picker.query.pop();
            Self::reset_list_picker_rows(picker);
            if let Some(target) = picker.base_target.as_mut() {
                target.selection_pending = false;
            }
        }
        self.reseat_after_query();
    }

    pub(super) fn list_picker_query_delete_word(&mut self) {
        if let Some(picker) = self.list_picker.as_mut() {
            crate::ui::edit::delete_last_word(&mut picker.query);
            Self::reset_list_picker_rows(picker);
        }
        self.reseat_after_query();
    }

    pub(super) fn list_picker_query_insert_text(&mut self, text: &str) {
        if let Some(picker) = self.list_picker.as_mut() {
            picker.query.push_str(text);
            Self::reset_list_picker_rows(picker);
            if let Some(target) = picker.base_target.as_mut() {
                target.selection_pending = false;
            }
        }
        self.reseat_after_query();
    }

    /// A task-picker query change lands the cursor on the first match, past the pinned rows.
    fn reseat_after_query(&mut self) {
        if self.list_picker_kind() == Some(ListPickerKind::After) {
            self.reseat_after_cursor();
        }
    }

    pub(super) fn cancel_list_picker(&mut self) {
        let return_mode = self
            .list_picker
            .take()
            .map_or(BoardInputMode::Normal, |picker| picker.return_mode);
        self.input_mode = return_mode;
    }

    /// Open the parent chooser for `task_id`: `no parent` first, then every other live,
    /// non-notice task in number order, from every scope. Tasks under this one stay listed but
    /// are marked and refused, since choosing one would form a cycle. Returns false when the
    /// task is not on the board.
    pub(super) fn open_parent_picker(&mut self, task_id: Uuid) -> bool {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id) else {
            return false;
        };
        let current = task.parent_task_id;
        let parent_of = |id: Uuid| {
            self.tasks
                .iter()
                .find(|task| task.id == id)
                .and_then(|task| task.parent_task_id)
        };
        let under_this_task = |candidate: Uuid| {
            let mut seen = BTreeSet::new();
            let mut cursor = Some(candidate);
            while let Some(id) = cursor {
                if id == task_id {
                    return true;
                }
                if !seen.insert(id) {
                    return false;
                }
                cursor = parent_of(id);
            }
            false
        };
        let mut candidates: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|task| task.id != task_id && !task.soft_deleted && !task.is_notice())
            .collect();
        candidates.sort_by_key(|task| (task.number.is_none(), task.number, task.id));
        let mut options = vec![ListPickerOption {
            label: "no parent".to_string(),
            count: None,
            value: ListPickerValue::Clear,
        }];
        for candidate in candidates {
            let mut label = match candidate.board_identifier() {
                Some(identifier) => format!("{identifier} {}", terminal_text(&candidate.title)),
                None => terminal_text(&candidate.title),
            };
            let cycle = under_this_task(candidate.id);
            if Some(candidate.id) == current {
                label.push_str(" · current");
            }
            if candidate.archived {
                label.push_str(" · archived");
            }
            if cycle {
                label.push_str(" · under this task");
            }
            options.push(ListPickerOption {
                label,
                count: None,
                value: if cycle {
                    ListPickerValue::Cycle(candidate.id)
                } else {
                    ListPickerValue::Task(candidate.id)
                },
            });
        }
        let selected = options
            .iter()
            .position(|option| match option.value {
                ListPickerValue::Clear => current.is_none(),
                ListPickerValue::Task(id) => Some(id) == current,
                _ => false,
            })
            .unwrap_or(0);
        self.list_picker = Some(ListPickerState {
            kind: ListPickerKind::Parent { task: task_id },
            options,
            selected,
            query: String::new(),
            return_mode: self.input_mode,
            assignee_target: None,
            on_target: None,
            base_target: None,
            after_target: None,
            active: None,
            tabs: None,
            rows: crate::ui::render::PickerRowState::following(),
        });
        true
    }

    /// The task the open parent chooser edits, when the chooser is open.
    pub fn parent_picker_task(&self) -> Option<Uuid> {
        match self.list_picker.as_ref()?.kind {
            ListPickerKind::Parent { task } => Some(task),
            _ => None,
        }
    }

    /// Fold the group `g` addresses: archived while the drawer is open and that
    /// group is painted, otherwise inbox when it is painted.
    pub(super) fn toggle_all_groups(&mut self) -> bool {
        let view = self.queue_view();
        let archived_joins = self.drawer_open
            && view
                .sections
                .iter()
                .any(|section| section.kind == SectionKind::Archived);
        if archived_joins {
            self.archived_collapsed = !self.archived_collapsed;
            return true;
        }
        if view
            .sections
            .iter()
            .any(|section| section.kind == SectionKind::Inbox)
        {
            self.inbox_collapsed = !self.inbox_collapsed;
            return true;
        }
        false
    }

    /// Whether the done drawer is open (session-only).
    pub fn drawer_open(&self) -> bool {
        self.drawer_open
    }

    /// Toggle the archived group's collapse. Session-only; never persisted.
    pub(super) fn toggle_archived_collapsed(&mut self) {
        self.archived_collapsed = !self.archived_collapsed;
    }

    /// Toggle the inbox group's collapse. Session-only; never persisted.
    pub(super) fn toggle_inbox_collapsed(&mut self) {
        self.inbox_collapsed = !self.inbox_collapsed;
    }

    /// Whether the inbox group's header row holds the selection.
    pub fn inbox_header_selected(&self) -> bool {
        self.selection_id == Some(queue::INBOX_HEADER_ROW_ID)
    }

    /// Pin the selection onto the inbox header row (chrome, not a task).
    pub(super) fn select_inbox_header(&mut self) -> bool {
        self.retarget_selection(
            Some(queue::INBOX_HEADER_ROW_ID),
            SelectionRetarget::Explicit,
        )
    }

    /// Whether the archived group's header row holds the selection.
    pub fn archived_header_selected(&self) -> bool {
        self.selection_id == Some(queue::ARCHIVED_HEADER_ROW_ID)
    }

    /// Pin the selection onto the archived header row (chrome, not a task).
    pub(super) fn select_archived_header(&mut self) -> bool {
        self.retarget_selection(
            Some(queue::ARCHIVED_HEADER_ROW_ID),
            SelectionRetarget::Explicit,
        )
    }

    /// Visible task ids from the queue section query (flat section order).
    pub fn visible_ids(&self) -> Vec<Uuid> {
        visible_task_ids(
            &self.queue_view(),
            self.archived_collapsed,
            self.inbox_collapsed,
        )
    }

    /// Visible tasks in queue section order.
    pub fn visible_tasks(&self) -> Vec<&Task> {
        self.visible_ids()
            .into_iter()
            .filter_map(|id| self.tasks.iter().find(|t| t.id == id))
            .collect()
    }

    /// Selected index into the visible list (derived from the id pin).
    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selection_id?;
        self.visible_ids().iter().position(|&row| row == id)
    }

    /// Selected task id, if any. Header rows are chrome, not a task: with one
    /// selected every task verb refuses with `select a task first`.
    pub fn selected_id(&self) -> Option<Uuid> {
        if self.selection_id.is_some_and(is_header_row) {
            None
        } else {
            self.selection_id
        }
    }

    /// Whether the explicit task marking mode is active on this board surface.
    pub fn mark_mode_active(&self) -> bool {
        self.mark_mode
    }

    /// Session-only marked task ids in stable id order.
    pub fn marked_ids(&self) -> &BTreeSet<Uuid> {
        &self.marked_ids
    }

    /// The marked tasks in the order they were marked.
    pub fn marked_in_order(&self) -> Vec<Uuid> {
        self.mark_order
            .iter()
            .copied()
            .filter(|id| self.marked_ids.contains(id))
            .collect()
    }

    /// Number of tasks the next bulk verb will target.
    pub fn marked_count(&self) -> usize {
        self.marked_ids.len()
    }

    pub(super) fn toggle_selected_mark(&mut self) -> bool {
        let Some(id) = self.selected_id() else {
            return false;
        };
        if !self.visible_ids().contains(&id) {
            return false;
        }
        if self.marked_ids.remove(&id) {
            self.mark_order.retain(|marked| *marked != id);
        } else {
            self.marked_ids.insert(id);
            self.mark_order.push(id);
        }
        true
    }

    pub(super) fn mark_selected(&mut self) -> bool {
        let Some(id) = self.selected_id() else {
            return false;
        };
        if !self.visible_ids().contains(&id) {
            return false;
        }
        let inserted = self.marked_ids.insert(id);
        if inserted {
            self.mark_order.push(id);
        }
        inserted
    }

    pub(super) fn toggle_mark_mode(&mut self) {
        if self.mark_mode {
            self.clear_marks();
        } else {
            self.mark_mode = true;
        }
    }

    pub fn clear_marks(&mut self) -> bool {
        let had_mark_state = self.mark_mode || !self.marked_ids.is_empty();
        self.mark_mode = false;
        self.marked_ids.clear();
        self.mark_order.clear();
        had_mark_state
    }

    /// Whether this model's task list owns resolved input, including while a task page is parked.
    pub(crate) fn task_list_owns_input(&self) -> bool {
        self.input_mode_local() == BoardInputMode::Normal
    }

    /// Whether a status verb currently targets the marked set rather than the cursor.
    pub fn bulk_verb_active(&self) -> bool {
        self.task_list_owns_input() && self.mark_mode && !self.marked_ids.is_empty()
    }

    /// Marked tasks a palette command would act on: the task list under an open palette owns
    /// the marks exactly as [`Self::bulk_verb_active`] would once the palette closes.
    pub(super) fn marks_under_palette(&self) -> usize {
        let list_underneath =
            self.input_mode == BoardInputMode::Normal && self.popup == BoardPopup::None;
        if list_underneath && self.mark_mode {
            self.marked_ids.len()
        } else {
            0
        }
    }

    /// Mark targets when the task list owns input, otherwise the cursor target.
    pub fn verb_target_ids(&self) -> Vec<Uuid> {
        if self.task_list_owns_input() && self.mark_mode && !self.marked_ids.is_empty() {
            self.marked_ids.iter().copied().collect()
        } else {
            self.action_target_id().into_iter().collect()
        }
    }

    /// The task the focused task page shows, when the task side owns input. A related task
    /// opened from the page may sit outside the list, so this is not always `selected_id`.
    pub(super) fn focused_page_task(&self) -> Option<Uuid> {
        if self.focused_surface() != FocusedSurface::Task {
            return None;
        }
        self.form
            .as_ref()
            .filter(|form| form.is_task())
            .and_then(BoardForm::task_id)
    }

    /// The task a cursor verb acts on: the focused page's own task while the task side owns
    /// input, otherwise the list selection. `selected_id` keeps its list meaning.
    pub(super) fn action_target_id(&self) -> Option<Uuid> {
        self.focused_page_task().or_else(|| self.selected_id())
    }

    /// Relation actions of the focused page, indexed like the painted relation rows: 0 is
    /// the parent line, `1 + k` the k-th direct child.
    pub(super) fn relation_targets(&self) -> Vec<RelationTarget> {
        let Some(task) = self
            .focused_page_task()
            .and_then(|id| self.tasks.iter().find(|task| task.id == id))
        else {
            return Vec::new();
        };
        let mut targets = vec![match crate::domain::parent_in(&self.tasks, task) {
            None => RelationTarget::SetParent,
            Some(crate::domain::ParentRelation::Resolved(parent)) => {
                RelationTarget::Open(parent.id)
            }
            Some(crate::domain::ParentRelation::Unresolved(id)) => RelationTarget::Missing(id),
        }];
        targets.extend(
            crate::domain::direct_children_in(&self.tasks, task.id)
                .into_iter()
                .map(|child| RelationTarget::Open(child.id)),
        );
        targets.extend(
            self.page_questions(task.id)
                .into_iter()
                .map(|record| RelationTarget::Question(record.reference.id)),
        );
        targets.extend(
            (0..crate::ui::links::task_links(task.notes.as_deref()).len())
                .map(|index| RelationTarget::Link(task.id, index)),
        );
        targets
    }

    /// The task page's question lines in paint order: questions and approvals first, then
    /// findings, each by Q number.
    pub(crate) fn page_questions(&self, task: Uuid) -> Vec<&crate::domain::QuestionRecord> {
        let mut records: Vec<&crate::domain::QuestionRecord> = self
            .questions
            .records()
            .iter()
            .filter(|record| record.reference.task_id == task)
            .collect();
        records.sort_by_key(|record| {
            (
                record.reference.kind == crate::domain::QuestionKind::Finding,
                record.reference.number,
            )
        });
        records
    }

    /// The archived project holding the focused page's task. A relation open can reach
    /// such a task from a live lens; its page is read-only.
    pub(super) fn page_archived_project(&self) -> Option<String> {
        let task = self
            .focused_page_task()
            .and_then(|id| self.tasks.iter().find(|task| task.id == id))?;
        match &task.scope {
            TaskScope::Project { path }
                if archived_path_contains(&self.archived_projects, path) =>
            {
                Some(path.clone())
            }
            _ => None,
        }
    }

    /// Current list viewport offset.
    pub fn list_scroll(&self) -> usize {
        self.list_scroll.get()
    }

    /// Surface that currently owns keyboard and pointer routing, derived from the stage.
    pub fn focused_surface(&self) -> FocusedSurface {
        self.wide_stage.focused_surface()
    }

    /// Whether a capture draft (quick-add expanded with `Tab`) owns the frame. The draft is
    /// neither the list nor a task, so the wide layout has no column for it: while it is open
    /// the frame is painted and pointer-routed as a single surface at every width.
    pub fn capture_draft_open(&self) -> bool {
        self.form.as_ref().is_some_and(|form| !form.is_task())
    }

    /// Live responsive geometry for `area`: list and detail, unless a capture draft owns the
    /// frame (see [`Self::capture_draft_open`]).
    pub fn responsive_geometry(
        &self,
        area: ratatui::layout::Rect,
    ) -> crate::ui::tier::ResponsiveGeometry {
        use crate::ui::tier::{
            resolve, resolve_responsive, ResponsiveGeometry, ResponsivePresentation,
        };
        if self.capture_draft_open() {
            return ResponsiveGeometry {
                presentation: ResponsivePresentation::SingleBoard,
                board: area,
                rule: ratatui::layout::Rect::default(),
                task: ratatui::layout::Rect::default(),
                density: resolve(area.width, area.height).tier,
            };
        }
        resolve_responsive(area.width, area.height, self.wide_stage)
    }

    /// Session-only focus owner. The board always opens with the list focused.
    pub fn wide_stage(&self) -> WideStage {
        self.wide_stage
    }

    /// Test surface. Retained task-page viewport offset.
    pub fn page_scroll(&self) -> usize {
        self.form
            .as_ref()
            .map(|form| form.notes_scroll)
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn page_scroll_horizon(&self) -> usize {
        self.form
            .as_ref()
            .filter(|form| form.is_task())
            .map(|form| form.notes_max_scroll.get())
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn set_page_scroll_horizon_for_test(&self, horizon: usize) {
        if let Some(form) = self.form.as_ref().filter(|form| form.is_task()) {
            form.notes_max_scroll.set(horizon);
        }
    }

    /// Test surface. Retained stored-step cursor, if active.
    pub fn step_cursor(&self) -> Option<usize> {
        self.form
            .as_ref()
            .filter(|form| form.is_task())
            .and_then(|form| form.steps.cursor)
    }
}

impl BoardModel {
    /// Current input mode (normal vs edit field).
    pub fn input_mode(&self) -> BoardInputMode {
        self.input_mode_local()
    }

    fn input_mode_local(&self) -> BoardInputMode {
        if self.input_mode == BoardInputMode::Help {
            return BoardInputMode::Help;
        }
        if self.surface == CommandSurface::Palette {
            return BoardInputMode::Palette;
        }
        match self.popup {
            BoardPopup::SaveRecovery => BoardInputMode::SaveRecovery,
            BoardPopup::LaunchCard => BoardInputMode::LaunchCard,
            BoardPopup::CleanupConfirm
                if self
                    .cleanup_prompt
                    .as_ref()
                    .is_some_and(|prompt| !prompt.can_clean_any()) =>
            {
                BoardInputMode::CleanupDirtyConfirm
            }
            BoardPopup::CleanupConfirm => BoardInputMode::CleanupConfirm,
            BoardPopup::DispatchConfirm => BoardInputMode::DispatchConfirm,
            BoardPopup::BlockCard => BoardInputMode::BlockCard,
            _ if self.focused_surface() == FocusedSurface::Board
                && self.input_mode == BoardInputMode::TaskPage =>
            {
                BoardInputMode::Normal
            }
            _ => self.input_mode,
        }
    }

    /// Edit buffer contents for the focused board-page text field.
    pub fn edit_buffer(&self) -> &str {
        let Some(form) = self.form.as_ref() else {
            return "";
        };
        match form.focus {
            CaptureField::Title
            | CaptureField::Scope
            | CaptureField::Base
            | CaptureField::After => form.title.value(),
            CaptureField::Notes => form.notes.value(),
            CaptureField::Assignee => "",
        }
    }

    /// Cursor position in the focused board-page text buffer, counted in Unicode scalar values.
    pub fn edit_cursor(&self) -> usize {
        let Some(form) = self.form.as_ref() else {
            return 0;
        };
        match form.focus {
            CaptureField::Title
            | CaptureField::Scope
            | CaptureField::Base
            | CaptureField::After => form.title.cursor(),
            CaptureField::Notes => form.notes.cursor(),
            CaptureField::Assignee => 0,
        }
    }

    /// Whether a board form currently owns keyboard input. The app loop uses this one predicate
    /// to route expanded quick-add and task-form keys through the same mapper.
    pub fn board_form_open(&self) -> bool {
        self.form.is_some()
    }

    /// Title draft displayed in the status-row quick-add line.
    pub fn quick_add_title_value(&self) -> &str {
        self.quick_add
            .as_ref()
            .map(|quick_add| quick_add.title.value())
            .unwrap_or("")
    }

    /// Whether a quick-add draft is open: the status-row line or its expanded page.
    /// The quick-capture popup session lives exactly as long as this draft.
    pub fn quick_add_open(&self) -> bool {
        self.quick_add.is_some()
    }

    /// Whether the expanded capture page is open over its retained quick-add line.
    /// Esc there would collapse the page back to the line, which in the quick-capture
    /// popup is the top-level draft Escape.
    pub fn expanded_capture_open(&self) -> bool {
        self.quick_add.is_some() && self.form.as_ref().is_some_and(|form| !form.is_task())
    }

    pub(super) fn clear_saved_task(&mut self) {
        self.saved_task = None;
    }

    pub(super) fn begin_quick_add_save(&mut self, id: Uuid, keep_open: bool) {
        self.quick_add_save = Some(QuickAddSave { id, keep_open });
    }

    pub(super) fn discard_quick_add(&mut self) {
        self.quick_add = None;
        self.quick_add_save = None;
        self.form = None;
        self.input_mode = BoardInputMode::Normal;
        self.clear_message();
    }

    /// Drop a retained expanded draft after the quick-add title changed.
    pub(super) fn invalidate_quick_add_stash(&mut self) {
        if self.form.as_ref().is_some_and(|form| !form.is_task()) {
            self.form = None;
        }
    }

    /// True when the current save-recovery result just completed a quick add.
    pub fn has_saved_task(&self) -> bool {
        self.saved_task.is_some()
    }

    fn finish_quick_add_save(&mut self) {
        let Some(pending) = self.quick_add_save.as_ref() else {
            return;
        };
        if !self.tasks.iter().any(|task| task.id == pending.id) {
            return;
        }
        let pending = self.quick_add_save.take().expect("checked quick-add save");
        self.saved_task = Some(pending.id);
        // Navigation never follows a save: the row flash is the feedback, and the pin
        // moves only when the current destination already renders the new task.
        if self.visible_ids().contains(&pending.id) {
            self.retarget_selection(Some(pending.id), SelectionRetarget::Reanchor);
            self.follow_list.set(true);
        } else if let Some(previous) = self.selection_id {
            let previous_visible = self.visible_ids();
            self.reanchor_selection(Some(previous), &previous_visible);
        }
        // The pending create is now durable. This is the only point an expanded quick-add
        // may release its complete form, so a failed save can still return to that stash.
        self.form = None;
        if pending.keep_open {
            if let Some(quick_add) = self.quick_add.as_mut() {
                quick_add.title = seeded_draft("");
            }
            self.input_mode = BoardInputMode::QuickAdd;
        } else {
            self.quick_add = None;
            self.input_mode = BoardInputMode::Normal;
        }
        self.clear_message();
    }

    fn finish_task_edit_save(&mut self) {
        if self.hold_task_edit_save {
            return;
        }
        let Some(pending) = self.task_edit_save.take() else {
            return;
        };
        let landed = self.tasks.iter().any(|task| {
            task.id == pending.id
                && task.title == pending.title
                && task.notes == pending.notes
                && task.scope == pending.scope
                && task.assigned() == pending.assignee
                && task.base == pending.base
                && pending.step_renames.iter().all(|(step_id, text)| {
                    task.steps
                        .iter()
                        .any(|step| step.id == *step_id && step.text == *text)
                })
                && pending
                    .step_removals
                    .iter()
                    .all(|step_id| !task.steps.iter().any(|step| step.id == *step_id))
        });
        if !landed {
            self.task_edit_save = Some(pending);
            return;
        }
        let saved_snapshot = self
            .tasks
            .iter()
            .find(|task| task.id == pending.id)
            .cloned();
        let selected_step_cursor = pending.selected_step.and_then(|selected_step| {
            saved_snapshot
                .as_ref()
                .and_then(|task| task.steps.iter().position(|step| step.id == selected_step))
        });
        if let Some(form) = self.form.as_mut().filter(|form| form.is_task()) {
            // Domain normalization (notably title trimming) is now durable. Refresh the
            // retained drafts so the page never paints a value that was not saved.
            form.title = seeded_draft(&pending.title);
            form.notes = seeded_draft(pending.notes.as_deref().unwrap_or_default());
            form.scope = pending.scope;
            form.scope_selected = form
                .scope_options
                .iter()
                .position(|option| option == &form.scope)
                .unwrap_or(0);
            form.assignee = pending.assignee;
            form.base = pending.base;
            form.after = pending.after;
            form.select_current_assignee();
            form.editing = false;
            form.steps.editor = None;
            form.steps.drafts.clear();
            form.steps.removals.clear();
            form.steps.cursor = selected_step_cursor;
            form.steps.add_selected = false;
            form.task_snapshot = saved_snapshot.map(Box::new);
        }
        self.input_mode = BoardInputMode::TaskPage;
        self.clear_message();
    }

    /// Hold a task form through the reducer's pre-persist sync.
    pub fn hold_task_edit_save(&mut self) {
        self.hold_task_edit_save = true;
    }

    /// Whether a task form is held through an unresolved save.
    pub fn task_edit_save_held(&self) -> bool {
        self.hold_task_edit_save
    }

    /// The task page form's title draft, if a task form is open.
    pub fn task_form_title(&self) -> Option<&str> {
        self.form
            .as_ref()
            .filter(|form| form.is_task())
            .map(|form| form.title.value())
    }

    /// Release a held form only after Retry or the initial persistence succeeds.
    pub fn release_task_edit_save(&mut self) {
        self.hold_task_edit_save = false;
    }

    /// Release a held inline step editor once persistence has confirmed its mutation
    /// (AC-14's release side).
    ///
    /// Mirrors [`Self::finish_quick_add_save`]'s identity check: the touched step
    /// must be present carrying its new text in the synced tasks before the row may
    /// close, or reopen empty for Shift+Enter. The Cancel path syncs the rolled-back
    /// baseline first, where the added step is absent, so the line stays held for
    /// [`Self::end_save_recovery`] to unwind to page view instead. The editor and its input mode outlive the save
    /// call precisely because nothing but this confirmed landing releases them.
    fn finish_step_editor_save(&mut self) {
        let Some(form) = self.form.as_ref().filter(|form| form.is_task()) else {
            return;
        };
        let Some(task_id) = form.task_id() else {
            return;
        };
        let Some(pending) = form.steps.pending_save.clone() else {
            return;
        };
        let landed = self.tasks.iter().any(|task| {
            task.id == task_id
                && task
                    .steps
                    .iter()
                    .any(|step| step.id == pending.step && step.text == pending.text)
        });
        if !landed {
            return;
        }
        let saved_snapshot = self.tasks.iter().find(|task| task.id == task_id).cloned();
        let form = self.form.as_mut().expect("task form checked above");
        form.steps.pending_save = None;
        form.task_snapshot = saved_snapshot.map(Box::new);
        if pending.exit_editing {
            form.editing = false;
            form.steps.editor = None;
            form.steps.add_selected = false;
            self.input_mode = BoardInputMode::TaskPage;
        } else if pending.reopen {
            // The rapid-capture loop: the in-place row reopens empty for the next step, its
            // mode never having left it. A positioned row follows the step it just added, so
            // consecutive steps keep their typed order and the selector rides along.
            let anchored = form
                .steps
                .editor
                .as_ref()
                .is_some_and(|editor| editor.insert_after.is_some());
            form.steps.add_selected = false;
            form.steps.editor = Some(StepEditor {
                buffer: seeded_draft(""),
                rename: None,
                pending_index: None,
                insert_after: anchored.then_some(pending.step),
                refusal: None,
            });
            let new_index = self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .and_then(|task| task.steps.iter().position(|step| step.id == pending.step));
            if let Some(index) = new_index.filter(|_| anchored) {
                form.steps.cursor = Some(index);
                // Measure the saved step at the last painted width, then keep the reopened
                // row below it in view; the next paint re-records the real heights.
                let width = form.steps.text_width.get();
                let rows = if width == 0 {
                    1
                } else {
                    crate::ui::edit::wrap_text(&pending.text, width)
                        .len()
                        .max(1)
                };
                let through_new: usize = {
                    let mut counts = form.steps.row_counts.borrow_mut();
                    if index <= counts.len() {
                        counts.insert(index, rows);
                    }
                    counts.iter().take(index + 1).sum()
                };
                steps_scroll_to_row(form, through_new, rows);
            }
        } else {
            form.steps.editor = None;
            form.steps.cursor = self
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .and_then(|task| task.steps.iter().position(|step| step.id == pending.step));
            form.steps.add_selected = false;
            self.input_mode = BoardInputMode::TaskPage;
        }
        self.clear_message();
    }

    /// Focused field of the shared board form, if one is open.
    pub fn form_focus(&self) -> Option<CaptureField> {
        self.form.as_ref().map(|form| form.focus)
    }

    /// Chosen scope draft of the shared board form.
    pub fn form_scope(&self) -> Option<&TaskScope> {
        self.form.as_ref().map(|form| &form.scope)
    }

    /// The shared form's choices. It never includes the board selector's all-projects option.
    pub fn form_scope_options(&self) -> Vec<TaskScope> {
        self.form
            .as_ref()
            .map(|form| form.scope_options.clone())
            .unwrap_or_default()
    }

    /// The currently highlighted scope choice while the form dropdown is open.
    pub fn form_scope_dropdown_choice(&self) -> Option<&TaskScope> {
        let form = self.form.as_ref()?;
        form.scope_options.get(form.scope_selected)
    }

    /// Apply the shared cursor-window origin contract whenever form focus enters Notes.
    fn set_form_focus(form: &mut BoardForm, focus: CaptureField) {
        form.focus = focus;
        form.manual_page_scroll = false;
        // Notes drafts are cursor-windowed rather than a full copy of the shared
        // content stream. Entering the editor therefore returns its window to the
        // visible origin, so a prior reading scroll cannot hide the draft or put
        // the terminal caret on a step row.
        if focus == CaptureField::Notes && form.is_task() {
            form.notes_scroll = 0;
        }
    }

    /// Enter field edit on the page's own focused field (Tab from view mode).
    pub(super) fn enter_page_field_focus(&mut self) {
        let Some(form) = self.form.as_mut().filter(|form| form.is_task()) else {
            return;
        };
        form.editing = true;
        Self::set_form_focus(form, form.focus);
        self.input_mode = form.parent_mode();
    }

    pub(super) fn move_form_focus(&mut self, forward: bool) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        if forward {
            form.focus_next();
        } else {
            form.focus_prev();
        }
        if form.is_task() {
            form.editing = true;
        }
        Self::set_form_focus(form, form.focus);
        self.input_mode = form.parent_mode();
    }

    /// Park an existing-step draft before another task field becomes active. An empty new-step
    /// add is discarded so a click or Tab can leave it. A typed add stays on its row.
    pub(super) fn park_rename_step_draft(&mut self) -> bool {
        if self.input_mode != BoardInputMode::EditStep {
            return true;
        }
        if self.empty_add_step_editor() {
            if let Some(form) = self.form.as_mut() {
                form.steps.editor = None;
            }
            self.input_mode = match self.form.as_ref() {
                Some(form) if form.is_task() => BoardInputMode::TaskPage,
                Some(form) => form.parent_mode(),
                None => BoardInputMode::TaskPage,
            };
            return true;
        }
        let Some(form) = self.form.as_mut() else {
            return false;
        };
        let Some(editor) = form.steps.editor.take() else {
            return true;
        };
        let Some(step_id) = editor.rename else {
            form.steps.editor = Some(editor);
            return false;
        };
        if !form.is_task() {
            form.steps.editor = Some(editor);
            return false;
        }
        form.steps.drafts.insert(step_id, editor.buffer);
        true
    }

    /// True when the inline add row is open and still empty after trim.
    pub fn empty_add_step_editor(&self) -> bool {
        self.input_mode == BoardInputMode::EditStep
            && self.form.as_ref().is_some_and(|form| {
                form.steps.editor.as_ref().is_some_and(|editor| {
                    editor.rename.is_none() && editor.buffer.value().trim().is_empty()
                })
            })
    }

    /// Copy the active existing-step buffer into the task session without releasing its row.
    /// The persistence boundary uses this for Shift+Enter so a failed save retains the editor.
    pub(super) fn stage_active_rename_draft(&mut self) -> bool {
        let Some(form) = self.form.as_mut().filter(|form| form.is_task()) else {
            return false;
        };
        let Some(editor) = form.steps.editor.as_ref() else {
            return false;
        };
        let Some(step_id) = editor.rename else {
            return false;
        };
        form.steps.drafts.insert(step_id, editor.buffer.clone());
        true
    }

    /// Whether the active inline row is an existing-step draft owned by the task session.
    pub fn has_active_step_rename(&self) -> bool {
        self.input_mode == BoardInputMode::EditStep
            && self
                .form
                .as_ref()
                .and_then(|form| form.steps.editor.as_ref())
                .is_some_and(|editor| editor.rename.is_some())
    }

    /// Focus one already-painted field without reopening or rebinding the shared form.
    pub(super) fn focus_form_field(&mut self, focus: CaptureField) {
        if !self.park_rename_step_draft() {
            return;
        }
        let Some(form) = self.form.as_mut() else {
            return;
        };
        if self.input_mode == BoardInputMode::FormDropdown {
            return;
        }
        if form.is_task() {
            form.editing = true;
        }
        Self::set_form_focus(form, focus);
        self.input_mode = form.parent_mode();
    }

    /// Whether the task page's cursor rests on a stored (not removed, not the `+ step`
    /// row) step. Enter toggles that step instead of acting on the page.
    pub fn stored_step_selected(&self) -> bool {
        if self.focused_surface() != FocusedSurface::Task
            || !matches!(
                self.input_mode,
                BoardInputMode::TaskPage | BoardInputMode::EditStep
            )
        {
            return false;
        }
        let Some(form) = self.form.as_ref().filter(|form| form.is_task()) else {
            return false;
        };
        if form.steps.add_selected {
            return false;
        }
        let (Some(task_id), Some(index)) = (form.task_id(), form.steps.cursor) else {
            return false;
        };
        self.tasks
            .iter()
            .find(|task| task.id == task_id)
            .and_then(|task| task.steps.get(index))
            .is_some_and(|step| !form.steps.removals.contains(&step.id))
    }

    /// The relation entry the task page's view-mode cursor rests on, resolved to its action.
    /// `Enter` acts on it instead of closing the page. An edit session has no relation cursor.
    pub fn selected_relation(&self) -> Option<RelationTarget> {
        if self.input_mode != BoardInputMode::TaskPage || self.task_editing() {
            return None;
        }
        let form = self.form.as_ref().filter(|form| form.is_task())?;
        if form.steps.cursor.is_some() || form.steps.add_selected {
            return None;
        }
        let index = form.steps.relation_cursor?;
        self.relation_targets().get(index).copied()
    }

    /// The intent one relation entry stands for: the parent-none line opens the parent
    /// picker; a parent or child (missing ones included, which then refuse) opens its page.
    /// Resolved at the keyboard and mouse boundaries, so the read-only gate and the save
    /// boundary classify the real intent.
    pub fn relation_intent(target: RelationTarget) -> crate::ui::input::BoardIntent {
        match target {
            RelationTarget::SetParent => crate::ui::input::BoardIntent::OpenParentPicker,
            RelationTarget::Open(id) | RelationTarget::Missing(id) => {
                crate::ui::input::BoardIntent::OpenRelatedTask(id)
            }
            RelationTarget::Question(id) => crate::ui::input::BoardIntent::Question(
                crate::ui::questions::QuestionIntent::FromTask(id),
            ),
            RelationTarget::Link(task, index) => crate::ui::input::BoardIntent::Question(
                crate::ui::questions::QuestionIntent::Link(task, index),
            ),
        }
    }

    /// The intent of the relation entry at `index` on the focused page, for a row click.
    pub fn relation_intent_at(&self, index: usize) -> Option<crate::ui::input::BoardIntent> {
        if self.input_mode != BoardInputMode::TaskPage || self.task_editing() {
            return None;
        }
        self.relation_targets()
            .get(index)
            .copied()
            .map(Self::relation_intent)
    }

    /// Whether the current task page has entered its edit session.
    pub fn task_editing(&self) -> bool {
        self.form
            .as_ref()
            .filter(|form| form.is_task())
            .is_some_and(|form| form.editing)
    }

    /// Whether the bound task session contains an unsaved user change.
    ///
    /// Geometry and editor presence are deliberately absent: opening an editor is clean. Only
    /// field values that differ from the bound snapshot, staged existing-step changes or removals,
    /// and a non-empty new-step draft make the session dirty.
    pub fn task_session_dirty(&self) -> bool {
        let Some(form) = self.form.as_ref().filter(|form| form.is_task()) else {
            return false;
        };
        let Some(snapshot) = form.task_snapshot.as_deref() else {
            return false;
        };
        if form.title.value() != snapshot.title
            || form.notes.value() != snapshot.notes.as_deref().unwrap_or_default()
            || form.assignee != snapshot.assigned()
            || form.base != snapshot.base
            || form.after != snapshot.after
            || form.scope != snapshot.scope
            || !form.steps.removals.is_empty()
        {
            return true;
        }
        let changed_existing_step = |id: Uuid, value: &str| {
            snapshot
                .steps
                .iter()
                .find(|step| step.id == id)
                .is_none_or(|step| step.text != value)
        };
        if form
            .steps
            .drafts
            .iter()
            .any(|(id, draft)| changed_existing_step(*id, draft.value()))
        {
            return true;
        }
        form.steps.editor.as_ref().is_some_and(|editor| {
            editor.rename.map_or_else(
                || !editor.buffer.value().is_empty(),
                |id| changed_existing_step(id, editor.buffer.value()),
            )
        })
    }

    pub(super) fn cycle_form_assignee(&mut self, forward: bool) {
        if let Some(form) = self.form.as_mut() {
            form.cycle_assignee(forward);
        }
    }

    /// Start the board from `config.toml`'s `[board]` message lines and name. Called once at
    /// open; the session's own changes are never written back. The board always groups by
    /// project, so `[board]` group and sort apply to `tsk list` only.
    pub fn set_layout_defaults(&mut self, defaults: crate::agents::BoardDefaults) {
        self.layout_defaults = defaults;
        self.messages_hidden = defaults.messages_hidden;
    }

    pub fn set_agent_profiles(&mut self, profiles: &crate::agents::AgentProfiles) {
        self.agent_names = profiles.names().map(str::to_string).collect();
        if let Some(form) = self.form.as_mut() {
            form.set_agent_names(&self.agent_names);
        }
    }

    pub(super) fn cycle_form_scope(&mut self) {
        if let Some(form) = self.form.as_mut() {
            form.cycle_scope();
        }
    }

    pub(super) fn open_form_dropdown(&mut self, field: CaptureField) {
        if !matches!(field, CaptureField::Scope | CaptureField::Assignee)
            || !self.park_rename_step_draft()
        {
            return;
        }
        let Some(form) = self.form.as_mut() else {
            return;
        };
        form.focus = field;
        match field {
            CaptureField::Scope => form.select_current_scope(),
            CaptureField::Assignee => form.select_current_assignee(),
            _ => unreachable!(),
        }
        self.input_mode = BoardInputMode::FormDropdown;
    }

    pub(super) fn close_form_dropdown(&mut self, apply: bool) -> bool {
        let Some(form) = self.form.as_mut() else {
            return false;
        };
        match (form.focus, apply) {
            (CaptureField::Scope, true) => form.apply_scope_selection(),
            (CaptureField::Scope, false) => form.select_current_scope(),
            (CaptureField::Assignee, true) => form.apply_assignee_selection(),
            (CaptureField::Assignee, false) => form.select_current_assignee(),
            _ => return false,
        }
        self.input_mode = form.parent_mode();
        true
    }

    pub(super) fn move_form_dropdown(&mut self, forward: bool) {
        if self.input_mode != BoardInputMode::FormDropdown {
            return;
        }
        if let Some(form) = self.form.as_mut() {
            match form.focus {
                CaptureField::Scope => form.move_scope_selection(forward),
                CaptureField::Assignee => form.move_assignee_selection(forward),
                _ => {}
            }
        }
    }

    /// Apply a clicked source option exactly as keyboard navigation plus Enter would.
    pub(super) fn select_form_dropdown_option(&mut self, index: usize) -> bool {
        if self.input_mode != BoardInputMode::FormDropdown {
            return false;
        }
        let Some(form) = self.form.as_mut() else {
            return false;
        };
        match form.focus {
            CaptureField::Scope if index < form.scope_options.len() => {
                form.scope_selected = index;
                form.apply_scope_selection();
            }
            CaptureField::Assignee if index < form.assignee_options.len() => {
                form.assignee_selected = index;
                form.apply_assignee_selection();
            }
            _ => return false,
        }
        self.input_mode = form.parent_mode();
        true
    }

    /// Compact key legend for a modal surface; the board itself has its verb bar instead.
    pub fn help_line(&self) -> &'static str {
        match self.input_mode() {
            BoardInputMode::Palette => COMMAND_SURFACE_HELP_LINE,
            BoardInputMode::SaveRecovery => SAVE_RECOVERY_HELP_LINE,
            BoardInputMode::LaunchCard => LAUNCH_CARD_HELP_LINE,
            BoardInputMode::Help => HELP_SURFACE_HELP_LINE,
            BoardInputMode::Search => crate::ui::input::SEARCH_HELP_LINE,
            _ => "",
        }
    }

    /// Chrome status/message line feedback.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref().or(self.cleanup_status.as_deref())
    }

    pub fn update_notice(&self) -> Option<&str> {
        self.update_notice.as_deref()
    }

    pub fn set_update_notice(&mut self, notice: Option<String>) {
        self.update_notice = notice;
    }

    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.message = Some(terminal_text(&msg.into()));
        self.message_expires_at = None;
        self.message_restore = None;
    }

    /// Status feedback that clears itself after `ttl` (copy confirmation, brief notices).
    ///
    /// A sticky status already on the line (save-recovery banner, etc.) is stashed and
    /// restored when the toast expires, so a `copied` flash cannot erase it.
    pub fn set_ephemeral_message(&mut self, msg: impl Into<String>, ttl: std::time::Duration) {
        if self.message_expires_at.is_none() {
            self.message_restore = self.message.clone();
        }
        self.message = Some(terminal_text(&msg.into()));
        self.message_expires_at = Some(Instant::now() + ttl);
    }

    /// The status line stays until the next action rather than expiring on its own.
    pub fn message_is_sticky(&self) -> bool {
        self.message.is_some() && self.message_expires_at.is_none()
    }

    pub fn clear_message(&mut self) {
        self.message = None;
        self.message_expires_at = None;
        self.message_restore = None;
    }

    /// Drop an ephemeral status line whose TTL has elapsed. Sticky messages are untouched;
    /// a stashed sticky under a toast is restored.
    pub fn expire_ephemeral_message(&mut self) {
        if self
            .message_expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.message = self.message_restore.take();
            self.message_expires_at = None;
        }
    }

    /// Title carried by the visible delete recovery notice, if one is armed.
    pub fn delete_notice(&self) -> Option<&str> {
        self.delete_notice.as_deref()
    }

    /// The deleted task's `T` number the visible notice paints before its title, if any.
    pub(super) fn delete_notice_label(&self) -> Option<&str> {
        self.delete_notice_label.as_deref()
    }

    /// Arm the notice for a task that has just been soft-deleted. `label` is its `T` number;
    /// a notice task (`N…`) passes `None` and keeps the title alone.
    pub(super) fn arm_delete_notice(&mut self, label: Option<&str>, title: &str) {
        self.delete_notice = Some(terminal_text(title));
        self.delete_notice_count = None;
        self.delete_notice_label = label.map(terminal_text);
        self.delete_notice_expires_at = Some(Instant::now() + DELETE_NOTICE_TTL);
    }

    /// Arm the counted notice for a bulk delete transaction.
    pub(super) fn arm_bulk_delete_notice(&mut self, count: usize) {
        self.delete_notice = Some(format!("{count} tasks"));
        self.delete_notice_count = Some(count);
        self.delete_notice_label = None;
        self.delete_notice_expires_at = Some(Instant::now() + DELETE_NOTICE_TTL);
    }

    /// Take a delete recovery notice down once its few seconds have passed. The footer's
    /// undo seat is untouched: it follows the undo entry, not the notice.
    pub fn expire_delete_notice(&mut self) {
        if self
            .delete_notice_expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.clear_delete_notice();
        }
    }

    /// Put back the undo hint the failed mutation's sync dropped. Call before syncing the
    /// restored baseline, which keeps it only if its entry is still the live top there.
    pub fn restore_undo_hint_before_save(&mut self) {
        if let Some(entry) = self.undo_hint_before_save.take() {
            self.undo_hint = Some(entry);
        }
    }

    /// True while the footer should keep offering `ctrl+u undo` for the last delete.
    pub(super) fn undo_hint_live(&self) -> bool {
        self.undo_hint.is_some()
    }

    pub(super) fn visible_delete_notice_count(&self) -> Option<usize> {
        self.visible_delete_notice()?;
        self.delete_notice_count
    }

    /// Take the notice down. See the chrome-row lifetime rule above [`apply_intent`].
    ///
    /// Private on purpose: the lifetime rule is one rule in one place, and a fourth caller
    /// clearing the notice on an event of its own is exactly the drift forbids.
    pub(super) fn clear_delete_notice(&mut self) {
        self.delete_notice = None;
        self.delete_notice_count = None;
        self.delete_notice_label = None;
        self.delete_notice_expires_at = None;
    }

    /// The notice as far as the chrome row is concerned: armed, and not hidden under a modal
    /// command surface.
    ///
    /// An open action sheet or palette is modal and owns every click ([`super::mouse`]), so
    /// the row does not paint a control the click path would swallow: painted state and
    /// clickable state say the same thing.
    pub(super) fn visible_delete_notice(&self) -> Option<&str> {
        if self.surface != CommandSurface::None {
            return None;
        }
        self.delete_notice.as_deref()
    }

    /// True when the chrome row is the board's plain message row rather than the row an
    /// open field edit owns (which keeps its save chord and `Esc` beside whatever it says).
    ///
    /// The renderer and the hit test both read this, so the notice's Undo control can never
    /// be painted where a click would miss it.
    pub fn chrome_row_is_plain(&self) -> bool {
        self.form.is_none()
    }

    /// The field edit the board has open, if any.
    ///
    /// The one answer to "is a field being edited", read by the chrome row, by the compact
    /// Browse edit band ([`super::mouse::BoardLayout::edit_band`]) and by the renderer, so
    /// none of the three can decide differently from the others.
    /// The task an open field edit is bound to, or `None` outside an edit session.
    ///
    /// Read-only, and deliberately so: [`apply_board_intent_with_save_recovery`] needs the bound
    /// identity to judge availability against the durable record before a confirm mutates
    /// anything, and that judgment must never be able to *change* the
    /// binding. Open is still the only writer.
    ///
    /// [`apply_board_intent_with_save_recovery`]: crate::app::apply_board_intent_with_save_recovery
    pub fn edit_target(&self) -> Option<Uuid> {
        self.form.as_ref().and_then(BoardForm::task_id)
    }

    pub fn open_field_edit(&self) -> Option<BoardInputMode> {
        match self.input_mode {
            mode @ (BoardInputMode::EditTitle
            | BoardInputMode::EditNotes
            | BoardInputMode::EditScope
            | BoardInputMode::EditStep) => Some(mode),
            _ => None,
        }
    }

    /// The chrome row an open field edit owns, composed for `width` columns.
    ///
    /// Same order as [`Self::chrome_row`], with two differences the mode forces: the edit's
    /// own legend is never dropped (it is the only statement of the save chord and `Esc` in
    /// Browse), and the notice is stated **without** its `u Undo` control, because `u` types
    /// a `u` into the draft here and there is no hit region on this row. The deletion stays
    /// visible; the route it names comes back with the row when the edit closes.
    /// Seed selection on open: first visible NEEDS YOU id, else IN MOTION, else ON DECK.
    ///
    /// Honors collapse state via [`Self::visible_ids`]: a seeded row must be one the
    /// renderer painted, or verbs would mutate a task the user cannot see.
    pub(super) fn seed_selection(&mut self) {
        let visible: HashSet<Uuid> = self.visible_ids().into_iter().collect();
        let view = self.queue_view();
        for kind in [
            SectionKind::NeedsYou,
            SectionKind::InMotion,
            SectionKind::OnDeck,
            SectionKind::Inbox,
        ] {
            if let Some(id) = view
                .sections
                .iter()
                .filter(|section| section.kind == kind)
                .flat_map(|section| section.task_ids.iter().copied())
                .find(|id| visible.contains(id))
            {
                self.retarget_selection(Some(id), SelectionRetarget::Reanchor);
                self.follow_list.set(true);
                return;
            }
        }
        if view
            .sections
            .iter()
            .any(|section| section.kind == SectionKind::Inbox)
            && self.inbox_collapsed
        {
            self.retarget_selection(
                Some(queue::INBOX_HEADER_ROW_ID),
                SelectionRetarget::Reanchor,
            );
        } else {
            // Every live row may sit in folded groups: their sub-headers are the only
            // selectable rows left, so rest on the first rather than on nothing.
            let first_header = self
                .visible_ids()
                .into_iter()
                .find(|id| queue::is_group_header_id(*id));
            self.retarget_selection(first_header, SelectionRetarget::Reanchor);
        }
        self.follow_list.set(true);
    }

    /// The only boundary that may replace the selected task while a task form is retained.
    /// A dirty form keeps both its selection and immutable binding on the same task.
    fn retarget_selection(&mut self, requested: Option<Uuid>, source: SelectionRetarget) -> bool {
        let bound = self.edit_target();
        let bound_is_visible = bound.is_some_and(|id| self.visible_ids().contains(&id));
        let must_keep_dirty_binding = source == SelectionRetarget::Explicit || bound_is_visible;
        if self.task_session_dirty() && must_keep_dirty_binding && requested != bound {
            if bound_is_visible {
                self.selection_id = bound;
            }
            if self.popup != BoardPopup::SaveRecovery {
                self.set_message(DIRTY_TASK_SWITCH_REFUSAL);
            }
            return false;
        }
        if source == SelectionRetarget::Explicit
            && requested != bound
            && self.form.as_ref().is_some_and(BoardForm::is_task)
        {
            let archived = self.archived_projects.clone();
            self.form = requested.and_then(|id| {
                self.tasks.iter().find(|task| task.id == id).map(|task| {
                    BoardForm::task(
                        task,
                        self.this_repo.as_deref(),
                        &self.tasks,
                        CaptureField::Title,
                        &archived,
                        &self.agent_names,
                    )
                })
            });
            self.input_mode = if self.form.is_some() {
                BoardInputMode::TaskPage
            } else {
                BoardInputMode::Normal
            };
        }
        // A direct list reselection ends the relation trail: Back belongs to the page the
        // relation opens left behind, not to the next task the list picks.
        if source == SelectionRetarget::Explicit {
            self.page_back.clear();
        }
        self.selection_id = requested;
        true
    }

    /// Re-pin selection after the visible set changes through the shared retarget boundary.
    pub(super) fn reanchor_selection(&mut self, previous: Option<Uuid>, previous_visible: &[Uuid]) {
        let new_visible = self.visible_ids();
        let requested = selection::reanchor(previous, previous_visible, &new_visible);
        // Group headers are selectable chrome, but when a task leaves the view and a
        // real task survives, keep the selection on that task instead of landing on
        // the inbox/archived heading merely because it was nearest by row index.
        let requested = if let (Some(previous), Some(candidate)) = (previous, requested) {
            if !is_header_row(previous)
                && is_header_row(candidate)
                && new_visible.iter().any(|id| !is_header_row(*id))
            {
                nearest_surviving_task(previous, previous_visible, &new_visible).or(Some(candidate))
            } else {
                Some(candidate)
            }
        } else {
            requested
        };
        if !self.retarget_selection(requested, SelectionRetarget::Reanchor) {
            return;
        }
        self.follow_list.set(true);
        // `reanchor` defers when `previous` was `None`; if rows exist again (e.g. CancelSave
        // restored a completed task into the deck), seed rather than leave the pin empty.
        if self.selection_id.is_none() && !new_visible.is_empty() {
            self.seed_selection();
        }
    }

    pub(super) fn select_next(&mut self) -> bool {
        let ids = self.visible_ids();
        let requested = if ids.is_empty() {
            None
        } else {
            Some(
                match self
                    .selection_id
                    .and_then(|id| ids.iter().position(|&row| row == id))
                {
                    Some(i) => ids[(i + 1) % ids.len()],
                    None => ids[0],
                },
            )
        };
        if !self.retarget_selection(requested, SelectionRetarget::Explicit) {
            return false;
        }
        self.close_popup();
        self.follow_list.set(true);
        true
    }

    pub(super) fn select_prev(&mut self) -> bool {
        let ids = self.visible_ids();
        let requested = if ids.is_empty() {
            None
        } else {
            Some(
                match self
                    .selection_id
                    .and_then(|id| ids.iter().position(|&row| row == id))
                {
                    Some(0) | None => ids[ids.len() - 1],
                    Some(i) => ids[i - 1],
                },
            )
        };
        if !self.retarget_selection(requested, SelectionRetarget::Explicit) {
            return false;
        }
        self.close_popup();
        self.follow_list.set(true);
        true
    }

    pub(super) fn select_index(&mut self, idx: usize) -> bool {
        let Some(&requested) = self.visible_ids().get(idx) else {
            return false;
        };
        if !self.retarget_selection(Some(requested), SelectionRetarget::Explicit) {
            return false;
        }
        self.close_popup();
        // Nudge only if this row is off-screen; a click on a visible row stays put.
        self.follow_list.set(true);
        true
    }

    pub(super) fn set_list_scroll(&mut self, offset: usize) {
        self.list_scroll.set(offset);
        self.follow_list.set(false);
    }

    /// Put the cursor on the first painted row of `project`'s group, or leave it where
    /// `seed_selection` put it when that project has no painted row.
    pub(super) fn focus_project_group(&mut self, project: &Path) {
        let wanted = project.to_string_lossy();
        let first = self.visible_ids().into_iter().find(|id| {
            self.tasks.iter().any(|task| {
                task.id == *id
                    && matches!(&task.scope, TaskScope::Project { path } if paths_equivalent(path, &wanted))
            })
        });
        if let Some(id) = first {
            self.retarget_selection(Some(id), SelectionRetarget::Reanchor);
            self.follow_list.set(true);
        }
    }

    /// Pin the board scope to one project, or to the desk for `None`. The board itself
    /// never offers this (it always shows every project grouped); embedders and tests do.
    pub fn set_selected_project(&mut self, project: Option<PathBuf>) {
        self.clear_marks();
        self.close_popup();
        self.page_back.clear();
        self.search_query.clear();
        self.search_pinned = false;
        if self.input_mode == BoardInputMode::Search {
            self.input_mode = BoardInputMode::Normal;
        }
        match project {
            None => self.switch_location(BoardLocation::Desk),
            Some(path) => {
                self.selected_project = Some(path.clone());
                self.switch_location(BoardLocation::Project(path));
            }
        }
    }
}
/// Compact label for one project path.
///
/// Prefers the last path segment so the Projects header stays readable on narrow panes.
pub fn project_option_label(path: &Path) -> String {
    terminal_text(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&path.to_string_lossy()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{HumanStatus, ProvenanceOrigin};

    fn run_row(number: u64, state: CleanupRowState) -> CleanupRunRow {
        CleanupRunRow {
            task_id: Uuid::new_v4(),
            number,
            slot: Some(0),
            inspected: None,
            state,
        }
    }

    fn cleaned() -> CleanupRowState {
        CleanupRowState::cleaned(None)
    }

    fn kept(short: &str) -> CleanupRowState {
        CleanupRowState::Kept {
            short: short.into(),
            full: short.into(),
        }
    }

    fn run(rows: Vec<CleanupRunRow>, bulk: Option<(usize, usize)>) -> CleanupRun {
        CleanupRun {
            job: crate::dispatch::CleanupJob::new(0),
            rows,
            bulk,
            refused: Vec::new(),
            settled: true,
            waiting: Vec::new(),
            plan: Vec::new(),
            start_deadline: Instant::now(),
            started: true,
            merged: Vec::new(),
            unmerged: Vec::new(),
            workspaces: Vec::new(),
        }
    }

    #[test]
    fn ctrl_g_addresses_inbox_unless_the_open_drawer_has_archived_rows() {
        let mut domain = DomainState::new();
        let task_a = create(&mut domain, "task-a", project(REPO_A));
        create(&mut domain, "task-b", project(REPO_B));

        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO_A)));
        // Todo has no separate inbox group to fold.
        assert!(!model.toggle_all_groups());
        assert!(!model.inbox_collapsed);
        model.drawer_open = true;
        assert!(!model.toggle_all_groups());
        assert!(!model.inbox_collapsed);

        domain.archive_task(task_a).expect("archive");
        model.sync_from_domain(&domain);
        model.drawer_open = true;
        // The archived group starts collapsed; the open drawer gives it priority.
        assert!(model.toggle_all_groups());
        assert!(!model.archived_collapsed);
        assert!(model.toggle_all_groups());
        assert!(model.archived_collapsed);
        assert!(!model.inbox_collapsed, "archived takes priority over inbox");
    }

    #[test]
    fn cleanup_summaries_count_and_name_only_what_was_kept() {
        let all = run(
            (160..165)
                .map(|number| run_row(number, cleaned()))
                .collect(),
            Some((5, 0)),
        );
        assert_eq!(all.summary(), ("done 5 · cleaned 5".to_string(), true));

        let mut rows: Vec<_> = (160..164)
            .map(|number| run_row(number, cleaned()))
            .collect();
        rows.push(run_row(164, kept("uncommitted changes")));
        assert_eq!(
            run(rows, Some((5, 0))).summary(),
            (
                "done 5 · cleaned 4 · kept T164 (uncommitted changes)".to_string(),
                false
            )
        );
        let branch = run(
            vec![
                run_row(156, cleaned()),
                run_row(
                    157,
                    CleanupRowState::cleaned(Some((
                        "not merged".into(),
                        "not merged into main".into(),
                    ))),
                ),
            ],
            Some((2, 0)),
        );
        assert_eq!(
            branch.summary(),
            (
                "done 2 · cleaned 2 · branch kept T157 (not merged)".to_string(),
                false
            )
        );

        // Past the width, counts only: never wider than the status row.
        let many = run(
            (1..=4)
                .map(|number| run_row(number, kept("uncommitted changes")))
                .chain([run_row(5, cleaned())])
                .collect(),
            Some((6, 1)),
        );
        let (text, clean) = many.summary();
        assert_eq!(text, "done 6 · cleaned 1 · kept 4 · 1 already gone");
        assert!(!clean);

        assert_eq!(
            run(vec![run_row(5, cleaned())], None).summary(),
            ("done T5 · cleaned".to_string(), true)
        );
        assert_eq!(
            run(vec![run_row(5, kept("uncommitted changes"))], None).summary(),
            ("done T5 · kept (uncommitted changes)".to_string(), false)
        );
    }

    #[test]
    fn a_clean_cleanup_summary_expires_and_one_that_kept_anything_stays() {
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.post_cleanup_summary("done 5 · cleaned 5".into(), true);
        assert!(!model.message_is_sticky());
        model.message_expires_at = Some(Instant::now());
        model.expire_ephemeral_message();
        assert_eq!(model.message(), None);

        model.post_cleanup_summary(
            "done 5 · cleaned 4 · kept T164 (uncommitted changes)".into(),
            false,
        );
        model.expire_ephemeral_message();
        assert_eq!(
            model.message(),
            Some("done 5 · cleaned 4 · kept T164 (uncommitted changes)")
        );
        assert!(model.message_is_sticky());
    }

    #[test]
    fn an_expired_clean_summary_never_restores_a_busy_refusal() {
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.set_message(CLEANUP_BUSY);
        model.post_cleanup_summary("done 2 · cleaned 2".into(), true);
        assert_eq!(model.message(), Some("done 2 · cleaned 2"));
        model.message_expires_at = Some(Instant::now());
        model.expire_ephemeral_message();
        assert_eq!(model.message(), None);
    }

    #[test]
    fn a_quit_waiting_on_cleanup_is_due_once_the_run_ends_or_its_bound_passes() {
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        let mut unfinished = run(vec![run_row(1, CleanupRowState::Removing)], None);
        unfinished.settled = false;
        model.begin_cleanup_run(unfinished);
        model.begin_quit_after_cleanup();
        assert_eq!(model.message(), Some("finishing cleanup…"));
        assert!(!model.quit_after_cleanup_due());
        model.quit_after_cleanup = Some(Instant::now());
        assert!(
            model.quit_after_cleanup_due(),
            "bounded: a stuck host never pins the board"
        );
        model.quit_after_cleanup = Some(Instant::now() + QUIT_CLEANUP_BOUND);
        model.finish_cleanup_run();
        assert!(model.quit_after_cleanup_due());
    }

    fn loading_picker(request: Uuid, ids: Vec<Uuid>) -> ListPickerState {
        ListPickerState {
            kind: ListPickerKind::Base,
            options: vec![
                BoardModel::default_base_option("default".into()),
                ListPickerOption {
                    label: REFRESHING_BRANCHES.into(),
                    count: None,
                    value: ListPickerValue::Unavailable,
                },
            ],
            selected: 0,
            query: String::new(),
            return_mode: BoardInputMode::Normal,
            assignee_target: None,
            on_target: None,
            base_target: Some(BasePickerTarget {
                ids,
                edit_draft: false,
                request,
                project: PathBuf::from("/repos/picker"),
                current: None,
                selection_pending: true,
            }),
            after_target: None,
            active: None,
            tabs: None,
            rows: Default::default(),
        }
    }

    #[test]
    fn delayed_base_picker_result_cannot_replace_reopened_generation_or_target() {
        let id = Uuid::new_v4();
        for ids in [vec![id], vec![Uuid::new_v4()], Vec::new()] {
            let mut model = BoardModel::from_domain(&DomainState::new(), None);
            let old_request = Uuid::new_v4();
            model.list_picker = Some(loading_picker(old_request, vec![id]));
            model.input_mode = BoardInputMode::ListPicker;
            let mailbox = Arc::clone(&model.base_picker_results);
            let (release, wait) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                wait.recv().unwrap();
                mailbox.lock().unwrap().push(BasePickerResult {
                    request: old_request,
                    project: PathBuf::from("/repos/picker"),
                    branches: Ok((vec!["stale".into()], None)),
                    default: "stale-default".into(),
                });
            });
            model.cancel_list_picker();
            let new_request = Uuid::new_v4();
            model.list_picker = Some(loading_picker(new_request, ids.clone()));
            model.input_mode = BoardInputMode::ListPicker;
            model.list_picker_query_insert_text("release");
            model
                .base_picker_results
                .lock()
                .unwrap()
                .push(BasePickerResult {
                    request: new_request,
                    project: PathBuf::from("/repos/picker"),
                    branches: Ok((vec!["main".into(), "release".into()], None)),
                    default: "main".into(),
                });
            assert!(model.poll_base_picker_results());
            assert_eq!(model.list_picker_query(), Some("release"));
            assert_eq!(
                model.selected_list_picker_option().unwrap().1.value,
                ListPickerValue::Base(Some("release".into()))
            );
            assert_eq!(
                model
                    .list_picker
                    .as_ref()
                    .unwrap()
                    .base_target
                    .as_ref()
                    .unwrap()
                    .ids,
                ids
            );
            release.send(()).unwrap();
            worker.join().unwrap();
            assert!(!model.poll_base_picker_results());
            assert_eq!(
                model.selected_list_picker_option().unwrap().1.value,
                ListPickerValue::Base(Some("release".into()))
            );
            assert!(!model
                .list_picker
                .as_ref()
                .unwrap()
                .options
                .iter()
                .any(|option| option.label.contains("stale")));
        }
    }

    #[test]
    fn base_picker_refreshes_when_its_fetch_lands_during_the_cached_snapshot() {
        let mut model = BoardModel::from_domain(&DomainState::new(), None);
        let project = PathBuf::from(format!("/repos/race-{}", Uuid::new_v4()));
        let fetched = std::cell::Cell::new(false);
        model.open_base_picker_from(
            vec![Uuid::new_v4()],
            None,
            project.clone(),
            false,
            "main".into(),
            |_| fetched.get(),
            |_| {
                // The worker's fetch completes while the pre-fetch refs are being read.
                fetched.set(true);
                Ok(vec!["main".into()])
            },
        );
        let labels: Vec<_> = model
            .visible_list_picker_options()
            .into_iter()
            .map(|(_, option)| option.label)
            .collect();
        assert_eq!(
            labels.last().map(String::as_str),
            Some(REFRESHING_BRANCHES),
            "a refresh is subscribed for the pre-fetch snapshot"
        );
    }

    #[test]
    fn base_picker_rejects_another_projects_result_even_with_current_request() {
        let mut model = BoardModel::from_domain(&DomainState::new(), None);
        let request = Uuid::new_v4();
        model.list_picker = Some(loading_picker(request, vec![Uuid::new_v4()]));
        model
            .base_picker_results
            .lock()
            .unwrap()
            .push(BasePickerResult {
                request,
                project: PathBuf::from("/repos/different"),
                branches: Ok((vec!["wrong-project".into()], None)),
                default: "wrong-default".into(),
            });
        assert!(!model.poll_base_picker_results());
        assert_eq!(
            model.visible_list_picker_options()[1].1.label,
            REFRESHING_BRANCHES
        );
    }

    #[test]
    fn base_picker_results_preserve_selection_and_show_disabled_warnings() {
        let mut model = BoardModel::from_domain(&DomainState::new(), None);
        let request = Uuid::new_v4();
        let mut picker = loading_picker(request, vec![Uuid::new_v4()]);
        picker.base_target.as_mut().unwrap().current = Some("release".into());
        model.list_picker = Some(picker);
        model
            .base_picker_results
            .lock()
            .unwrap()
            .push(BasePickerResult {
                request,
                project: PathBuf::from("/repos/picker"),
                branches: Ok((
                    vec!["main".into(), "release".into()],
                    Some("offline: using cached refs".into()),
                )),
                default: "main".into(),
            });
        assert!(model.poll_base_picker_results());
        assert_eq!(
            model.selected_list_picker_option().unwrap().1.value,
            ListPickerValue::Base(Some("release".into()))
        );
        model.move_list_picker(true);
        assert_eq!(
            model.selected_list_picker_option().unwrap().1.value,
            ListPickerValue::Base(None)
        );
        assert!(!model.select_list_picker_option(3));
        assert_eq!(model.list_picker_selected(), 0);
        model.cancel_list_picker();
        model
            .base_picker_results
            .lock()
            .unwrap()
            .push(BasePickerResult {
                request,
                project: PathBuf::from("/repos/picker"),
                branches: Err("failed".into()),
                default: "default".into(),
            });
        assert!(!model.poll_base_picker_results());
        assert!(model.list_picker.is_none());
    }

    const REPO_A: &str = "/repos/a";
    const REPO_B: &str = "/repos/b";

    fn project(path: &str) -> TaskScope {
        TaskScope::Project {
            path: path.to_string(),
        }
    }

    fn create(domain: &mut DomainState, title: &str, scope: TaskScope) -> Uuid {
        domain
            .create(title, None, scope, ProvenanceOrigin::Manual)
            .expect("create")
    }

    #[test]
    fn ephemeral_status_clears_after_deadline() {
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.set_ephemeral_message("copied", std::time::Duration::from_millis(1));
        assert_eq!(model.message(), Some("copied"));
        std::thread::sleep(std::time::Duration::from_millis(5));
        model.expire_ephemeral_message();
        assert!(model.message().is_none());
        assert!(model.message_expires_at.is_none());
    }

    #[test]
    fn delete_notice_clears_after_its_deadline_but_the_undo_hint_stays() {
        let mut domain = DomainState::new();
        let id = create(&mut domain, "doomed", TaskScope::Global);
        let mut model = BoardModel::from_domain(&domain, None);
        domain.soft_delete(id).expect("delete");
        model.arm_delete_notice(Some("T1"), "doomed");
        model.undo_hint = domain.last_undo().cloned();
        model.sync_from_domain(&domain);

        model.expire_delete_notice();
        assert_eq!(model.delete_notice(), Some("doomed"), "not yet due");

        model.delete_notice_expires_at = Some(Instant::now() - std::time::Duration::from_secs(1));
        model.expire_delete_notice();
        assert!(model.delete_notice().is_none());
        assert!(model.delete_notice_label().is_none());
        assert!(
            model.undo_hint_live(),
            "the footer keeps offering undo while the delete's entry is live"
        );
    }

    #[test]
    fn ephemeral_toast_restores_sticky_save_recovery_banner() {
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.set_message("save failed: disk full · Retry or Cancel");
        model.set_ephemeral_message("copied", std::time::Duration::from_millis(1));
        assert_eq!(model.message(), Some("copied"));
        std::thread::sleep(std::time::Duration::from_millis(5));
        model.expire_ephemeral_message();
        assert_eq!(
            model.message(),
            Some("save failed: disk full · Retry or Cancel"),
            "sticky banner must return after the toast, not vanish"
        );
        assert!(model.message_expires_at.is_none());
    }

    #[test]
    fn sticky_status_survives_expire_tick() {
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.set_message("save failed");
        model.expire_ephemeral_message();
        assert_eq!(model.message(), Some("save failed"));
    }

    #[test]
    fn startup_in_a_repo_stays_on_every_project_and_seeds_on_its_group() {
        let mut domain = DomainState::new();
        let desk = create(&mut domain, "desk", TaskScope::Global);
        let b1 = create(&mut domain, "task-b", project(REPO_B));
        let a1 = create(&mut domain, "task-a", project(REPO_A));
        for id in [desk, b1, a1] {
            domain.set_status(id, HumanStatus::Doing).expect("start");
        }

        let model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO_A)));
        assert_eq!(model.nav_tab(), NavTab::Board);
        assert_eq!(model.board_location, BoardLocation::All);
        assert_eq!(model.active_project(), None);
        assert_eq!(
            model.selected_id(),
            Some(a1),
            "the cursor starts on the launch project's first row"
        );

        let model = BoardModel::from_tasks(domain.tasks().to_vec(), None);
        assert_eq!(model.board_location, BoardLocation::All);
        assert_eq!(model.sort(), BoardSort::Project);
    }

    #[test]
    fn startup_seeds_on_the_launch_project_under_an_equivalent_spelling() {
        let mut domain = DomainState::new();
        let desk = create(&mut domain, "desk", TaskScope::Global);
        let a1 = create(&mut domain, "task-a", project(REPO_A));
        for id in [desk, a1] {
            domain.set_status(id, HumanStatus::Doing).expect("start");
        }
        let launch = PathBuf::from(format!("{REPO_A}/"));
        let model = BoardModel::from_domain(&domain, Some(launch));
        assert_eq!(model.selected_id(), Some(a1));
    }

    #[test]
    fn every_project_board_groups_desk_first_then_by_project_path() {
        let mut domain = DomainState::new();
        let b1 = create(&mut domain, "b first", project(REPO_B));
        let desk = create(&mut domain, "desk", TaskScope::Global);
        let a1 = create(&mut domain, "a later", project(REPO_A));
        for id in [b1, desk, a1] {
            domain.set_status(id, HumanStatus::Doing).expect("start");
        }
        domain.assign_numbers_for_persistence();

        let mut model = BoardModel::from_tasks(domain.tasks().to_vec(), None);
        model.seed_selection();
        assert_eq!(model.sort(), BoardSort::Project);
        assert_eq!(
            model.visible_ids(),
            vec![desk, a1, b1],
            "project order: desk first, then project path"
        );
    }

    #[test]
    fn quick_add_from_every_scope_goes_to_the_live_launch_project_else_the_desk() {
        let domain = DomainState::new();
        let mut model =
            BoardModel::from_tasks(domain.tasks().to_vec(), Some(PathBuf::from(REPO_A)));
        model.board_location = BoardLocation::All;
        assert_eq!(model.quick_add_scope(), Some(TaskScope::Global));
        model.board_location = BoardLocation::Desk;
        assert_eq!(model.quick_add_scope(), Some(TaskScope::Global));
        model.board_location = BoardLocation::Project(PathBuf::from(REPO_B));
        assert_eq!(model.quick_add_scope(), Some(project(REPO_B)));

        // Launched inside a project: every-project and desk add there while it is live.
        model.session_default_scope = Some(project(REPO_A));
        model.board_location = BoardLocation::All;
        assert_eq!(model.quick_add_scope(), Some(project(REPO_A)));
        model.archived_projects = vec![REPO_A.to_string()].into_iter().collect();
        assert_eq!(model.quick_add_scope(), Some(TaskScope::Global));
    }

    #[test]
    fn picker_desk_choice_lands_on_the_desk_scope() {
        let mut domain = DomainState::new();
        create(&mut domain, "task-a", project(REPO_A));

        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO_A)));
        model.set_selected_project(Some(PathBuf::from(REPO_B)));
        model.set_selected_project(None);
        assert_eq!(model.board_location, BoardLocation::Desk);
    }

    #[test]
    fn sync_from_domain_reanchors_by_prior_order_not_new_order() {
        let mut domain = DomainState::new();
        let t1 = create(&mut domain, "task-1", project(REPO_A));
        let t2 = create(&mut domain, "task-2", project(REPO_A));
        let t3 = create(&mut domain, "task-3", project(REPO_A));
        let t4 = create(&mut domain, "task-4", project(REPO_A));

        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO_A)));
        model.selection_id = Some(t3);

        // One sync carries both an external removal of the rows before the selection
        // (t1, t2 gone) and the local completion of the selection itself (t3 done).
        domain.soft_delete(t2).expect("soft delete");
        domain.soft_delete(t1).expect("soft delete");
        domain.set_status(t3, HumanStatus::Done).expect("status");
        model.sync_from_domain(&domain);

        assert_eq!(
            model.selected_id(),
            Some(t4),
            "selection must reanchor to the old-order neighbor, not the first new row"
        );
    }

    #[test]
    fn sync_from_domain_does_not_yank_home_tab_for_externally_created_tasks() {
        let mut domain = DomainState::new();
        let desk_task = create(&mut domain, "desk task", TaskScope::Global);

        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO_A)));
        model.board_location = BoardLocation::Desk;
        model.seed_selection();

        // Another process adds a project task between two syncs.
        create(&mut domain, "external task", project(REPO_B));
        model.sync_from_domain(&domain);

        assert_eq!(
            model.board_location,
            BoardLocation::Desk,
            "a background merge must not move the user's scope"
        );
        assert_eq!(model.selected_id(), Some(desk_task));
    }

    #[test]
    fn sync_from_domain_surfaces_the_first_arriving_task_only_when_it_is_visible_here() {
        let domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO_A)));
        model.board_location = BoardLocation::Desk;
        assert!(model.visible_ids().is_empty());

        // Another process captures a desk-visible task while this board shows nothing:
        // the desk's global lanes render it, so the pin moves to it.
        let mut domain = DomainState::new();
        let started = create(&mut domain, "first capture", TaskScope::Global);
        domain
            .set_status(started, HumanStatus::Doing)
            .expect("start");
        model.sync_from_domain(&domain);

        assert_eq!(
            model.board_location,
            BoardLocation::Desk,
            "surfacing never moves the scope"
        );
        assert_eq!(model.selected_id(), Some(started));

        // A task the current destination does not render never drags the view there.
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO_A)));
        model.board_location = BoardLocation::Desk;
        model.selection_id = None;
        let mut domain = DomainState::new();
        create(&mut domain, "elsewhere", project(REPO_B));
        model.sync_from_domain(&domain);
        assert_eq!(model.board_location, BoardLocation::Desk);
        assert_eq!(
            model.selected_id(),
            None,
            "an invisible arrival stays invisible rather than switching tabs"
        );
    }

    #[test]
    fn pinned_quick_add_save_does_not_pin_invisible_row_under_project_focus() {
        let mut domain = DomainState::new();
        let in_a = create(&mut domain, "focus task", project(REPO_A));
        let in_b = create(&mut domain, "other task", project(REPO_B));

        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO_A)));
        model.board_location = BoardLocation::Project(PathBuf::from(REPO_A));
        model.selection_id = Some(in_a);

        // A quick-add draft saved with a !p token for another project: the saved task
        // exists but the focused lens does not render it.
        model.begin_quick_add_save(in_b, false);
        model.sync_from_domain(&domain);

        assert_eq!(
            model.selected_id(),
            Some(in_a),
            "under project focus a save outside the scope must not pin an invisible row,              and must keep instead of jumping to the first row"
        );
    }
}
