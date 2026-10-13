//! The questions tab (`2`): the user-facing view of an Agent board's task-linked questions.
//!
//! Everything here reads one session snapshot of the private question records (the feed)
//! that the app loads at board open, on every store change, and after each question
//! action. Nothing is polled and nothing is cached on disk. Who a question waits on comes
//! only from its explicit audience: a record without one is shown as "target unknown",
//! never guessed from names, status, or answer origin.

use std::cell::Cell;
use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use uuid::Uuid;

use crate::domain::{
    ApprovalTerms, Audience, FindingAction, FindingKind, FindingLink, QuestionKind, QuestionRecord,
    Task,
};
use crate::ui::input::BoardIntent;
use crate::ui::queue::NavTab;
use crate::ui::render::{self, QueueHitMap, QueueHitTarget};
use crate::ui::tier;

/// The session snapshot of the board's question records.
#[derive(Debug, Clone, Default)]
pub enum QuestionFeed {
    /// The board is not an explicit Agent board (the personal `~/.tsk`): it records no
    /// Agent questions, findings, or Threads. Nothing waits and the tab says so.
    #[default]
    Disabled,
    /// The records could not be read. Never shown as zero or as an empty list.
    Unavailable(String),
    /// Every record, in store order.
    Loaded(Vec<QuestionRecord>),
}

impl QuestionFeed {
    pub fn records(&self) -> &[QuestionRecord] {
        match self {
            Self::Loaded(records) => records,
            _ => &[],
        }
    }

    pub fn get(&self, id: Uuid) -> Option<&QuestionRecord> {
        self.records()
            .iter()
            .find(|record| record.reference.id == id)
    }
}

/// The approval check (`Space`), bound to the exact revision and terms the user read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCheck {
    pub id: Uuid,
    pub revision: Uuid,
    pub terms: ApprovalTerms,
}

/// A free-text answer being typed for a question without options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerInput {
    pub id: Uuid,
    pub revision: Uuid,
    pub corrects: Option<Uuid>,
    pub text: String,
}

/// What a confirmed commit still owes once it is known to be durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    /// Send the identifier-only hint to the asking Thread.
    NotifyOrigin,
    /// The same, then the run request to the record's Lead address.
    NotifyOriginAndLead,
}

/// A question save the board could not call done. The form stays; nothing retries blindly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingCommit {
    /// The commit is visible but its durability is not proven: `Enter` confirms this exact
    /// revision, never a new answer.
    Uncertain {
        id: Uuid,
        revision: Uuid,
        follow_up: FollowUp,
    },
    /// The save failed and readback could not tell whether it committed: `Enter` reads the
    /// record back first. No retry, no task recreation.
    Unknown {
        id: Uuid,
        prior_revision: Uuid,
        follow_up: FollowUp,
        attempt: AnswerAttempt,
    },
}

/// What one save attempted, kept for this session only. An unknown commit's readback
/// accepts the visible answer as this save only when it is the one answer added since and
/// matches every field here; origin alone never proves it. The text is kept as a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnswerAttempt {
    pub answers_before: usize,
    pub routes_before: usize,
    pub text_hash: u64,
    pub corrects: Option<Uuid>,
    pub action: Option<FindingAction>,
    pub approve: bool,
}

impl PendingCommit {
    pub fn id(&self) -> Uuid {
        match *self {
            Self::Uncertain { id, .. } | Self::Unknown { id, .. } => id,
        }
    }
}

/// Session-only state of the questions tab. Never persisted.
#[derive(Debug, Clone, Default)]
pub struct QuestionsState {
    pub feed: QuestionFeed,
    /// Whether tab 2 owns the frame and the keyboard.
    pub active: bool,
    /// The list row the cursor rests on.
    pub cursor: Option<Uuid>,
    /// The open question: a full page below 110 columns, the focused detail at 110 and up.
    pub page: Option<Uuid>,
    /// Option (or finding action) chosen per question, by index.
    pub options: BTreeMap<Uuid, usize>,
    pub check: Option<ApprovalCheck>,
    /// A correction in progress: (question, the revision it was read at).
    pub correcting: Option<(Uuid, Uuid)>,
    pub answer: Option<AnswerInput>,
    pub pending: Option<PendingCommit>,
    /// The task page a question was opened from: `Esc` returns to it.
    pub return_task: Option<Uuid>,
    /// The question a task page was opened from: a clean `Esc` on that page returns here.
    pub return_question: Option<Uuid>,
    /// First painted row of the open question page.
    pub scroll: Cell<usize>,
    /// Whether the next paint scrolls the focus row (chosen option, check, input) into
    /// view. A selection change sets it; an explicit scroll (wheel, `PgUp`/`PgDn`, `j`/`k`
    /// on a page without choices) clears it so rows above or below the focus stay readable.
    pub follow: Cell<bool>,
    /// The last save's hint outcome for one question, shown on its page.
    pub report: Option<(Uuid, String)>,
}

/// One questions-tab action. Pure moves apply on [`QuestionsState`]; the rest go through
/// the app boundary, which owns the store and the Herdr hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionIntent {
    /// `j`/`k`/arrows: the list cursor, the chosen option, or the page scroll.
    Move(bool),
    /// The wheel: the list cursor, or the page scroll (never the chosen option).
    Scroll(bool),
    /// `Enter`/`l`/`→` on the list.
    Open,
    /// `Esc`/`h`/`←` on a page.
    Back,
    /// `J`/`K`: the next or previous question in the list's own order.
    Step(bool),
    /// `Enter` on a page: save the chosen answer, decide a finding, or confirm a pending save.
    Enter,
    /// `Space`: the approval check.
    ToggleCheck,
    /// `y`.
    Approve,
    /// `n`.
    Reject,
    /// `c`: correct the current answer as a new revision.
    Correct,
    /// `t`: open the question's task.
    OpenTask,
    /// `r`: resend the run request of a finding decided as create-and-run.
    Resend,
    /// A row click.
    Row(Uuid),
    /// A question line on a task page.
    FromTask(Uuid),
    /// A reference line on a task page (`task`, index into its links).
    Link(Uuid, usize),
    /// A click on an action line of the shown page; at 110 and up it opens the cursor's
    /// preview first, so the click acts on the record the user saw.
    Page(PageClick),
    AnswerInsert(String),
    AnswerBackspace,
    AnswerCancel,
}

