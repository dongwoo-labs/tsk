//! The reports tab (`3`): independent Markdown completion documents attached to a Task.
//!
//! Everything here reads one session snapshot of the report records (the feed) that the app
//! loads at board open, whenever `reports.json` changes on disk, and after `mark_read`. The
//! CLI registers a report through `tsk report`; this tab never writes Markdown, only the read
//! marker, and only for the exact revision the open page shows when it opens.
//!
//! Rendering reuses two small OSS crates rather than a hand-rolled parser and scroller:
//! `tui-markdown` (default-features = false: no syntax highlighting, no colour) turns the
//! stored Markdown into a styled [`ratatui::text::Text`], and `tui-scrollview` paints that
//! text into a viewport taller than the frame. Every logical source line passes through
//! [`crate::ui::terminal_text`] first (the line breaks themselves stay, so the parser still
//! sees the document's structure), and every span's colour is stripped before paint
//! (`sanitize_markdown_text`), so a malicious or just-odd document cannot inject colour or
//! control sequences onto the board.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Rect, Size};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use tui_markdown::StyleSheet as MarkdownStyleSheet;
use tui_scrollview::{ScrollView, ScrollViewState, ScrollbarVisibility};
use uuid::Uuid;

use crate::domain::Task;
use crate::store::ReportRecord;
use crate::ui::input::BoardIntent;
use crate::ui::queue::NavTab;
use crate::ui::render::{self, QueueHitMap, QueueHitTarget};

/// The session snapshot of the board's completion reports.
#[derive(Debug, Clone)]
pub enum ReportFeed {
    /// Nothing has been read from `reports.json` yet (board not opened, or a read failed on
    /// the last attempt). Never shown as zero: the tab says why instead.
    Unavailable(String),
    /// Every record, in store order.
    Loaded(Vec<ReportRecord>),
}

impl Default for ReportFeed {
    fn default() -> Self {
        Self::Unavailable("not loaded yet".to_string())
    }
}

impl ReportFeed {
    pub fn records(&self) -> &[ReportRecord] {
        match self {
            Self::Loaded(records) => records,
            Self::Unavailable(_) => &[],
        }
    }

    pub fn get(&self, id: Uuid) -> Option<&ReportRecord> {
        self.records().iter().find(|record| record.task_id == id)
    }
}

/// Unread reports, for the tab's `·n` count.
pub fn tab_count(feed: &ReportFeed) -> Option<usize> {
    match feed {
        ReportFeed::Loaded(records) => {
            Some(records.iter().filter(|record| record.unread()).count()).filter(|n| *n > 0)
        }
        ReportFeed::Unavailable(_) => None,
    }
}

/// The reports tab's text for the selector row, with its unread count when one is known.
pub fn tab_texts(feed: &ReportFeed) -> String {
    match tab_count(feed) {
        Some(n) => format!(" 3 reports \u{b7}{n} "),
        None => " 3 reports ".to_string(),
    }
}

/// Per-task report marker: `▤` bold while the report is unread, dim once it has been read.
/// `None` when the task has no report. A task can lose its report marker only when its
/// report is deregistered, which this store never does; the lookup is by task id, so a
/// task that is done, archived, or missing from this snapshot still carries its mark.
pub fn task_marks(feed: &ReportFeed) -> std::collections::BTreeMap<Uuid, bool> {
    feed.records()
        .iter()
        .map(|record| (record.task_id, record.unread()))
        .collect()
}

/// Listed order: unread first, then read, each by task number ascending.
pub fn list_order(records: &[ReportRecord]) -> Vec<Uuid> {
    let mut unread: Vec<&ReportRecord> = records.iter().filter(|r| r.unread()).collect();
    let mut read: Vec<&ReportRecord> = records.iter().filter(|r| !r.unread()).collect();
    unread.sort_by_key(|r| r.task_number);
    read.sort_by_key(|r| r.task_number);
    unread.into_iter().chain(read).map(|r| r.task_id).collect()
}

