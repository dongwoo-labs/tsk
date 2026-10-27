//! Queue Section Query: pure derivation of the board sections from a task snapshot.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::SystemTime;

use uuid::Uuid;

use crate::domain::{BlockOn, HumanStatus, Task, TaskScope};
use crate::scope::PathIdentityCache;

/// The board's persistent navigation tabs: `1` the task board, `2` the questions, `3` the
/// completion reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavTab {
    #[default]
    Board,
    Questions,
    Reports,
}

/// Session-only row order of every board section. Task numbers are addresses, so the
/// default order is stable and predictable: a status change never moves a row to the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoardSort {
    /// Notices first, then ascending task number.
    #[default]
    Number,
    /// Desk first, then project path; ascending task number inside each project.
    Project,
}

/// Which list region a section belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// Blocked and review tasks that need a human.
    NeedsYou,
    InMotion,
    OnDeck,
    /// Open tasks under ON DECK (header + dim rows, collapsible).
    Inbox,
    Done,
    /// The done drawer's archived group (header + dim rows, collapsible).
    Archived,
}

/// Row id the archived group's header occupies in `visible_task_ids`.
/// Chrome, not a task: `BoardModel::selected_id()` hides it from verbs.
pub const ARCHIVED_HEADER_ROW_ID: Uuid = Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_0000_0000_0000_0001);

/// Row id the inbox group's header occupies in `visible_task_ids`.
/// Chrome, not a task: `BoardModel::selected_id()` hides it from verbs.
pub const INBOX_HEADER_ROW_ID: Uuid = Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_0000_0000_0000_0002);

/// Session thread filter for the project board. It narrows every status section,
/// done drawer included; `All` is the unfiltered board.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ThreadFilter {
    #[default]
    All,
    /// Only tasks whose normalized thread matches this name (ASCII-case-insensitive).
    Named(String),
    /// Only tasks with no thread at all.
    Without,
}

impl ThreadFilter {
    /// Whether `task` passes this filter.
    pub(crate) fn admits(&self, task: &Task) -> bool {
        match self {
            ThreadFilter::All => true,
            ThreadFilter::Named(name) => task
                .thread
                .as_deref()
                .is_some_and(|thread| thread.eq_ignore_ascii_case(name)),
            ThreadFilter::Without => task.thread.is_none(),
        }
    }

    /// The label the board's thread control paints for this filter.
    pub fn label(&self) -> String {
        match self {
            ThreadFilter::All => "all".to_string(),
            ThreadFilter::Named(name) => format!("#{name}"),
            ThreadFilter::Without => "without a thread".to_string(),
        }
    }
}

/// Session assignee filter for the project board. It combines with the thread filter
/// (both must admit a task) and narrows every status section, done drawer included.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AssigneeFilter {
    #[default]
    All,
    /// Only tasks assigned to this name (ASCII-case-insensitive), even when its profile
    /// was removed from `config.toml`.
    Named(String),
    /// Only tasks with no assignee.
    Unassigned,
}

impl AssigneeFilter {
    /// Whether `task` passes this filter.
    fn admits(&self, task: &Task) -> bool {
        match self {
            AssigneeFilter::All => true,
            AssigneeFilter::Named(name) => task
                .assignee
                .as_deref()
                .is_some_and(|assignee| assignee.eq_ignore_ascii_case(name)),
            AssigneeFilter::Unassigned => task.assignee.is_none(),
        }
    }

    /// The label the board's filter control paints for this filter.
    pub fn label(&self) -> String {
        match self {
            AssigneeFilter::All => "all".to_string(),
            AssigneeFilter::Named(name) => crate::ui::render::assignee_handle(name),
            AssigneeFilter::Unassigned => "unassigned".to_string(),
        }
    }
}

/// The project board's combined filter: a thread choice AND an assignee choice.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoardFilter {
    pub thread: ThreadFilter,
    pub assignee: AssigneeFilter,
}

impl BoardFilter {
    /// A thread-only filter (assignee `All`).
    pub fn thread(thread: ThreadFilter) -> Self {
        Self {
            thread,
            assignee: AssigneeFilter::All,
        }
    }

    /// An assignee-only filter (thread `All`).
    pub fn assignee(assignee: AssigneeFilter) -> Self {
        Self {
            thread: ThreadFilter::All,
            assignee,
        }
    }

    pub fn is_all(&self) -> bool {
        self.thread == ThreadFilter::All && self.assignee == AssigneeFilter::All
    }

    /// The chip label: `all`, one active choice, or both joined by a space
    /// (`#release @claude`).
    pub fn label(&self) -> String {
        match (&self.thread, &self.assignee) {
            (ThreadFilter::All, AssigneeFilter::All) => "all".to_string(),
            (thread, AssigneeFilter::All) => thread.label(),
            (ThreadFilter::All, assignee) => assignee.label(),
            (thread, assignee) => format!("{} {}", thread.label(), assignee.label()),
        }
    }
}

/// Session project filter for the desk's live sections. `All` is the unfiltered desk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectFilter {
    #[default]
    All,
    /// Only projectless (desk) tasks.
    Desk,
    /// Only tasks of the project at this stored path (path spellings compared by identity).
    Named(String),
}

impl ProjectFilter {
    /// The label the desk's filter control paints for this dimension, `None` when unset.
    pub fn label(&self) -> Option<String> {
        match self {
            ProjectFilter::All => None,
            ProjectFilter::Desk => Some("desk".to_string()),
            ProjectFilter::Named(path) => Some(short_project_name(path).to_string()),
        }
    }
}

/// The desk's session filter: one project, one thread, and one assignee choice, combined.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeskFilter {
    pub project: ProjectFilter,
    pub thread: ThreadFilter,
    pub assignee: AssigneeFilter,
}

impl DeskFilter {
    pub fn is_all(&self) -> bool {
        self.project == ProjectFilter::All
            && self.thread == ThreadFilter::All
            && self.assignee == AssigneeFilter::All
    }

    /// The desk filter control's label: `all`, or the active choices, e.g.
    /// `tsk #release @claude`.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(project) = self.project.label() {
            parts.push(project);
        }
        if self.thread != ThreadFilter::All {
            parts.push(self.thread.label());
        }
        if self.assignee != AssigneeFilter::All {
            parts.push(self.assignee.label());
        }
        if parts.is_empty() {
            "all".to_string()
        } else {
            parts.join(" ")
        }
    }

    fn admits(&self, task: &Task, identities: &PathIdentityCache) -> bool {
        let project = match (&self.project, &task.scope) {
            (ProjectFilter::All, _) => true,
            (ProjectFilter::Desk, scope) => matches!(scope, TaskScope::Global),
            (ProjectFilter::Named(wanted), TaskScope::Project { path }) => {
                identities.equivalent(path, wanted)
            }
            (ProjectFilter::Named(_), TaskScope::Global) => false,
        };
        project && self.thread.admits(task) && self.assignee.admits(task)
    }
}

/// One-level grouping inside the live status sections. Session-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupBy {
    #[default]
    None,
    Project,
    Thread,
    Assignee,
}

impl GroupBy {
    pub const ALL: [GroupBy; 4] = [
        GroupBy::None,
        GroupBy::Project,
        GroupBy::Thread,
        GroupBy::Assignee,
    ];

    pub fn word(self) -> &'static str {
        match self {
            GroupBy::None => "none",
            GroupBy::Project => "project",
            GroupBy::Thread => "thread",
            GroupBy::Assignee => "assignee",
        }
    }

    /// The choice a [`Self::word`] names, as `config.toml` spells it.
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|group| group.word() == word)
    }
}

/// Row order inside a group, or inside a section while ungrouped. Session-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    /// Each section's own order (status-change recency, picks, captures).
    #[default]
    Recent,
    Project,
    Thread,
    Title,
    Assignee,
}

impl SortBy {
    pub const ALL: [SortBy; 5] = [
        SortBy::Recent,
        SortBy::Project,
        SortBy::Thread,
        SortBy::Title,
        SortBy::Assignee,
    ];

    pub fn word(self) -> &'static str {
        match self {
            SortBy::Recent => "recent",
            SortBy::Project => "project",
            SortBy::Thread => "thread",
            SortBy::Title => "title",
            SortBy::Assignee => "assignee",
        }
    }

    /// The choice a [`Self::word`] names, as `config.toml` spells it.
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sort| sort.word() == word)
    }
}

/// The order of the project lists: the project picker and the Projects view. Session-only,
/// never written to `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectSort {
    /// By project name, case-insensitive; ties by path.
    #[default]
    NameAsc,
    NameDesc,
    /// By the project's earliest task, newest project first.
    Newest,
    Oldest,
}

impl ProjectSort {
    pub const ALL: [ProjectSort; 4] = [
        ProjectSort::NameAsc,
        ProjectSort::NameDesc,
        ProjectSort::Newest,
        ProjectSort::Oldest,
    ];

    /// The picker's and the picker footer's name for the order.
    pub fn label(self) -> &'static str {
        match self {
            ProjectSort::NameAsc => "name A–Z",
            ProjectSort::NameDesc => "name Z–A",
            ProjectSort::Newest => "newest first",
            ProjectSort::Oldest => "oldest first",
        }
    }

    /// The Projects view header's word (`sort: name ▾`).
    pub fn word(self) -> &'static str {
        match self {
            ProjectSort::NameAsc => "name",
            ProjectSort::NameDesc => "name Z–A",
            ProjectSort::Newest => "newest",
            ProjectSort::Oldest => "oldest",
        }
    }

    /// The order `s` in the project picker steps to.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|sort| *sort == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// Put `items` in `sort` order, reading each item's project path with `path_of`. A project's