/// The action lines a question page paints as click targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageClick {
    /// Choose this option (index into the page's choices); it does not save.
    Option(usize),
    /// The `enter` line: write, save, decide, or confirm, as `Enter` does.
    Enter,
    /// The approval check line, as `Space` does.
    Check,
    /// The task line, as `t` does.
    OpenTask,
    /// The `c corrects` line.
    Correct,
    /// The `r resends` line.
    Resend,
}

/// The groups of the list, in paint order. `Unrouted` is the compatibility block for open
/// records without an audience: shown, never counted as waiting on you.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionGroup {
    NeedsApproval,
    ForYou,
    Findings,
    WithLead,
    Unrouted,
    Closed,
}

impl QuestionGroup {
    pub const ORDER: [Self; 6] = [
        Self::NeedsApproval,
        Self::ForYou,
        Self::Findings,
        Self::WithLead,
        Self::Unrouted,
        Self::Closed,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::NeedsApproval => "NEEDS APPROVAL",
            Self::ForYou => "FOR YOU",
            Self::Findings => "FINDINGS",
            Self::WithLead => "WITH LEAD",
            Self::Unrouted => "target unknown · route with tsk route",
            Self::Closed => "CLOSED",
        }
    }
}

pub fn group_of(record: &QuestionRecord) -> QuestionGroup {
    if !record.is_open() {
        return QuestionGroup::Closed;
    }
    match record.draft.audience {
        None => QuestionGroup::Unrouted,
        Some(Audience::Lead) => QuestionGroup::WithLead,
        Some(Audience::User) => match record.reference.kind {
            QuestionKind::Approval => QuestionGroup::NeedsApproval,
            QuestionKind::Question => QuestionGroup::ForYou,
            QuestionKind::Finding => QuestionGroup::Findings,
        },
    }
}

/// The non-empty groups with their records, each in Q-number order.
pub fn grouped(records: &[QuestionRecord]) -> Vec<(QuestionGroup, Vec<&QuestionRecord>)> {
    QuestionGroup::ORDER
        .iter()
        .filter_map(|group| {
            let mut members: Vec<&QuestionRecord> = records
                .iter()
                .filter(|record| group_of(record) == *group)
                .collect();
            members.sort_by_key(|record| record.reference.number);
            (!members.is_empty()).then_some((*group, members))
        })
        .collect()
}

/// Every listed question id, in paint order: what `j`/`k` and `J`/`K` walk.
pub fn list_order(records: &[QuestionRecord]) -> Vec<Uuid> {
    grouped(records)
        .into_iter()
        .flat_map(|(_, members)| members.into_iter().map(|record| record.reference.id))
        .collect()
}

/// Open, user-targeted questions by kind, plus open records with no audience.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Waiting {
    pub approvals: usize,
    pub questions: usize,
    pub findings: usize,
    pub unrouted: usize,
}

impl Waiting {
    pub fn for_you(&self) -> usize {
        self.approvals + self.questions + self.findings
    }
}

pub fn waiting(records: &[QuestionRecord]) -> Waiting {
    let mut out = Waiting::default();
    for record in records {
        match group_of(record) {
            QuestionGroup::NeedsApproval => out.approvals += 1,
            QuestionGroup::ForYou => out.questions += 1,
            QuestionGroup::Findings => out.findings += 1,
            QuestionGroup::Unrouted => out.unrouted += 1,
            QuestionGroup::WithLead | QuestionGroup::Closed => {}
        }
    }
    out
}

/// The board's waiting line, or none when nothing waits (and on the personal board).
pub fn waiting_line(feed: &QuestionFeed) -> Option<String> {
    match feed {
        QuestionFeed::Disabled => None,
        QuestionFeed::Unavailable(_) => {
            Some("waiting on you · questions cannot be read · 2 shows why".to_string())
        }
        QuestionFeed::Loaded(records) => {
            let counts = waiting(records);
            let mut parts: Vec<String> = [
                ("approval", counts.approvals),
                ("question", counts.questions),
                ("finding", counts.findings),
            ]
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(word, n)| format!("{word} {n}"))
            .collect();
            if counts.unrouted > 0 {
                parts.push(format!("target unknown {}", counts.unrouted));
            }
            if parts.is_empty() {
                return None;
            }
            Some(format!("waiting on you  {}  → 2", parts.join(" · ")))
        }
    }
}

/// The `·n` beside the questions tab: open questions waiting on you.
pub fn tab_count(feed: &QuestionFeed) -> Option<usize> {
    match feed {
        QuestionFeed::Loaded(records) => Some(waiting(records).for_you()).filter(|n| *n > 0),
        _ => None,
    }
}

/// Per-task row marks: `!` for an open approval waiting on you, `?n` for open questions and
/// findings waiting on you (approvals excluded). Lead and unknown targets are not counted.
pub fn task_marks(feed: &QuestionFeed) -> BTreeMap<Uuid, String> {
    let mut counts: BTreeMap<Uuid, (bool, usize)> = BTreeMap::new();
    for record in feed.records() {
        let entry = counts.entry(record.reference.task_id).or_default();
        match group_of(record) {
            QuestionGroup::NeedsApproval => entry.0 = true,
            QuestionGroup::ForYou | QuestionGroup::Findings => entry.1 += 1,
            _ => {}
        }
    }
    counts
        .into_iter()
        .filter_map(|(task, (approval, n))| {
            let mark = match (approval, n) {
                (false, 0) => return None,
                (true, 0) => "!".to_string(),
                (false, n) => format!("?{n}"),
                (true, n) => format!("! ?{n}"),
            };
            Some((task, mark))
        })
        .collect()
}

fn task_label(tasks: &[Task], id: Uuid) -> String {
    tasks
        .iter()
        .find(|task| task.id == id)
        .and_then(Task::board_identifier)
        .unwrap_or_else(|| "a missing task".to_string())
}

/// The closed state, or the open state's target, as one short label.
pub fn state_label(record: &QuestionRecord, tasks: &[Task]) -> String {
    let Some(answer) = record.current_answer() else {
        return match record.draft.audience {
            None => "open · target unknown".to_string(),
            Some(Audience::User) => "open · for you".to_string(),
            Some(Audience::Lead) => "open · with lead".to_string(),
        };
    };
    match record.reference.kind {
        QuestionKind::Approval if answer.approval.is_some() => "approved".to_string(),
        QuestionKind::Approval => "rejected".to_string(),
        QuestionKind::Finding => match (record.finding_link(), answer.action) {
            (FindingLink::Ambiguous, _) => "several tasks linked".to_string(),
            (FindingLink::One(id), _) => format!("→ {}", task_label(tasks, id)),
            (FindingLink::None, Some(FindingAction::Close)) => "closed".to_string(),
            (FindingLink::None, _) => "answered".to_string(),
        },
        QuestionKind::Question => "answered".to_string(),
    }
}