/// A `mark_read` write the board could not call done: the store refused a stale revision (a
/// newer one landed while the page was open) or a transient I/O failure. The page shows this
/// plainly instead of claiming success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkReadFailure {
    pub task_id: Uuid,
    pub message: String,
}

/// Session-only state of the reports tab. Never persisted.
#[derive(Debug, Clone, Default)]
pub struct ReportsState {
    pub feed: ReportFeed,
    /// Whether tab 3 owns the frame and the keyboard.
    pub active: bool,
    /// The list row the cursor rests on.
    pub cursor: Option<Uuid>,
    /// The open document: the task id of the report whose page is shown.
    pub page: Option<Uuid>,
    /// The exact snapshot the open page is showing. A later `register` landing in the feed
    /// does not silently replace this: the open page keeps showing the revision the user
    /// started reading until they leave it and reopen.
    pub page_snapshot: Option<ReportRecord>,
    /// The document scroll position of the open page. `ScrollViewState` is `Copy`, so the
    /// painter -- which only ever borrows `&ReportsState` -- can read and write it through a
    /// `Cell` exactly like [`crate::ui::questions::QuestionsState::scroll`] does for its own
    /// page scroll, without a separate renderer-to-reducer sync step.
    pub scroll: Cell<ScrollViewState>,
    /// The last `mark_read` failure, shown on the page it happened on.
    pub mark_read_failure: Option<MarkReadFailure>,
    /// The task page a report was opened from: `Esc` returns to it.
    pub return_task: Option<Uuid>,
}

impl ReportsState {
    pub fn records(&self) -> &[ReportRecord] {
        self.feed.records()
    }

    pub fn order(&self) -> Vec<Uuid> {
        list_order(self.records())
    }

    /// Replace the snapshot. The open page's own snapshot is untouched: a new registration or
    /// another revision landing in the feed never swaps what an open page is showing, and
    /// never marks a revision the user has not actually opened as read.
    pub fn set_feed(&mut self, feed: ReportFeed) {
        self.feed = feed;
        self.reanchor();
    }

    pub fn reanchor(&mut self) {
        // An unreadable feed is not an empty one: keep the cursor, the open page's frozen
        // snapshot, and any acknowledgement failure on it until a real read says otherwise.
        if matches!(self.feed, ReportFeed::Unavailable(_)) {
            return;
        }
        let order = self.order();
        if !self.cursor.is_some_and(|id| order.contains(&id)) {
            self.cursor = order.first().copied();
        }
        if let Some(page) = self.page {
            if self.feed.get(page).is_none() {
                self.close_page();
            }
        }
    }

    fn close_page(&mut self) {
        self.page = None;
        self.page_snapshot = None;
        self.mark_read_failure = None;
        self.scroll.set(ScrollViewState::new());
    }

    /// Move the list cursor; a no-op past either end.
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
        self.cursor = Some(order[next]);
    }

    /// `Enter`/click on a list row: open its document at the snapshot the feed shows now.
    /// This only snapshots; the app boundary ([`crate::report_actions`]) then acknowledges
    /// exactly that snapshot's revision. Selecting a row never reaches this.
    pub fn open(&mut self, id: Uuid) {
        let Some(record) = self.feed.get(id).cloned() else {
            return;
        };
        self.cursor = Some(id);
        self.page = Some(id);
        self.page_snapshot = Some(record);
        self.mark_read_failure = None;
        self.scroll.set(ScrollViewState::new());
    }

    /// `Esc` on the open page: back to the list (or the task page that opened it).
    pub fn back(&mut self) {
        self.close_page();
        if self.return_task.take().is_some() {
            self.active = false;
        }
    }

    /// The acknowledgement failure of the open page, if its last `mark_read` failed.
    pub fn page_failure(&self) -> Option<&MarkReadFailure> {
        let page = self.page?;
        self.mark_read_failure
            .as_ref()
            .filter(|failure| failure.task_id == page)
    }

    pub fn scroll_line(&self, forward: bool) {
        let mut scroll = self.scroll.get();
        if forward {
            scroll.scroll_down();
        } else {
            scroll.scroll_up();
        }
        self.scroll.set(scroll);
    }

    pub fn scroll_page(&self, forward: bool) {
        let mut scroll = self.scroll.get();
        if forward {
            scroll.scroll_page_down();
        } else {
            scroll.scroll_page_up();
        }
        self.scroll.set(scroll);
    }
}