/// age is its earliest task's creation: tsk keeps no project date. A project with no task
/// yet (the current repository, before its first capture) counts as the newest.
pub fn sort_projects<T: Clone>(
    items: &mut [T],
    path_of: impl Fn(&T) -> String,
    tasks: &[Task],
    sort: ProjectSort,
) {
    let identities = PathIdentityCache::default();
    // Earliest task per stored spelling first, so aliases are matched once per spelling.
    let mut earliest: HashMap<&str, SystemTime> = HashMap::new();
    for task in tasks.iter().filter(|task| !task.soft_deleted) {
        if let TaskScope::Project { path } = &task.scope {
            earliest
                .entry(path.as_str())
                .and_modify(|at| *at = (*at).min(task.created_at))
                .or_insert(task.created_at);
        }
    }
    let mut keyed: Vec<(String, String, Option<SystemTime>, usize)> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let path = path_of(item);
            let first = earliest
                .iter()
                .filter(|(spelling, _)| identities.equivalent(spelling, &path))
                .map(|(_, at)| *at)
                .min();
            (short_project_name(&path).to_lowercase(), path, first, index)
        })
        .collect();
    keyed.sort_by(|a, b| {
        let by_name = || a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1));
        match sort {
            ProjectSort::NameAsc => by_name(),
            ProjectSort::NameDesc => by_name().reverse(),
            // `None` (no task yet) is the newest: it sorts after every time, then reverses.
            ProjectSort::Newest => match (a.2, b.2) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (Some(a), Some(b)) => b.cmp(&a),
            }
            .then_with(by_name),
            ProjectSort::Oldest => match (a.2, b.2) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(&b),
            }
            .then_with(by_name),
        }
    });
    let sorted: Vec<T> = keyed
        .into_iter()
        .map(|(_, _, _, index)| items[index].clone())
        .collect();
    items.clone_from_slice(&sorted);
}

/// One sub-group of a status section: a dim, foldable sub-header over its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueGroup {
    /// Painted name: a project basename, `desk`, `#thread`, `no thread`, `@assignee`, or
    /// `unassigned`.
    pub label: String,
    /// Chrome row id of the sub-header. Always selectable; its rows follow unless folded.
    pub header_id: Uuid,
    pub task_ids: Vec<Uuid>,
    /// Where Enter or a double-click on the sub-header goes.
    pub target: GroupTarget,
}

/// The board a group sub-header opens: its project's board, the desk, or its thread's or
/// assignee's cross-project view. `no thread` and `unassigned` open nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupTarget {
    Project(String),
    Desk,
    Thread(String),
    Assignee(String),
    None,
}

impl GroupTarget {
    fn of(task: &Task, group: GroupBy) -> Self {
        match (group, &task.scope, task.thread.as_deref()) {
            (GroupBy::Project, TaskScope::Project { path }, _) => {
                GroupTarget::Project(path.clone())
            }
            (GroupBy::Project, TaskScope::Global, _) => GroupTarget::Desk,
            (GroupBy::Thread, _, Some(thread)) => GroupTarget::Thread(thread.to_string()),
            (GroupBy::Assignee, _, _) => {
                task.assignee.as_deref().map_or(GroupTarget::None, |name| {
                    GroupTarget::Assignee(name.to_string())
                })
            }
            (GroupBy::Thread, _, None) | (GroupBy::None, _, _) => GroupTarget::None,
        }
    }
}

/// High 64 bits shared by every group sub-header row id (chrome, never a task).
const GROUP_HEADER_ID_PREFIX: u128 = 0xFFFF_FFFF_FFFF_FFFE << 64;

/// Whether `id` is a group sub-header's chrome row id.
pub fn is_group_header_id(id: Uuid) -> bool {
    id.as_u128() >> 64 == GROUP_HEADER_ID_PREFIX >> 64
}

/// Whether `id` is any fold header's chrome row id: a group sub-header, the inbox, or the
/// archived group.
pub fn is_fold_header_id(id: Uuid) -> bool {
    matches!(id, ARCHIVED_HEADER_ROW_ID | INBOX_HEADER_ROW_ID) || is_group_header_id(id)
}

/// Deterministic sub-header id for one group key inside one section kind, so fold state
/// survives repaints and snapshot merges while the group exists.
fn group_header_id(kind: SectionKind, key: &str) -> Uuid {
    // FNV-1a over the section tag and key: stable across runs and platforms.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let tag = format!("{kind:?}\u{0}{key}");
    for byte in tag.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    Uuid::from_u128(GROUP_HEADER_ID_PREFIX | u128::from(hash))
}

/// Normalized content-search terms. Every term must match, but each may match a
/// different task field.
pub fn search_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether a task matches every content-search term in its title, notes, steps,
/// assignee (with or without its `@`), or painted task number. Search is
/// Unicode-case-insensitive substring.
pub fn task_matches(task: &Task, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut searchable = task.title.to_lowercase();
    if let Some(notes) = task.notes.as_deref() {
        searchable.push('\n');
        searchable.push_str(&notes.to_lowercase());
    }
    for step in &task.steps {
        searchable.push('\n');
        searchable.push_str(&step.text.to_lowercase());
    }
    // `@claude` and `claude` both find the assignee, as the row's `@name` reads.
    if let Some(assignee) = task.assignee.as_deref() {
        searchable.push_str("\n@");
        searchable.push_str(&assignee.to_lowercase());
    }
    if let Some(identifier) = task.board_identifier() {
        searchable.push('\n');
        searchable.push_str(&identifier.to_lowercase());
    }
    words.iter().all(|word| searchable.contains(word))
}

/// How the board query is scoped for one paint/selection pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardLens<'a> {
    /// Every live task of every scope, desk included.
    All,
    Desk,
    Project(&'a Path),
    /// Read-only focus on an archived project (AC-41): the same shape as `Project`, but
    /// the project's archived state does not hide its tasks.
    ArchivedProject(&'a Path),
}

/// One ordered section of the queue board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSection {
    pub kind: SectionKind,
    /// Project path for a project-group section. `None` for every status section on
    /// the desk, project board, and drawer.
    pub project_label: Option<String>,
    /// Exact flattening for task-only consumers and selection.
    pub task_ids: Vec<Uuid>,
    pub count: usize,
    /// True when a section has zero tasks (renderer paints the empty hint).
    pub empty_hint: bool,
    /// Sub-groups in paint order while the board is grouped; empty when ungrouped.
    /// `task_ids` stays their exact concatenation.
    pub groups: Vec<QueueGroup>,
}

/// Status-line counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusCounts {
    pub in_motion: usize,
    pub done: usize,
    pub need: usize,
}

/// Ordered sections plus status-line counts for one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueView {
    pub sections: Vec<QueueSection>,
    pub counts: StatusCounts,
    /// The dimension [`arrange_view`] grouped by. Only sections with `groups` are grouped.
    pub group: GroupBy,
}

/// Derive queue sections for the active board lens in the default number order.
pub fn query_lens(tasks: &[Task], lens: BoardLens<'_>, drawer_open: bool) -> QueueView {
    query_board(
        tasks,
        &BTreeSet::new(),
        lens,
        drawer_open,
        BoardSort::Number,
    )
}

/// Derive queue sections with an archived-project set. A task is hidden when it is
/// soft-deleted, archived, or belongs to an archived project: no working lens paints it.
pub fn query_board(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    lens: BoardLens<'_>,
    drawer_open: bool,
    sort: BoardSort,
) -> QueueView {
    query_board_search(tasks, archived_projects, lens, drawer_open, sort, "")
}

/// Derive queue sections with the same lens rules as [`query_board`], then narrow
/// every task pool by the content query and order every section by `sort`.
pub fn query_board_search(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    lens: BoardLens<'_>,
    drawer_open: bool,
    sort: BoardSort,
    query: &str,
) -> QueueView {
    let identities = PathIdentityCache::default();
    let words = search_words(query);
    let mut view = match lens {
        BoardLens::All => query_all(tasks, archived_projects, drawer_open, &identities, &words),
        BoardLens::Desk => query_desk(
            tasks,
            archived_projects,
            drawer_open,
            &DeskFilter::default(),
            &identities,
            &words,
        ),
        BoardLens::Project(path) => query_project_focus(
            tasks,
            archived_projects,
            path,
            drawer_open,
            &identities,
            &words,
        ),
        // AC-41/AC-45: the read-only focus is the only lens that paints an archived
        // project's tasks. It is the project focus computed as if the project were live.
        BoardLens::ArchivedProject(path) => query_project_focus(
            tasks,
            &BTreeSet::new(),
            path,
            drawer_open,
            &identities,
            &words,
        ),
    };
    if !words.is_empty() {
        view.sections.retain(|section| !section.empty_hint);
    }
    order_sections(&mut view, tasks, sort);
    view
}

/// Re-order every section's rows by the board order. Section membership is untouched.
fn order_sections(view: &mut QueueView, tasks: &[Task], sort: BoardSort) {
    let by_id: std::collections::HashMap<Uuid, &Task> =
        tasks.iter().map(|task| (task.id, task)).collect();
    for section in &mut view.sections {
        section
            .task_ids
            .sort_by(|a, b| match (by_id.get(a), by_id.get(b)) {
                (Some(a), Some(b)) => cmp_board_order(a, b, sort),
                _ => a.cmp(b),
            });
    }
}

/// The key the project order groups by: the desk first, then the stored project path.
pub fn board_group_key(task: &Task) -> (u8, &str) {
    match &task.scope {
        TaskScope::Global => (0, ""),
        TaskScope::Project { path } => (1, path.as_str()),
    }
}

/// The board's row order inside one section. Notices lead; numbered tasks follow by
/// ascending number; a task the persistence boundary has not numbered yet sorts last by
/// capture time. `BoardSort::Project` groups by [`board_group_key`] first.
pub fn cmp_board_order(a: &Task, b: &Task, sort: BoardSort) -> Ordering {
    let group = match sort {
        BoardSort::Number => Ordering::Equal,
        BoardSort::Project => board_group_key(a).cmp(&board_group_key(b)),
    };
    let notice_number = |task: &Task| task.notice.as_ref().and_then(|notice| notice.number);
    group
        .then_with(|| b.is_notice().cmp(&a.is_notice()))
        .then_with(|| notice_number(a).is_none().cmp(&notice_number(b).is_none()))
        .then_with(|| notice_number(a).cmp(&notice_number(b)))
        .then_with(|| a.number.is_none().cmp(&b.number.is_none()))
        .then_with(|| a.number.cmp(&b.number))
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.id.cmp(&b.id))
}

/// A task survives the working-lens filter: not soft-deleted, not archived, and no
/// archived project owns its scope.
fn is_live(
    task: &Task,
    archived_projects: &BTreeSet<String>,
    identities: &PathIdentityCache,
) -> bool {
    if task.soft_deleted || task.archived {
        return false;
    }
    match &task.scope {
        TaskScope::Global => true,
        TaskScope::Project { path } => !identities.contains(archived_projects, path),
    }
}