/// The question's actionable choices: its options, or the three finding actions.
pub fn choices(record: &QuestionRecord) -> Vec<String> {
    if record.reference.kind == QuestionKind::Finding {
        return FINDING_ACTIONS
            .iter()
            .map(|(_, label)| (*label).to_string())
            .collect();
    }
    record.draft.options.clone()
}

pub const FINDING_ACTIONS: [(FindingAction, &str); 3] = [
    (FindingAction::CreateAndRun, "create the task and run it"),
    (FindingAction::CreateOnly, "create the task only (todo)"),
    (FindingAction::Close, "close · no task"),
];

/// The choice index the page starts on: the recommendation when it names an option.
fn default_choice(record: &QuestionRecord) -> usize {
    if record.reference.kind == QuestionKind::Finding {
        return 0;
    }
    record
        .draft
        .recommendation
        .as_deref()
        .and_then(|rec| {
            record
                .draft
                .options
                .iter()
                .position(|option| option.trim() == rec.trim())
        })
        .unwrap_or(0)
}

impl QuestionsState {
    pub fn records(&self) -> &[QuestionRecord] {
        self.feed.records()
    }

    pub fn order(&self) -> Vec<Uuid> {
        list_order(self.records())
    }

    /// The question the detail shows: the open page, else (wide only) the cursor's row.
    pub fn shown(&self, wide: bool) -> Option<&QuestionRecord> {
        let id = self.page.or(if wide { self.cursor } else { None })?;
        self.feed.get(id)
    }

    /// Keep the cursor on a listed row after a reload.
    pub fn reanchor(&mut self) {
        let order = self.order();
        if !self.cursor.is_some_and(|id| order.contains(&id)) {
            self.cursor = order.first().copied();
        }
        if let Some(page) = self.page {
            if self.feed.get(page).is_none() {
                self.page = None;
                self.answer = None;
                self.correcting = None;
            }
        }
        self.reset_stale_check();
    }

    /// Replace the snapshot. The approval check survives only for the same revision and terms.
    pub fn set_feed(&mut self, feed: QuestionFeed) {
        self.feed = feed;
        self.reanchor();
    }

    fn reset_stale_check(&mut self) {
        let keep = self.check.as_ref().is_some_and(|check| {
            self.feed.get(check.id).is_some_and(|record| {
                record.reference.revision == check.revision
                    && record.draft.approval.as_ref() == Some(&check.terms)
                    && record.is_open()
            })
        });
        if !keep {
            self.check = None;
        }
        let keep_correction = self.correcting.is_some_and(|(id, revision)| {
            self.feed
                .get(id)
                .is_some_and(|record| record.reference.revision == revision)
        });
        if !keep_correction {
            self.correcting = None;
        }
    }

    /// Whether quitting would lose a typed answer or an unresolved save.
    pub fn has_unsaved_work(&self) -> bool {
        self.pending.is_some()
            || self
                .answer
                .as_ref()
                .is_some_and(|input| !input.text.is_empty())
    }

    /// The typed answer opening `id` would discard: one in progress on another question.
    pub fn answer_elsewhere(&self, id: Uuid) -> Option<Uuid> {
        self.answer
            .as_ref()
            .filter(|input| input.id != id)
            .map(|input| input.id)
    }

    pub fn choice(&self, record: &QuestionRecord) -> usize {
        let count = choices(record).len();
        self.options
            .get(&record.reference.id)
            .copied()
            .unwrap_or_else(|| default_choice(record))
            .min(count.saturating_sub(1))
    }

    /// Whether the page offers its choices right now (open, or under correction).
    pub fn choosing(&self, record: &QuestionRecord) -> bool {
        record.reference.kind != QuestionKind::Approval
            && !choices(record).is_empty()
            && ((record.is_open() && record.draft.audience.is_some())
                || self
                    .correcting
                    .is_some_and(|(id, _)| id == record.reference.id))
    }

    fn clear_page_state(&mut self) {
        self.check = None;
        self.correcting = None;
        self.answer = None;
        self.scroll.set(0);
        // A page opens at its top: the body and terms first, the focus once it moves.
        self.follow.set(false);
    }

    /// An explicit page scroll by one row; the focus no longer pins the view.
    pub fn scroll_page(&self, forward: bool) {
        let scroll = self.scroll.get();
        self.scroll.set(if forward {
            scroll + 1
        } else {
            scroll.saturating_sub(1)
        });
        self.follow.set(false);
    }

    pub fn open(&mut self, id: Uuid) {
        if self.page != Some(id) {
            self.clear_page_state();
        }
        self.cursor = Some(id);
        self.page = Some(id);
    }

    /// Move the list cursor; returns a refusal message at either end.
    pub fn move_cursor(&mut self, forward: bool) {
        let order = self.order();
        if order.is_empty() {
            return;
        }
        let position = self
            .cursor
            .and_then(|id| order.iter().position(|row| *row == id));
        let next = match (position, forward) {
            (None, _) => 0,
            (Some(index), true) => (index + 1).min(order.len() - 1),
            (Some(index), false) => index.saturating_sub(1),
        };
        if self.cursor != Some(order[next]) {
            // The wide detail follows the cursor: a new record shows from its top.
            self.scroll.set(0);
            self.follow.set(false);
        }
        self.cursor = Some(order[next]);
    }

    /// `J`/`K` on a page. Off-list questions refuse and keep the page.
    pub fn step(&mut self, forward: bool) -> Option<String> {
        let page = self.page?;
        let order = self.order();
        let Some(index) = order.iter().position(|id| *id == page) else {
            return Some("this question is not in the list · esc goes back".to_string());
        };
        let next = if forward {
            index + 1
        } else if index == 0 {
            return Some("first question".to_string());
        } else {
            index - 1
        };
        let Some(&id) = order.get(next) else {
            return Some("last question".to_string());
        };
        self.return_task = None;
        self.open(id);
        Some(format!("{} / {}", next + 1, order.len()))
    }