/// One reports-tab action. Pure moves apply on [`ReportsState`]; opening a document also
/// asks the app boundary to call `mark_read` on the exact shown revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportIntent {
    Move(bool),
    Scroll(bool),
    ScrollLine(bool),
    Open,
    Row(Uuid),
    /// `Enter` on an open page whose acknowledgement failed: retry it for the same shown
    /// revision.
    RetryMarkRead,
    Back,
}

/// Map one key while tab 3 owns the keyboard.
pub fn map_key(state: &ReportsState, key: KeyEvent) -> Option<BoardIntent> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('c')) {
        return Some(BoardIntent::Quit);
    }
    if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) {
        return None;
    }
    match key.code {
        KeyCode::Char('1') => return Some(BoardIntent::SelectNavTab(NavTab::Board)),
        KeyCode::Char('2') => return Some(BoardIntent::SelectNavTab(NavTab::Questions)),
        KeyCode::Char('3') => return Some(BoardIntent::SelectNavTab(NavTab::Reports)),
        KeyCode::Char('?') => return Some(BoardIntent::OpenHelp),
        _ => {}
    }
    let q = |intent| Some(BoardIntent::Report(intent));
    if state.page.is_none() {
        return match key.code {
            KeyCode::Down | KeyCode::Char('j') => q(ReportIntent::Move(true)),
            KeyCode::Up | KeyCode::Char('k') => q(ReportIntent::Move(false)),
            KeyCode::Enter => q(ReportIntent::Open),
            KeyCode::Esc => Some(BoardIntent::Quit),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Esc => q(ReportIntent::Back),
        KeyCode::Down | KeyCode::Char('j') => q(ReportIntent::ScrollLine(true)),
        KeyCode::Up | KeyCode::Char('k') => q(ReportIntent::ScrollLine(false)),
        KeyCode::PageDown => q(ReportIntent::Scroll(true)),
        KeyCode::PageUp => q(ReportIntent::Scroll(false)),
        KeyCode::Enter if state.page_failure().is_some() => q(ReportIntent::RetryMarkRead),
        _ => None,
    }
}

/// `tui-markdown`'s default style sheet prints `#`/`##`/... before a heading's text, the same
/// marker the source used. The board's own hand-rolled Markdown renderer
/// ([`crate::ui::markdown`]) hides that marker and carries the heading as bold text alone;
/// this style sheet keeps the two renderers visually consistent. Every colour this sheet (or
/// any of `StyleSheet`'s other color defaults) sets is stripped afterward by
/// [`sanitize_markdown_text`] regardless, so only the marker and modifier choices here matter.
#[derive(Debug, Clone, Copy, Default)]
struct ReportsStyleSheet;

impl MarkdownStyleSheet for ReportsStyleSheet {
    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }
}