/// Whether an archived project owns the task's scope (its own flag is irrelevant here).
fn task_owned_by_archived_project(
    task: &Task,
    archived_projects: &BTreeSet<String>,
    identities: &PathIdentityCache,
) -> bool {
    match &task.scope {
        TaskScope::Global => false,
        TaskScope::Project { path } => identities.contains(archived_projects, path),
    }
}

/// Task ids visible for selection, honoring inbox and archived group collapse.
pub fn visible_task_ids(
    view: &QueueView,
    archived_collapsed: bool,
    inbox_collapsed: bool,
) -> Vec<Uuid> {
    visible_row_ids(view, archived_collapsed, inbox_collapsed, &BTreeSet::new())
}

/// Selectable row ids: [`visible_task_ids`] plus every group sub-header. Each sub-header
/// is a stop of its own (so ←/→ can fold it and Enter can open it); a folded group
/// contributes only its sub-header.
pub fn visible_row_ids(
    view: &QueueView,
    archived_collapsed: bool,
    inbox_collapsed: bool,
    folded: &BTreeSet<Uuid>,
) -> Vec<Uuid> {
    let mut out = Vec::new();
    for section in &view.sections {
        match section.kind {
            SectionKind::Archived => {
                // The archived header is always selectable so ←/→ can fold it;
                // its rows paint only when the group is expanded.
                out.push(ARCHIVED_HEADER_ROW_ID);
                if !archived_collapsed {
                    out.extend(section.task_ids.iter().copied());
                }
            }
            SectionKind::Inbox => {
                out.push(INBOX_HEADER_ROW_ID);
                if inbox_collapsed {
                    continue;
                }
                if section.groups.is_empty() {
                    out.extend(section.task_ids.iter().copied());
                } else {
                    push_group_rows(&mut out, section, folded);
                }
            }
            _ if !section.groups.is_empty() => push_group_rows(&mut out, section, folded),
            _ => out.extend(section.task_ids.iter().copied()),
        }
    }
    out
}

/// Every live scope at once: NEEDS YOU, IN MOTION and ON DECK across the desk and every
/// live project, plus the drawer. Desk tasks keep their desk ownership.
fn query_all(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    drawer_open: bool,
    identities: &PathIdentityCache,
    search: &[String],
) -> QueueView {
    let live: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            is_live(task, archived_projects, identities)
                && (task_matches(task, search)
                    || (!drawer_open && task.status == HumanStatus::Done))
        })
        .collect();
    let archived_pool: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            !task_owned_by_archived_project(task, archived_projects, identities)
                && (!drawer_open || task_matches(task, search))
        })
        .collect();
    let need: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_needs_you(t, tasks))
        .collect();
    let motion: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_in_motion(t, tasks))
        .collect();
    let deck: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_on_deck_status(t.status))
        .collect();

    let mut sections = Vec::new();
    push_needs_you(&mut sections, &need);
    if !motion.is_empty() {
        sections.push(section_from(SectionKind::InMotion, None, &motion));
    }
    push_deck(&mut sections, None, &deck, &need);
    append_done(&mut sections, &live, drawer_open, search);
    append_archived(&mut sections, &archived_pool, drawer_open);

    QueueView {
        sections,
        counts: status_counts(&live, tasks),
        group: GroupBy::None,
    }
}

/// A grouped section's selectable rows: each group's header, then its tasks unless folded.
fn push_group_rows(out: &mut Vec<Uuid>, section: &QueueSection, folded: &BTreeSet<Uuid>) {
    for group in &section.groups {
        out.push(group.header_id);
        if !folded.contains(&group.header_id) {
            out.extend(group.task_ids.iter().copied());
        }
    }
}

fn query_desk(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    drawer_open: bool,
    filter: &DeskFilter,
    identities: &PathIdentityCache,
    search: &[String],
) -> QueueView {
    let live: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            is_live(task, archived_projects, identities)
                && filter.admits(task, identities)
                && (task_matches(task, search)
                    || (!drawer_open && task.status == HumanStatus::Done))
        })
        .collect();
    let archived_pool: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            !task_owned_by_archived_project(task, archived_projects, identities)
                && filter.admits(task, identities)
                && (!drawer_open || task_matches(task, search))
        })
        .collect();

    // NEEDS YOU is the global attention lane: blocked and review across every live
    // scope, desk included. Wide rows label each task's project and thread.
    let mut need: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_needs_you(t, tasks))
        .collect();
    sort_by_status_change_desc(&mut need);

    let mut motion: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_in_motion(t, tasks))
        .collect();
    sort_by_status_change_desc(&mut motion);

    // ON DECK · desk is the personal backlog: desk-scope ready and open tasks only.
    // Project backlogs live on their project boards, never here.
    let deck: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_on_deck_status(t.status) && matches!(t.scope, TaskScope::Global))
        .collect();

    let mut sections = Vec::new();
    push_needs_you(&mut sections, &need);
    if !motion.is_empty() {
        sections.push(section_from(SectionKind::InMotion, None, &motion));
    }
    push_deck(&mut sections, None, &deck, &need);

    append_done(&mut sections, &live, drawer_open, search);
    append_archived(&mut sections, &archived_pool, drawer_open);

    QueueView {
        sections,
        counts: status_counts(&live, tasks),
        group: GroupBy::None,
    }
}

fn query_project_focus(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    path: &Path,
    drawer_open: bool,
    identities: &PathIdentityCache,
    search: &[String],
) -> QueueView {
    let live: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            is_live(task, archived_projects, identities)
                && task_matches_scope(task, path, identities)
                && (task_matches(task, search)
                    || (!drawer_open && task.status == HumanStatus::Done))
        })
        .collect();

    let mut motion: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_in_motion(t, tasks))
        .collect();
    sort_by_status_change_desc(&mut motion);

    // `pending` carries both NEEDS YOU and ON DECK rows, unsorted: NEEDS YOU is
    // re-sorted to status-change recency after the split and `push_deck` orders
    // its own ready and inbox halves.
    let pending: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| !is_in_motion(t, tasks) && t.status != HumanStatus::Done)
        .collect();
    let (mut need, ready) = split_needs_you(&pending, tasks);
    sort_by_status_change_desc(&mut need);

    let label = live
        .iter()
        .find_map(|task| match &task.scope {
            TaskScope::Project { path: stored }
                if identities.equivalent(stored, &path.to_string_lossy()) =>
            {
                Some(stored.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned());

    let mut sections = Vec::new();
    push_needs_you(&mut sections, &need);
    if !motion.is_empty() {
        sections.push(section_from(SectionKind::InMotion, None, &motion));
    }
    push_deck(&mut sections, Some(label.clone()), &ready, &need);

    append_done(&mut sections, &live, drawer_open, search);
    let in_scope: Vec<&Task> = tasks
        .iter()
        .filter(|task| task_matches_scope(task, path, identities))
        .filter(|task| !task_owned_by_archived_project(task, archived_projects, identities))
        .filter(|task| {
            let belongs_to_closed_drawer = task.status == HumanStatus::Done || task.archived;
            task_matches(task, search) || (!drawer_open && belongs_to_closed_drawer)
        })
        .collect();
    append_archived(&mut sections, &in_scope, drawer_open);

    // An empty board (or a search that hides everything) keeps one hinted section, so
    // the project board always answers with an add affordance instead of a bare pane.
    if !sections.iter().any(|section| {
        !matches!(
            section.kind,
            SectionKind::Done | SectionKind::Archived | SectionKind::Inbox
        )
    }) {
        sections.push(QueueSection {
            kind: SectionKind::OnDeck,
            project_label: Some(label),
            task_ids: Vec::new(),
            count: 0,
            empty_hint: true,
            groups: Vec::new(),
        });
    }

    QueueView {
        sections,
        counts: status_counts(&live, tasks),
        group: GroupBy::None,
    }
}

/// DONE paints its rows only while the drawer is open. Closed, it keeps its heading and
/// the count of done tasks the lens and search would show, with no rows: the collapsed
/// section has no `task_ids`, so selection, marks, and `J`/`K` never reach a done task.
fn append_done(
    sections: &mut Vec<QueueSection>,
    live: &[&Task],
    drawer_open: bool,
    search: &[String],
) {
    let mut done: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| t.status == HumanStatus::Done && task_matches(t, search))
        .collect();
    if done.is_empty() {
        return;
    }
    if !drawer_open {
        sections.push(QueueSection {
            kind: SectionKind::Done,
            project_label: None,
            task_ids: Vec::new(),
            count: done.len(),
            empty_hint: false,
            groups: Vec::new(),
        });
        return;
    }
    sort_by_status_change_desc(&mut done);
    sections.push(section_from(SectionKind::Done, None, &done));
}

/// The done drawer's archived group: individually archived tasks the drawer's scope
/// would otherwise show (not soft-deleted, not owned by an archived project), newest
/// status change first. Paints only while the drawer is open, and only when non-empty.
fn append_archived(sections: &mut Vec<QueueSection>, in_scope: &[&Task], drawer_open: bool) {
    if !drawer_open {
        return;
    }
    let mut archived: Vec<&Task> = in_scope
        .iter()
        .copied()
        .filter(|task| task.archived && !task.soft_deleted)
        .collect();
    sort_by_status_change_desc(&mut archived);
    if !archived.is_empty() {
        sections.push(section_from(SectionKind::Archived, None, &archived));
    }
}

fn status_counts(live: &[&Task], tasks: &[Task]) -> StatusCounts {
    let in_motion = live.iter().filter(|t| is_in_motion(t, tasks)).count();
    let done = live
        .iter()
        .filter(|t| t.status == HumanStatus::Done)
        .count();
    let need = live.iter().filter(|t| is_needs_you(t, tasks)).count();
    StatusCounts {
        in_motion,
        done,
        need,
    }
}

/// What a blocked task waits on, as the board reads it. A block on another task that is done
/// (or gone) is waiting on you again: the owner decides whether to unblock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockWait {
    /// Blocked on you, or blocked with no recorded block.
    You,
    /// Blocked on a task that is not done yet, or on something else.
    Elsewhere,
    /// Blocked on a task that is now done.
    BlockerDone(u64),
    /// Blocked on a task that is no longer on the board.
    BlockerGone(u64),
}