    /// `j`/`k` on a page: move the chosen option, else scroll.
    pub fn move_on_page(&mut self, forward: bool) {
        let Some(record) = self.page.and_then(|id| self.feed.get(id)).cloned() else {
            return;
        };
        if self.answer.is_none() && self.choosing(&record) {
            let count = choices(&record).len();
            let current = self.choice(&record);
            let next = if forward {
                (current + 1).min(count - 1)
            } else {
                current.saturating_sub(1)
            };
            self.options.insert(record.reference.id, next);
            self.follow.set(true);
            return;
        }
        self.scroll_page(forward);
    }

    /// A click on an option: choose it on the open page. It never saves.
    pub fn choose(&mut self, index: usize) {
        let Some(record) = self.page.and_then(|id| self.feed.get(id)).cloned() else {
            return;
        };
        if self.answer.is_some() || !self.choosing(&record) || index >= choices(&record).len() {
            return;
        }
        self.options.insert(record.reference.id, index);
        self.follow.set(true);
    }

    pub fn toggle_check(&mut self) -> Option<&'static str> {
        let record = self.page.and_then(|id| self.feed.get(id))?;
        if record.reference.kind != QuestionKind::Approval
            || !record.is_open()
            || record.draft.audience.is_none()
        {
            return None;
        }
        let terms = record.draft.approval.clone()?;
        let check = ApprovalCheck {
            id: record.reference.id,
            revision: record.reference.revision,
            terms,
        };
        self.follow.set(true);
        if self.check.as_ref() == Some(&check) {
            self.check = None;
            Some("check cleared")
        } else {
            self.check = Some(check);
            Some("checked · y approves this exact revision")
        }
    }

    pub fn checked(&self, record: &QuestionRecord) -> bool {
        self.check.as_ref().is_some_and(|check| {
            check.id == record.reference.id
                && check.revision == record.reference.revision
                && record.draft.approval.as_ref() == Some(&check.terms)
        })
    }
}

/// The waiting-on-target words of an open record.
fn audience_word(record: &QuestionRecord) -> &'static str {
    match record.draft.audience {
        None => "target unknown",
        Some(Audience::User) => "you",
        Some(Audience::Lead) => "lead",
    }
}

fn kind_word(record: &QuestionRecord) -> String {
    match record.reference.kind {
        QuestionKind::Approval => "approval".to_string(),
        QuestionKind::Question => "question".to_string(),
        QuestionKind::Finding => match record.draft.finding.as_ref().map(|finding| finding.kind) {
            Some(FindingKind::Issue) => "finding · issue".to_string(),
            Some(FindingKind::Opportunity) => "finding · opportunity".to_string(),
            None => "finding".to_string(),
        },
    }
}

/// The paint style of one page row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStyle {
    Plain,
    Dim,
    Bold,
    Selected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRow {
    pub text: String,
    pub style: RowStyle,
    /// The row the scroll must keep in view (the chosen option, the check, the input).
    pub focus: bool,
    /// What a click on this row does, if anything.
    pub click: Option<QueueHitTarget>,
    /// Bytes of `text` that are a field label, painted dim before the value.
    pub label: usize,
}

struct PageBuilder {
    rows: Vec<PageRow>,
    width: usize,
}

impl PageBuilder {
    fn push(&mut self, text: &str, style: RowStyle, focus: bool) {
        self.indented("", text, style, focus);
    }

    /// Wrap `text` under a fixed lead; continuation rows indent under the text.
    fn indented(&mut self, lead: &str, text: &str, style: RowStyle, focus: bool) {
        let lead_w = render::display_width(lead);
        let room = self.width.saturating_sub(lead_w).max(1);
        let mut first = true;
        for line in text.split('\n') {
            for row in crate::ui::edit::wrap_text(line, room) {
                let prefix = if first {
                    lead.to_string()
                } else {
                    " ".repeat(lead_w)
                };
                self.rows.push(PageRow {
                    text: format!("{prefix}{}", row.text),
                    style,
                    focus: focus && first,
                    click: None,
                    label: 0,
                });
                first = false;
            }
        }
        if first {
            self.rows.push(PageRow {
                text: lead.to_string(),
                style,
                focus,
                click: None,
                label: 0,
            });
        }
    }

    /// A `label  value` field: the label paints dim so the value reads first.
    fn field(&mut self, label: &str, text: &str, style: RowStyle) {
        let at = self.rows.len();
        self.indented(label, text, style, false);
        if let Some(row) = self.rows.get_mut(at) {
            row.label = label.len();
        }
    }

    /// Make every row pushed since `from` a click target.
    fn click_from(&mut self, from: usize, click: PageClick) {
        for row in &mut self.rows[from..] {
            row.click = Some(QueueHitTarget::QuestionPage(click));
        }
    }

    fn blank(&mut self) {
        self.rows.push(PageRow {
            text: String::new(),
            style: RowStyle::Plain,
            focus: false,
            click: None,
            label: 0,
        });
    }
}

/// Lead of the question's original text under its `local original` heading.
const BODY_INDENT: &str = "  ";

