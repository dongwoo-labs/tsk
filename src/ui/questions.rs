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

/// The approval terms the page showed when it opened, bound to their exact revision.
/// Picking **Approve these exact terms** saves only while the snapshot and a fresh read of
/// the record both still match them; a mismatch refuses and rebinds to the terms now shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCheck {
    pub id: Uuid,
    pub revision: Uuid,
    pub terms: ApprovalTerms,
}

/// A free-text answer being typed for a question without options, a secret's value, or an
/// approval's comment.
#[derive(Clone, PartialEq, Eq)]
pub struct AnswerInput {
    pub id: Uuid,
    pub revision: Uuid,
    pub corrects: Option<Uuid>,
    pub text: String,
    /// A secret question's value: painted as `•`, handed only to fnox, wiped on drop.
    pub secret: bool,
    /// Free text beside an approval's decision: `Enter` or `Esc` stages it for the next
    /// approve or reject pick, it is never itself an answer and never saved on its own.
    pub comment: bool,
    /// Whether the field owns the keyboard. `Esc` leaves a typed answer's field and keeps the
    /// draft on the page; secret and comment inputs exist only while focused.
    pub focused: bool,
}

impl AnswerInput {
    /// An empty masked input for a secret question at `revision`.
    pub fn secret(id: Uuid, revision: Uuid) -> Self {
        Self {
            id,
            revision,
            corrects: None,
            // Room up front, so typing rarely reallocates and leaves copies behind.
            text: String::with_capacity(256),
            secret: true,
            comment: false,
            focused: true,
        }
    }

    /// An empty, focused answer field for a question at `revision`; `corrects` names the
    /// answer revision a correction replaces.
    pub fn typed(id: Uuid, revision: Uuid, corrects: Option<Uuid>) -> Self {
        Self {
            id,
            revision,
            corrects,
            text: String::new(),
            secret: false,
            comment: false,
            focused: true,
        }
    }

    /// A comment input for an open approval at `revision`, pre-filled with any text
    /// already staged for it.
    pub fn comment(id: Uuid, revision: Uuid, text: String) -> Self {
        Self {
            id,
            revision,
            corrects: None,
            text,
            secret: false,
            comment: true,
            focused: true,
        }
    }
}

impl std::fmt::Debug for AnswerInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text: &dyn std::fmt::Debug = if self.secret { &"<secret>" } else { &self.text };
        f.debug_struct("AnswerInput")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("corrects", &self.corrects)
            .field("text", text)
            .field("secret", &self.secret)
            .field("comment", &self.comment)
            .field("focused", &self.focused)
            .finish()
    }
}

impl Drop for AnswerInput {
    fn drop(&mut self) {
        if self.secret {
            crate::fnox::wipe(&mut self.text);
        }
    }
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
    /// Hash of the comment staged with this attempt, if any; `None` when none was staged.
    pub comment_hash: Option<u64>,
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
    /// The highlighted row of each record's answer form, by index into [`form`].
    pub highlight: BTreeMap<Uuid, usize>,
    /// The approval terms the open page showed (see [`ApprovalCheck`]).
    pub seen: Option<ApprovalCheck>,
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
    /// Whether the next paint scrolls the focus row (highlighted choice, input) into view.
    /// A highlight change sets it; an explicit scroll (wheel, `PgUp`/`PgDn`, `j`/`k` on a
    /// page without choices) clears it so rows above or below the focus stay readable.
    pub follow: Cell<bool>,
    /// The last save's hint outcome for one question, shown on its page.
    pub report: Option<(Uuid, String)>,
    /// Whether the CLOSED group lists its records. Folded by default: answered records
    /// leave the walk of `j`/`k` and `J`/`K` until `z` (or a header click) unfolds them.
    pub closed_open: bool,
    /// The pending save's list slot from before it, so its confirmation keeps that place.
    pub pending_slot: Option<usize>,
    /// Comment text staged for an open approval, by question id: typed on the form's
    /// comment row (or `m`), kept until approve or reject saves it beside the decision or
    /// the record closes.
    pub comments: BTreeMap<Uuid, String>,
}

/// One questions-tab action. Pure moves apply on [`QuestionsState`]; the rest go through
/// the app boundary, which owns the store and the Herdr hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionIntent {
    /// `j`/`k`/arrows: the list cursor, the highlighted choice, or the page scroll.
    Move(bool),
    /// The wheel: the list cursor, or the page scroll (never the highlight).
    Scroll(bool),
    /// `Enter`/`l`/`→` on the list.
    Open,
    /// `Esc`/`h`/`←` on a page.
    Back,
    /// `J`/`K`: the next or previous question in the list's own order.
    Step(bool),
    /// `Enter` on a page: pick the highlighted choice, save a typed answer, or confirm a
    /// pending save.
    Enter,
    /// `1`-`9`, or a click on a choice: pick that row of the form at once.
    Pick(usize),
    /// `c`: correct the current answer as a new revision.
    Correct,
    /// `m`: the approval form's comment row, as picking it does.
    Comment,
    /// `t`: open the question's task.
    OpenTask,
    /// `r`: resend the run request of a finding decided as create-and-run.
    Resend,
    /// `z` on the list, or a click on the CLOSED header: fold or unfold answered records.
    ToggleClosed,
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
    /// Pick this row of the form (index into [`form`]) at once, as its digit does.
    Choice(usize),
    /// The pending-save line, as `Enter` does.
    Enter,
    /// The task line, as `t` does.
    OpenTask,
    /// The `c corrects` line.
    Correct,
    /// The `r resends` line.
    Resend,
}