/// Read a task's block or review round against the whole store. `None` unless the task is
/// blocked or in review.
pub fn block_wait(task: &Task, tasks: &[Task]) -> Option<BlockWait> {
    if !matches!(task.status, HumanStatus::Blocked | HumanStatus::Review) {
        return None;
    }
    Some(match task.block.as_ref().map(|block| &block.on) {
        None | Some(BlockOn::You) => BlockWait::You,
        Some(BlockOn::Agent(_) | BlockOn::Other(_)) => BlockWait::Elsewhere,
        Some(BlockOn::Task(number)) => match tasks
            .iter()
            .find(|other| other.number == Some(*number) && !other.is_notice())
        {
            Some(other) if other.soft_deleted => BlockWait::BlockerGone(*number),
            Some(other) if other.status == HumanStatus::Done => BlockWait::BlockerDone(*number),
            Some(_) => BlockWait::Elsewhere,
            None => BlockWait::BlockerGone(*number),
        },
    })
}

/// NEEDS YOU: blocked or in review, unless the record waits elsewhere.
pub fn is_needs_you(task: &Task, tasks: &[Task]) -> bool {
    match task.status {
        HumanStatus::Review | HumanStatus::Blocked => {
            block_wait(task, tasks) != Some(BlockWait::Elsewhere)
        }
        _ => false,
    }
}

/// IN MOTION: started, and blocked or in review on another task, an agent or something else.
pub fn is_in_motion(task: &Task, tasks: &[Task]) -> bool {
    task.status == HumanStatus::Started || block_wait(task, tasks) == Some(BlockWait::Elsewhere)
}

fn is_on_deck_status(status: HumanStatus) -> bool {
    matches!(status, HumanStatus::Open | HumanStatus::Ready)
}

fn split_needs_you<'a>(tasks: &[&'a Task], all: &[Task]) -> (Vec<&'a Task>, Vec<&'a Task>) {
    let mut need = Vec::new();
    let mut rest = Vec::new();
    for task in tasks {
        if is_needs_you(task, all) {
            need.push(*task);
        } else {
            rest.push(*task);
        }
    }
    (need, rest)
}

fn push_needs_you(sections: &mut Vec<QueueSection>, need: &[&Task]) {
    if !need.is_empty() {
        sections.push(section_from(SectionKind::NeedsYou, None, need));
    }
}

/// Keep an empty desk/ON DECK header only when NEEDS YOU is also empty.
/// Ready rows sit on ON DECK; open rows follow under a collapsible inbox heading.
fn push_deck(
    sections: &mut Vec<QueueSection>,
    project_label: Option<String>,
    deck: &[&Task],
    need: &[&Task],
) {
    let mut ready: Vec<&Task> = deck
        .iter()
        .copied()
        .filter(|task| is_on_deck_status(task.status))
        .collect();
    sort_by_created_asc(&mut ready);
    if ready.is_empty()
        && (!need.is_empty()
            || sections
                .iter()
                .any(|section| section.kind == SectionKind::InMotion && section.count > 0))
    {
        return;
    }
    let count = ready.len();
    sections.push(QueueSection {
        kind: SectionKind::OnDeck,
        project_label,
        task_ids: ready.iter().map(|task| task.id).collect(),
        count,
        empty_hint: count == 0,
        groups: Vec::new(),
    });
}

/// Whether a task belongs to the project at `path`, tolerating the same directory
/// reached through different path spellings (symlinks, `/tmp` vs `/private/tmp`).
fn task_matches_scope(task: &Task, path: &Path, identities: &PathIdentityCache) -> bool {
    match &task.scope {
        TaskScope::Project { path: stored } => {
            identities.equivalent(stored, &path.to_string_lossy())
        }
        TaskScope::Global => false,
    }
}

/// Section ordering rule. NEEDS YOU, IN MOTION, DONE and ARCHIVED put the most
/// recent status change first (`status_changed_at`: the last `StatusSet`,
/// `Completed` or `Reopened` event, falling back to `created_at`). Ready rows on
/// ON DECK are oldest pick first (`status_changed_at` ascending). Inbox (`open`)
/// stays FIFO by `created_at`. Plain edits, step changes, archive and restore
/// never reorder a section. Ties break by `created_at` then `id` so the order is
/// total and stable across reloads.
fn sort_by_status_change_desc(tasks: &mut [&Task]) {
    tasks.sort_by(|a, b| cmp_status_change_desc(a, b));
}

fn cmp_status_change_desc(a: &Task, b: &Task) -> Ordering {
    b.status_changed_at()
        .cmp(&a.status_changed_at())
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.id.cmp(&b.id))
}

/// Backlogs are FIFO: oldest capture first, ties by id. Notice rows (the starter tour,
/// release notes) lead regardless: they are seeded once and would otherwise sink under an
/// existing user's backlog, unseen.
fn sort_by_created_asc(tasks: &mut [&Task]) {
    tasks.sort_by(|a, b| cmp_created_asc(a, b));
}

fn cmp_created_asc(a: &Task, b: &Task) -> Ordering {
    b.is_notice()
        .cmp(&a.is_notice())
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.id.cmp(&b.id))
}

/// The board's order between two tasks of the same status, for surfaces outside the
/// board (`tsk list`) that group by status: ready by oldest pick, open by oldest
/// capture, every other status by most recent status change.
pub fn cmp_within_status(a: &Task, b: &Task) -> Ordering {
    match a.status {
        HumanStatus::Ready => b
            .is_notice()
            .cmp(&a.is_notice())
            .then_with(|| a.status_changed_at().cmp(&b.status_changed_at()))
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id)),
        HumanStatus::Open => cmp_created_asc(a, b),
        _ => cmp_status_change_desc(a, b),
    }
}

fn section_from(kind: SectionKind, project_label: Option<String>, tasks: &[&Task]) -> QueueSection {
    let task_ids: Vec<Uuid> = tasks.iter().map(|t| t.id).collect();
    let count = task_ids.len();
    QueueSection {
        kind,
        project_label,
        task_ids,
        count,
        empty_hint: false,
        groups: Vec::new(),
    }
}

/// Group and sort the live status sections in place. NEEDS YOU, IN MOTION, ON DECK and
/// the inbox group (inbox sub-groups nest under its own fold); the done drawer and
/// archived group keep their own order. Project grouping is a no-op on the desk's ON DECK
/// and inbox, which only ever hold desk tasks. Groups paint hottest first: the group whose
/// newest row changed status most recently leads, ties by label. Sorting is stable over
/// each section's own order, so ties keep recent order; notice rows keep leading.
pub fn arrange_view(
    view: &mut QueueView,
    tasks: &[Task],
    lens: BoardLens<'_>,
    group: GroupBy,
    sort: SortBy,
) {
    view.group = group;
    if group == GroupBy::None && sort == SortBy::Recent {
        return;
    }
    let identities = PathIdentityCache::default();
    let by_id: HashMap<Uuid, &Task> = tasks.iter().map(|task| (task.id, task)).collect();
    for section in &mut view.sections {
        let groupable = matches!(
            section.kind,
            SectionKind::NeedsYou
                | SectionKind::InMotion
                | SectionKind::OnDeck
                | SectionKind::Inbox
        );
        if !groupable || section.task_ids.is_empty() {
            continue;
        }
        let mut rows: Vec<&Task> = section
            .task_ids
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .collect();
        sort_rows(&mut rows, sort);
        // The desk's ON DECK and inbox hold desk tasks only: one project, nothing to group.
        let desk_deck = matches!(section.kind, SectionKind::OnDeck | SectionKind::Inbox)
            && matches!(lens, BoardLens::Desk);
        let group = if desk_deck && group == GroupBy::Project {
            GroupBy::None
        } else {
            group
        };
        if group == GroupBy::None {
            section.task_ids = rows.iter().map(|task| task.id).collect();
            continue;
        }
        let buckets = bucket_rows(rows, group, &identities);
        section.groups = buckets
            .into_iter()
            .map(|(key, label, rows)| QueueGroup {
                label,
                header_id: group_header_id(section.kind, &key),
                target: rows
                    .first()
                    .map(|task| GroupTarget::of(task, group))
                    .unwrap_or(GroupTarget::None),
                task_ids: rows.iter().map(|task| task.id).collect(),
            })
            .collect();
        section.task_ids = section
            .groups
            .iter()
            .flat_map(|group| group.task_ids.iter().copied())
            .collect();
    }
}

/// Split already-sorted rows into one-level groups, hottest first: the group whose newest
/// row changed status most recently leads, ties by label. Rows keep their order inside a
/// group. Returns `(key, label, rows)`; the board and `tsk list --group` share it.
fn bucket_rows<'a>(
    rows: Vec<&'a Task>,
    group: GroupBy,
    identities: &PathIdentityCache,
) -> Vec<(String, String, Vec<&'a Task>)> {
    let mut buckets: Vec<(String, String, Vec<&Task>)> = Vec::new();
    for task in rows {
        let (key, label) = group_key(task, group);
        // One project, one group: path spellings of the same repository (a trailing
        // slash, `/tmp` vs `/private/tmp`, a symlink) share a bucket, keyed by the
        // smallest spelling so the fold id survives re-sorting.
        let same = |other: &str| match (key.strip_prefix("p:"), other.strip_prefix("p:")) {
            (Some(a), Some(b)) => identities.equivalent(a, b),
            _ => other == key,
        };
        match buckets.iter_mut().find(|bucket| same(&bucket.0)) {
            Some(bucket) => {
                if key < bucket.0 {
                    bucket.0 = key;
                    bucket.1 = label;
                }
                bucket.2.push(task);
            }
            None => buckets.push((key, label, vec![task])),
        }
    }
    let hottest = |rows: &[&Task]| rows.iter().map(|task| task.status_changed_at()).max();
    buckets.sort_by(|a, b| {
        hottest(&b.2)
            .cmp(&hottest(&a.2))
            .then_with(|| a.1.cmp(&b.1))
    });
    buckets
}