/// The question page's rows at `width`, from the snapshot only.
pub fn page_rows(
    state: &QuestionsState,
    record: &QuestionRecord,
    tasks: &[Task],
    width: usize,
) -> Vec<PageRow> {
    let mut page = PageBuilder {
        rows: Vec::new(),
        width: width.max(1),
    };
    let draft = &record.draft;
    let header = format!(
        "Q{} · {} · {}",
        record.reference.number,
        kind_word(record),
        state_label(record, tasks)
    );
    page.push(&header, RowStyle::Bold, false);
    let task_text = tasks
        .iter()
        .find(|task| task.id == record.reference.task_id)
        .map(|task| {
            format!(
                "{} {} · t opens",
                task.board_identifier().unwrap_or_default(),
                task.title
            )
        })
        .unwrap_or_else(|| format!("{} not found", record.reference.task_id));
    let at = page.rows.len();
    page.field("task    ", &task_text, RowStyle::Plain);
    page.click_from(at, PageClick::OpenTask);
    page.field(
        "from    ",
        &format!("{} → {}", draft.origin_thread, audience_word(record)),
        RowStyle::Plain,
    );
    if let Some(lead) = draft.lead_recipient.as_deref() {
        page.field("lead    ", lead, RowStyle::Plain);
    }
    if let Some(path) = draft
        .finding
        .as_ref()
        .and_then(|finding| finding.path.as_deref())
    {
        page.field("file    ", path, RowStyle::Plain);
    }
    if let Some(scope) = draft.blocking_scope.as_deref() {
        page.field("blocks  ", scope, RowStyle::Plain);
    }
    page.blank();
    page.push(&record.reference.summary, RowStyle::Bold, false);
    page.push("local original · never shared", RowStyle::Dim, false);
    // The original sits indented under its heading so it reads as one quoted block.
    page.indented(BODY_INDENT, &draft.text, RowStyle::Plain, false);
    if let Some(finding) = draft.finding.as_ref() {
        page.indented(BODY_INDENT, &finding.detail, RowStyle::Plain, false);
    }
    if let Some(impact) = draft.impact.as_deref() {
        page.blank();
        page.field("impact  ", impact, RowStyle::Plain);
    }

    if let Some(terms) = draft.approval.as_ref() {
        page.blank();
        page.push(
            "hard to undo · check the target, scope, and revision",
            RowStyle::Bold,
            false,
        );
        // What is being approved reads bold; how far it reaches and how to undo it, plain.
        page.field("action    ", &terms.action, RowStyle::Bold);
        page.field("target    ", &terms.target, RowStyle::Bold);
        page.field("scope     ", &terms.scope, RowStyle::Plain);
        page.field("revision  ", &terms.h, RowStyle::Bold);
        page.field("undo      ", &terms.rollback, RowStyle::Plain);
        if record.is_open() && record.draft.audience.is_some() {
            let checked = state.checked(record);
            page.blank();
            let at = page.rows.len();
            page.push(
                &format!(
                    "{} I checked the target, scope, and revision (space)",
                    if checked { "[x]" } else { "[ ]" }
                ),
                if checked {
                    RowStyle::Selected
                } else {
                    RowStyle::Plain
                },
                true,
            );
            page.click_from(at, PageClick::Check);
            page.push(
                if checked {
                    "y approves · n rejects · no answer keeps waiting"
                } else {
                    "y approves once checked · n rejects · no answer keeps waiting"
                },
                RowStyle::Dim,
                false,
            );
        }
    }

    let choosing = state.choosing(record) && state.answer.is_none();
    if choosing {
        let list = choices(record);
        let chosen = state.choice(record);
        page.blank();
        let heading = if record.reference.kind == QuestionKind::Finding {
            "decide"
        } else {
            "options"
        };
        page.push(heading, RowStyle::Dim, false);
        for (index, label) in list.iter().enumerate() {
            let mark = if index == chosen { "(•) " } else { "( ) " };
            let recommended = draft
                .recommendation
                .as_deref()
                .is_some_and(|rec| rec.trim() == label.trim())
                && record.reference.kind != QuestionKind::Finding;
            let text = if recommended {
                format!("{label} · recommended")
            } else {
                label.clone()
            };
            let at = page.rows.len();
            page.indented(
                mark,
                &text,
                if index == chosen {
                    RowStyle::Selected
                } else {
                    RowStyle::Plain
                },
                index == chosen,
            );
            page.click_from(at, PageClick::Option(index));
        }
        let at = page.rows.len();
        page.push(
            if record.reference.kind == QuestionKind::Finding {
                "enter decides"
            } else if state.correcting.is_some() {
                "enter saves the correction · the earlier answer stays in history"
            } else {
                "enter saves the answer"
            },
            RowStyle::Dim,
            false,
        );
        page.click_from(at, PageClick::Enter);
    } else if record.draft.audience.is_none() {
        page.blank();
        page.push(
            "target unknown · route it with tsk route before anyone answers or corrects here",
            RowStyle::Dim,
            false,
        );
    } else if record.is_open()
        && record.reference.kind == QuestionKind::Question
        && state.answer.is_none()
    {
        page.blank();
        let at = page.rows.len();
        page.push("enter writes an answer", RowStyle::Dim, false);
        page.click_from(at, PageClick::Enter);
    }

    if let Some(input) = state
        .answer
        .as_ref()
        .filter(|input| input.id == record.reference.id)
    {
        page.blank();
        page.indented(
            "answer  ",
            &format!("{}▏", input.text),
            RowStyle::Selected,
            true,
        );
        page.push("enter saves · esc cancels", RowStyle::Dim, false);
    }

    if let Some(answer) = record.current_answer() {
        page.blank();
        page.field("answer  ", &answer.text, RowStyle::Plain);
        if let FindingLink::One(id) = record.finding_link() {
            page.field("task    ", &task_label(tasks, id), RowStyle::Plain);
        }
        if answer.action == Some(FindingAction::CreateAndRun) {
            let at = page.rows.len();
            page.push(
                "r resends the run request · it never creates another task",
                RowStyle::Dim,
                false,
            );
            page.click_from(at, PageClick::Resend);
        }
        if record.reference.kind != QuestionKind::Approval
            && state.correcting.is_none()
            && record.draft.audience.is_some()
        {
            let at = page.rows.len();
            page.push(
                "c corrects · the earlier answer stays in history",
                RowStyle::Dim,
                false,
            );
            page.click_from(at, PageClick::Correct);
        }
    }

    if let Some((_, report)) = state
        .report
        .as_ref()
        .filter(|(id, _)| *id == record.reference.id)
    {
        page.blank();
        page.push(report, RowStyle::Bold, false);
    }

    if let Some(pending) = state.pending.filter(|p| p.id() == record.reference.id) {
        page.blank();
        let text = match pending {
            PendingCommit::Uncertain { .. } => {
                "saved, durability not proven · enter confirms this revision · nothing retries"
            }
            PendingCommit::Unknown { .. } => {
                "save result unknown · enter reads the record back first · nothing retries"
            }
        };
        let at = page.rows.len();
        page.push(text, RowStyle::Bold, true);
        page.click_from(at, PageClick::Enter);
    }

    page.blank();
    page.push("history", RowStyle::Dim, false);
    let mut events = 0usize;
    for (index, answer) in record.answers.iter().enumerate() {
        for route in record
            .routes
            .iter()
            .filter(|route| route.after_answers == index)
        {
            page.push(&route_text(route), RowStyle::Dim, false);
            events += 1;
        }
        let correction = if answer.corrects_revision.is_some() {
            " (correction)"
        } else {
            ""
        };
        page.push(
            &format!("{}{correction} · {}", answer.actor, answer.text),
            RowStyle::Plain,
            false,
        );
        events += 1;
    }
    for route in record
        .routes
        .iter()
        .filter(|route| route.after_answers >= record.answers.len())
    {
        page.push(&route_text(route), RowStyle::Dim, false);
        events += 1;
    }
    if events == 0 {
        page.push("asked · no answer yet", RowStyle::Dim, false);
    }
    page.rows
}