/// Strip every span's colour, keeping only the mono modifiers the board allows, and own every
/// span's text so the result outlives the borrowed Markdown source. `tui-markdown` never sets
/// a background with this crate's default style sheet, but a future upstream default or a
/// style-sheet change must not reach the frame uncleaned.
fn sanitize_markdown_text(text: Text<'_>) -> Text<'static> {
    Text::from(
        text.lines
            .into_iter()
            .map(|line| {
                let line_style = line.style;
                Line::from(
                    line.spans
                        .into_iter()
                        .map(|span| {
                            Span::styled(
                                span.content.into_owned(),
                                render::strip_color(line_style.patch(span.style)),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// Escape control characters on each logical source line but keep the line breaks: the
/// Markdown parser needs them for paragraphs, lists, tables, and code fences. A `\r\n` pair
/// counts as one break. The stored document is never changed; this only feeds the painter.
fn safe_markdown_source(markdown: &str) -> String {
    crate::ui::split_line_breaks(markdown)
        .map(crate::ui::terminal_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wrap rendered Markdown lines to `width` cells through the board's one wrap engine
/// ([`crate::ui::edit::wrap_text`]), keeping each character's span style, so a long
/// paragraph, table row, or code line continues on the next row instead of being clipped.
fn wrap_markdown_text(text: Text<'static>, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in text.lines {
        let cells: Vec<(char, ratatui::style::Style)> = line
            .spans
            .iter()
            .flat_map(|span| span.content.chars().map(move |c| (c, span.style)))
            .collect();
        let flat: String = cells.iter().map(|(c, _)| *c).collect();
        for row in crate::ui::edit::wrap_text(&flat, width) {
            let from = row.first_raw.min(cells.len());
            let to = (row.first_raw + row.cell_of.len()).min(cells.len());
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (c, style) in &cells[from..to] {
                match spans.last_mut() {
                    Some(last) if last.style == *style => last.content.to_mut().push(*c),
                    _ => spans.push(Span::styled(c.to_string(), *style)),
                }
            }
            out.push(Line::from(spans));
        }
    }
    out
}

/// Render `markdown` through `tui-markdown`, sanitized to the board's mono style set. The raw
/// source already passed [`safe_markdown_source`] at the caller, so no control sequence
/// reaches the parser; this step only ever has to drop colour, not escape anything. Tables
/// wrap their cells to `width` so a box-drawn row never has to be cut mid-border.
fn render_markdown(markdown: &str, width: u16) -> Text<'static> {
    let options = tui_markdown::Options::new(ReportsStyleSheet).table_width(width);
    sanitize_markdown_text(tui_markdown::from_str_with_options(markdown, &options))
}

/// The footer key hint of the tab's current state.
pub fn footer_hint(state: &ReportsState) -> String {
    if state.page_failure().is_some() {
        "enter retries \u{b7} j/k scroll \u{b7} esc back".to_string()
    } else if state.page.is_some() {
        "j/k scroll \u{b7} pgup/pgdn page \u{b7} 1 board \u{b7} 2 questions \u{b7} esc back"
            .to_string()
    } else {
        "enter open \u{b7} j/k move \u{b7} 1 board \u{b7} 2 questions \u{b7} ? keys".to_string()
    }
}

/// List rows: (task id or none for a note row, text, bold for the row's own selection style).
fn list_rows(state: &ReportsState, tasks: &[Task], width: usize) -> Vec<(Option<Uuid>, String)> {
    let mut rows: Vec<(Option<Uuid>, String)> = Vec::new();
    match &state.feed {
        ReportFeed::Unavailable(reason) => {
            for row in crate::ui::edit::wrap_text(
                &format!("  reports cannot be read: {reason} \u{b7} nothing is shown as zero"),
                width.max(1),
            ) {
                rows.push((None, row.text));
            }
            return rows;
        }
        ReportFeed::Loaded(records) if records.is_empty() => {
            rows.push((
                None,
                "  no completion reports yet \u{b7} tsk report <task> --file registers one"
                    .to_string(),
            ));
            return rows;
        }
        ReportFeed::Loaded(_) => {}
    }
    rows.push((None, String::new()));
    for id in state.order() {
        let Some(record) = state.feed.get(id) else {
            continue;
        };
        let selected = state.cursor == Some(id);
        let marker = if selected { " \u{25b8} " } else { "   " };
        let identifier = tasks
            .iter()
            .find(|task| task.id == record.task_id)
            .and_then(Task::board_identifier)
            .unwrap_or_else(|| format!("T{}", record.task_number));
        let status = if record.unread() { "unread" } else { "read" };
        let text = format!(
            "{marker}{} {identifier} {} \u{b7} rev {} \u{b7} {status}",
            render::REPORT_MARK,
            record.task_title,
            record.revision
        );
        rows.push((Some(id), text));
    }
    rows
}

/// Inputs of one reports-tab frame.
pub struct ReportsFrame<'a> {
    pub state: &'a ReportsState,
    /// The questions tab's label with its count, as the board's own top row paints it.
    pub questions_tab: &'a str,
    pub tasks: &'a [Task],
    pub message: Option<&'a str>,
}

fn style(bold: bool) -> ratatui::style::Style {
    if bold {
        render::style_bold()
    } else {
        render::style_plain()
    }
}

fn put(frame: &mut Frame<'_>, x: u16, y: u16, width: u16, text: &str, bold: bool) {
    if width == 0 {
        return;
    }
    let shown = crate::ui::present_line(text, width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(shown, style(bold)))),
        Rect::new(x, y, width, 1),
    );
}

/// Paint the reports tab into `area`: a list on the left of a document page, or a full-frame
/// list / page below the wide split threshold. Returns the hit map the mouse router reads.
pub fn draw(out: &mut Frame<'_>, frame: &ReportsFrame<'_>, area: Rect) -> QueueHitMap {
    let mut hits = QueueHitMap::default();
    let width = area.width;
    let height = area.height;
    if width == 0 || height == 0 {
        return hits;
    }
    out.render_widget(Paragraph::new(Line::from("")), area);
    let geo = crate::ui::tier::resolve(width, height);
    let state = frame.state;
    let wide = width >= crate::ui::tier::WIDE_SPLIT_MIN_WIDTH;

    // Tabs: the same three-tab row, shrink rules, and hit spans as the board's own.
    if let Some(row) = geo.selector_row {
        let y = area.y + row;
        let (spans, tab_hits, _) = render::nav_tab_spans(
            frame.questions_tab,
            &tab_texts(&state.feed),
            NavTab::Reports,
            width as usize,
        );
        out.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, y, width, 1),
        );
        for (target, x, w) in tab_hits {
            hits.push(target, Rect::new(area.x + x, y, w, 1));
        }
    }

    let body_top = area.y + geo.viewport_top;
    let body_h = geo.viewport_height;
    if wide {
        let split =
            crate::ui::tier::resolve_responsive(width, height, crate::ui::tier::WideStage::Board);
        let list_rect = Rect::new(area.x + split.board.x, body_top, split.board.width, body_h);
        draw_list(out, frame, list_rect, &mut hits);
        if split.rule.width > 0 {
            for y in 0..body_h {
                put(
                    out,
                    area.x + split.rule.x,
                    body_top + y,
                    1,
                    "\u{2502}",
                    false,
                );
            }
        }
        let detail = split.task_content();
        let detail_rect = Rect::new(area.x + detail.x, body_top, detail.width, body_h);
        if state.page.is_some() {
            draw_page(out, state, detail_rect, &mut hits);
        } else {
            put(
                out,
                detail_rect.x,
                detail_rect.y + 1,
                detail_rect.width,
                "pick a report on the left",
                false,
            );
        }
    } else if state.page.is_some() {
        let rect = Rect::new(area.x, body_top, width, body_h);
        draw_page(out, state, rect, &mut hits);
    } else {
        let rect = Rect::new(area.x, body_top, width, body_h);
        draw_list(out, frame, rect, &mut hits);
    }

    if let Some(row) = geo.rule_row {
        put(
            out,
            area.x,
            area.y + row,
            width,
            &"\u{2500}".repeat(width as usize),
            false,
        );
    }
    if let Some(row) = geo.status_row {
        let status = frame
            .message
            .map(str::to_string)
            .unwrap_or_else(|| match &state.feed {
                ReportFeed::Unavailable(_) => "reports \u{b7} cannot be read".to_string(),
                ReportFeed::Loaded(records) => {
                    let unread = records.iter().filter(|r| r.unread()).count();
                    format!(
                        "reports \u{b7} {unread} unread \u{b7} {} total",
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
            false,
        );
    }
    if let Some(row) = geo.verb_row {
        put(
            out,
            area.x,
            area.y + row,
            width,
            &format!(" {}", footer_hint(state)),
            false,
        );
    }
    hits
}

fn draw_list(out: &mut Frame<'_>, frame: &ReportsFrame<'_>, rect: Rect, hits: &mut QueueHitMap) {
    let rows = list_rows(frame.state, frame.tasks, rect.width as usize);
    let height = rect.height as usize;
    if height == 0 {
        return;
    }
    // Keep the cursor row in view, the way the questions list follows its focus row: the
    // last painted row becomes a `↓ n more` marker while rows follow it.
    let max_scroll = rows.len().saturating_sub(height);
    let mut scroll = 0;
    if let Some(focus) = rows
        .iter()
        .position(|(id, _)| id.is_some() && *id == frame.state.cursor)
    {
        if focus >= height {
            scroll = focus + 1 - height;
        }
        if height > 1 && focus + 1 == scroll + height && rows.len() > scroll + height {
            scroll = (scroll + 1).min(max_scroll);
        }
    }
    let more_below = rows.len() > scroll + height;
    for (offset, (id, text)) in rows.iter().skip(scroll).take(height).enumerate() {
        let y = rect.y + offset as u16;
        if offset + 1 == height && more_below {
            put(
                out,
                rect.x,
                y,
                rect.width,
                &format!(
                    "  \u{2193} {} more \u{b7} j/k",
                    rows.len() - scroll - height + 1
                ),
                false,
            );
            break;
        }
        let bold = *id == frame.state.cursor && id.is_some();
        put(out, rect.x, y, rect.width, text, bold);
        if let Some(id) = id {
            hits.push(
                QueueHitTarget::Report(*id),
                Rect::new(rect.x, y, rect.width, 1),
            );
            hits.push_copyable(Rect::new(rect.x, y, rect.width, 1));
        }
    }
}

/// Paint the open document: a header line, then the scrollable Markdown body via
/// `tui-scrollview`. The header and any mark-read failure are painted rows this scroll view
/// never owns, so they stay visible however far the body has scrolled.
fn draw_page(out: &mut Frame<'_>, state: &ReportsState, rect: Rect, hits: &mut QueueHitMap) {
    let Some(record) = state.page_snapshot.as_ref() else {
        return;
    };
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let header = format!(
        "T{} \u{b7} {} \u{b7} revision {} \u{b7} {}",
        record.task_number,
        record.task_title,
        record.revision,
        if record.unread() { "unread" } else { "read" }
    );
    let mut y = rect.y;
    put(out, rect.x, y, rect.width, &header, true);
    y += 1;
    if let Some(failure) = state.page_failure() {
        put(
            out,
            rect.x,
            y,
            rect.width,
            &format!("mark read failed: {}", failure.message),
            true,
        );
        y += 1;
    }
    let body_rect = Rect::new(
        rect.x,
        y,
        rect.width,
        rect.height.saturating_sub(y - rect.y),
    );
    if body_rect.height == 0 {
        return;
    }
    // Sanitize each source line before the Markdown parser ever sees it, then render, strip
    // colour, and wrap: no control sequence or ANSI escape in a stored document reaches the
    // frame, and no line runs past the page's right edge.
    // One column stays free for the scroll view's own vertical scrollbar.
    let content_width = body_rect.width.saturating_sub(1).max(1);
    let text = render_markdown(&safe_markdown_source(&record.markdown), content_width);
    let lines = wrap_markdown_text(text, content_width as usize);
    let content_height = (lines.len() as u16).max(1);
    let mut scroll_view = ScrollView::new(Size::new(content_width, content_height))
        .horizontal_scrollbar_visibility(ScrollbarVisibility::Never);
    scroll_view.render_widget(
        Paragraph::new(Text::from(lines)),
        Rect::new(0, 0, content_width, content_height),
    );
    let mut scroll_state = state.scroll.get();
    out.render_stateful_widget(scroll_view, body_rect, &mut scroll_state);
    // The widget clamps the offset to the content it just measured (never past the last
    // line); write that back so the next `j`/`k`/PageDown starts from where this frame
    // actually painted, not from a stale pre-clamp value.
    state.scroll.set(scroll_state);
    hits.push_copyable(body_rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(task_number: u64, revision: u64, read_revision: Option<u64>) -> ReportRecord {
        ReportRecord {
            task_id: Uuid::new_v4(),
            task_number,
            task_title: format!("task {task_number}"),
            project: None,
            revision,
            markdown: "# Done\n\nbody".into(),
            read_revision,
            unread: read_revision != Some(revision),
            revisions: vec![],
        }
    }

    #[test]
    fn unread_reports_list_before_read_ones_by_task_number() {
        let records = vec![
            record(3, 1, Some(1)),
            record(1, 2, None),
            record(2, 1, Some(1)),
            record(4, 1, None),
        ];
        let order: Vec<u64> = list_order(&records)
            .into_iter()
            .map(|id| {
                records
                    .iter()
                    .find(|r| r.task_id == id)
                    .unwrap()
                    .task_number
            })
            .collect();
        assert_eq!(order, vec![1, 4, 2, 3]);
    }

    #[test]
    fn opening_a_row_snapshots_the_revision_it_shows() {
        let r = record(1, 1, None);
        let id = r.task_id;
        let mut state = ReportsState {
            feed: ReportFeed::Loaded(vec![r.clone()]),
            ..ReportsState::default()
        };
        state.cursor = Some(id);
        // A cursor move alone is no open: nothing is marked, no page is shown.
        state.move_cursor(true);
        assert!(state.page.is_none());
        state.open(id);
        assert_eq!(state.page, Some(id));
        assert_eq!(state.page_snapshot.as_ref().map(|r| r.revision), Some(1));
        assert!(r.unread());
    }

    #[test]
    fn a_later_registration_does_not_replace_an_open_pages_snapshot() {
        let r1 = record(1, 1, None);
        let id = r1.task_id;
        let mut state = ReportsState {
            feed: ReportFeed::Loaded(vec![r1.clone()]),
            ..ReportsState::default()
        };
        state.open(id);
        let mut r2 = r1.clone();
        r2.revision = 2;
        r2.markdown = "new content".into();
        state.set_feed(ReportFeed::Loaded(vec![r2]));
        assert_eq!(
            state.page_snapshot.as_ref().map(|r| r.revision),
            Some(1),
            "the open page keeps showing the revision it opened"
        );
    }

    #[test]
    fn an_unreadable_feed_keeps_the_open_page_and_its_failure() {
        let r = record(1, 1, None);
        let id = r.task_id;
        let mut state = ReportsState {
            feed: ReportFeed::Loaded(vec![r]),
            ..ReportsState::default()
        };
        state.open(id);
        state.mark_read_failure = Some(MarkReadFailure {
            task_id: id,
            message: "io".into(),
        });
        state.set_feed(ReportFeed::Unavailable("invalid".into()));
        assert_eq!(state.page, Some(id), "the page stays open");
        assert_eq!(state.page_snapshot.as_ref().map(|r| r.revision), Some(1));
        assert!(state.page_failure().is_some(), "the failure stays shown");
        assert_eq!(state.cursor, Some(id));
        state.set_feed(ReportFeed::Loaded(Vec::new()));
        assert_eq!(state.page, None, "a real read without the report closes it");
    }

    #[test]
    fn esc_closes_the_page_and_returns_to_the_task_it_opened_from() {
        let r = record(1, 1, None);
        let id = r.task_id;
        let mut state = ReportsState {
            feed: ReportFeed::Loaded(vec![r]),
            active: true,
            return_task: Some(Uuid::new_v4()),
            ..ReportsState::default()
        };
        state.open(id);
        state.back();
        assert!(state.page.is_none());
        assert!(
            !state.active,
            "closing from a task-opened report returns there"
        );
    }

    #[test]
    fn map_key_opens_moves_and_leaves_without_a_mutating_chord() {
        let r = record(1, 1, None);
        let id = r.task_id;
        let mut state = ReportsState {
            feed: ReportFeed::Loaded(vec![r]),
            ..ReportsState::default()
        };
        state.cursor = Some(id);
        assert_eq!(
            map_key(&state, KeyEvent::from(KeyCode::Enter)),
            Some(BoardIntent::Report(ReportIntent::Open))
        );
        state.open(id);
        assert_eq!(
            map_key(&state, KeyEvent::from(KeyCode::Char('j'))),
            Some(BoardIntent::Report(ReportIntent::ScrollLine(true)))
        );
        assert_eq!(
            map_key(&state, KeyEvent::from(KeyCode::Esc)),
            Some(BoardIntent::Report(ReportIntent::Back))
        );
        assert_eq!(
            map_key(&state, KeyEvent::from(KeyCode::Char('1'))),
            Some(BoardIntent::SelectNavTab(NavTab::Board))
        );
        // Opening already acknowledged the page: no bare mutating key remains on it, and
        // `Enter` only retries a failed acknowledgement.
        for code in [KeyCode::Char('m'), KeyCode::Enter] {
            assert_eq!(map_key(&state, KeyEvent::from(code)), None, "{code:?}");
        }
        state.mark_read_failure = Some(MarkReadFailure {
            task_id: id,
            message: "io".into(),
        });
        assert_eq!(
            map_key(&state, KeyEvent::from(KeyCode::Enter)),
            Some(BoardIntent::Report(ReportIntent::RetryMarkRead))
        );
    }

    #[test]
    fn a_document_keeps_its_markdown_structure_and_wraps_to_the_page() {
        let source = "# 완료 보고서\r\n\n첫 문단입니다.\n\n| key | value |\n| --- | --- |\n| a | 1 |\n\n```\nfn main() { let long_name = 1; }\n```";
        let text = render_markdown(&safe_markdown_source(source), 12);
        let rows: Vec<String> = wrap_markdown_text(text, 12)
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let flat = rows.join("\n");
        assert!(
            !flat.contains("\\u{"),
            "line breaks are not escaped: {flat}"
        );
        assert_eq!(
            rows[0], "완료 보고서",
            "the heading is its own row: {rows:?}"
        );
        assert_eq!(
            rows[1], "",
            "a blank row separates heading and paragraph: {rows:?}"
        );
        assert!(flat.contains("문단입니다."), "{rows:?}");
        assert!(flat.contains('│'), "the table renders: {rows:?}");
        assert!(
            flat.contains("long_name"),
            "the code block survives wrap: {rows:?}"
        );
        for row in &rows {
            assert!(
                render::display_width(row) <= 12,
                "{row:?} overflows 12 cells"
            );
        }
    }

    #[test]
    fn markdown_rendering_hides_markers_and_strips_color() {
        let text = render_markdown("# Heading\n\n**bold** and `code`", 80);
        let flat = text
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(flat.contains("Heading"));
        assert!(!flat.contains('#'));
        for line in &text.lines {
            for span in &line.spans {
                assert_eq!(
                    span.style.fg, None,
                    "no foreground colour survives sanitize"
                );
                assert_eq!(
                    span.style.bg, None,
                    "no background colour survives sanitize"
                );
            }
        }
    }

    #[test]
    fn a_control_sequence_in_the_document_is_escaped_before_the_parser_sees_it() {
        let hostile = "safe\u{1b}[31mred\u{1b}[0m text";
        let text = render_markdown(&safe_markdown_source(hostile), 80);
        let flat = text
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !flat.contains('\u{1b}'),
            "raw ESC must never reach the frame"
        );
        assert!(
            flat.contains("\\u{001b}"),
            "it is shown as a visible escape"
        );
    }
}