/// Sort and group one status's rows the way the board arranges a section, for surfaces
/// outside the board (`tsk list --group/--sort`). Rows arrive in [`cmp_within_status`]
/// order; ungrouped output is one unlabeled group.
pub fn arrange_rows(
    mut rows: Vec<&Task>,
    group: GroupBy,
    sort: SortBy,
) -> Vec<(Option<String>, Vec<&Task>)> {
    sort_rows(&mut rows, sort);
    if group == GroupBy::None {
        return vec![(None, rows)];
    }
    bucket_rows(rows, group, &PathIdentityCache::default())
        .into_iter()
        .map(|(_, label, rows)| (Some(label), rows))
        .collect()
}

/// A task's group identity and painted label for one grouping dimension.
fn group_key(task: &Task, group: GroupBy) -> (String, String) {
    match (group, &task.scope, task.thread.as_deref()) {
        (GroupBy::Project, TaskScope::Project { path }, _) => (
            format!(
                "p:{}",
                path.trim_end_matches(
                    |character| character == '/' || (cfg!(windows) && character == '\\')
                )
            ),
            short_project_name(path).to_string(),
        ),
        (GroupBy::Project, TaskScope::Global, _) => ("desk".to_string(), "desk".to_string()),
        (GroupBy::Thread, _, Some(thread)) => {
            let thread = thread.to_ascii_lowercase();
            (format!("t:{thread}"), format!("#{thread}"))
        }
        (GroupBy::Assignee, _, _) => match task.assignee.as_deref() {
            Some(name) => {
                let name = name.to_ascii_lowercase();
                let label = crate::ui::render::assignee_handle(&name);
                (format!("a:{name}"), label)
            }
            None => ("unassigned".to_string(), "unassigned".to_string()),
        },
        (GroupBy::Thread, _, None) | (GroupBy::None, _, _) => {
            ("none".to_string(), "no thread".to_string())
        }
    }
}

/// Stable in-place sort by the chosen key. Desk tasks sort after projects, unthreaded
/// tasks after threads, and unassigned tasks after assigned ones, with yours first; notice
/// rows lead regardless.
fn sort_rows(rows: &mut [&Task], sort: SortBy) {
    let key = |task: &Task| -> (bool, String) {
        match sort {
            SortBy::Recent => (false, String::new()),
            SortBy::Project => match &task.scope {
                TaskScope::Project { path } => (false, short_project_name(path).to_lowercase()),
                TaskScope::Global => (true, String::new()),
            },
            SortBy::Thread => match task.thread.as_deref() {
                Some(thread) => (false, thread.to_lowercase()),
                None => (true, String::new()),
            },
            SortBy::Title => (false, task.title.to_lowercase()),
            SortBy::Assignee => match task.assignee.as_deref() {
                // Yours sort first: the empty key precedes every name.
                Some(crate::domain::OWNER) => (false, String::new()),
                Some(assignee) => (false, assignee.to_lowercase()),
                None => (true, String::new()),
            },
        }
    };
    if sort == SortBy::Recent {
        return;
    }
    rows.sort_by(|a, b| {
        b.is_notice()
            .cmp(&a.is_notice())
            .then_with(|| key(a).cmp(&key(b)))
    });
}