fn route_text(route: &crate::domain::QuestionRoute) -> String {
    let to = match route.audience {
        Audience::User => "you",
        Audience::Lead => "lead",
    };
    format!("routed to {to}")
}

/// The footer key hint of the tab's current state.
pub fn footer_hint(state: &QuestionsState, wide: bool) -> &'static str {
    if state.answer.is_some() {
        return "enter save · esc cancel";
    }
    let Some(record) = state.page.and_then(|id| state.feed.get(id)) else {
        return if wide {
            "enter/→ open · j/k move · 1 board · ? keys"
        } else {
            "enter open · j/k move · 1 board · ? keys"
        };
    };
    if state.pending.is_some_and(|p| p.id() == record.reference.id) {
        return "enter confirm · J/K next · esc back";
    }
    if record.draft.audience.is_none() {
        return "J/K next · t task · esc back";
    }
    if record.reference.kind == QuestionKind::Approval && record.is_open() {
        return "space check · y approve · n reject · J/K next · esc back";
    }
    if state.choosing(record) {
        return "j/k choose · enter save · J/K next · t task · esc back";
    }
    if record.is_open() {
        return "enter answer · J/K next · t task · esc back";
    }
    if record.reference.kind == QuestionKind::Approval {
        return "t task · J/K next · esc back";
    }
    "c correct · t task · J/K next · esc back"
}

/// The key a footer entry names, or none for a pair that only moves (`j/k`).
fn footer_key(token: &str) -> Option<KeyCode> {
    match token {
        "enter" | "enter/→" => Some(KeyCode::Enter),
        "esc" => Some(KeyCode::Esc),
        "space" => Some(KeyCode::Char(' ')),
        // `J/K next`: a click steps forward.
        "J/K" => Some(KeyCode::Char('J')),
        _ => {
            let mut chars = token.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(KeyCode::Char(c)),
                _ => None,
            }
        }
    }
}

/// Map one key while tab 2 owns the keyboard.
pub fn map_key(state: &QuestionsState, wide: bool, key: KeyEvent) -> Option<BoardIntent> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let extra = mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
    let q = |intent| Some(BoardIntent::Question(intent));
    // Text entry owns every key: only Enter saves and Esc cancels, so no chord quits here.
    if state.answer.is_some() {
        return match key.code {
            KeyCode::Esc => q(QuestionIntent::AnswerCancel),
            KeyCode::Enter if !extra => q(QuestionIntent::Enter),
            KeyCode::Backspace => q(QuestionIntent::AnswerBackspace),
            KeyCode::Char(c) if !extra => q(QuestionIntent::AnswerInsert(c.to_string())),
            _ => None,
        };
    }
    if ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('c')) {
        return Some(BoardIntent::Quit);
    }
    if extra {
        return None;
    }
    match key.code {
        KeyCode::Char('1') => return Some(BoardIntent::SelectNavTab(NavTab::Board)),
        KeyCode::Char('2') => return Some(BoardIntent::SelectNavTab(NavTab::Questions)),
        KeyCode::Char('?') => return Some(BoardIntent::OpenHelp),
        _ => {}
    }
    if state.page.is_none() {
        return match key.code {
            KeyCode::Down | KeyCode::Char('j') => q(QuestionIntent::Move(true)),
            KeyCode::Up | KeyCode::Char('k') => q(QuestionIntent::Move(false)),
            KeyCode::Enter => q(QuestionIntent::Open),
            KeyCode::Right | KeyCode::Char('l') if wide => q(QuestionIntent::Open),
            // The tab's list is a top level, like the board's: Esc there exits.
            KeyCode::Esc => Some(BoardIntent::Quit),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Esc => q(QuestionIntent::Back),
        KeyCode::Left | KeyCode::Char('h') if wide => q(QuestionIntent::Back),
        KeyCode::Down | KeyCode::Char('j') => q(QuestionIntent::Move(true)),
        KeyCode::Up | KeyCode::Char('k') => q(QuestionIntent::Move(false)),
        KeyCode::PageDown => q(QuestionIntent::Scroll(true)),
        KeyCode::PageUp => q(QuestionIntent::Scroll(false)),
        KeyCode::Char('J') => q(QuestionIntent::Step(true)),
        KeyCode::Char('K') => q(QuestionIntent::Step(false)),
        KeyCode::Enter => q(QuestionIntent::Enter),
        KeyCode::Char(' ') => q(QuestionIntent::ToggleCheck),
        KeyCode::Char('y') => q(QuestionIntent::Approve),
        KeyCode::Char('n') => q(QuestionIntent::Reject),
        KeyCode::Char('c') => q(QuestionIntent::Correct),
        KeyCode::Char('t') => q(QuestionIntent::OpenTask),
        KeyCode::Char('r') => q(QuestionIntent::Resend),
        _ => None,
    }
}

/// The text the selector row paints for the two tabs.
pub fn tab_texts(feed: &QuestionFeed) -> (String, String) {
    let questions = match tab_count(feed) {
        Some(n) => format!(" 2 questions ·{n} "),
        None => " 2 questions ".to_string(),
    };
    (" 1 board ".to_string(), questions)
}

fn style(style: RowStyle) -> ratatui::style::Style {
    match style {
        RowStyle::Plain => render::style_plain(),
        RowStyle::Dim => render::style_dim(),
        RowStyle::Bold => render::style_bold(),
        RowStyle::Selected => render::style_reverse(),
    }
}

fn put(frame: &mut Frame<'_>, x: u16, y: u16, width: u16, text: &str, row_style: RowStyle) {
    if width == 0 {
        return;
    }
    let shown = crate::ui::present_line(text, width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(shown, style(row_style)))),
        Rect::new(x, y, width, 1),
    );
}

/// Inputs of one questions-tab frame.
pub struct QuestionsFrame<'a> {
    pub state: &'a QuestionsState,
    pub tasks: &'a [Task],
    pub message: Option<&'a str>,
}