/// One row of a record's answer form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The question's option at this index.
    Option(usize),
    /// The free-text row (`Type something…`), or the lone field of a question without
    /// options or a secret.
    Text,
    /// Approve these exact terms.
    Approve,
    /// Reject the approval.
    Reject,
    /// The comment beside an approval's decision.
    Comment,
    /// The finding action at this index into [`FINDING_ACTIONS`].
    Finding(usize),
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
            QuestionKind::Question | QuestionKind::Secret => QuestionGroup::ForYou,
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
        QuestionKind::Secret if answer.text == crate::store::SECRET_REJECTED => {
            "rejected".to_string()
        }
        QuestionKind::Secret => "set in fnox".to_string(),
    }
}

pub const FINDING_ACTIONS: [(FindingAction, &str); 3] = [
    (FindingAction::CreateAndRun, "create the task and run it"),
    (FindingAction::CreateOnly, "create the task only (todo)"),
    (FindingAction::Close, "close · no task"),
];

/// What the finding form's rows read; the saved answer text stays [`FINDING_ACTIONS`].
const FINDING_LABELS: [&str; 3] = [
    "Create the task and run it",
    "Create the task only",
    "Close",
];

/// The record's answer form: every row a pick can submit, in paint order. Empty when the
/// record is read-only (answered and not under correction, lead-addressed, or with no
/// target). A question without options has one row, its lone text field; a secret has its
/// masked field plus a reject row that closes it with no value.
pub fn form(state: &QuestionsState, record: &QuestionRecord) -> Vec<Choice> {
    if record.draft.audience.is_none() {
        return Vec::new();
    }
    let correcting = state
        .correcting
        .is_some_and(|(id, _)| id == record.reference.id);
    let open = record.is_open();
    match record.reference.kind {
        QuestionKind::Approval if open => vec![Choice::Approve, Choice::Reject, Choice::Comment],
        QuestionKind::Secret if open => vec![Choice::Text, Choice::Reject],
        QuestionKind::Finding if open || correcting => {
            (0..FINDING_ACTIONS.len()).map(Choice::Finding).collect()
        }
        QuestionKind::Question if open || correcting => (0..record.draft.options.len())
            .map(Choice::Option)
            .chain(std::iter::once(Choice::Text))
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether the form is one text field and nothing else (no numbered choices).
pub fn lone_field(form: &[Choice]) -> bool {
    form == [Choice::Text]
}

/// The form row the page starts on: the recommendation when it names an option.
fn default_highlight(record: &QuestionRecord) -> usize {
    if record.reference.kind != QuestionKind::Question {
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

    /// The listed rows in paint order; a folded CLOSED group lists none of its records.
    pub fn order(&self) -> Vec<Uuid> {
        if self.closed_open {
            return list_order(self.records());
        }
        self.waiting_order()
    }

    /// The listed rows outside CLOSED, in list order.
    fn waiting_order(&self) -> Vec<Uuid> {
        grouped(self.records())
            .into_iter()
            .filter(|(group, _)| *group != QuestionGroup::Closed)
            .flat_map(|(_, members)| members.into_iter().map(|record| record.reference.id))
            .collect()
    }

    /// Fold or unfold the CLOSED group; the cursor never rests on a hidden row.
    pub fn toggle_closed(&mut self) {
        self.closed_open = !self.closed_open;
        self.reanchor();
    }

    /// After a save lands: back to the list, the cursor on the row that now holds the
    /// answered record's slot (the next one waiting), or the task page it came from.
    /// `slot` is the record's index in the list before the save.
    pub fn back_to_list(&mut self, id: Uuid, slot: Option<usize>) {
        self.page = None;
        self.clear_page_state();
        if self.return_task.take().is_some() {
            // Opened from a task page: the answer done, that page is where the user was.
            self.active = false;
        }
        // A record that just closed gives its place to the waiting row now in its slot,
        // even when CLOSED is unfolded and the record is still listed below.
        let waiting = self.waiting_order();
        self.cursor = if waiting.contains(&id) {
            Some(id)
        } else {
            slot.and_then(|slot| waiting.get(slot.min(waiting.len().saturating_sub(1))))
                .copied()
                .or_else(|| self.order().first().copied())
        };
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
                self.seen = None;
            }
        }
        self.reset_stale_check();
    }

    /// Replace the snapshot. The terms the page showed survive it, so a pick of Approve
    /// after the terms changed refuses instead of approving terms the user never read.
    pub fn set_feed(&mut self, feed: QuestionFeed) {
        self.feed = feed;
        self.reanchor();
    }

    fn reset_stale_check(&mut self) {
        let keep = self.seen.as_ref().is_some_and(|seen| {
            self.page == Some(seen.id) && self.feed.get(seen.id).is_some_and(|r| r.is_open())
        });
        if !keep {
            self.seen = None;
        }
        let keep_correction = self.correcting.is_some_and(|(id, revision)| {
            self.feed
                .get(id)
                .is_some_and(|record| record.reference.revision == revision)
        });
        if !keep_correction {
            self.correcting = None;
        }
        // A staged comment outlives a reload only while its approval is still open: once
        // decided or moved on, it is either saved already or no longer this record's wait.
        self.comments
            .retain(|id, _| self.feed.get(*id).is_some_and(QuestionRecord::is_open));
    }

    /// Whether quitting would lose a typed answer, a staged comment, or an unresolved save.
    pub fn has_unsaved_work(&self) -> bool {
        self.pending.is_some()
            || self
                .answer
                .as_ref()
                .is_some_and(|input| !input.text.is_empty())
            || !self.comments.is_empty()
    }

    /// The typed answer opening `id` would discard: text in progress on another question.
    /// An empty field is no work and never blocks.
    pub fn answer_elsewhere(&self, id: Uuid) -> Option<Uuid> {
        self.answer
            .as_ref()
            .filter(|input| input.id != id && !input.text.is_empty())
            .map(|input| input.id)
    }

    /// The text field that owns the keyboard: a focused input on the open page.
    pub fn typing(&self) -> Option<&AnswerInput> {
        self.answer
            .as_ref()
            .filter(|input| input.focused && self.page == Some(input.id))
    }

    /// The highlighted row of the record's form (index into [`form`]).
    pub fn highlighted(&self, record: &QuestionRecord) -> usize {
        let count = form(self, record).len();
        self.highlight
            .get(&record.reference.id)
            .copied()
            .unwrap_or_else(|| default_highlight(record))
            .min(count.saturating_sub(1))
    }

    /// The highlighted row's choice on the open page, if it has a form.
    pub fn highlighted_choice(&self) -> Option<Choice> {
        let record = self.page.and_then(|id| self.feed.get(id))?;
        form(self, record).get(self.highlighted(record)).copied()
    }

    fn clear_page_state(&mut self) {
        self.seen = None;
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
        let fresh = self.page != Some(id);
        if fresh {
            self.clear_page_state();
        }
        self.cursor = Some(id);
        self.page = Some(id);
        if fresh {
            self.arm(id);
        }
    }

    /// A page just opened: bind an approval to the terms it shows, and focus the lone field
    /// of a question without options or a secret, as the answer form opens.
    fn arm(&mut self, id: Uuid) {
        let Some(record) = self.feed.get(id) else {
            return;
        };
        if record.draft.audience.is_none() || !record.is_open() {
            return;
        }
        let revision = record.reference.revision;
        match record.reference.kind {
            QuestionKind::Approval => {
                self.seen = record.draft.approval.clone().map(|terms| ApprovalCheck {
                    id,
                    revision,
                    terms,
                });
            }
            QuestionKind::Secret => self.answer = Some(AnswerInput::secret(id, revision)),
            QuestionKind::Question if record.draft.options.is_empty() => {
                self.answer = Some(AnswerInput::typed(id, revision, None));
            }
            _ => {}
        }
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

    /// `j`/`k` on a page: move the highlighted choice, else scroll.
    pub fn move_on_page(&mut self, forward: bool) {
        let Some(record) = self.page.and_then(|id| self.feed.get(id)).cloned() else {
            return;
        };
        let rows = form(self, &record);
        if !rows.is_empty() && !lone_field(&rows) {
            let current = self.highlighted(&record);
            let next = if forward {
                (current + 1).min(rows.len() - 1)
            } else {
                current.saturating_sub(1)
            };
            self.highlight.insert(record.reference.id, next);
            self.follow.set(true);
            return;
        }
        self.scroll_page(forward);
    }

    /// Whether `seen` still names exactly the terms and revision `record` shows.
    pub fn seen_matches(&self, record: &QuestionRecord) -> bool {
        self.seen.as_ref().is_some_and(|seen| {
            seen.id == record.reference.id
                && seen.revision == record.reference.revision
                && record.draft.approval.as_ref() == Some(&seen.terms)
        })
    }

    /// Bind the approval to the terms `record` shows now (after a refusal showed them).
    pub fn rebind_seen(&mut self, record: &QuestionRecord) {
        self.seen = record.draft.approval.clone().map(|terms| ApprovalCheck {
            id: record.reference.id,
            revision: record.reference.revision,
            terms,
        });
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
        QuestionKind::Secret => "secret".to_string(),
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
    if let Some(target) = draft.secret.as_ref() {
        // Where the value goes. The value itself is never stored, so never shown.
        page.field("key     ", &target.key, RowStyle::Bold);
        page.field(
            "profile ",
            target.profile.as_deref().unwrap_or("default"),
            RowStyle::Plain,
        );
        page.field(
            "config  ",
            target.config.as_deref().unwrap_or("global"),
            RowStyle::Plain,
        );
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
    }

    let rows = form(state, record);
    let typing = state
        .answer
        .as_ref()
        .filter(|input| input.id == record.reference.id);
    let correcting = state
        .correcting
        .is_some_and(|(id, _)| id == record.reference.id);
    if lone_field(&rows) || (rows.is_empty() && typing.is_some_and(|input| !input.comment)) {
        // A draft refused because the record moved on stays visible until `Esc` drops it.
        form_field(&mut page, record, typing, correcting);
    } else if !rows.is_empty() {
        form_choices(&mut page, state, record, &rows, typing, correcting);
    } else if record.draft.audience.is_none() {
        page.blank();
        page.push(
            "target unknown · route it with tsk route before anyone answers or corrects here",
            RowStyle::Dim,
            false,
        );
    }

    if let Some(answer) = record.current_answer() {
        page.blank();
        page.field("answer  ", &answer.text, RowStyle::Plain);
        if let Some(comment) = answer.comment.as_deref() {
            page.field("comment ", comment, RowStyle::Plain);
        }
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
        if !matches!(
            record.reference.kind,
            QuestionKind::Approval | QuestionKind::Secret
        ) && state.correcting.is_none()
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
        let comment = answer
            .comment
            .as_deref()
            .map(|text| format!(" · {text}"))
            .unwrap_or_default();
        page.push(
            &format!("{}{correction} · {}{comment}", answer.actor, answer.text),
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

/// The lone text field of a question without options or a secret.
fn form_field(
    page: &mut PageBuilder,
    record: &QuestionRecord,
    input: Option<&AnswerInput>,
    correcting: bool,
) {
    let secret = record.reference.kind == QuestionKind::Secret;
    let lead = if secret { "secret  " } else { "answer  " };
    page.blank();
    let at = page.rows.len();
    match input.filter(|input| !input.comment) {
        Some(input) if input.secret => {
            // One `•` per character typed: the length shows, the value never does.
            let mask = "•".repeat(input.text.chars().count());
            page.indented(lead, &format!("{mask}▏"), RowStyle::Selected, true);
        }
        Some(input) if input.focused => {
            page.indented(lead, &format!("{}▏", input.text), RowStyle::Selected, true);
        }
        Some(input) if !input.text.is_empty() => {
            page.indented(lead, &input.text, RowStyle::Bold, true);
        }
        _ if secret => page.indented(lead, "Type the value…", RowStyle::Dim, true),
        _ => page.indented(lead, "Type something…", RowStyle::Dim, true),
    }
    page.click_from(at, PageClick::Choice(0));
    let focused = input.is_some_and(|input| input.focused);
    page.push(
        match (secret, focused, correcting) {
            (true, true, _) => "enter hands it to fnox set · esc cancels and wipes it",
            (true, false, _) => {
                "enter opens the field · tsk hands it to fnox set, never stores or shows it"
            }
            (false, true, true) => {
                "enter saves the correction · the earlier answer stays in history · esc leaves the field"
            }
            (false, true, false) => "enter saves · esc leaves the field, the draft stays",
            (false, false, _) => "enter opens the field",
        },
        RowStyle::Dim,
        false,
    );
}

/// The numbered choices of a question with options, an approval, or a finding.
fn form_choices(
    page: &mut PageBuilder,
    state: &QuestionsState,
    record: &QuestionRecord,
    rows: &[Choice],
    input: Option<&AnswerInput>,
    correcting: bool,
) {
    let highlighted = state.highlighted(record);
    let draft = &record.draft;
    page.blank();
    page.push(
        match record.reference.kind {
            QuestionKind::Approval | QuestionKind::Finding => "decide",
            _ => "options",
        },
        RowStyle::Dim,
        false,
    );
    for (index, choice) in rows.iter().enumerate() {
        let on = index == highlighted;
        let mut style = if on {
            RowStyle::Selected
        } else {
            RowStyle::Plain
        };
        let text = match *choice {
            Choice::Option(at) => {
                let label = draft.options.get(at).cloned().unwrap_or_default();
                let recommended = draft
                    .recommendation
                    .as_deref()
                    .is_some_and(|rec| rec.trim() == label.trim());
                if recommended {
                    format!("{label} · recommended")
                } else {
                    label
                }
            }
            Choice::Text if record.reference.kind == QuestionKind::Secret => {
                match input.filter(|input| input.secret) {
                    Some(input) => {
                        style = RowStyle::Selected;
                        // One `•` per character typed: the length shows, the value never does.
                        format!("{}▏", "•".repeat(input.text.chars().count()))
                    }
                    None => "Type the value…".to_string(),
                }
            }
            Choice::Text => match input.filter(|input| !input.comment && !input.secret) {
                Some(input) if input.focused => {
                    style = RowStyle::Selected;
                    format!("{}▏", input.text)
                }
                Some(input) if !input.text.is_empty() => input.text.clone(),
                _ => "Type something…".to_string(),
            },
            Choice::Approve => "Approve these exact terms".to_string(),
            Choice::Reject => "Reject".to_string(),
            Choice::Comment => match input.filter(|input| input.comment) {
                Some(input) => {
                    style = RowStyle::Selected;
                    format!("{}▏", input.text)
                }
                None => match state.comments.get(&record.reference.id) {
                    Some(text) => format!("Comment · {text}"),
                    None => "Add a comment…".to_string(),
                },
            },
            Choice::Finding(at) => FINDING_LABELS[at].to_string(),
        };
        let marker = if on { "▸ " } else { "  " };
        let at = page.rows.len();
        page.indented(&format!("{marker}{}. ", index + 1), &text, style, on);
        page.click_from(at, PageClick::Choice(index));
    }
    let n = rows.len();
    let hint = match (input.map(|input| (input.focused, input.comment)), correcting) {
        (Some((true, true)), _) => {
            "enter or esc keeps the comment · it saves with approve or reject, either way"
                .to_string()
        }
        (Some((true, false)), true) => {
            "enter saves the correction · the earlier answer stays in history · esc leaves the field"
                .to_string()
        }
        (Some((true, false)), false) => {
            "enter saves · esc leaves the field, the draft stays".to_string()
        }
        (_, true) => format!(
            "enter or 1-{n} saves the correction · the earlier answer stays in history"
        ),
        _ if record.reference.kind == QuestionKind::Approval => {
            "1 approves exactly these terms · a comment rides approve or reject · no answer keeps waiting"
                .to_string()
        }
        _ => format!("enter or 1-{n} picks at once"),
    };
    page.push(&hint, RowStyle::Dim, false);
}

fn route_text(route: &crate::domain::QuestionRoute) -> String {
    let to = match route.audience {
        Audience::User => "you",
        Audience::Lead => "lead",
    };
    format!("routed to {to}")
}

/// The footer key hint of the tab's current state.
pub fn footer_hint(state: &QuestionsState, wide: bool) -> String {
    if let Some(input) = state.typing() {
        return if input.secret {
            "enter fnox set · esc cancel"
        } else if input.comment {
            "enter keep · esc keep"
        } else {
            "enter save · esc leave"
        }
        .to_string();
    }
    let Some(record) = state.page.and_then(|id| state.feed.get(id)) else {
        return if wide {
            "enter/→ open · j/k move · z closed · 1 board · ? keys"
        } else {
            "enter open · j/k move · z closed · 1 board · ? keys"
        }
        .to_string();
    };
    if state.pending.is_some_and(|p| p.id() == record.reference.id) {
        return "enter confirm · J/K next · esc back".to_string();
    }
    if record.draft.audience.is_none() {
        return "J/K next · t task · esc back".to_string();
    }
    let rows = form(state, record);
    if lone_field(&rows) {
        return "enter type · J/K next · t task · esc back".to_string();
    }
    if !rows.is_empty() {
        let n = rows.len();
        // On the text row letters type, so `t` is text there and the footer drops it.
        if rows.get(state.highlighted(record)) == Some(&Choice::Text) {
            return format!("↑↓ move · enter type · 1-{n} pick · J/K next · esc back");
        }
        return format!("↑↓ move · enter pick · 1-{n} pick · J/K next · t task · esc back");
    }
    if matches!(
        record.reference.kind,
        QuestionKind::Approval | QuestionKind::Secret
    ) {
        return "t task · J/K next · esc back".to_string();
    }
    "c correct · t task · J/K next · esc back".to_string()
}

/// The key a footer entry names, or none for a pair that only moves (`j/k`).
fn footer_key(token: &str) -> Option<KeyCode> {
    match token {
        "enter" | "enter/→" => Some(KeyCode::Enter),
        "esc" => Some(KeyCode::Esc),
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
    // Text entry owns every key: only Enter saves and Esc leaves, so no chord quits here.
    if state.typing().is_some() {
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
    // The open page's answer form: a digit picks its row at once, and on the highlighted
    // `Type something…` row a typed character opens the field with it. A lone field left
    // with `Esc` reopens on `Enter`, so its page keys (`t`, `J`/`K`) keep their meaning.
    if let (Some(record), KeyCode::Char(c)) =
        (state.page.and_then(|id| state.feed.get(id)), key.code)
    {
        let rows = form(state, record);
        let lone = lone_field(&rows);
        if let Some(digit) = c.to_digit(10).filter(|d| (1..=9).contains(d)) {
            let index = digit as usize - 1;
            if !lone && index < rows.len() {
                return q(QuestionIntent::Pick(index));
            }
        }
        let on_text = !lone && rows.get(state.highlighted(record)) == Some(&Choice::Text);
        let moves = matches!(c, 'j' | 'k' | 'J' | 'K');
        if on_text && !moves && !c.is_ascii_digit() {
            return q(QuestionIntent::AnswerInsert(c.to_string()));
        }
    }
    match key.code {
        KeyCode::Char('1') => return Some(BoardIntent::SelectNavTab(NavTab::Board)),
        KeyCode::Char('2') => return Some(BoardIntent::SelectNavTab(NavTab::Questions)),
        KeyCode::Char('?') => return Some(BoardIntent::OpenHelp),
        KeyCode::Char('z') => return q(QuestionIntent::ToggleClosed),
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
        KeyCode::Char('c') => q(QuestionIntent::Correct),
        KeyCode::Char('m') => q(QuestionIntent::Comment),
        KeyCode::Char('t') => q(QuestionIntent::OpenTask),
        KeyCode::Char('r') => q(QuestionIntent::Resend),
        _ => None,
    }
}

/// Where a paste goes on the tab: `Some(secret)` when a field takes it (the focused one,
/// or the open page's highlighted text row, which a paste opens), `None` otherwise.
pub fn paste_target(state: &QuestionsState) -> Option<bool> {
    if let Some(input) = state.typing() {
        return Some(input.secret);
    }
    let record = state.page.and_then(|id| state.feed.get(id))?;
    (state.highlighted_choice() == Some(Choice::Text))
        .then_some(record.reference.kind == QuestionKind::Secret)
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
        let folded = group == QuestionGroup::Closed && !state.closed_open;
        let header = match (group, folded) {
            (QuestionGroup::Closed, true) => {
                format!(" {} {} · z shows", group.title(), members.len())
            }
            (QuestionGroup::Closed, false) => {
                format!(" {} {} · z hides", group.title(), members.len())
            }
            _ => format!(" {} {}", group.title(), members.len()),
        };
        rows.push((
            None,
            PageRow {
                text: header,
                style: if group == QuestionGroup::Unrouted {
                    RowStyle::Dim
                } else {
                    RowStyle::Bold
                },
                focus: false,
                click: (group == QuestionGroup::Closed)
                    .then_some(QueueHitTarget::QuestionKey(KeyCode::Char('z'))),
                label: 0,
            },
        ));
        if folded {
            continue;
        }
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

/// The keys that scroll a page: `j`/`k` unless they move a highlight or edit an answer.
fn scroll_hint(state: &QuestionsState, record: &QuestionRecord) -> &'static str {
    let rows = form(state, record);
    if state.typing().is_none() && (rows.is_empty() || lone_field(&rows)) {
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
        let hint = hint.as_str();
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
    fn secret_record() -> QuestionRecord {
        use crate::domain::{QuestionDraft, QuestionRef, SecretTarget};
        let task = Uuid::from_u128(3);
        QuestionRecord {
            reference: QuestionRef {
                id: Uuid::from_u128(2),
                number: 5,
                task_id: task,
                kind: QuestionKind::Secret,
                summary: "staging key".into(),
                revision: Uuid::from_u128(1),
            },
            draft: QuestionDraft {
                task_id: task,
                kind: QuestionKind::Secret,
                summary: "staging key".into(),
                origin_thread: "thr-a".into(),
                recipient: "lead-a".into(),
                text: "Paste the key".into(),
                options: Vec::new(),
                recommendation: None,
                impact: None,
                blocking_scope: None,
                approval: None,
                finding: None,
                source: None,
                audience: Some(Audience::User),
                lead_recipient: None,
                secret: Some(SecretTarget {
                    key: "STAGING_KEY".into(),
                    profile: None,
                    config: None,
                }),
            },
            created_at: std::time::SystemTime::now(),
            answers: Vec::new(),
            routes: Vec::new(),
        }
    }

    #[test]
    fn a_secret_page_shows_its_target_and_masks_the_typed_value() {
        let record = secret_record();
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            active: true,
            ..QuestionsState::default()
        };
        state.open(id);
        let texts = |state: &QuestionsState| -> Vec<String> {
            page_rows(state, &record, &[], 80)
                .into_iter()
                .map(|row| row.text)
                .collect()
        };
        let before = texts(&state).join("\n");
        assert!(before.contains("Q5 · secret · open · for you"), "{before}");
        assert!(before.contains("key     STAGING_KEY"), "{before}");
        assert!(before.contains("profile default"), "{before}");
        assert!(before.contains("config  global"), "{before}");
        // The page opens on its masked field, already focused, plus a reject row.
        assert!(before.contains("1. ▏"), "{before}");
        assert!(before.contains("2. Reject"), "{before}");
        assert!(state.typing().is_some_and(|input| input.secret));
        assert_eq!(group_of(&record), QuestionGroup::ForYou);

        state
            .answer
            .as_mut()
            .expect("armed secret field")
            .text
            .push_str("hunter2");
        let typed = texts(&state).join("\n");
        assert!(!typed.contains("hunter2"), "{typed}");
        assert!(typed.contains("•••••••▏"), "{typed}");
        assert_eq!(footer_hint(&state, false), "enter fnox set · esc cancel");
        let debug = format!("{state:?}");
        assert!(!debug.contains("hunter2"), "Debug never prints the value");
        // Every printable key is text while the masked input is open.
        assert_eq!(
            map_key(&state, false, KeyEvent::from(KeyCode::Char('q'))),
            Some(BoardIntent::Question(QuestionIntent::AnswerInsert(
                "q".into()
            )))
        );
    }

    #[test]
    fn a_set_secret_offers_no_correction() {
        let mut record = secret_record();
        record.answers.push(crate::domain::QuestionAnswer {
            revision: Uuid::from_u128(9),
            created_task_id: None,
            corrects_revision: None,
            text: "set".into(),
            actor: "user".into(),
            origin: crate::domain::AnswerOrigin::UserUi,
            at: std::time::SystemTime::now(),
            approval: None,
            user_chat: None,
            source: None,
            action: None,
            comment: None,
        });
        record.reference.revision = Uuid::from_u128(9);
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            ..QuestionsState::default()
        };
        state.open(id);
        let page = page_rows(&state, &record, &[], 80)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(page.contains("set in fnox"), "{page}");
        assert!(!page.contains("c corrects"), "{page}");
        assert_eq!(footer_hint(&state, false), "t task · J/K next · esc back");
    }

    #[test]
    fn picking_reject_on_an_open_secret_closes_it_with_no_fnox_call() {
        let record = secret_record();
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            active: true,
            ..QuestionsState::default()
        };
        state.open(id);
        assert_eq!(form(&state, &record), vec![Choice::Text, Choice::Reject]);
        // While the masked field is focused, text entry owns every key, digits included.
        assert_eq!(
            map_key(&state, false, KeyEvent::from(KeyCode::Char('2'))),
            Some(BoardIntent::Question(QuestionIntent::AnswerInsert(
                "2".into()
            ))),
            "a focused field takes every key as text, including digits"
        );
        // Leaving the field (Esc) wipes the secret and the page falls back to its form,
        // where `2` now picks the reject row.
        assert_eq!(
            map_key(&state, false, KeyEvent::from(KeyCode::Esc)),
            Some(BoardIntent::Question(QuestionIntent::AnswerCancel))
        );
        state.answer = None;
        assert_eq!(
            map_key(&state, false, KeyEvent::from(KeyCode::Char('2'))),
            Some(BoardIntent::Question(QuestionIntent::Pick(1)))
        );
    }

    #[test]
    fn a_rejected_secret_shows_rejected_not_set_in_fnox() {
        let mut record = secret_record();
        record.answers.push(crate::domain::QuestionAnswer {
            revision: Uuid::from_u128(9),
            created_task_id: None,
            corrects_revision: None,
            text: crate::store::SECRET_REJECTED.into(),
            actor: "user".into(),
            origin: crate::domain::AnswerOrigin::UserUi,
            at: std::time::SystemTime::now(),
            approval: None,
            user_chat: None,
            source: None,
            action: None,
            comment: None,
        });
        record.reference.revision = Uuid::from_u128(9);
        assert_eq!(state_label(&record, &[]), "rejected");
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            ..QuestionsState::default()
        };
        state.open(id);
        let page = page_rows(&state, &record, &[], 80)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!page.contains("set in fnox"), "{page}");
        assert!(
            !page.contains("c corrects"),
            "a secret is never corrected: {page}"
        );
        assert_eq!(footer_hint(&state, false), "t task · J/K next · esc back");
    }

    fn approval_record() -> QuestionRecord {
        use crate::domain::{ApprovalTerms, QuestionDraft, QuestionRef};
        let task = Uuid::from_u128(3);
        QuestionRecord {
            reference: QuestionRef {
                id: Uuid::from_u128(2),
                number: 6,
                task_id: task,
                kind: QuestionKind::Approval,
                summary: "deploy prod".into(),
                revision: Uuid::from_u128(1),
            },
            draft: QuestionDraft {
                task_id: task,
                kind: QuestionKind::Approval,
                summary: "deploy prod".into(),
                origin_thread: "thr-a".into(),
                recipient: "user".into(),
                text: "please approve the deploy".into(),
                options: Vec::new(),
                recommendation: None,
                impact: None,
                blocking_scope: None,
                approval: Some(ApprovalTerms {
                    target: "prod".into(),
                    action: "deploy".into(),
                    h: "0".repeat(40),
                    scope: "web".into(),
                    rollback: "redeploy previous".into(),
                }),
                finding: None,
                source: None,
                audience: Some(Audience::User),
                lead_recipient: None,
                secret: None,
            },
            created_at: std::time::SystemTime::now(),
            answers: Vec::new(),
            routes: Vec::new(),
        }
    }

    #[test]
    fn an_approval_form_offers_approve_reject_and_comment_and_no_check_keys() {
        let record = approval_record();
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            active: true,
            ..QuestionsState::default()
        };
        state.open(id);
        let text = |state: &QuestionsState| {
            page_rows(state, &record, &[], 80)
                .into_iter()
                .map(|row| row.text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let page = text(&state);
        // The exact terms come first, then the three numbered choices.
        let terms = page.find("target    prod").expect("terms shown");
        let first = page
            .find("▸ 1. Approve these exact terms")
            .expect("approve row");
        assert!(terms < first, "{page}");
        assert!(page.contains("  2. Reject"), "{page}");
        assert!(page.contains("  3. Add a comment…"), "{page}");
        assert!(!page.contains("[ ]"), "no check line: {page}");
        assert_eq!(
            form(&state, &record),
            vec![Choice::Approve, Choice::Reject, Choice::Comment]
        );
        let key = |state: &QuestionsState, code| map_key(state, false, KeyEvent::from(code));
        // Digits pick at once; the old check and decide keys are gone.
        assert_eq!(
            key(&state, KeyCode::Char('1')),
            Some(BoardIntent::Question(QuestionIntent::Pick(0)))
        );
        assert_eq!(
            key(&state, KeyCode::Char('3')),
            Some(BoardIntent::Question(QuestionIntent::Pick(2)))
        );
        assert_eq!(key(&state, KeyCode::Char('4')), None);
        for gone in [' ', 'y', 'n'] {
            assert_eq!(key(&state, KeyCode::Char(gone)), None, "{gone:?}");
        }
        assert_eq!(
            key(&state, KeyCode::Char('m')),
            Some(BoardIntent::Question(QuestionIntent::Comment))
        );
        assert_eq!(
            footer_hint(&state, false),
            "↑↓ move · enter pick · 1-3 pick · J/K next · t task · esc back"
        );

        // The comment row is an inline field; once kept, the row shows it.
        state.highlight.insert(id, 2);
        state.answer = Some(AnswerInput::comment(
            id,
            record.reference.revision,
            "looks safe".into(),
        ));
        let typing = text(&state);
        assert!(typing.contains("▸ 3. looks safe▏"), "{typing}");
        assert_eq!(footer_hint(&state, false), "enter keep · esc keep");
        state.answer = None;
        state.comments.insert(id, "looks safe".into());
        let staged = text(&state);
        assert!(staged.contains("3. Comment · looks safe"), "{staged}");

        // A closed record's stale staged comment is dropped on reload.
        let mut closed = record.clone();
        closed.answers.push(crate::domain::QuestionAnswer {
            revision: Uuid::from_u128(9),
            created_task_id: None,
            corrects_revision: None,
            text: "rejected".into(),
            actor: "user".into(),
            origin: crate::domain::AnswerOrigin::UserUi,
            at: std::time::SystemTime::now(),
            approval: None,
            user_chat: None,
            source: None,
            action: None,
            comment: Some("looks safe".into()),
        });
        closed.reference.revision = Uuid::from_u128(9);
        state.set_feed(QuestionFeed::Loaded(vec![closed.clone()]));
        assert!(
            state.comments.is_empty(),
            "a decided approval's stale staged comment is pruned, not resurfaced"
        );
        assert!(
            form(&state, &closed).is_empty(),
            "a decided approval is read-only"
        );
    }

    #[test]
    fn approve_stays_bound_to_the_terms_the_page_showed_across_a_reload() {
        let record = approval_record();
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            active: true,
            ..QuestionsState::default()
        };
        state.open(id);
        assert!(state.seen_matches(&record), "opening binds the shown terms");
        // An agent re-asks with a new target while the page is open: the reload shows the
        // new terms, but the binding still names the old ones, so a pick of 1 refuses.
        let mut moved = record.clone();
        moved.reference.revision = Uuid::from_u128(4);
        if let Some(terms) = moved.draft.approval.as_mut() {
            terms.target = "prod-eu".into();
        }
        state.set_feed(QuestionFeed::Loaded(vec![moved.clone()]));
        assert!(!state.seen_matches(&moved), "changed terms never pass");
        // The refusal rebinds to what is now on screen; the next pick may approve those.
        state.rebind_seen(&moved);
        assert!(state.seen_matches(&moved));
        // Same revision, changed terms: still refused.
        let mut swapped = moved.clone();
        if let Some(terms) = swapped.draft.approval.as_mut() {
            terms.scope = "everything".into();
        }
        assert!(!state.seen_matches(&swapped));
    }

    fn options_record() -> QuestionRecord {
        let mut record = secret_record();
        record.reference.kind = QuestionKind::Question;
        record.draft.kind = QuestionKind::Question;
        record.draft.secret = None;
        record.draft.options = vec!["alpha".into(), "beta".into()];
        record.draft.recommendation = Some("beta".into());
        record
    }

    #[test]
    fn a_question_form_numbers_its_options_then_a_text_row_and_starts_on_the_recommendation() {
        let record = options_record();
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            active: true,
            ..QuestionsState::default()
        };
        state.open(id);
        let page = page_rows(&state, &record, &[], 80)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(page.contains("  1. alpha"), "{page}");
        assert!(page.contains("▸ 2. beta · recommended"), "{page}");
        assert!(page.contains("  3. Type something…"), "{page}");
        assert!(state.typing().is_none(), "no field is focused yet");
        let key = |state: &QuestionsState, code| map_key(state, false, KeyEvent::from(code));
        assert_eq!(
            key(&state, KeyCode::Char('1')),
            Some(BoardIntent::Question(QuestionIntent::Pick(0)))
        );
        assert_eq!(
            key(&state, KeyCode::Enter),
            Some(BoardIntent::Question(QuestionIntent::Enter))
        );
        // A letter on an option row is no answer; on the text row it opens the field.
        assert_eq!(key(&state, KeyCode::Char('x')), None);
        state.move_on_page(true);
        assert_eq!(state.highlighted_choice(), Some(Choice::Text));
        state.move_on_page(true);
        assert_eq!(state.highlighted(&record), 2);
        assert_eq!(
            footer_hint(&state, false),
            "↑↓ move · enter type · 1-3 pick · J/K next · esc back"
        );
        assert_eq!(
            key(&state, KeyCode::Char('x')),
            Some(BoardIntent::Question(QuestionIntent::AnswerInsert(
                "x".into()
            )))
        );
        assert_eq!(
            key(&state, KeyCode::Char('k')),
            Some(BoardIntent::Question(QuestionIntent::Move(false))),
            "j/k still move off the text row"
        );
        assert_eq!(
            paste_target(&state),
            Some(false),
            "a paste opens the text row"
        );
        // An unfocused draft stays on its row.
        let mut draft = AnswerInput::typed(id, record.reference.revision, None);
        draft.text.push_str("gamma");
        draft.focused = false;
        state.answer = Some(draft);
        let kept = page_rows(&state, &record, &[], 80)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(kept.contains("▸ 3. gamma"), "{kept}");
        assert!(state.has_unsaved_work());
        // On the list, digits switch tabs as before.
        state.page = None;
        assert_eq!(
            key(&state, KeyCode::Char('1')),
            Some(BoardIntent::SelectNavTab(NavTab::Board))
        );
    }

    #[test]
    fn a_question_without_options_opens_on_its_focused_field() {
        let mut record = options_record();
        record.draft.options.clear();
        record.draft.recommendation = None;
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            active: true,
            ..QuestionsState::default()
        };
        state.open(id);
        assert_eq!(form(&state, &record), vec![Choice::Text]);
        assert!(state.typing().is_some_and(|input| !input.secret));
        let page = page_rows(&state, &record, &[], 80)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(page.contains("answer  ▏"), "{page}");
        assert!(!page.contains("1. "), "{page}");
        // Every printable key, digits included, is text in the field.
        assert_eq!(
            map_key(&state, false, KeyEvent::from(KeyCode::Char('1'))),
            Some(BoardIntent::Question(QuestionIntent::AnswerInsert(
                "1".into()
            )))
        );
        assert_eq!(footer_hint(&state, false), "enter save · esc leave");
        // An empty field never blocks opening another record.
        assert_eq!(state.answer_elsewhere(Uuid::from_u128(77)), None);
        // Left with Esc, the lone field reopens on Enter and page keys work again.
        state.answer.as_mut().expect("field").focused = false;
        assert_eq!(
            map_key(&state, false, KeyEvent::from(KeyCode::Char('t'))),
            Some(BoardIntent::Question(QuestionIntent::OpenTask))
        );
        assert_eq!(
            map_key(&state, false, KeyEvent::from(KeyCode::Enter)),
            Some(BoardIntent::Question(QuestionIntent::Enter))
        );
        assert_eq!(
            footer_hint(&state, false),
            "enter type · J/K next · t task · esc back"
        );
    }

    #[test]
    fn a_finding_form_names_its_three_actions() {
        let mut record = options_record();
        record.reference.kind = QuestionKind::Finding;
        record.draft.kind = QuestionKind::Finding;
        record.draft.options.clear();
        let id = record.reference.id;
        let mut state = QuestionsState {
            feed: QuestionFeed::Loaded(vec![record.clone()]),
            active: true,
            ..QuestionsState::default()
        };
        state.open(id);
        let page = page_rows(&state, &record, &[], 80)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(page.contains("▸ 1. Create the task and run it"), "{page}");
        assert!(page.contains("  2. Create the task only"), "{page}");
        assert!(page.contains("  3. Close"), "{page}");
    }
}