/// Basename of a project path, for labels and search.
pub fn short_project_name(path: &str) -> &str {
    path.trim_end_matches(|character| character == '/' || (cfg!(windows) && character == '\\'))
        .rsplit(|character| character == '/' || (cfg!(windows) && character == '\\'))
        .find(|component| !component.is_empty())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    use crate::domain::{HumanStatus, ProvenanceOrigin, Step, TaskEvent, TaskEventKind, TaskScope};

    fn task(
        id: u128,
        status: HumanStatus,
        scope: TaskScope,
        soft_deleted: bool,
        updated_secs: u64,
    ) -> Task {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(updated_secs);
        Task {
            id: Uuid::from_u128(id),
            number: None,
            parent_task_id: None,
            after: Vec::new(),
            notice: None,
            revision: Uuid::from_u128(id),
            merge_base_revision: None,
            title: format!("task-{id}"),
            notes: None,
            thread: None,
            assignee: None,
            assignee_kind: Default::default(),
            base: None,
            dispatch: None,
            status,
            block: None,
            past_blocks: Vec::new(),
            scope,
            provenance: ProvenanceOrigin::Manual,
            history: vec![TaskEvent::new(TaskEventKind::Created, at)],
            steps: Vec::new(),
            soft_deleted,
            archived: false,
            created_at: at,
            updated_at: at,
        }
    }

    fn project(path: &str) -> TaskScope {
        TaskScope::Project {
            path: path.to_string(),
        }
    }

    /// Appends a status-change event at `at_secs`, the only history kind that may
    /// reorder a section.
    fn with_status_event(task: Task, kind: TaskEventKind, at_secs: u64) -> Task {
        let mut task = task;
        task.history.push(TaskEvent::new(
            kind,
            SystemTime::UNIX_EPOCH + Duration::from_secs(at_secs),
        ));
        task
    }

    /// Simulates `record_mutation` for a non-status mutation (edit, step tick):
    /// `updated_at` moves and a non-status event lands, but the task's
    /// status-change time does not.
    fn mutated(task: Task, kind: TaskEventKind, at_secs: u64) -> Task {
        let mut task = task;
        task.updated_at = SystemTime::UNIX_EPOCH + Duration::from_secs(at_secs);
        task.history.push(TaskEvent::new(kind, task.updated_at));
        task
    }

    fn ids(section: &QueueSection) -> Vec<Uuid> {
        section.task_ids.clone()
    }

    #[cfg(windows)]
    #[test]
    fn project_name_accepts_windows_separators() {
        assert_eq!(short_project_name(r"C:\work\tsk"), "tsk");
        assert_eq!(short_project_name(r"C:\work\tsk\"), "tsk");
        assert_eq!(short_project_name("/work/tsk/"), "tsk");
    }

    #[cfg(unix)]
    #[test]
    fn project_name_keeps_a_literal_unix_backslash() {
        assert_eq!(short_project_name(r"/work/tsk\name"), r"tsk\name");
    }

    #[test]
    fn task_search_requires_every_word_across_content_fields_and_number() {
        let mut candidate = task(52, HumanStatus::Doing, project("/repos/app"), false, 10);
        candidate.title = "Ship Login flow".into();
        candidate.notes = Some("Handle OAuth callback".into());
        candidate.steps = vec![Step {
            id: Uuid::from_u128(99),
            text: "Rotate TOKEN".into(),
            done: false,
        }];
        candidate.number = Some(52);

        let words = search_words("LOGIN callback token t52");
        assert!(task_matches(&candidate, &words));
        assert!(!task_matches(&candidate, &search_words("login billing")));
    }

    #[test]
    fn task_search_prunes_empty_sections_and_recounts_hidden_done_work() {
        let mut tasks = vec![
            task(1, HumanStatus::Doing, TaskScope::Global, false, 40),
            task(2, HumanStatus::Doing, TaskScope::Global, false, 30),
            task(3, HumanStatus::Done, TaskScope::Global, false, 20),
            task(4, HumanStatus::Done, TaskScope::Global, false, 10),
        ];
        tasks[0].title = "login motion".into();
        tasks[1].title = "unrelated motion".into();
        tasks[2].title = "login completed".into();
        tasks[3].title = "unrelated completed".into();

        let closed = query_board_search(
            &tasks,
            &BTreeSet::new(),
            BoardLens::Desk,
            false,
            BoardSort::Number,
            "login",
        );
        assert_eq!(
            section_ids(&closed, SectionKind::InMotion),
            vec![Uuid::from_u128(1)]
        );
        assert_eq!(closed.counts.in_motion, 1);
        assert_eq!(closed.counts.done, 2, "closed drawer is not searched");
        let heading: Vec<_> = closed
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Done)
            .map(|s| (s.count, s.task_ids.len()))
            .collect();
        assert_eq!(heading, vec![(1, 0)], "the closed heading counts matches");
        assert!(
            closed.sections.iter().all(|section| !section.empty_hint),
            "content search hides empty section hints"
        );

        let open = query_board_search(
            &tasks,
            &BTreeSet::new(),
            BoardLens::Desk,
            true,
            BoardSort::Number,
            "login",
        );
        assert_eq!(
            section_ids(&open, SectionKind::Done),
            vec![Uuid::from_u128(3)]
        );
    }

    fn section_ids(view: &QueueView, kind: SectionKind) -> Vec<Uuid> {
        view.sections
            .iter()
            .filter(|s| s.kind == kind)
            .flat_map(|s| s.task_ids.iter().copied())
            .collect()
    }

    fn all_listed_ids(view: &QueueView) -> Vec<Uuid> {
        view.sections
            .iter()
            .flat_map(|s| s.task_ids.iter().copied())
            .collect()
    }

    #[test]
    fn archived_group_follows_the_drawer_scope() {
        let mut gone = BTreeSet::new();
        gone.insert("/repos/gone".to_string());
        let mut tasks = vec![
            task(1, HumanStatus::Done, project("/repos/a"), false, 10),
            task(2, HumanStatus::Done, TaskScope::Global, false, 20),
            task(3, HumanStatus::Done, project("/repos/gone"), false, 30),
        ];
        for task in &mut tasks {
            task.archived = true;
        }

        // Desk drawer: every scope's archived tasks, newest status change first. A task whose
        // project is archived stays hidden (task 3).
        let home = query_board(&tasks, &gone, BoardLens::Desk, true, BoardSort::Number);
        let group = home
            .sections
            .iter()
            .find(|s| s.kind == SectionKind::Archived)
            .expect("archived group at desk");
        assert_eq!(
            ids(group),
            vec![Uuid::from_u128(1), Uuid::from_u128(2)],
            "board order, archived project's task excluded"
        );
        assert_eq!(group.count, 2);

        // Project focus: only that project's archived tasks.
        let focus = query_board(
            &tasks,
            &gone,
            BoardLens::Project(Path::new("/repos/a")),
            true,
            BoardSort::Number,
        );
        let group = focus
            .sections
            .iter()
            .find(|s| s.kind == SectionKind::Archived)
            .expect("archived group in project focus");
        assert_eq!(ids(group), vec![Uuid::from_u128(1)]);
    }

    #[test]
    fn archived_project_hides_its_tasks_from_every_working_lens() {
        let mut archived = BTreeSet::new();
        archived.insert("/repos/gone".to_string());
        let tasks = vec![
            task(1, HumanStatus::Doing, project("/repos/gone"), false, 50),
            task(2, HumanStatus::Todo, project("/repos/gone"), false, 40),
            task(3, HumanStatus::Todo, project("/repos/gone"), false, 30),
            task(4, HumanStatus::Doing, TaskScope::Global, false, 20),
            task(5, HumanStatus::Todo, project("/repos/here"), false, 10),
        ];

        // Desk IN MOTION: the archived project's started task is gone, the global one stays.
        let desk = query_board(&tasks, &archived, BoardLens::Desk, false, BoardSort::Number);
        let motion = section_ids(&desk, SectionKind::InMotion);
        assert!(!motion.contains(&Uuid::from_u128(1)));
        assert!(motion.contains(&Uuid::from_u128(4)));

        // Every scope: the archived project contributes nothing, the live project stays.
        let all = query_board(&tasks, &archived, BoardLens::All, false, BoardSort::Number);
        let listed = all_listed_ids(&all);
        assert!(!listed.contains(&Uuid::from_u128(1)));
        assert!(!listed.contains(&Uuid::from_u128(3)));
        assert!(listed.contains(&Uuid::from_u128(5)));
    }

    #[test]
    fn desk_in_motion_is_global_started_in_board_order() {
        // Status-change order (60, 100, 80) never reorders: unnumbered rows follow capture
        // time (10, 50, 90).
        let tasks = vec![
            with_status_event(
                task(1, HumanStatus::Doing, TaskScope::Global, false, 90),
                TaskEventKind::StatusSet,
                60,
            ),
            with_status_event(
                task(2, HumanStatus::Doing, project("/repos/a"), false, 10),
                TaskEventKind::StatusSet,
                100,
            ),
            task(3, HumanStatus::Doing, TaskScope::Global, true, 40),
            task(4, HumanStatus::Todo, TaskScope::Global, false, 50),
            with_status_event(
                task(5, HumanStatus::Doing, project("/repos/b"), false, 50),
                TaskEventKind::StatusSet,
                80,
            ),
        ];

        let view = query_lens(&tasks, BoardLens::Desk, false);

        let motion: Vec<_> = view
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::InMotion)
            .collect();
        assert_eq!(motion.len(), 1);
        assert_eq!(
            ids(motion[0]),
            vec![Uuid::from_u128(2), Uuid::from_u128(5), Uuid::from_u128(1),],
            "capture order across every live scope, not status-change recency"
        );

        let desk: Vec<_> = view
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::OnDeck && s.project_label.is_none())
            .collect();
        assert_eq!(desk.len(), 1);
        assert_eq!(ids(desk[0]), vec![Uuid::from_u128(4)]);
        assert!(!section_ids(&view, SectionKind::OnDeck).contains(&Uuid::from_u128(2)));
    }

    #[test]
    fn neither_a_step_tick_nor_a_status_change_reorders_in_motion() {
        let older = with_status_event(
            task(1, HumanStatus::Doing, TaskScope::Global, false, 10),
            TaskEventKind::StatusSet,
            20,
        );
        let newer = with_status_event(
            task(2, HumanStatus::Doing, TaskScope::Global, false, 30),
            TaskEventKind::StatusSet,
            50,
        );
        // The step tick lands long after both status changes and bumps updated_at
        // past the newer task's; the section order must not move.
        let tasks = vec![mutated(older, TaskEventKind::StepChecked, 200), newer];

        let view = query_lens(&tasks, BoardLens::Desk, false);

        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![Uuid::from_u128(1), Uuid::from_u128(2)],
            "rows stay in board order after a newer status change and a step tick"
        );
    }

    #[test]
    fn board_order_is_task_number_and_project_order_groups_desk_then_path() {
        let numbered = |id: u128, number: u64, scope: TaskScope, created: u64| {
            let mut task = task(id, HumanStatus::Doing, scope, false, created);
            task.number = Some(number);
            task
        };
        let mut notice = task(9, HumanStatus::Doing, TaskScope::Global, false, 99);
        notice.notice = Some(crate::domain::Notice {
            catalog_id: "guide.welcome".into(),
            number: Some(1),
        });
        // Capture time runs opposite to the numbers, so only the number can order them.
        let tasks = vec![
            numbered(1, 3, project("/repos/b"), 10),
            numbered(2, 1, project("/repos/a"), 30),
            numbered(3, 2, TaskScope::Global, 20),
            task(4, HumanStatus::Doing, TaskScope::Global, false, 5),
            notice,
        ];
        let order = |sort| {
            section_ids(
                &query_board(&tasks, &BTreeSet::new(), BoardLens::All, false, sort),
                SectionKind::InMotion,
            )
        };
        let id = Uuid::from_u128;
        assert_eq!(
            order(BoardSort::Number),
            vec![id(9), id(2), id(3), id(1), id(4)],
            "notices lead, then ascending T number, an unnumbered row last"
        );
        assert_eq!(
            order(BoardSort::Project),
            vec![id(9), id(3), id(4), id(2), id(1)],
            "desk first, then project path, number order inside each"
        );
    }

    #[test]
    fn on_deck_is_oldest_first_and_an_edit_does_not_move_a_row() {
        let tasks = vec![
            task(1, HumanStatus::Todo, TaskScope::Global, false, 10),
            // Task 2 is edited at 90, past both neighbours: it stays in FIFO place.
            mutated(
                task(2, HumanStatus::Todo, TaskScope::Global, false, 20),
                TaskEventKind::Edited,
                90,
            ),
            task(3, HumanStatus::Todo, TaskScope::Global, false, 30),
        ];

        let view = query_lens(&tasks, BoardLens::Desk, false);

        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)],
            "ON DECK is the backlog: oldest created first, edits never reorder"
        );
    }

    #[test]
    fn notices_lead_on_deck_ahead_of_an_older_backlog() {
        // A starter guide seeded today (created 90) on a desk with tasks from 10 and 20:
        // FIFO alone would bury it, so notices lead and the backlog keeps its own order.
        let mut guide = task(3, HumanStatus::Todo, TaskScope::Global, false, 90);
        guide.notice = Some(crate::domain::Notice {
            catalog_id: "guide.welcome".into(),
            number: None,
        });
        let tasks = vec![
            task(1, HumanStatus::Todo, TaskScope::Global, false, 10),
            task(2, HumanStatus::Todo, TaskScope::Global, false, 20),
            guide,
        ];

        let view = query_lens(&tasks, BoardLens::Desk, false);

        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(3), Uuid::from_u128(1), Uuid::from_u128(2)],
        );
    }

    fn blocked_on(id: u128, number: u64, on: crate::domain::BlockOn) -> Task {
        let mut blocked = task(id, HumanStatus::Blocked, TaskScope::Global, false, 10);
        blocked.number = Some(number);
        blocked.block = Some(crate::domain::Block::open(
            crate::domain::BlockDraft {
                on,
                ..Default::default()
            },
            "claude",
            SystemTime::UNIX_EPOCH,
        ));
        blocked
    }

    #[test]
    fn a_block_waiting_elsewhere_rides_in_motion_until_its_blocker_is_done() {
        let mut blocker = task(1, HumanStatus::Started, TaskScope::Global, false, 10);
        blocker.number = Some(1);
        let on_task = blocked_on(2, 2, crate::domain::BlockOn::Task(1));
        let on_other = blocked_on(3, 3, crate::domain::BlockOn::Other("legal".into()));
        let on_you = blocked_on(4, 4, crate::domain::BlockOn::You);
        let legacy = task(5, HumanStatus::Blocked, TaskScope::Global, false, 10);
        let mut tasks = vec![blocker, on_task, on_other, on_you, legacy];

        let view = query_lens(&tasks, BoardLens::Desk, false);
        let mut motion = section_ids(&view, SectionKind::InMotion);
        motion.sort();
        assert_eq!(
            motion,
            vec![Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)]
        );
        let mut need = section_ids(&view, SectionKind::NeedsYou);
        need.sort();
        assert_eq!(need, vec![Uuid::from_u128(4), Uuid::from_u128(5)]);
        assert_eq!((view.counts.in_motion, view.counts.need), (3, 2));

        tasks[0].status = HumanStatus::Done;
        let view = query_lens(&tasks, BoardLens::Desk, false);
        assert!(
            section_ids(&view, SectionKind::NeedsYou).contains(&Uuid::from_u128(2)),
            "a done blocker hands the task back to you"
        );
        assert_eq!(
            block_wait(&tasks[1], &tasks),
            Some(BlockWait::BlockerDone(1))
        );
        assert_eq!(tasks[1].status, HumanStatus::Blocked, "tsk never unblocks");
    }

    #[test]
    fn a_block_on_a_trashed_or_missing_task_comes_back_to_you() {
        let mut blocker = task(1, HumanStatus::Started, TaskScope::Global, false, 10);
        blocker.number = Some(1);
        blocker.soft_deleted = true;
        let on_trashed = blocked_on(2, 2, crate::domain::BlockOn::Task(1));
        let on_missing = blocked_on(3, 3, crate::domain::BlockOn::Task(9));
        let tasks = vec![blocker, on_trashed, on_missing];

        assert_eq!(
            block_wait(&tasks[1], &tasks),
            Some(BlockWait::BlockerGone(1)),
            "a started blocker in the trash is gone, not still in motion"
        );
        assert_eq!(
            block_wait(&tasks[2], &tasks),
            Some(BlockWait::BlockerGone(9))
        );
        let view = query_lens(&tasks, BoardLens::Desk, false);
        let mut need = section_ids(&view, SectionKind::NeedsYou);
        need.sort();
        assert_eq!(need, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
        assert!(section_ids(&view, SectionKind::InMotion).is_empty());
        assert_eq!((view.counts.in_motion, view.counts.need), (0, 2));
        assert!(
            tasks[1..]
                .iter()
                .all(|task| task.status == HumanStatus::Blocked),
            "tsk never unblocks"
        );
    }

    #[test]
    fn project_board_puts_a_block_on_another_task_in_motion_not_on_deck() {
        let mut on_task = blocked_on(2, 2, crate::domain::BlockOn::Other("vendor".into()));
        on_task.scope = project("/repos/a");
        let mut on_you = blocked_on(3, 3, crate::domain::BlockOn::You);
        on_you.scope = project("/repos/a");
        let tasks = vec![on_task, on_you];
        let view = query_lens(&tasks, BoardLens::Project(Path::new("/repos/a")), false);
        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::NeedsYou),
            vec![Uuid::from_u128(3)]
        );
        assert!(section_ids(&view, SectionKind::OnDeck).is_empty());
    }

    #[test]
    fn board_search_matches_the_assignee_with_or_without_its_at() {
        let mut assigned = task(1, HumanStatus::Ready, TaskScope::Global, false, 0);
        assigned.assignee = Some("claude".to_string());
        let other = task(2, HumanStatus::Ready, TaskScope::Global, false, 0);
        for query in ["claude", "@claude", "@CLA"] {
            let view = query_board_search(
                &[assigned.clone(), other.clone()],
                &BTreeSet::new(),
                BoardLens::Desk,
                false,
                BoardSort::Number,
                query,
            );
            assert_eq!(
                visible_task_ids(&view, false, false)
                    .into_iter()
                    .filter(|id| *id != INBOX_HEADER_ROW_ID)
                    .collect::<Vec<_>>(),
                vec![assigned.id],
                "{query}"
            );
        }
        assert!(!task_matches(&other, &search_words("@")));
    }

    #[test]
    fn desk_needs_you_is_global_across_every_live_scope() {
        let tasks = vec![
            task(1, HumanStatus::Doing, TaskScope::Global, false, 10),
            task(2, HumanStatus::Doing, project("/repos/a"), false, 30),
            task(3, HumanStatus::Todo, TaskScope::Global, false, 20),
            task(4, HumanStatus::Doing, project("/repos/b"), false, 40),
            task(5, HumanStatus::Doing, project("/repos/a"), false, 50),
            task(6, HumanStatus::Doing, project("/repos/gone"), false, 60),
        ];
        let mut gone = BTreeSet::new();
        gone.insert("/repos/gone".to_string());

        let view = query_board(&tasks, &gone, BoardLens::Desk, false, BoardSort::Number);
        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![
                Uuid::from_u128(1),
                Uuid::from_u128(2),
                Uuid::from_u128(4),
                Uuid::from_u128(5),
            ],
            "blocked/review from every live project plus desk, in board order"
        );
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(3)],
            "project ready tasks never leak into the desk backlog"
        );
        assert_eq!(view.sections[0].kind, SectionKind::InMotion);
    }

    #[test]
    fn project_board_puts_blocked_and_review_in_needs_you() {
        let tasks = vec![
            task(1, HumanStatus::Doing, project("/repos/a"), false, 100),
            task(2, HumanStatus::Doing, project("/repos/a"), false, 80),
            task(3, HumanStatus::Doing, project("/repos/a"), false, 90),
            task(4, HumanStatus::Todo, project("/repos/a"), false, 70),
            task(5, HumanStatus::Doing, project("/repos/b"), false, 60),
        ];

        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            BoardLens::Project(Path::new("/repos/a")),
            false,
            BoardSort::Number,
        );
        // Board order is the task number, falling back to creation order for these
        // unnumbered fixtures; a status change never moves a row to the top.
        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![Uuid::from_u128(2), Uuid::from_u128(3), Uuid::from_u128(1)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(4)]
        );
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(5)));
        assert_eq!(view.sections[0].kind, SectionKind::InMotion);
    }

    #[test]
    fn empty_deck_is_omitted_when_needs_you_has_rows() {
        let desk_tasks = vec![task(1, HumanStatus::Doing, TaskScope::Global, false, 10)];
        let desk = query_lens(&desk_tasks, BoardLens::Desk, false);
        assert!(desk.sections.iter().all(|s| s.kind != SectionKind::OnDeck));
        assert!(!desk.sections.iter().any(|s| s.empty_hint));

        let project_tasks = vec![task(2, HumanStatus::Doing, project("/repos/a"), false, 10)];
        let project = query_board(
            &project_tasks,
            &BTreeSet::new(),
            BoardLens::Project(Path::new("/repos/a")),
            false,
            BoardSort::Number,
        );
        assert!(project
            .sections
            .iter()
            .all(|s| s.kind != SectionKind::OnDeck));
        assert!(!project.sections.iter().any(|s| s.empty_hint));
    }

    #[test]
    fn empty_project_board_keeps_one_hinted_deck_section() {
        let tasks = vec![task(9, HumanStatus::Todo, project("/repos/b"), false, 10)];
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            BoardLens::Project(Path::new("/repos/a")),
            false,
            BoardSort::Number,
        );
        let deck = view
            .sections
            .iter()
            .find(|s| s.kind == SectionKind::OnDeck)
            .expect("empty project keeps the deck section");
        assert!(deck.empty_hint);
        assert!(deck.task_ids.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn project_board_matches_tasks_across_path_spellings() {
        use std::fs;

        // The stored scope was captured through one spelling of the directory; the
        // invocation resolved the same directory through a symlink (the /tmp vs
        // /private/tmp mismatch). The lens must still match, without rewriting the
        // stored identity.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tsk-path-spelling-{}-{seq}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let link = std::env::temp_dir().join(format!(
            "tsk-path-spelling-link-{}-{seq}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&dir, &link).expect("symlink");

        let stored = dir.to_string_lossy().into_owned();
        let invoked = link.to_string_lossy().into_owned();
        let tasks = vec![task(1, HumanStatus::Todo, project(&stored), false, 10)];
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            BoardLens::Project(Path::new(&invoked)),
            false,
            BoardSort::Number,
        );
        let matched = section_ids(&view, SectionKind::OnDeck) == vec![Uuid::from_u128(1)];
        fs::remove_file(&link).ok();
        fs::remove_dir_all(&dir).ok();
        assert!(
            matched,
            "equivalent path spellings must match the stored project scope"
        );
    }

    #[test]
    fn done_drawer_lists_non_deleted_done_in_board_order() {
        let tasks = vec![
            task(1, HumanStatus::Done, TaskScope::Global, false, 10),
            task(2, HumanStatus::Done, project("/repos/a"), false, 30),
            task(3, HumanStatus::Done, TaskScope::Global, true, 40),
            task(4, HumanStatus::Todo, TaskScope::Global, false, 50),
        ];

        // Closed, DONE keeps its heading and count but no rows, so nothing selects them.
        let closed = query_lens(&tasks, BoardLens::Desk, false);
        let heading: Vec<_> = closed
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Done)
            .map(|s| (s.count, s.task_ids.len()))
            .collect();
        assert_eq!(heading, vec![(2, 0)]);
        assert_eq!(
            visible_task_ids(&closed, false, false),
            vec![Uuid::from_u128(4)]
        );

        let open = query_lens(&tasks, BoardLens::Desk, true);
        let done: Vec<_> = open
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Done)
            .collect();
        assert_eq!(done.len(), 1);
        assert_eq!(ids(done[0]), vec![Uuid::from_u128(1), Uuid::from_u128(2),]);
    }

    #[test]
    fn project_board_filters_sections_to_matching_project() {
        let tasks = vec![
            task(1, HumanStatus::Doing, project("/repos/a"), false, 100),
            task(2, HumanStatus::Doing, project("/repos/b"), false, 90),
            task(10, HumanStatus::Todo, project("/repos/a"), false, 70),
            task(20, HumanStatus::Done, project("/repos/a"), false, 40),
        ];

        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            BoardLens::Project(Path::new("/repos/a")),
            true,
            BoardSort::Number,
        );

        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![Uuid::from_u128(1)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::Done),
            vec![Uuid::from_u128(20)]
        );
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(2)));
    }

    #[test]
    fn visible_task_ids_keeps_done_drawer_rows_selectable() {
        let tasks = vec![
            task(1, HumanStatus::Todo, project("/repos/a"), false, 10),
            task(2, HumanStatus::Done, project("/repos/a"), false, 40),
        ];
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            BoardLens::Project(Path::new("/repos/a")),
            true,
            BoardSort::Number,
        );
        assert_eq!(
            section_ids(&view, SectionKind::Done),
            vec![Uuid::from_u128(2)]
        );

        // No archived tasks: the drawer paints DONE rows only.
        let visible = visible_task_ids(&view, false, false);
        assert_eq!(
            visible,
            vec![Uuid::from_u128(1), Uuid::from_u128(2)],
            "painted DONE drawer rows must stay in the selectable set"
        );

        // With an archived task present, the header row is always selectable so ←/→
        // can fold the group; the collapsed group paints only its header.
        let mut tasks = tasks.clone();
        let mut archived = task(3, HumanStatus::Todo, project("/repos/a"), false, 50);
        archived.archived = true;
        tasks.push(archived);
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            BoardLens::Project(Path::new("/repos/a")),
            true,
            BoardSort::Number,
        );
        let expanded = visible_task_ids(&view, false, false);
        assert!(
            expanded.contains(&ARCHIVED_HEADER_ROW_ID) && expanded.contains(&Uuid::from_u128(3))
        );
        let collapsed = visible_task_ids(&view, true, false);
        assert_eq!(
            collapsed,
            vec![
                Uuid::from_u128(1),
                Uuid::from_u128(2),
                ARCHIVED_HEADER_ROW_ID
            ],
            "the collapsed archived group paints only its header"
        );
    }

    #[test]
    fn on_deck_puts_ready_picks_above_inbox_open_rows() {
        let tasks = vec![
            with_status_event(
                task(1, HumanStatus::Todo, TaskScope::Global, false, 10),
                TaskEventKind::StatusSet,
                80,
            ),
            with_status_event(
                task(2, HumanStatus::Todo, TaskScope::Global, false, 20),
                TaskEventKind::StatusSet,
                40,
            ),
            task(3, HumanStatus::Todo, TaskScope::Global, false, 30),
            task(4, HumanStatus::Todo, TaskScope::Global, false, 5),
        ];
        let view = query_lens(&tasks, BoardLens::Desk, false);
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![
                Uuid::from_u128(4),
                Uuid::from_u128(1),
                Uuid::from_u128(2),
                Uuid::from_u128(3)
            ],
            "all Todo tasks are oldest capture first"
        );
        assert_eq!(
            section_ids(&view, SectionKind::Inbox),
            Vec::<Uuid>::new(),
            "Todo has no separate inbox section"
        );
        let deck = view
            .sections
            .iter()
            .find(|section| section.kind == SectionKind::OnDeck)
            .expect("on deck");
        assert_eq!(deck.count, 4, "ON DECK totals ready plus open");
    }

    #[test]
    fn inbox_heading_is_absent_without_open_and_present_without_ready() {
        let ready_only = vec![task(1, HumanStatus::Todo, TaskScope::Global, false, 10)];
        let ready_view = query_lens(&ready_only, BoardLens::Desk, false);
        assert!(
            ready_view
                .sections
                .iter()
                .all(|section| section.kind != SectionKind::Inbox),
            "no open tasks means no inbox heading"
        );

        let open_only = vec![task(2, HumanStatus::Todo, TaskScope::Global, false, 10)];
        let open_view = query_lens(&open_only, BoardLens::Desk, false);
        let deck = open_view
            .sections
            .iter()
            .find(|section| section.kind == SectionKind::OnDeck)
            .expect("on deck still paints");
        assert_eq!(deck.task_ids, vec![Uuid::from_u128(2)]);
        assert_eq!(deck.count, 1);
        assert!(!deck.empty_hint);
        assert_eq!(
            section_ids(&open_view, SectionKind::Inbox),
            Vec::<Uuid>::new()
        );
    }

    #[test]
    fn folded_inbox_hides_open_rows() {
        let tasks = vec![
            task(1, HumanStatus::Todo, TaskScope::Global, false, 10),
            task(2, HumanStatus::Todo, TaskScope::Global, false, 20),
        ];
        let view = query_lens(&tasks, BoardLens::Desk, false);
        let expanded = visible_task_ids(&view, false, false);
        assert_eq!(expanded, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
        let collapsed = visible_task_ids(&view, false, true);
        assert_eq!(
            collapsed,
            vec![Uuid::from_u128(1), Uuid::from_u128(2)],
            "legacy inbox fold state cannot hide Todo tasks"
        );
        assert!(
            collapsed.contains(&Uuid::from_u128(2)),
            "Todo selection remains available"
        );
    }

    #[test]
    fn desk_on_deck_excludes_project_ready_and_open() {
        let tasks = vec![
            task(1, HumanStatus::Todo, TaskScope::Global, false, 10),
            task(2, HumanStatus::Todo, TaskScope::Global, false, 20),
            task(3, HumanStatus::Todo, project("/repos/a"), false, 30),
            task(4, HumanStatus::Todo, project("/repos/a"), false, 40),
        ];
        let view = query_lens(&tasks, BoardLens::Desk, false);
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(1), Uuid::from_u128(2)]
        );
        assert_eq!(section_ids(&view, SectionKind::Inbox), Vec::<Uuid>::new());
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(3)));
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(4)));
    }

    fn task_with_thread(
        id: u128,
        status: HumanStatus,
        scope: TaskScope,
        soft_deleted: bool,
        updated_secs: u64,
        thread: Option<&str>,
    ) -> Task {
        let mut task = task(id, status, scope, soft_deleted, updated_secs);
        task.thread = thread.map(str::to_string);
        task
    }

    /// The desk query in its own status-change order, before the board's number order.
    fn desk_view(tasks: &[Task], filter: &DeskFilter) -> QueueView {
        query_desk(
            tasks,
            &BTreeSet::new(),
            false,
            filter,
            &PathIdentityCache::default(),
            &[],
        )
    }

    fn section(view: &QueueView, kind: SectionKind) -> &QueueSection {
        view.sections
            .iter()
            .find(|section| section.kind == kind)
            .expect("section present")
    }

    fn labels(section: &QueueSection) -> Vec<&str> {
        section
            .groups
            .iter()
            .map(|group| group.label.as_str())
            .collect()
    }

    #[test]
    fn grouping_by_project_orders_groups_hottest_first_and_keeps_the_section_total() {
        let tasks = vec![
            task(1, HumanStatus::Started, project("/r/alpha"), false, 10),
            task(2, HumanStatus::Started, project("/r/beta"), false, 30),
            task(3, HumanStatus::Started, TaskScope::Global, false, 20),
            task(4, HumanStatus::Started, project("/r/alpha"), false, 5),
        ];
        let mut view = desk_view(&tasks, &DeskFilter::default());
        arrange_view(
            &mut view,
            &tasks,
            BoardLens::Desk,
            GroupBy::Project,
            SortBy::Recent,
        );
        let motion = section(&view, SectionKind::InMotion);
        assert_eq!(labels(motion), vec!["beta", "desk", "alpha"]);
        assert_eq!(motion.count, 4, "the section header keeps its total");
        assert_eq!(
            motion.task_ids,
            vec![2, 3, 1, 4]
                .into_iter()
                .map(Uuid::from_u128)
                .collect::<Vec<_>>(),
            "task_ids is the exact concatenation of the groups"
        );
        assert!(motion
            .groups
            .iter()
            .all(|group| is_group_header_id(group.header_id)));
    }

    #[test]
    fn grouping_by_thread_names_unthreaded_rows_and_sort_ties_keep_recent_order() {
        let tasks = vec![
            task_with_thread(
                1,
                HumanStatus::Blocked,
                TaskScope::Global,
                false,
                40,
                Some("ship"),
            ),
            task_with_thread(2, HumanStatus::Review, project("/r/a"), false, 30, None),
            task_with_thread(
                3,
                HumanStatus::Blocked,
                project("/r/b"),
                false,
                20,
                Some("ship"),
            ),
            task_with_thread(
                4,
                HumanStatus::Review,
                project("/r/a"),
                false,
                10,
                Some("api"),
            ),
        ];
        let mut view = desk_view(&tasks, &DeskFilter::default());
        arrange_view(
            &mut view,
            &tasks,
            BoardLens::Desk,
            GroupBy::Thread,
            SortBy::Project,
        );
        let need = section(&view, SectionKind::NeedsYou);
        assert_eq!(labels(need), vec!["#ship", "no thread", "#api"]);
        // Inside #ship, sort by project: `b` before the desk row (desk sorts last).
        assert_eq!(
            need.groups[0].task_ids,
            vec![Uuid::from_u128(3), Uuid::from_u128(1)]
        );
    }

    #[test]
    fn the_done_drawer_and_archived_group_stay_ungrouped() {
        let mut archived = task(3, HumanStatus::Done, project("/r/a"), false, 5);
        archived.archived = true;
        let tasks = vec![
            task(1, HumanStatus::Done, project("/r/a"), false, 10),
            task(2, HumanStatus::Done, project("/r/b"), false, 20),
            archived,
        ];
        let mut view = query_desk(
            &tasks,
            &BTreeSet::new(),
            true,
            &DeskFilter::default(),
            &PathIdentityCache::default(),
            &[],
        );
        arrange_view(
            &mut view,
            &tasks,
            BoardLens::Desk,
            GroupBy::Project,
            SortBy::Title,
        );
        for kind in [SectionKind::Done, SectionKind::Archived] {
            let section = section(&view, kind);
            assert!(section.groups.is_empty(), "{kind:?} stays ungrouped");
        }
        assert_eq!(
            section(&view, SectionKind::Done).task_ids,
            vec![Uuid::from_u128(2), Uuid::from_u128(1)],
            "and keeps its own order (newest status change first), not title order"
        );
    }

    #[test]
    fn sub_headers_are_always_selectable_and_folding_hides_their_rows() {
        let tasks = vec![
            task(1, HumanStatus::Started, project("/r/a"), false, 10),
            task(2, HumanStatus::Started, project("/r/b"), false, 20),
        ];
        let mut view = desk_view(&tasks, &DeskFilter::default());
        arrange_view(
            &mut view,
            &tasks,
            BoardLens::Desk,
            GroupBy::Project,
            SortBy::Recent,
        );
        let groups = section(&view, SectionKind::InMotion).groups.clone();
        assert_eq!(
            visible_row_ids(&view, true, false, &BTreeSet::new()),
            vec![
                groups[0].header_id,
                Uuid::from_u128(2),
                groups[1].header_id,
                Uuid::from_u128(1)
            ],
            "an unfolded sub-header is a stop of its own, so ← can fold it"
        );
        let folded: BTreeSet<Uuid> = [groups[0].header_id].into();
        assert_eq!(
            visible_row_ids(&view, true, false, &folded),
            vec![groups[0].header_id, groups[1].header_id, Uuid::from_u128(1)]
        );
    }

    #[test]
    fn the_desk_filter_combines_project_and_thread_across_every_status() {
        let tasks = vec![
            task_with_thread(
                1,
                HumanStatus::Started,
                project("/r/a"),
                false,
                10,
                Some("ship"),
            ),
            task_with_thread(2, HumanStatus::Started, project("/r/a"), false, 20, None),
            task_with_thread(
                3,
                HumanStatus::Blocked,
                project("/r/b"),
                false,
                30,
                Some("ship"),
            ),
            task_with_thread(
                4,
                HumanStatus::Ready,
                TaskScope::Global,
                false,
                40,
                Some("ship"),
            ),
            task_with_thread(
                5,
                HumanStatus::Done,
                project("/r/a"),
                false,
                50,
                Some("ship"),
            ),
            task_with_thread(
                6,
                HumanStatus::Done,
                project("/r/b"),
                false,
                60,
                Some("ship"),
            ),
        ];
        let filter = DeskFilter {
            project: ProjectFilter::Named("/r/a".into()),
            thread: ThreadFilter::Named("ship".into()),
            assignee: AssigneeFilter::All,
        };
        assert_eq!(filter.label(), "a #ship");
        let view = desk_view(&tasks, &filter);
        assert_eq!(all_listed_ids(&view), vec![Uuid::from_u128(1)]);
        assert_eq!(view.counts.in_motion, 1);
        assert_eq!(view.counts.need, 0);
        assert_eq!(view.counts.done, 1, "the drawer count narrows too");

        let desk_only = DeskFilter {
            project: ProjectFilter::Desk,
            thread: ThreadFilter::All,
            assignee: AssigneeFilter::All,
        };
        assert_eq!(
            all_listed_ids(&desk_view(&tasks, &desk_only)),
            vec![Uuid::from_u128(4)]
        );
        assert_eq!(DeskFilter::default().label(), "all");
    }
}