/// List rows: (question id or none for headings, text, style).
fn list_rows(frame: &QuestionsFrame<'_>, width: usize, wide: bool) -> Vec<(Option<Uuid>, PageRow)> {
    let state = frame.state;
    let mut rows: Vec<(Option<Uuid>, PageRow)> = Vec::new();
    let note = |text: &str| {
        (
            None,
            PageRow {
                text: text.to_string(),
                style: RowStyle::Dim,
                focus: false,
                click: None,
                label: 0,
            },
        )
    };
    match &state.feed {
        QuestionFeed::Disabled => {
            for row in crate::ui::edit::wrap_text(
                "  questions live on an Agent board (an explicit TSK_STATE_DIR) · this personal board records no Agent questions, findings, or Threads",
                width.max(1),
            ) {
                rows.push(note(&row.text));
            }
            return rows;
        }
        QuestionFeed::Unavailable(reason) => {
            for row in crate::ui::edit::wrap_text(
                &format!("  questions cannot be read: {reason} · nothing is shown as zero"),
                width.max(1),
            ) {
                rows.push(note(&row.text));
            }
            return rows;
        }
        QuestionFeed::Loaded(records) if records.is_empty() => {
            rows.push(note("  no questions"));
            return rows;
        }
        QuestionFeed::Loaded(_) => {}
    }
    for (group, members) in grouped(state.records()) {
        rows.push((
            None,
            PageRow {
                text: String::new(),
                style: RowStyle::Plain,
                focus: false,
                click: None,
                label: 0,
            },
        ));
        rows.push((
            None,
            PageRow {
                text: format!(" {} {}", group.title(), members.len()),
                style: if group == QuestionGroup::Unrouted {
                    RowStyle::Dim
                } else {
                    RowStyle::Bold
                },
                focus: false,
                click: None,
                label: 0,
            },
        ));
        for record in members {
            let id = record.reference.id;
            let selected = state.cursor == Some(id);
            let marker = if selected { " ▸ " } else { "   " };
            let meta = match group {
                QuestionGroup::Closed => format!(" · {}", state_label(record, frame.tasks)),
                QuestionGroup::Unrouted => " · target unknown".to_string(),
                _ => String::new(),
            };
            let lead = format!("{marker}Q{} ", record.reference.number);
            let lead_w = render::display_width(&lead);
            let text = format!("{}{meta}", record.reference.summary);
            let room = width.saturating_sub(lead_w).max(1);
            let focused_list = state.page.is_none() || !wide;
            for (index, row) in crate::ui::edit::wrap_text(&text, room).iter().enumerate() {
                let prefix = if index == 0 {
                    lead.clone()
                } else {
                    " ".repeat(lead_w)
                };
                rows.push((
                    Some(id),
                    PageRow {
                        text: format!("{prefix}{}", row.text),
                        style: if selected && focused_list {
                            RowStyle::Bold
                        } else {
                            RowStyle::Plain
                        },
                        focus: selected && index == 0,
                        click: None,
                        label: 0,
                    },
                ));
            }
        }
    }
    rows
}

/// The first painted row: `scroll` clamped to the rows, and with `follow` moved just enough
/// that the focus row shows above the `↓ more rows` marker instead of under it.
fn page_scroll(
    rows: &[(Option<Uuid>, PageRow)],
    height: usize,
    scroll: usize,
    follow: bool,
) -> usize {
    let max_scroll = rows.len().saturating_sub(height);
    let mut scroll = scroll.min(max_scroll);
    if !follow {
        return scroll;
    }
    if let Some(focus) = rows.iter().position(|(_, row)| row.focus) {
        if focus < scroll {
            scroll = focus;
        } else if focus >= scroll + height {
            scroll = focus + 1 - height;
        }
        // The last row turns into the marker while more rows follow it.
        if height > 1 && focus + 1 == scroll + height && rows.len() > scroll + height {
            scroll = (scroll + 1).min(max_scroll);
        }
    }
    scroll
}

/// Paint `rows` into `rect` from `scroll` (see [`page_scroll`]); returns the first painted
/// row. `hint` names the keys that scroll this page.
fn paint_rows(
    out: &mut Frame<'_>,
    rect: Rect,
    rows: &[(Option<Uuid>, PageRow)],
    (scroll, follow): (usize, bool),
    hint: &str,
    hits: &mut QueueHitMap,
) -> usize {
    let height = rect.height as usize;
    if height == 0 {
        return 0;
    }
    let scroll = page_scroll(rows, height, scroll, follow);
    let more_below = rows.len() > scroll + height;
    for (offset, (id, row)) in rows.iter().skip(scroll).take(height).enumerate() {
        let y = rect.y + offset as u16;
        let last = offset + 1 == height;
        if last && more_below {
            put(
                out,
                rect.x,
                y,
                rect.width,
                &format!(
                    "  ↓ {} more rows · {hint}",
                    rows.len() - scroll - height + 1
                ),
                RowStyle::Dim,
            );
            break;
        }
        if row.label > 0 && row.label <= row.text.len() {
            let (label, value) = row.text.split_at(row.label);
            let label_w = (render::display_width(label) as u16).min(rect.width);
            put(out, rect.x, y, label_w, label, RowStyle::Dim);
            put(
                out,
                rect.x + label_w,
                y,
                rect.width - label_w,
                value,
                row.style,
            );
        } else {
            put(out, rect.x, y, rect.width, &row.text, row.style);
        }
        if let Some(id) = id {
            hits.push(
                QueueHitTarget::Question(*id),
                Rect::new(rect.x, y, rect.width, 1),
            );
            hits.push_copyable(Rect::new(rect.x, y, rect.width, 1));
        } else if !row.text.is_empty() {
            if let Some(click) = row.click {
                hits.push(click, Rect::new(rect.x, y, rect.width, 1));
            }
            hits.push_copyable(Rect::new(rect.x, y, rect.width, 1));
        }
    }
    scroll
}

/// A question page keeps one blank column on its left, as the list's rows do.
fn page_gutter(rect: Rect) -> Rect {
    if rect.width > 1 {
        Rect::new(rect.x + 1, rect.y, rect.width - 1, rect.height)
    } else {
        rect
    }
}

/// The keys that scroll a page: `j`/`k` unless they choose an option or edit an answer.
fn scroll_hint(state: &QuestionsState, record: &QuestionRecord) -> &'static str {
    if state.answer.is_none() && !state.choosing(record) {
        "j/k"
    } else {
        "pgup/pgdn"
    }
}

