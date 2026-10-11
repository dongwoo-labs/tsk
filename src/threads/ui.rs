//! The threads pane: live agent sessions grouped by whose move is next, with focus on Enter.

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Terminal;

use super::{herdr, visible_rows, Group, ThreadRow};
use crate::ui::{present_line, terminal_text};

/// How often the pane re-reads `herdr agent list`.
pub const REFRESH: Duration = Duration::from_millis(1500);

/// Pane title; also the label `scripts/open-threads.sh` looks the pane up by.
pub const THREADS_PANE_LABEL: &str = "tsk threads";

/// Where a status was first seen, so a row can say how long it has held.
#[derive(Debug, Clone, Copy)]
struct Seen {
    seq: u64,
    status: super::AgentStatus,
    since: Instant,
}

/// The view's whole state, separate from the terminal so it renders in tests.
#[derive(Debug, Default)]
pub struct ThreadsView {
    pub rows: Vec<ThreadRow>,
    /// The last lookup's failure; the previous rows stay visible beside it.
    pub error: Option<String>,
    pub show_all: bool,
    /// Selected row's pane id, so selection follows the agent across refreshes.
    selected: Option<String>,
    pub message: Option<String>,
    seen: HashMap<String, Seen>,
}

impl ThreadsView {
    pub fn visible(&self) -> Vec<ThreadRow> {
        visible_rows(&self.rows, self.show_all)
    }

    fn selected_index(&self, visible: &[ThreadRow]) -> Option<usize> {
        if visible.is_empty() {
            return None;
        }
        Some(
            self.selected
                .as_ref()
                .and_then(|pane| visible.iter().position(|row| &row.pane_id == pane))
                .unwrap_or(0),
        )
    }

    pub fn selected_row(&self) -> Option<ThreadRow> {
        let visible = self.visible();
        self.selected_index(&visible).map(|i| visible[i].clone())
    }