/// Paint the questions tab into `area`: one screen below 110 columns, list and detail side
/// by side at 110 and up. Returns the hit map the mouse router reads.
pub fn draw(out: &mut Frame<'_>, frame: &QuestionsFrame<'_>, area: Rect) -> QueueHitMap {
    let mut hits = QueueHitMap::default();
    let width = area.width;
    let height = area.height;
    if width == 0 || height == 0 {
        return hits;
    }
    out.render_widget(Paragraph::new(Line::from("")), area);
    let geo = tier::resolve(width, height);
    let state = frame.state;
    let wide = width >= tier::WIDE_SPLIT_MIN_WIDTH;

    // Tabs.
    if let Some(row) = geo.selector_row {
        let (board, questions) = tab_texts(&state.feed);
        let y = area.y + row;
        let board_w = render::display_width(&board) as u16;
        put(out, area.x, y, board_w.min(width), &board, RowStyle::Dim);
        hits.push(
            QueueHitTarget::NavTab(NavTab::Board),
            Rect::new(area.x, y, board_w.min(width), 1),
        );
        let sep_x = area.x + board_w.min(width);
        put(
            out,
            sep_x,
            y,
            3.min(width.saturating_sub(board_w)),
            " · ",
            RowStyle::Dim,
        );
        let q_x = sep_x + 3;
        let q_w =
            (render::display_width(&questions) as u16).min(width.saturating_sub(q_x - area.x));
        out.render_widget(
            Paragraph::new(Line::from(Span::styled(
                crate::ui::present_line(&questions, q_w as usize),
                render::style_heading(),
            ))),
            Rect::new(q_x, y, q_w, 1),
        );
        hits.push(
            QueueHitTarget::NavTab(NavTab::Questions),
            Rect::new(q_x, y, q_w, 1),
        );
    }

    let body_top = area.y + geo.viewport_top;
    let body_h = geo.viewport_height;
    let shown = state.shown(wide).cloned();
    if wide {
        let split = tier::resolve_responsive(width, height, tier::WideStage::Board);
        let list_rect = Rect::new(area.x + split.board.x, body_top, split.board.width, body_h);
        let rows = list_rows(frame, list_rect.width as usize, true);
        paint_rows(out, list_rect, &rows, (0, true), "j/k", &mut hits);
        if split.rule.width > 0 {
            for y in 0..body_h {
                put(
                    out,
                    area.x + split.rule.x,
                    body_top + y,
                    1,
                    "│",
                    RowStyle::Dim,
                );
            }
        }
        let detail = split.task_content();
        let detail_rect = Rect::new(
            area.x + detail.x,
            area.y,
            detail.width,
            geo.viewport_top + body_h,
        );
        if let Some(record) = shown.as_ref() {
            // The detail column starts under the tab row.
            let rect = page_gutter(Rect::new(
                detail_rect.x,
                body_top,
                detail_rect.width,
                body_h,
            ));
            let page: Vec<(Option<Uuid>, PageRow)> =
                page_rows(state, record, frame.tasks, rect.width as usize)
                    .into_iter()
                    .map(|row| (None, row))
                    .collect();
            let at = (state.scroll.get(), state.follow.get());
            let scroll = paint_rows(out, rect, &page, at, scroll_hint(state, record), &mut hits);
            state.scroll.set(scroll);
        } else {
            put(
                out,
                detail_rect.x,
                body_top + 1,
                detail_rect.width,
                "pick a question on the left",
                RowStyle::Dim,
            );
        }
    } else if let Some(record) = shown.as_ref() {
        let rect = page_gutter(Rect::new(area.x, body_top, width, body_h));
        let page: Vec<(Option<Uuid>, PageRow)> =
            page_rows(state, record, frame.tasks, rect.width as usize)
                .into_iter()
                .map(|row| (None, row))
                .collect();
        let at = (state.scroll.get(), state.follow.get());
        let scroll = paint_rows(out, rect, &page, at, scroll_hint(state, record), &mut hits);
        state.scroll.set(scroll);
    } else {
        let rect = Rect::new(area.x, body_top, width, body_h);
        let rows = list_rows(frame, width as usize, false);
        paint_rows(out, rect, &rows, (0, true), "j/k", &mut hits);
    }

    // Footer: rule, status, keys.
    if let Some(row) = geo.rule_row {
        put(
            out,
            area.x,
            area.y + row,
            width,
            &"─".repeat(width as usize),
            RowStyle::Dim,
        );
    }
    if let Some(row) = geo.status_row {
        let status = frame
            .message
            .map(str::to_string)
            .unwrap_or_else(|| match &state.feed {
                QuestionFeed::Disabled => "questions · personal board".to_string(),
                QuestionFeed::Unavailable(_) => "questions · cannot be read".to_string(),
                QuestionFeed::Loaded(records) => {
                    let counts = waiting(records);
                    format!(
                        "questions · {} waiting on you · {} total",
                        counts.for_you(),
                        records.len()
                    )
                }
            });
        put(
            out,
            area.x,
            area.y + row,
            width,
            &format!(" {status}"),
            RowStyle::Plain,
        );
    }
    if let Some(row) = geo.verb_row {
        let y = area.y + row;
        let hint = footer_hint(state, wide);
        put(out, area.x, y, width, &format!(" {hint}"), RowStyle::Dim);
        // Each `key label` entry is a button that maps as its key does.
        let mut x = area.x + 1;
        for (index, entry) in hint.split(" · ").enumerate() {
            if index > 0 {
                x = x.saturating_add(3);
            }
            let entry_w = render::display_width(entry) as u16;
            let end = (area.x + width).min(x.saturating_add(entry_w));
            if let Some(code) = entry.split(' ').next().and_then(footer_key) {
                if end > x {
                    hits.push(
                        QueueHitTarget::QuestionKey(code),
                        Rect::new(x, y, end - x, 1),
                    );
                }
            }
            x = x.saturating_add(entry_w);
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(count: usize, focus: usize) -> Vec<(Option<Uuid>, PageRow)> {
        (0..count)
            .map(|index| {
                (
                    None,
                    PageRow {
                        text: format!("row {index}"),
                        style: RowStyle::Plain,
                        focus: index == focus,
                        click: None,
                        label: 0,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn explicit_scroll_reads_above_the_focus_and_follow_clears_the_marker() {
        // 40 rows, a 10-row view, the check on row 30.
        let page = rows(40, 30);
        // A wheel or PgUp scroll is not pulled back to the focus.
        assert_eq!(page_scroll(&page, 10, 5, false), 5);
        assert_eq!(page_scroll(&page, 10, 0, false), 0);
        // A selection change brings the focus into view above the `↓ more rows` marker,
        // never on the last row that the marker paints over.
        let shown = page_scroll(&page, 10, 0, true);
        assert!(
            shown <= 30 && 30 < shown + 9,
            "focus row 30 at scroll {shown}"
        );
        // Explicit scrolls past the end still clamp to the last page.
        assert_eq!(page_scroll(&page, 10, 99, false), 30);
    }
}