    /// Apply one lookup result at `now`.
    pub fn apply(&mut self, result: Result<Vec<ThreadRow>, String>, now: Instant) {
        match result {
            Ok(rows) => {
                self.seen
                    .retain(|pane, _| rows.iter().any(|r| &r.pane_id == pane));
                for row in &rows {
                    let entry = self.seen.entry(row.pane_id.clone()).or_insert(Seen {
                        seq: row.state_change_seq,
                        status: row.status,
                        since: now,
                    });
                    if entry.seq != row.state_change_seq || entry.status != row.status {
                        *entry = Seen {
                            seq: row.state_change_seq,
                            status: row.status,
                            since: now,
                        };
                    }
                }
                self.rows = rows;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let visible = self.visible();
        let Some(current) = self.selected_index(&visible) else {
            return;
        };
        let next = (current as isize + delta).clamp(0, visible.len() as isize - 1) as usize;
        self.selected = Some(visible[next].pane_id.clone());
    }

    fn held_for(&self, row: &ThreadRow, now: Instant) -> Option<Duration> {
        self.seen
            .get(&row.pane_id)
            .map(|seen| now.saturating_duration_since(seen.since))
    }

    /// The pane's lines at `width` columns, and the line index of the selected row.
    pub fn lines(&self, width: u16, now: Instant) -> (Vec<Line<'static>>, usize) {
        let width = usize::from(width.max(1));
        let visible = self.visible();
        let selected = self.selected_index(&visible);
        let mut lines = Vec::new();
        let mut selected_line = 0;

        let scope = if self.show_all {
            "all agents"
        } else {
            "lead and threads"
        };
        lines.push(Line::from(Span::styled(
            present_line(&format!("tsk threads · {} · {scope}", visible.len()), width),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        if let Some(error) = &self.error {
            lines.push(Line::from(present_line(&format!("! {error}"), width)));
        }
        if visible.is_empty() && self.error.is_none() {
            lines.push(Line::from(""));
            lines.push(Line::from(present_line(
                if self.show_all {
                    "No live agents in this Herdr session."
                } else {
                    "No lead- or thr- sessions. Press a to show every agent."
                },
                width,
            )));
        }

        let mut index = 0;
        for group in Group::ALL {
            let members: Vec<&ThreadRow> =
                visible.iter().filter(|row| row.group() == group).collect();
            if members.is_empty() {
                continue;
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("{} {}", group.title(), members.len()),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for row in members {
                let is_selected = selected == Some(index);
                if is_selected {
                    selected_line = lines.len();
                }
                lines.extend(self.row_lines(row, is_selected, width, now));
                index += 1;
            }
        }

        lines.push(Line::from(""));
        if let Some(message) = &self.message {
            lines.push(Line::from(present_line(message, width)));
        }
        lines.push(Line::from(Span::styled(
            present_line(
                "enter focus · ↑↓ move · a all agents · r refresh · q quit",
                width,
            ),
            Style::default().add_modifier(Modifier::DIM),
        )));
        (lines, selected_line)
    }

    fn row_lines(
        &self,
        row: &ThreadRow,
        selected: bool,
        width: usize,
        now: Instant,
    ) -> Vec<Line<'static>> {
        let held = self
            .held_for(row, now)
            .map(|d| format!(" {}", short_duration(d)))
            .unwrap_or_default();
        let marker = if selected { "▸ " } else { "  " };
        let focused = if row.focused { " ●" } else { "" };
        let head = format!(
            "{marker}{}{focused}  {}{held}",
            row.display_name(),
            row.status.label()
        );
        if !selected {
            let tail = row
                .inbox
                .as_deref()
                .map(|line| format!("  {line}"))
                .unwrap_or_default();
            return vec![Line::from(present_line(&format!("{head}{tail}"), width))];
        }

        // The selected row unfolds: every detail on its own wrapped line.
        let mut lines = vec![Line::from(Span::styled(
            present_line(&head, width),
            Style::default().add_modifier(Modifier::BOLD),
        ))];
        let detail = |text: String| Line::from(format!("    {}", terminal_text(&text)));
        if let Some(inbox) = &row.inbox {
            lines.push(detail(inbox.clone()));
        }
        let mut meta = Vec::new();
        if let Some(key) = row.issue_key() {
            meta.push(key);
        }
        if let Some(agent) = &row.agent {
            meta.push(agent.clone());
        }
        meta.push(format!("{} {}", row.tab_id, row.pane_id));
        lines.push(Line::from(Span::styled(
            format!("    {}", terminal_text(&meta.join(" · "))),
            Style::default().add_modifier(Modifier::DIM),
        )));
        if let Some(cwd) = &row.cwd {
            lines.push(Line::from(Span::styled(
                format!("    {}", terminal_text(cwd)),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        lines
    }
}

/// "12s", "3m", "2h", "4d".
pub fn short_duration(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=172_799 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// What a key asks the loop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    None,
    Quit,
    Refresh,
    Focus,
}

pub fn handle_key(view: &mut ThreadsView, key: KeyEvent) -> KeyAction {
    if key.kind == KeyEventKind::Release {
        return KeyAction::None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => KeyAction::Quit,
        KeyCode::Char('q') | KeyCode::Esc => KeyAction::Quit,
        KeyCode::Up | KeyCode::Char('k') => {
            view.move_selection(-1);
            KeyAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            view.move_selection(1);
            KeyAction::None
        }
        KeyCode::Home => {
            view.move_selection(isize::MIN / 2);
            KeyAction::None
        }
        KeyCode::End => {
            view.move_selection(isize::MAX / 2);
            KeyAction::None
        }
        KeyCode::Char('a') => {
            view.show_all = !view.show_all;
            KeyAction::None
        }
        KeyCode::Char('r') => KeyAction::Refresh,
        KeyCode::Enter => KeyAction::Focus,
        _ => KeyAction::None,
    }
}

fn draw<B: Backend>(terminal: &mut Terminal<B>, view: &ThreadsView) -> io::Result<()>
where
    B::Error: Into<io::Error>,
{
    terminal
        .draw(|frame| {
            let area = frame.area();
            let (lines, selected_line) = view.lines(area.width, Instant::now());
            let height = usize::from(area.height.max(1));
            // Keep the selected row and its unfolded detail on screen.
            let scroll = selected_line.saturating_sub(height.saturating_sub(6));
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
                area,
            );
        })
        .map(|_| ())
        .map_err(Into::into)
}

/// Run the threads pane until the user quits.
pub fn run() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = event_loop(&mut terminal);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()>
where
    B::Error: Into<io::Error>,
{
    let mut view = ThreadsView::default();
    view.apply(herdr::list_agents(), Instant::now());
    let mut last_refresh = Instant::now();
    loop {
        draw(terminal, &view)?;
        let wait = REFRESH.saturating_sub(last_refresh.elapsed());
        if event::poll(wait.min(Duration::from_millis(250)))? {
            if let Event::Key(key) = event::read()? {
                match handle_key(&mut view, key) {
                    KeyAction::Quit => return Ok(()),
                    KeyAction::Refresh => {
                        view.apply(herdr::list_agents(), Instant::now());
                        last_refresh = Instant::now();
                    }
                    KeyAction::Focus => {
                        view.message = match view.selected_row() {
                            Some(row) => match herdr::focus(&row) {
                                Ok(()) => Some(format!("focused {}", row.display_name())),
                                Err(error) => Some(format!("! {error}")),
                            },
                            None => None,
                        };
                    }
                    KeyAction::None => {}
                }
            }
        }
        if last_refresh.elapsed() >= REFRESH {
            view.apply(herdr::list_agents(), Instant::now());
            last_refresh = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::parse_agent_list;

    const LIST: &str = r#"{"result":{"agents":[
      {"name":"thr-dongwoo-2648","agent":"claude","agent_status":"working",
       "tokens":{"inbox":"2 · tsk fork 위치는?"},"workspace_id":"w1","tab_id":"w1:t2",
       "pane_id":"w1:p3","state_change_seq":4},
      {"name":"lead-main","agent":"claude","agent_status":"idle","workspace_id":"w1",
       "tab_id":"w1:t1","pane_id":"w1:p1","focused":true},
      {"name":"thr-dongwoo-2650","agent":"claude","agent_status":"blocked",
       "tokens":{"inbox":"Allow push main to origin? 허용?"},"workspace_id":"w1",
       "tab_id":"w1:t3","pane_id":"w1:p4"},
      {"agent":"codex","agent_status":"working","workspace_id":"w2","tab_id":"w2:t1",
       "pane_id":"w2:p2"}
    ]}}"#;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn renders_groups_with_the_first_row_unfolded() {
        let now = Instant::now();
        let mut view = ThreadsView::default();
        view.apply(parse_agent_list(LIST), now);
        let (lines, selected) = view.lines(80, now + Duration::from_secs(190));
        assert_eq!(
            text(&lines),
            [
                "tsk threads · 3 · lead and threads",
                "",
                "NEEDS YOU 1",
                "▸ thr-dongwoo-2650  blocked 3m",
                "    Allow push main to origin? 허용?",
                "    DONGWOO-2650 · claude · w1:t3 w1:p4",
                "",
                "IN MOTION 1",
                "  thr-dongwoo-2648  working 3m  2 · tsk fork 위치는?",
                "",
                "ON DECK 1",
                "  lead-main ●  idle 3m",
                "",
                "enter focus · ↑↓ move · a all agents · r refresh · q quit",
            ]
        );
        assert_eq!(selected, 3);
    }

    #[test]
    fn selection_follows_the_agent_across_refreshes() {
        let now = Instant::now();
        let mut view = ThreadsView::default();
        view.apply(parse_agent_list(LIST), now);
        handle_key(&mut view, key(KeyCode::Down));
        assert_eq!(view.selected_row().unwrap().name, "thr-dongwoo-2648");
        // The blocked thread finishes and moves down a group; the cursor stays put.
        let next = LIST.replace("\"blocked\"", "\"idle\"");
        view.apply(parse_agent_list(&next), now);
        assert_eq!(view.selected_row().unwrap().name, "thr-dongwoo-2648");
    }

    #[test]
    fn a_status_change_restarts_the_held_timer() {
        let now = Instant::now();
        let mut view = ThreadsView::default();
        view.apply(parse_agent_list(LIST), now);
        let later = now + Duration::from_secs(600);
        let next = LIST.replace("\"state_change_seq\":4", "\"state_change_seq\":5");
        view.apply(parse_agent_list(&next), later);
        let (lines, _) = view.lines(80, later + Duration::from_secs(5));
        assert!(
            text(&lines).contains(&"  thr-dongwoo-2648  working 5s  2 · tsk fork 위치는?".into())
        );
        assert!(text(&lines).contains(&"  lead-main ●  idle 10m".into()));
    }

    #[test]
    fn a_failed_lookup_keeps_rows_and_says_why() {
        let now = Instant::now();
        let mut view = ThreadsView::default();
        view.apply(parse_agent_list(LIST), now);
        view.apply(Err("herdr agent list failed: down".into()), now);
        let (lines, _) = view.lines(80, now);
        let text = text(&lines);
        assert_eq!(text[1], "! herdr agent list failed: down");
        assert!(text.iter().any(|l| l.contains("thr-dongwoo-2648")));
    }

    #[test]
    fn a_toggles_every_agent_and_keys_map_to_actions() {
        let mut view = ThreadsView::default();
        view.apply(parse_agent_list(LIST), Instant::now());
        assert_eq!(view.visible().len(), 3);
        assert_eq!(
            handle_key(&mut view, key(KeyCode::Char('a'))),
            KeyAction::None
        );
        assert_eq!(view.visible().len(), 4);
        assert_eq!(handle_key(&mut view, key(KeyCode::Enter)), KeyAction::Focus);
        assert_eq!(
            handle_key(&mut view, key(KeyCode::Char('r'))),
            KeyAction::Refresh
        );
        assert_eq!(handle_key(&mut view, key(KeyCode::Esc)), KeyAction::Quit);
    }

    #[test]
    fn control_sequences_in_tokens_are_escaped() {
        let now = Instant::now();
        let mut view = ThreadsView::default();
        let evil = LIST.replace("2 · tsk fork 위치는?", "x\\u001b]0;evil");
        view.apply(parse_agent_list(&evil), now);
        let (lines, _) = view.lines(80, now);
        assert!(text(&lines).iter().all(|l| !l.contains('\u{1b}')));
    }
}
