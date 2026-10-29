//! Queue chrome, overlays, verb bar, and frame drawing hooks.

use std::path::Path;

use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::domain::{direct_children_in, parent_in, HumanStatus, ParentRelation, Task, TaskScope};
use crate::ui::capture::CaptureField;
use crate::ui::edit::{
    escaped_line_window, wrap_text, wrapped_draft_rows, wrapped_edit_rows, EditBuffer,
};
use crate::ui::input::help_card_lines_for_query;
use crate::ui::render::{
    self, BoardSurface, FormDropdown, NavPaint, PaletteCommandRow, QueueFrameModel, QueueOverlay,
    RelationRow, VerbEntry,
};
use crate::ui::tier;
use crate::ui::{present_line, terminal_text};

use super::chrome::{notice_framed, row_width, DELETE_NOTICE_UNDO};
use super::commands::CommandSurface;
use super::launch_card::LaunchPrompt;
use super::model::{
    project_option_label, BoardForm, BoardInputMode, BoardModel, CleanupPrompt, CleanupRow,
    CleanupRowState, CleanupRun, DispatchPrompt, RelaunchPrompt, StartAnywayPrompt,
};
use crate::ui::render::{CleanupCardLine, CleanupTitle};

fn home_dir() -> Option<String> {
    std::env::var("HOME").ok().filter(|home| !home.is_empty())
}

/// Show a path under the home directory as `~/…`.
pub(crate) fn tilde_path(path: &str, home: Option<&str>) -> String {
    let Some(home) = home.map(|home| home.trim_end_matches('/')) else {
        return path.to_string();
    };
    match path.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') && !home.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// A task the card left exactly as it was, after `T7  `.
fn unchanged_line(reason: &str) -> String {
    if reason.starts_with("merge failed") || reason.contains("queued") {
        format!("{reason} · unchanged")
    } else {
        format!("unchanged · {reason}")
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn plain_title(text: String) -> CleanupTitle {
    CleanupTitle {
        full: text.clone(),
        short: text.clone(),
        bare: text,
        question: String::new(),
    }
}

/// How many tasks the card is about, for its `Done N tasks` title.
fn bulk_total(prompt: &CleanupPrompt, bulk: &super::BulkCleanup) -> usize {
    prompt.rows.len() + bulk.refused.len() + bulk.missing.len() + bulk.plain.len()
}

/// The card's rows that need no cleanup: no dispatch, worktree gone, can't inspect.
fn bulk_extra_lines(bulk: &super::BulkCleanup, lines: &mut Vec<CleanupCardLine>) {
    for (_, identifier, reason) in &bulk.refused {
        lines.push(CleanupCardLine::Field {
            label: identifier.clone(),
            value: format!("can't inspect: {reason}"),
        });
    }
    for (_, identifier, _) in &bulk.missing {
        lines.push(CleanupCardLine::Field {
            label: identifier.clone(),
            value: "can't tell if merged: worktree already gone".into(),
        });
    }
    for identifier in &bulk.plain {
        lines.push(CleanupCardLine::Field {
            label: identifier.clone(),
            value: "no dispatch".into(),
        });
    }
}

/// The PR line while `m` merges this row.
fn merging_line(row: &CleanupRow) -> String {
    let number = row
        .pr_number()
        .map_or(String::new(), |number| format!(" #{number}"));
    format!("Merging PR{number} with gh…")
}

/// The cleanup card's semantic body: where each task stands, then one line per option saying
/// what it does to the PR or branch and then locally, then (one task) its paths, then the way
/// out. Nothing on it says what happened: that is the run's card.
pub(crate) fn cleanup_overlay<'a>(prompt: &CleanupPrompt, home: Option<&str>) -> QueueOverlay<'a> {
    let merging_ids = prompt
        .merging
        .as_ref()
        .map(|(_, ids)| ids.clone())
        .unwrap_or_default();
    let mut lines = Vec::new();
    let title = match (&prompt.bulk, prompt.rows.first()) {
        (None, Some(row)) => {
            if let Some(settled) = &prompt.settled {
                let reason = settled.first().map_or("", |(_, reason)| reason.as_str());
                lines.push(CleanupCardLine::Text(format!(
                    "{} · nothing changed",
                    capitalized(reason)
                )));
            } else if merging_ids.contains(&row.task_id) {
                lines.push(CleanupCardLine::Text(merging_line(row)));
            } else {
                lines.push(CleanupCardLine::Text(capitalized(&row.verdict_line())));
            }
            format!("Done T{}", row.number)
        }
        (Some(bulk), _) => {
            if let Some(settled) = &prompt.settled {
                for (identifier, reason) in settled {
                    lines.push(CleanupCardLine::Field {
                        label: identifier.clone(),
                        value: unchanged_line(reason),
                    });
                }
            } else {
                for row in &prompt.rows {
                    lines.push(CleanupCardLine::Field {
                        label: format!("T{}", row.number),
                        value: if merging_ids.contains(&row.task_id) {
                            merging_line(row)
                        } else {
                            row.row_line()
                        },
                    });
                }
                bulk_extra_lines(bulk, &mut lines);
            }
            format!("Done {} tasks", bulk_total(prompt, bulk))
        }
        (None, None) => String::new(),
    };
    if let Some(note) = &prompt.merge_note {
        lines.push(CleanupCardLine::Text(format!("can't merge: {note}")));
    }
    let options = prompt.options();
    if !options.is_empty() {
        lines.push(CleanupCardLine::Blank);
        lines.extend(prompt.selector.lines(&options));
    }
    if let (None, Some(row), None) = (&prompt.bulk, prompt.rows.first(), &prompt.settled) {
        lines.push(CleanupCardLine::Blank);
        lines.push(CleanupCardLine::Field {
            label: "worktree".into(),
            value: tilde_path(&row.worktree, home),
        });
        lines.push(CleanupCardLine::Field {
            label: "branch".into(),
            value: row.branch.clone(),
        });
    }
    // `m` is merging: Esc waits for gh, so the card offers no way out until it answers.
    if !prompt.merging() {
        lines.push(CleanupCardLine::Blank);
        lines.push(CleanupCardLine::Esc(
            if prompt.settled.is_some() {
                "esc close"
            } else {
                "esc cancel"
            }
            .into(),
        ));
    }
    QueueOverlay::CleanupConfirm {
        title: plain_title(title),
        lines,
        scroll: prompt.scroll,
        follow: prompt.follow,
    }
}

/// A confirmed card: what happened to each task so far, as it lands.
pub(crate) fn cleanup_run_overlay<'a>(
    prompt: &CleanupPrompt,
    run: &CleanupRun,
) -> QueueOverlay<'a> {
    let finished = run.finished();
    let (landed, total) = run.progress();
    let mut lines = Vec::new();
    let title = match (run.bulk, run.rows.first()) {
        (None, Some(row)) => {
            lines.push(CleanupCardLine::Text(match &row.state {
                CleanupRowState::Kept { full, .. } => format!("Cleanup refused: {full}"),
                _ => match run.row_outcome(row) {
                    Some(outcome) => format!("Done T{} · {outcome}", row.number),
                    None => "Cleaning up…".into(),
                },
            }));
            format!("Done T{}", row.number)
        }
        (bulk, _) => {
            let current = (landed + 1).min(total.max(1));
            for row in &run.rows {
                let value = match &row.state {
                    CleanupRowState::Checking | CleanupRowState::Queued => "waiting".to_string(),
                    CleanupRowState::Removing => format!("Cleaning {current} of {total}…"),
                    _ => run.row_outcome(row).unwrap_or_default(),
                };
                lines.push(CleanupCardLine::Field {
                    label: format!("T{}", row.number),
                    value,
                });
            }
            for (identifier, reason) in &run.refused {
                lines.push(CleanupCardLine::Field {
                    label: identifier.clone(),
                    value: format!("done · can't inspect: {reason}"),
                });
            }
            if let Some(bulk) = &prompt.bulk {
                for (_, identifier, _) in &bulk.missing {
                    let workspace = run
                        .workspaces
                        .iter()
                        .find(|(closed, _)| closed == identifier)
                        .map(|(_, closed)| match closed {
                            Ok(()) => " · workspace closed".to_string(),
                            Err(why) => format!(" · {why}"),
                        })
                        .unwrap_or_default();
                    lines.push(CleanupCardLine::Field {
                        label: identifier.clone(),
                        value: format!("done · worktree already gone{workspace}"),
                    });
                }
                for identifier in &bulk.plain {
                    lines.push(CleanupCardLine::Field {
                        label: identifier.clone(),
                        value: "done".into(),
                    });
                }
            }
            for (identifier, reason) in &run.unmerged {
                lines.push(CleanupCardLine::Field {
                    label: identifier.clone(),
                    value: unchanged_line(reason),
                });
            }
            let done = bulk.map_or(run.rows.len(), |(done, _)| done);
            format!("Done {} tasks", done + run.unmerged.len())
        }
    };
    lines.push(CleanupCardLine::Blank);
    lines.push(CleanupCardLine::Esc(
        if finished {
            "esc close"
        } else {
            "esc hide, keep cleaning"
        }
        .into(),
    ));
    QueueOverlay::CleanupConfirm {
        title: plain_title(title),
        lines,
        scroll: prompt.scroll,
        follow: false,
    }
}

/// The block card over the cursor task or a marked set.
fn block_card_overlay<'a>(
    model: &BoardModel,
    card: &crate::ui::board::BlockCard,
) -> QueueOverlay<'a> {
    use crate::ui::board::{BlockCardField, OnKind};
    let identifier = |id: &uuid::Uuid| {
        model
            .tasks
            .iter()
            .find(|task| task.id == *id)
            .and_then(|task| task.board_identifier())
    };
    let review = card.kind() == crate::domain::BlockKind::Review;
    let noun = if review { "review" } else { "block" };
    let title = match (card.is_edit(), card.targets()) {
        (true, [id]) => format!("edit {noun} {}", identifier(id).unwrap_or_default()),
        (false, [id]) => format!("{noun} {}", identifier(id).unwrap_or_default()),
        (_, targets) => format!("{noun} {} tasks", targets.len()),
    };
    let (focus, cursor) = match card.field() {
        BlockCardField::Why => (0, card.why().cursor()),
        BlockCardField::On if review => (3, card.on_text().cursor()),
        BlockCardField::On => (1, card.on_text().cursor()),
        BlockCardField::Needs => (2, card.needs().cursor()),
        BlockCardField::Done => (0, card.done().cursor()),
        BlockCardField::Check => (1, card.checks().cursor()),
        BlockCardField::Next => (2, card.next().cursor()),
    };
    QueueOverlay::BlockCard(render::BlockCardPaint {
        title: title.trim().to_string(),
        review,
        why: terminal_text(card.why().value()),
        needs: terminal_text(card.needs().value()),
        done: terminal_text(card.done().value()),
        checks: card
            .checks()
            .value()
            .split('\n')
            .map(terminal_text)
            .collect::<Vec<_>>()
            .join("\n"),
        next: terminal_text(card.next().value()),
        on_kind: match card.on_kind() {
            OnKind::You => "you",
            OnKind::Task => "task",
            OnKind::Agent => "agent",
            OnKind::Other => "other",
        },
        on_text: terminal_text(card.on_text().value()),
        focus,
        cursor,
        refusal: card.refusal().map(str::to_string),
        edit: card.is_edit(),
    })
}

/// The bulk start card: one row per task `y` launches (assignee and base), then the start-only
/// and skipped tasks, then its one option and the way out. `default_branch` names a
/// repository's remote default.
pub(crate) fn dispatch_overlay<'a>(
    prompt: &DispatchPrompt,
    default_branch: impl Fn(&Path) -> String,
) -> QueueOverlay<'a> {
    if let Some(relaunch) = &prompt.relaunch {
        return relaunch_overlay(prompt, relaunch);
    }
    if let Some(start) = &prompt.start_anyway {
        return start_anyway_overlay(prompt, start);
    }
    let count = prompt.launch.len();
    let noun = if count == 1 { "task" } else { "tasks" };
    let title = if count == 0 {
        let starts = prompt.start_only.len();
        let noun = if starts == 1 { "task" } else { "tasks" };
        CleanupTitle {
            full: format!("Start {starts} {noun}?"),
            short: format!("Start {starts}?"),
            bare: "Start".into(),
            question: format!("Start {starts} {noun}?"),
        }
    } else {
        CleanupTitle {
            full: format!("Dispatch {count} {noun}?"),
            short: format!("Dispatch {count}?"),
            bare: "Dispatch".into(),
            question: format!("Dispatch {count} {noun}?"),
        }
    };
    let mut lines = Vec::new();
    for eligible in &prompt.launch {
        let base = match eligible.base() {
            Some(base) => format!("from {base}"),
            None => match default_branch(eligible.project()) {
                default if default == "default" => "from default".to_string(),
                default => format!("from default ({default})"),
            },
        };
        let checking = if prompt.checking() {
            "  checking…"
        } else {
            ""
        };
        lines.push(CleanupCardLine::Field {
            label: format!("T{}", eligible.number),
            value: format!("@{}  {base}{checking}", eligible.assignee),
        });
    }
    if !prompt.start_only.is_empty() {
        if !lines.is_empty() {
            lines.push(CleanupCardLine::Blank);
        }
        lines.push(CleanupCardLine::Text("start only".into()));
        for (identifier, id) in &prompt.start_only {
            let value =
                if let Some((_, reason)) = prompt.no_launch.iter().find(|(row, _)| row == id) {
                    format!("started · no launch: {reason}")
                } else if prompt.corrections.contains(id) {
                    "status correction, no launch".to_string()
                } else {
                    "started, no launch".to_string()
                };
            lines.push(CleanupCardLine::Field {
                label: identifier.clone(),
                value,
            });
        }
    }
    if !prompt.skipped.is_empty() {
        if !lines.is_empty() {
            lines.push(CleanupCardLine::Blank);
        }
        lines.push(CleanupCardLine::Text("not started".into()));
        for (identifier, reason) in &prompt.skipped {
            lines.push(CleanupCardLine::Field {
                label: identifier.clone(),
                value: reason.clone(),
            });
        }
    }
    start_card(prompt, title, lines, Vec::new())
}

/// A start card's body, then its options, then `trailer` (fields such as the worktree), then
/// the dim `esc cancel` line, one blank line between sections.
fn start_card<'a>(
    prompt: &DispatchPrompt,
    title: CleanupTitle,
    mut lines: Vec<CleanupCardLine>,
    trailer: Vec<CleanupCardLine>,
) -> QueueOverlay<'a> {
    let options = prompt.options();
    if !options.is_empty() {
        if !lines.is_empty() {
            lines.push(CleanupCardLine::Blank);
        }
        lines.extend(prompt.selector.lines(&options));
    }
    if !trailer.is_empty() {
        lines.push(CleanupCardLine::Blank);
        lines.extend(trailer);
    }
    lines.push(CleanupCardLine::Blank);
    lines.push(CleanupCardLine::Esc("esc cancel".into()));
    QueueOverlay::CleanupConfirm {
        title,
        lines,
        scroll: prompt.scroll,
        follow: prompt.follow,
    }
}

/// The launch card: the board opened in an archived project's directory. No title: the one
/// body line asks the question, then the two options and the way out (`esc` keeps it archived).
fn launch_overlay<'a>(prompt: &LaunchPrompt) -> QueueOverlay<'a> {
    let name = prompt.name();
    let mut lines = vec![
        CleanupCardLine::Text(format!(
            "project {name} is archived, would you like to unarchive it?"
        )),
        CleanupCardLine::Blank,
    ];
    lines.extend(prompt.selector.lines(&prompt.options()));
    lines.push(CleanupCardLine::Blank);
    lines.push(CleanupCardLine::Esc("esc keep archived".into()));
    QueueOverlay::CleanupConfirm {
        title: CleanupTitle {
            full: String::new(),
            short: String::new(),
            bare: String::new(),
            question: String::new(),
        },
        lines,
        scroll: prompt.scroll,
        follow: prompt.follow,
    }
}

/// The start-anyway card: `T203 runs after T202 (started).` per waiting target.
fn start_anyway_overlay<'a>(
    prompt: &DispatchPrompt,
    start: &StartAnywayPrompt,
) -> QueueOverlay<'a> {
    let title = CleanupTitle {
        full: "Start anyway?".into(),
        short: "Start anyway?".into(),
        bare: "Start".into(),
        question: "Start anyway?".into(),
    };
    let lines = start
        .waiting
        .iter()
        .map(|line| CleanupCardLine::Text(format!("{line}.")))
        .collect();
    start_card(prompt, title, lines, Vec::new())
}

/// The relaunch card: one cursor task whose dispatched agent is gone.
fn relaunch_overlay<'a>(prompt: &DispatchPrompt, relaunch: &RelaunchPrompt) -> QueueOverlay<'a> {
    let number = relaunch.number;
    let assignee = &relaunch.assignee;
    let title = CleanupTitle {
        full: format!("T{number} · relaunch @{assignee}?"),
        short: format!("Relaunch @{assignee}?"),
        bare: format!("T{number}"),
        question: format!("Relaunch @{assignee}?"),
    };
    let lines = vec![CleanupCardLine::Text(format!(
        "@{assignee} is no longer running."
    ))];
    let worktree = vec![CleanupCardLine::Field {
        label: "worktree".into(),
        value: relaunch.worktree.clone(),
    }];
    start_card(prompt, title, lines, worktree)
}

/// Verb bar for the base board list.
///
/// The bar is a prompt, not a keymap: the few things you are most likely to do next from
/// where the cursor is, then the way out, then `: commands · ? help`. Everything else lives
/// in `:` and `?`. Every surface reads the same shape (primary · status · create · out ·
/// commands · help). A narrow footer drops the status and create seats from the right,
/// whole; the primary action, the way out, `: commands`, and `? help` stay.
pub fn board_verb_items(model: &BoardModel) -> Vec<VerbEntry<'static>> {
    const HELP: VerbEntry<'static> = VerbEntry {
        key: "?",
        label: "help",
        shed: 0,
    };
    const COMMANDS: VerbEntry<'static> = render::COMMANDS_VERB;
    const ADD: VerbEntry<'static> = VerbEntry {
        key: "+",
        label: "add",
        shed: 1,
    };
    const OPEN: VerbEntry<'static> = VerbEntry {
        key: "enter",
        label: "open",
        shed: 0,
    };
    const UNDO: VerbEntry<'static> = VerbEntry {
        key: "u",
        label: "undo",
        shed: 0,
    };
    // After a delete the board's navigation legends lead with its undo. The seat follows the
    // delete's undo entry, not the status-row notice: the notice clears on the next move or
    // after a few seconds, while `ctrl+u` keeps meaning "undo that delete" until the entry is
    // undone, superseded, or stale. It is Normal-mode only, and an open palette covers it
    // just as it covers the notice.
    let undo_seat = model.undo_hint_live()
        && model.input_mode() == BoardInputMode::Normal
        && model.surface == CommandSurface::None;

    if model.input_mode() == BoardInputMode::Search {
        return vec![
            VerbEntry {
                key: "enter",
                label: "pin",
                shed: 0,
            },
            VerbEntry {
                key: "esc",
                label: "clear",
                shed: 0,
            },
        ];
    }

    // The inline step editor: Enter saves this step (and opens the next row on an add),
    // Shift+Enter saves the whole task session.
    if model.input_mode() == BoardInputMode::EditStep {
        return vec![
            VerbEntry {
                key: "enter",
                label: "save step",
                shed: 0,
            },
            VerbEntry {
                key: "shift+enter",
                label: "save all",
                shed: 1,
            },
            VerbEntry {
                key: "esc",
                label: "cancel",
                shed: 0,
            },
        ];
    }

    if model.input_mode() == BoardInputMode::EditReply && model.reply_is_feedback() {
        return vec![
            VerbEntry {
                key: "shift+enter",
                label: "save",
                shed: 0,
            },
            VerbEntry {
                key: "ctrl+s",
                label: "send back",
                shed: 1,
            },
            VerbEntry {
                key: "ctrl+d",
                label: "approve",
                shed: 1,
            },
            VerbEntry {
                key: "esc",
                label: "cancel",
                shed: 0,
            },
        ];
    }

    if model.input_mode() == BoardInputMode::EditReply {
        return vec![
            VerbEntry {
                key: "shift+enter",
                label: "save",
                shed: 0,
            },
            VerbEntry {
                key: "ctrl+s",
                label: "save and unblock",
                shed: 1,
            },
            VerbEntry {
                key: "esc",
                label: "cancel",
                shed: 0,
            },
        ];
    }

    // The task page's view mode: its own legend, true for the bound task.
    let page_task = if model.input_mode() == BoardInputMode::TaskPage {
        model
            .form
            .as_ref()
            .filter(|form| form.is_task())
            .and_then(|form| form.task_id())
            .and_then(|id| model.tasks.iter().find(|t| t.id == id))
    } else {
        None
    };
    if let Some(task) = page_task {
        return task_page_verb_items(model, task);
    }

    // Header rows hold the selection: their own verbs only.
    if model.archived_header_selected() || model.inbox_header_selected() {
        let collapsed = if model.archived_header_selected() {
            model.archived_collapsed
        } else {
            model.inbox_collapsed
        };
        let mut items = Vec::with_capacity(5);
        if undo_seat {
            items.push(UNDO);
        }
        items.push(VerbEntry {
            key: "enter",
            label: if collapsed { "expand" } else { "collapse" },
            shed: 0,
        });
        items.extend([ADD, COMMANDS, HELP]);
        return items;
    }

    // On a task row the undo seat keeps only the selected row's first truthful status verb,
    // so the compact budget sheds only `+ add` and no tier clips an action. The
    // remaining status verbs stay on their keys and in `:`. On an archived row `ctrl+u`
    // restores that row instead, so the seat stands down there rather than name the wrong
    // action.
    let selected_task = model
        .selected_id()
        .and_then(|id| model.tasks.iter().find(|t| t.id == id));
    if undo_seat && !selected_task.is_some_and(|task| task.archived) {
        let mut armed = vec![UNDO];
        if let Some(task) = selected_task {
            armed.push(OPEN);
            if let Some(verb) = status_verbs(task.status).into_iter().next() {
                armed.push(verb);
            }
        }
        armed.push(ADD);
        armed.push(COMMANDS);
        armed.push(HELP);
        return armed;
    }
    let mut entries = Vec::with_capacity(7);
    if let Some(task) = selected_task {
        entries.push(OPEN);
        entries.extend(status_verbs(task.status));
        // Add is useful from the backlog and inbox, but the status-heavy in-motion and
        // done legends use that seat for their truthful lifecycle actions.
        if !matches!(
            task.status,
            HumanStatus::Started | HumanStatus::Blocked | HumanStatus::Review | HumanStatus::Done
        ) {
            entries.push(ADD);
        }
    }
    entries.extend([COMMANDS, HELP]);
    entries
}

/// The human progress verbs a status offers.
fn status_verbs(status: HumanStatus) -> Vec<VerbEntry<'static>> {
    const DOING: VerbEntry<'static> = VerbEntry {
        key: "s",
        label: "doing",
        shed: 1,
    };
    const TODO: VerbEntry<'static> = VerbEntry {
        key: "o",
        label: "todo",
        shed: 1,
    };
    const DONE: VerbEntry<'static> = VerbEntry {
        key: "d",
        label: "done",
        shed: 1,
    };
    match status {
        HumanStatus::Open | HumanStatus::Ready => vec![DOING, DONE],
        HumanStatus::Started => vec![
            TODO,
            DONE,
            VerbEntry {
                key: "b",
                label: "block",
                shed: 1,
            },
        ],
        HumanStatus::Blocked => vec![
            DONE,
            VerbEntry {
                key: "b",
                label: "unblock",
                shed: 1,
            },
            TODO,
        ],
        HumanStatus::Review => vec![
            DONE,
            VerbEntry {
                key: "b",
                label: "block",
                shed: 1,
            },
            TODO,
        ],
        HumanStatus::Done => vec![
            TODO,
            VerbEntry {
                key: "u",
                label: "undo",
                shed: 1,
            },
        ],
    }
}

fn task_page_verb_items(model: &BoardModel, task: &crate::domain::Task) -> Vec<VerbEntry<'static>> {
    // A parked edit session (dirty draft, no editor open) is about saving or discarding.
    if model.task_editing() {
        return vec![
            VerbEntry {
                key: "shift+enter",
                label: "save",
                shed: 0,
            },
            VerbEntry {
                key: "a",
                label: "add step",
                shed: 1,
            },
            VerbEntry {
                key: "esc",
                label: "cancel",
                shed: 0,
            },
            render::COMMANDS_VERB,
        ];
    }
    let mut entries = Vec::with_capacity(7);
    entries.push(VerbEntry {
        key: "e",
        label: "edit",
        shed: 0,
    });
    entries.extend(status_verbs(task.status));
    entries.push(VerbEntry {
        key: "esc",
        label: "close",
        shed: 0,
    });
    entries.push(render::COMMANDS_VERB);
    entries
}

/// Build the task page's paint payload from the open task form. View mode wraps the notes
/// draft and windows it by the page scroll; field edits reuse the form's cursor windowing.
fn build_task_page_overlay<'a>(
    model: &'a BoardModel,
    form: &'a BoardForm,
    geo: &tier::TierGeometry,
    scope_dropdown: Option<FormDropdown<'a>>,
    column: bool,
) -> QueueOverlay<'a> {
    let width = geo.row_width as usize;
    let bound_task = form
        .task_id()
        .and_then(|id| model.tasks.iter().find(|task| task.id == id));
    // The section consumes extracted, word-wrapped step views, never the raw storage. The
    // reserved width is the scrollbar-safe content width less the step glyph and trailing
    // pad, matching Notes' no-truncation behavior.
    let step_text_width = width.saturating_sub(8);
    form.steps.text_width.set(step_text_width);
    // Task-edit removals are staged, not durable, but they immediately leave the rendered
    // list. Keep the task's source index only in the session state, then derive this compact
    // visible list so the renderer never receives a row it must hide conditionally.
    let mut step_views: Vec<render::StepView> = bound_task
        .map(|task| {
            task.steps
                .iter()
                .filter(|step| !form.steps.removals.contains(&step.id))
                .map(|step| render::StepView {
                    done: step.done,
                    inserted: false,
                    rows: crate::ui::edit::wrap_text(&step.text, step_text_width)
                        .into_iter()
                        .map(|row| row.text)
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();
    for text in &form.steps.pending_adds {
        step_views.push(render::StepView {
            done: false,
            inserted: false,
            rows: crate::ui::edit::wrap_text(text, step_text_width)
                .into_iter()
                .map(|row| row.text)
                .collect(),
        });
    }
    let stored_step_count = step_views.len();
    // Existing-step edits are task-session drafts. Paint every parked draft first, then the
    // active row over it. Only the active EditStep mode receives cursor metadata, so moving to
    // Title, Notes, or Scope leaves the changed step visible without a second cursor.
    if let Some(task) = bound_task {
        for (visible, step) in task
            .steps
            .iter()
            .filter(|step| !form.steps.removals.contains(&step.id))
            .enumerate()
        {
            if let Some(draft) = form.steps.drafts.get(&step.id) {
                let (rows, _, _) = wrapped_edit_rows(draft, step_text_width);
                step_views[visible].rows = rows;
            }
        }
    }
    let inline_step_editor = form.steps.editor.as_ref().and_then(|editor| {
        let index = match editor.rename {
            Some(step_id) => bound_task?
                .steps
                .iter()
                .filter(|step| !form.steps.removals.contains(&step.id))
                .position(|step| step.id == step_id)?,
            // A positioned add row paints directly below its anchor; otherwise it trails.
            None => editor
                .insert_after
                .and_then(|anchor| {
                    bound_task?
                        .steps
                        .iter()
                        .filter(|step| !form.steps.removals.contains(&step.id))
                        .position(|step| step.id == anchor)
                })
                .map_or(step_views.len(), |anchor| anchor + 1),
        };
        let (rows, cursor_row, cursor_col) = wrapped_edit_rows(&editor.buffer, step_text_width);
        if editor.rename.is_some() {
            step_views[index].rows = rows;
        } else {
            let inserted = index < step_views.len();
            step_views.insert(
                index,
                render::StepView {
                    done: false,
                    rows,
                    inserted,
                },
            );
        }
        (model.input_mode() == BoardInputMode::EditStep).then_some(render::InlineStepEditor {
            index,
            cursor_row: u16::try_from(cursor_row).unwrap_or(u16::MAX),
            cursor_col: u16::try_from(cursor_col).unwrap_or(u16::MAX),
            refusal: editor.refusal.as_deref(),
        })
    });
    // The page cursor stores source indices, while the overlay only carries visible rows.
    // Translate at the boundary, so a staged removal cannot make the selector jump to a
    // different stored step just because its former array slot disappeared.
    let visible_step_index = |source_index: usize| {
        if let Some(task) = bound_task {
            task.steps
                .iter()
                .enumerate()
                .filter(|(_, step)| !form.steps.removals.contains(&step.id))
                .position(|(index, _)| index == source_index)
        } else {
            // Capture steps are locally staged, so their cursor is already an index into the
            // rendered pending-add rows rather than a source index in a bound task.
            (source_index < stored_step_count).then_some(source_index)
        }
    };
    // Cursor state uses source indices, so keep the recorded row counts source-aligned too.
    // Staged removals occupy zero rows; the active add row is not addressable by this cursor.
    let mut visible_counts = step_views
        .iter()
        .filter(|step| !step.inserted)
        .take(stored_step_count)
        .map(|step| step.rows.len().max(1));
    form.steps.row_counts.replace(
        bound_task
            .map(|task| {
                task.steps
                    .iter()
                    .map(|step| {
                        if form.steps.removals.contains(&step.id) {
                            0
                        } else {
                            visible_counts.next().unwrap_or(1)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
    );
    // A notes edit always keeps one row: the layout reserves it (the section caps
    // around it), so an active edit can never be scrolled/clamped out of the frame.
    let page_geo = *geo;
    let status = bound_task
        .map(|task| task.status)
        .unwrap_or(HumanStatus::Todo);
    // AC-8: an archived task's header slot reads `archived` in place of the status word.
    let status_word = match bound_task {
        Some(task) => page_status_word(task),
        None => status_name(status),
    };
    let glyph = bound_task
        .map(|task| render::task_status_glyph(task, &model.tasks))
        .unwrap_or_else(|| render::status_glyph(status));

    // Meta footer: assignee · base · after · scope. The PAPER TRAIL carries when the task was
    // created and last changed. The identifier belongs in the header, so it never competes with
    // footer hits.
    // A wide column moves the project up into its header slot: the scope footer paints only
    // while the edit session can change it, so its control stays reachable by mouse.
    let editing_session = !form.is_task() || form.editing || model.open_field_edit().is_some();
    let show_scope = !column || editing_session;
    let meta_scope = if show_scope {
        match &form.scope {
            TaskScope::Project { path } => render::short_project(path).to_string(),
            TaskScope::Global => "desk".to_string(),
        }
    } else {
        String::new()
    };
    let meta_scope_width = u16::try_from(render::display_width(&meta_scope)).unwrap_or(u16::MAX);
    let mut meta = String::new();
    let capture_form = !form.is_task();
    let shown_assignee = if capture_form || (form.is_task() && form.editing) {
        form.assignee.as_ref().map(|assignee| assignee.name.clone())
    } else {
        bound_task.and_then(|task| task.assignee.clone())
    };
    // View mode offers the quick picker on an unassigned task: a clickable `+ assign` in the
    // assignee slot, but only when a profile exists to choose. The peek never paints it.
    let assignee_segment = shown_assignee
        .map(|assignee| render::assignee_handle(&assignee))
        .or_else(|| {
            (capture_form || (form.is_task() && form.editing)).then(|| "assignee".to_string())
        })
        .or_else(|| {
            (bound_task.is_some_and(|task| !task.is_notice()) && !model.agent_names.is_empty())
                .then(|| "+ assign".to_string())
        });
    let meta_assignee_x = assignee_segment.as_ref().map(|_| 0);
    let meta_assignee_width = assignee_segment
        .as_deref()
        .map(render::display_width)
        .and_then(|width| u16::try_from(width).ok())
        .unwrap_or(0);
    if let Some(segment) = assignee_segment {
        meta.push_str(&segment);
    }

    let shown_base = if capture_form || (form.is_task() && form.editing) {
        form.base.as_deref()
    } else {
        bound_task.and_then(|task| task.base.as_deref())
    };
    let show_base = capture_form || bound_task.is_some_and(|task| !task.is_notice());
    let base_segment = show_base.then(|| {
        shown_base.map_or_else(
            || {
                let scope = if capture_form || form.editing {
                    &form.scope
                } else {
                    bound_task.map_or(&form.scope, |task| &task.scope)
                };
                let default = match scope {
                    TaskScope::Project { path } => model.default_branch_name(Path::new(path)),
                    TaskScope::Global => "default".to_string(),
                };
                if default == "default" {
                    "⎇ default".to_string()
                } else {
                    format!("⎇ {default} (default)")
                }
            },
            |base| format!("⎇ {}", terminal_text(base)),
        )
    });
    let meta_base_x = base_segment.as_ref().map(|_| {
        u16::try_from(render::display_width(&meta) + usize::from(!meta.is_empty()) * 3)
            .unwrap_or(u16::MAX)
    });
    let meta_base_width = base_segment
        .as_deref()
        .map(render::display_width)
        .and_then(|width| u16::try_from(width).ok())
        .unwrap_or(0);
    if let Some(segment) = base_segment {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(&segment);
    }

    // `after T202, T205` beside the base, hidden when empty outside an edit session; the
    // read-only `before T203` follows, derived from the other tasks' `after`.
    let editing_links = capture_form || (form.is_task() && form.editing);
    let shown_after: &[u64] = if editing_links {
        &form.after
    } else {
        bound_task.map_or(&[], |task| task.after.as_slice())
    };
    let after_segment = if !shown_after.is_empty() {
        Some(format!(
            "after {}",
            shown_after
                .iter()
                .map(|number| format!("T{number}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    } else {
        editing_links.then(|| "after".to_string())
    };
    let before_segment = bound_task
        .filter(|_| !capture_form)
        .map(|task| render::before_numbers(task, &model.tasks))
        .filter(|before| !before.is_empty())
        .map(|before| {
            format!(
                "before {}",
                before
                    .iter()
                    .map(|number| format!("T{number}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
    for segment in [after_segment, before_segment].into_iter().flatten() {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(&segment);
    }

    if !meta_scope.is_empty() && !meta.is_empty() {
        meta.push_str(" · ");
    }
    let meta_scope_x = u16::try_from(render::display_width(&meta)).unwrap_or(u16::MAX);
    if !meta_scope.is_empty() {
        meta.push_str(&meta_scope);
    }
    if !editing_session && bound_task.is_none_or(|task| !task.is_notice()) {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str("machine ");
        meta.push_str(&terminal_text(bound_task.map_or("current", |task| {
            task.machine.as_deref().unwrap_or("unspecified")
        })));
    }
    // Header: indent + glyph + the WRAPPED title rows + right-aligned status word
    // on row 0. A long title wraps onto further bold rows indented under the
    // glyph instead of truncating; edit mode wraps the draft with its cursor.
    // The uniform budget keeps every row's wrap identical.
    let editing_title = model.input_mode() == BoardInputMode::EditTitle;
    let header_identifier = (!editing_title)
        .then(|| bound_task.and_then(|task| task.board_identifier()))
        .flatten();
    let mut header_rows: Vec<String> = Vec::new();
    let mut title_cursor = None;
    // The wide column paints its own header (`draw_task_column`): its rows are the bare
    // wrapped title, without the glyph prefix, at the room the identifier and state slot
    // leave. They are capped so one body row survives under the header rule.
    if column {
        let editing = editing_title && model.focused_surface() == tier::FocusedSurface::Task;
        let glyph_w = render::display_width(glyph);
        let id_w = bound_task
            .and_then(|task| task.board_identifier())
            .as_deref()
            .map(render::display_width)
            .unwrap_or(0);
        // The painted state slot keeps one trailing pad cell.
        let state_w = bound_task
            .map(|task| render::display_width(&task_header_state(model, form, task)))
            .unwrap_or(0)
            + 1;
        let room = render::task_header_title_room(width, glyph_w, id_w, state_w).max(1);
        let header_cap = render::task_column_title_cap(&page_geo);
        form.title_wrap_width.set(room);
        if editing {
            let (mut rows, cursor_row, cursor_col) = wrapped_edit_rows(&form.title, room);
            let overflowed = rows.len() > header_cap;
            rows.truncate(header_cap);
            if overflowed {
                if let Some(last) = rows.last_mut() {
                    *last = present_line(last, room.saturating_sub(1));
                }
            }
            let shown_cursor_row = cursor_row.min(rows.len().saturating_sub(1));
            let shown_cursor_col = if cursor_row > shown_cursor_row {
                rows.last()
                    .map(|last| render::display_width(last))
                    .unwrap_or(0)
            } else {
                cursor_col
            };
            title_cursor = Some((
                u16::try_from(shown_cursor_row).unwrap_or(u16::MAX),
                u16::try_from(shown_cursor_col).unwrap_or(u16::MAX),
            ));
            header_rows = rows;
        } else {
            let mut rows: Vec<String> = wrap_text(form.title.value(), room)
                .iter()
                .map(|row| row.text.clone())
                .collect();
            let overflowed = rows.len() > header_cap;
            rows.truncate(header_cap);
            if overflowed {
                if let Some(last) = rows.last_mut() {
                    *last = present_line(last, room.saturating_sub(1));
                }
            }
            header_rows = rows;
        }
    } else {
        let word_cells = status_word.chars().count() + 1;
        let identifier_cells = header_identifier
            .as_deref()
            .map(render::display_width)
            .unwrap_or(0);
        let title_avail = width
            .saturating_sub(4 + word_cells + identifier_cells + usize::from(identifier_cells > 0));
        // The header may grow only inside the page body: it must stop one row short
        // of the lowest chrome row with one note row still living under it, or a
        // pathological title would eat the page (and the painter's chrome).
        let page_bottom = [page_geo.rule_row, page_geo.status_row, page_geo.verb_row]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(page_geo.height);
        let footer_rows = wrap_text(&meta, width.saturating_sub(2)).len();
        let footer_rows = u16::try_from(footer_rows)
            .unwrap_or(u16::MAX)
            .min(page_bottom.saturating_sub(3));
        let header_cap = page_bottom
            .saturating_sub(footer_rows.saturating_add(2))
            .max(1) as usize;
        form.title_wrap_width.set(title_avail);
        if editing_title {
            let (mut rows, cursor_row, cursor_col) = wrapped_edit_rows(&form.title, title_avail);
            let overflowed = rows.len() > header_cap;
            rows.truncate(header_cap);
            if overflowed {
                if let Some(last) = rows.last_mut() {
                    *last = present_line(last, title_avail.saturating_sub(1));
                }
            }
            for (offset, segment) in rows.iter().enumerate() {
                if offset == 0 {
                    header_rows.push(format!("{glyph} {segment}"));
                } else {
                    header_rows.push(segment.clone());
                }
            }
            // A caret hidden below the cap parks at the END of the last shown row:
            // its own hidden column would otherwise paint an unrelated position on
            // the ellipsis row.
            let shown_cursor_row = cursor_row.min(rows.len().saturating_sub(1));
            let shown_cursor_col = if cursor_row > shown_cursor_row {
                rows.last()
                    .map(|last| render::display_width(last))
                    .unwrap_or(0)
            } else {
                cursor_col
            };
            title_cursor = Some((
                u16::try_from(shown_cursor_row).unwrap_or(u16::MAX),
                u16::try_from(shown_cursor_col).unwrap_or(u16::MAX),
            ));
        } else {
            let mut rows: Vec<String> = wrap_text(form.title.value(), title_avail)
                .iter()
                .map(|row| row.text.clone())
                .collect();
            let overflowed = rows.len() > header_cap;
            rows.truncate(header_cap);
            if overflowed {
                if let Some(last) = rows.last_mut() {
                    *last = present_line(last, title_avail.saturating_sub(1));
                }
            }
            for (offset, row) in rows.iter().enumerate() {
                if offset == 0 {
                    header_rows.push(format!("{glyph} {row}"));
                } else {
                    header_rows.push(row.clone());
                }
            }
        }
    }
    let lay = if column {
        render::task_column_layout_with_meta(&page_geo, header_rows.len().max(1) as u16, &meta)
    } else {
        render::task_page_layout_with_meta(&page_geo, header_rows.len().max(1) as u16, &meta)
    };
    // The renderer and input reducer share this viewport size for page scrolling.
    form.steps.window_rows.set(lay.notes_rows as usize);

    // View mode supplies every wrapped note row; Notes edit mode wraps too, with the
    // caret mapped into wrapped coordinates. The shared page painter combines that
    // stream with the steps, then windows it once against the fixed viewport.
    // Wrap at the width the painter can show WHOLE: the content region less its
    // two-cell gutter and one further reserved cell, taken in the scrollbar state
    // (content_width - 3 there), so an overflowing page never re-wraps rows that
    // were already painted -- and no wrapped row ever ends in the presenter's … .
    let notes_width = width.saturating_sub(6);
    let want = lay.notes_rows as usize;
    let editing_notes = model.input_mode() == BoardInputMode::EditNotes;
    // An edit session paints the raw wrap rows, which a click maps straight into the
    // draft; reading outside one paints the notes as rendered markdown, wrapped on the
    // rendered text (see `crate::ui::markdown::render`).
    let mut notes_lines = Vec::new();
    let (mut notes_rows, notes_cursor, more_lines, notes_scroll) = if editing_notes {
        let (all_rows, cursor_row, cursor_column) = wrapped_edit_rows(&form.notes, notes_width);
        // Explicit pointer scrolling may leave the caret offscreen to reach steps.
        // Typing or moving the caret restores automatic following.
        let follow = if form.manual_page_scroll {
            form.notes_scroll
        } else {
            form.notes_scroll.clamp(
                cursor_row.saturating_sub(want.saturating_sub(1)),
                cursor_row,
            )
        };
        (
            all_rows,
            (!form.manual_page_scroll).then_some((
                u16::try_from(cursor_row).unwrap_or(u16::MAX),
                u16::try_from(cursor_column).unwrap_or(u16::MAX),
            )),
            0,
            follow,
        )
    } else if form.notes.value().trim().is_empty() {
        (Vec::new(), None, 0, form.notes_scroll)
    } else if editing_session {
        (
            wrapped_draft_rows(&form.notes, notes_width),
            None,
            0,
            form.notes_scroll,
        )
    } else {
        notes_lines =
            crate::ui::markdown::render(form.notes.value(), notes_width, render::style_plain());
        (
            notes_lines
                .iter()
                .map(crate::ui::markdown::line_text)
                .collect(),
            None,
            0,
            form.notes_scroll,
        )
    };
    if !editing_notes {
        if let Some(dispatch) = bound_task.and_then(|task| task.dispatch.as_ref()) {
            if !notes_rows.is_empty() {
                notes_rows.push(String::new());
            }
            let when = render::format_age(model.now(), dispatch.at);
            let mut dispatch_lines = vec![
                if dispatch.cleaned {
                    "dispatch · cleaned".to_string()
                } else {
                    "dispatch".to_string()
                },
                if dispatch.cleaned {
                    format!("worktree removed · {}", terminal_text(&dispatch.worktree))
                } else {
                    format!("worktree {}", terminal_text(&dispatch.worktree))
                },
                format!("branch {}", terminal_text(&dispatch.branch)),
            ];
            if let Some(base) = dispatch.base.as_ref() {
                let commit = dispatch
                    .base_commit
                    .as_deref()
                    .map(|commit| commit.chars().take(7).collect::<String>());
                dispatch_lines.push(match commit {
                    Some(commit) => format!("from {} @ {commit}", terminal_text(base)),
                    None => format!("from {}", terminal_text(base)),
                });
            }
            dispatch_lines.push(format!("when {when} ago"));
            for line in dispatch_lines {
                notes_rows.extend(
                    wrap_text(&line, notes_width)
                        .into_iter()
                        .map(|row| row.text),
                );
            }
        }
    }
    // A blocked task leads its page with the BLOCKED section, a task in review with the REVIEW
    // section, outside an edit session.
    let (block_rows, block_stops, block_cursor) = match bound_task {
        Some(task) if !editing_session && super::block::has_open_section(task) => {
            block_page_rows(model, form, task, notes_width)
        }
        _ => (Vec::new(), Vec::new(), None),
    };
    // The task's own form draws the trail, so the wide task column paints it too, through an
    // edit session as well (the page body never jumps as one starts or ends).
    let (trail_rows, trail_stops) = match bound_task.filter(|task| !task.is_notice()) {
        Some(task) => trail_page_rows(model, form, task, notes_width),
        None => (Vec::new(), Vec::new()),
    };
    let step_rows: usize = step_views.iter().map(|step| step.rows.len().max(1)).sum();
    // Relation actions are view-mode navigation: an edit session paints no relation cursor.
    let relation_cursor = (!form.editing)
        .then_some(form.steps.relation_cursor)
        .flatten();
    let relation_rows = bound_task
        .map(|task| {
            let mut rows = relation_rows(&model.tasks, task, notes_width, relation_cursor);
            let first = 1 + crate::domain::direct_children_in(&model.tasks, task.id).len();
            question_and_link_rows(
                &mut rows,
                &model.tasks,
                task,
                &model.page_questions(task.id),
                notes_width,
                relation_cursor,
                first,
            );
            push_report_hint_row(
                &mut rows,
                model.reports.feed.get(task.id),
                notes_width,
                relation_cursor,
            );
            rows
        })
        .unwrap_or_default();
    let parent_rows = relation_rows.iter().filter(|row| !row.after_steps).count();
    let child_rows = relation_rows.len() - parent_rows;
    // Match the painter's stream exactly: it always paints one notes row and a trailing
    // `+ step` row, even when both stored notes and stored steps are empty.
    let content = render::page_content_layout(
        block_rows.len() + notes_rows.len().max(1),
        parent_rows,
        child_rows,
        step_rows + 1,
        trail_rows.len(),
        lay.notes_rows,
    );
    form.block.rows.replace(
        block_stops
            .into_iter()
            .chain(
                trail_stops
                    .into_iter()
                    .map(|(stop, row)| (stop, content.trail_start + row)),
            )
            .collect(),
    );
    // The reply box's caret stays in view while it is typed into.
    let notes_scroll = match block_cursor {
        Some((row, _)) if !form.manual_page_scroll => notes_scroll
            .clamp(row.saturating_sub(want.saturating_sub(1)), row)
            .min(content.max_scroll),
        _ => notes_scroll,
    };
    form.notes_max_scroll.set(content.max_scroll);
    form.steps.content_start.set(content.steps_start);
    // Absolute first row and row count of every relation entry, so keyboard movement can
    // scroll the selected entry into the shared viewport.
    {
        let mut geometry: Vec<(usize, usize)> = Vec::new();
        let mut parent_offset = 0usize;
        let mut child_offset = 0usize;
        for row in &relation_rows {
            let absolute = if row.after_steps {
                child_offset += 1;
                content.children_start + child_offset - 1
            } else {
                parent_offset += 1;
                content.relation_start + parent_offset - 1
            };
            let Some(target) = row.target else {
                continue;
            };
            // Targets arrive in order (0, then 1 + k per child), each over contiguous rows.
            match geometry.get_mut(target) {
                Some(entry) => entry.1 += 1,
                None => geometry.push((absolute, 1)),
            }
        }
        *form.steps.relation_geometry.borrow_mut() = geometry;
    }
    form.notes_width.set(notes_width);

    let focus = match model.input_mode() {
        BoardInputMode::EditTitle => Some(CaptureField::Title),
        BoardInputMode::EditNotes => Some(CaptureField::Notes),
        BoardInputMode::SelectBase => Some(CaptureField::Base),
        BoardInputMode::SelectAfter => Some(CaptureField::After),
        BoardInputMode::EditScope => Some(CaptureField::Scope),
        BoardInputMode::EditAssignee => Some(CaptureField::Assignee),
        BoardInputMode::FormDropdown => Some(form.focus),
        _ => None,
    };

    let scope_dropdown = scope_dropdown.map(|mut dropdown| {
        let field_x = match dropdown.field {
            CaptureField::Assignee => meta_assignee_x.unwrap_or(0),
            CaptureField::Base => meta_base_x.unwrap_or(0),
            CaptureField::Scope => meta_scope_x,
            _ => 0,
        };
        dropdown.anchor_x = 2u16.saturating_add(field_x);
        dropdown
    });

    QueueOverlay::TaskPage {
        header_rows,
        header_identifier,
        header_identifier_task: (!editing_title).then(|| form.task_id()).flatten(),
        title_cursor,
        status_word,
        notes_rows,
        notes_lines,
        notes_cursor,
        more_lines,
        relation_rows,
        step_views,
        stored_step_count,
        step_cursor: form.steps.cursor.and_then(visible_step_index),
        step_add_selected: form.steps.add_selected,
        step_scroll: notes_scroll,
        step_marked: form.steps.delete_mark.and_then(visible_step_index),
        inline_step_editor,
        block_rows,
        block_cursor,
        trail_rows,
        bottom_input: None,
        meta,
        meta_assignee_x,
        meta_assignee_width,
        meta_base_x,
        meta_base_width,
        meta_scope_x,
        meta_scope_width,
        focus,
        scope_dropdown,
    }
}

fn status_name(status: HumanStatus) -> &'static str {
    match status {
        HumanStatus::Open => "todo",
        HumanStatus::Ready => "ready",
        HumanStatus::Started => "doing",
        HumanStatus::Blocked => "blocked",
        HumanStatus::Review => "review",
        HumanStatus::Done => "done",
    }
}

/// AC-8: an archived task reads `archived` in place of its status word.
fn page_status_word(task: &Task) -> &'static str {
    if task.archived {
        "archived"
    } else {
        status_name(task.status)
    }
}

/// `T12 ` for a numbered task, nothing for one that has no number yet.
fn identifier_prefix(task: &Task) -> String {
    task.board_identifier()
        .map(|identifier| format!("{identifier} "))
        .unwrap_or_default()
}

/// One relation entry before wrapping: its action index, if it has one, and where it paints.
struct RelationEntry<'a> {
    lead: &'a str,
    text: &'a str,
    dim: bool,
    target: Option<usize>,
    after_steps: bool,
}

/// Wrap `text` to `width` cells: the first row carries `lead`, later rows align under the text.
/// Every wrapped row keeps the entry's action target; only the first carries the cursor.
fn push_wrapped_relation(
    rows: &mut Vec<RelationRow>,
    entry: RelationEntry<'_>,
    width: usize,
    cursor: Option<usize>,
) {
    let lead_w = render::display_width(entry.lead);
    let wrapped = wrap_text(entry.text, width.saturating_sub(lead_w).max(1));
    for (index, row) in wrapped.iter().enumerate() {
        let prefix = if index == 0 {
            entry.lead.to_string()
        } else {
            " ".repeat(lead_w)
        };
        rows.push(RelationRow {
            text: format!("{prefix}{}", row.text),
            dim: entry.dim,
            target: entry.target,
            selected: index == 0 && entry.target.is_some() && entry.target == cursor,
            after_steps: entry.after_steps,
        });
    }
}

/// The task page's relationship block: one parent line (none, resolved, or not found) before
/// the steps, then the direct children with their done count after the `+ step` row.
/// Counting matches `tsk list`: every scope and status, archived included, deleted tasks and
/// notices excluded. A missing or deleted parent keeps its stored id on screen; nothing here
/// changes either task. Action indices match [`BoardModel::relation_targets`].
fn relation_rows(
    tasks: &[Task],
    task: &Task,
    width: usize,
    cursor: Option<usize>,
) -> Vec<RelationRow> {
    let mut rows = Vec::new();
    let parent_text = match parent_in(tasks, task) {
        None => ("none · P to set".to_string(), true),
        Some(ParentRelation::Resolved(parent)) => (
            format!(
                "\u{2191} {}{} · {}",
                identifier_prefix(parent),
                terminal_text(&parent.title),
                page_status_word(parent)
            ),
            false,
        ),
        Some(ParentRelation::Unresolved(id)) => {
            (format!("\u{2191} {id} not found · link kept"), true)
        }
    };
    push_wrapped_relation(
        &mut rows,
        RelationEntry {
            lead: "parent  ",
            text: &parent_text.0,
            dim: parent_text.1,
            target: Some(0),
            after_steps: false,
        },
        width,
        cursor,
    );
    let children = direct_children_in(tasks, task.id);
    if !children.is_empty() {
        let done = children
            .iter()
            .filter(|child| child.status == HumanStatus::Done)
            .count();
        let label = format!("children {done}/{} done", children.len());
        push_wrapped_relation(
            &mut rows,
            RelationEntry {
                lead: "",
                text: &label,
                dim: true,
                target: None,
                after_steps: true,
            },
            width,
            cursor,
        );
        for (index, child) in children.iter().enumerate() {
            let lead = format!("{} ", render::status_glyph(child.status));
            let mut text = format!(
                "{}{}",
                identifier_prefix(child),
                terminal_text(&child.title)
            );
            if child.archived {
                text.push_str(" · archived");
            }
            push_wrapped_relation(
                &mut rows,
                RelationEntry {
                    lead: &lead,
                    text: &text,
                    dim: child.archived,
                    target: Some(1 + index),
                    after_steps: true,
                },
                width,
                cursor,
            );
        }
        if done == children.len() && task.status != HumanStatus::Done {
            push_wrapped_relation(
                &mut rows,
                RelationEntry {
                    lead: "",
                    text: "all children done · ctrl+d completes this task",
                    dim: true,
                    target: None,
                    after_steps: true,
                },
                width,
                cursor,
            );
        }
    }
    rows
}

/// The task page's question, finding, and reference sections, after the children. Action
/// indices continue from `first` and match [`BoardModel::relation_targets`].
fn question_and_link_rows(
    rows: &mut Vec<RelationRow>,
    tasks: &[Task],
    task: &Task,
    records: &[&crate::domain::QuestionRecord],
    width: usize,
    cursor: Option<usize>,
    first: usize,
) {
    use crate::domain::QuestionKind;
    let mut next = first;
    let questions: Vec<_> = records
        .iter()
        .filter(|record| record.reference.kind != QuestionKind::Finding)
        .collect();
    let findings: Vec<_> = records
        .iter()
        .filter(|record| record.reference.kind == QuestionKind::Finding)
        .collect();
    let section = |rows: &mut Vec<RelationRow>, label: String| {
        push_wrapped_relation(
            rows,
            RelationEntry {
                lead: "",
                text: &label,
                dim: true,
                target: None,
                after_steps: true,
            },
            width,
            cursor,
        );
    };
    if !questions.is_empty() {
        let open = questions.iter().filter(|record| record.is_open()).count();
        section(rows, format!("questions {open} open"));
        for record in questions {
            let lead = if record.reference.kind == QuestionKind::Approval {
                "! "
            } else {
                "? "
            };
            let text = format!(
                "Q{} {} · {}",
                record.reference.number,
                terminal_text(&record.reference.summary),
                crate::ui::questions::state_label(record, tasks)
            );
            push_wrapped_relation(
                rows,
                RelationEntry {
                    lead,
                    text: &text,
                    dim: !record.is_open(),
                    target: Some(next),
                    after_steps: true,
                },
                width,
                cursor,
            );
            next += 1;
        }
    }
    if !findings.is_empty() {
        section(rows, "findings".to_string());
        for record in findings {
            let kind = match record.draft.finding.as_ref().map(|finding| finding.kind) {
                Some(crate::domain::FindingKind::Opportunity) => "opportunity",
                _ => "issue",
            };
            let text = format!(
                "Q{} {kind} · {} · {}",
                record.reference.number,
                terminal_text(&record.reference.summary),
                crate::ui::questions::state_label(record, tasks)
            );
            push_wrapped_relation(
                rows,
                RelationEntry {
                    lead: "◇ ",
                    text: &text,
                    dim: !record.is_open(),
                    target: Some(next),
                    after_steps: true,
                },
                width,
                cursor,
            );
            next += 1;
        }
    }
    let links = crate::ui::links::task_links(task.notes.as_deref());
    if !links.is_empty() {
        section(rows, "links".to_string());
        for link in links {
            let lead = format!("{:<7}", link.label());
            let text = terminal_text(&link.text());
            push_wrapped_relation(
                rows,
                RelationEntry {
                    lead: &lead,
                    text: &text,
                    dim: matches!(link, crate::ui::links::TaskLink::UnverifiableThread { .. }),
                    target: Some(next),
                    after_steps: true,
                },
                width,
                cursor,
            );
            next += 1;
        }
    }
}

/// A dim, non-interactive hint row naming how to reach this task's completion report, when it
/// has one: `r` is a navigation-only chord (never a verb bar seat), so this row is the
/// discovery path for it, the same purpose `"c corrects"` serves on a question page.
fn push_report_hint_row(
    rows: &mut Vec<RelationRow>,
    report: Option<&crate::store::ReportRecord>,
    width: usize,
    cursor: Option<usize>,
) {
    let Some(record) = report else {
        return;
    };
    let text = if record.unread() {
        "r opens the unread completion report"
    } else {
        "r opens the completion report (read)"
    };
    push_wrapped_relation(
        rows,
        RelationEntry {
            lead: "",
            text,
            dim: true,
            target: None,
            after_steps: true,
        },
        width,
        cursor,
    );
}

/// The PAPER TRAIL section after the steps, all dim: its `PAPER TRAIL · N ▸` heading, and while
/// it is expanded (`▾`) every entry, newest first, with closed records expandable in place. Also
/// returns each stop's row, counted from the section's first row.
fn trail_page_rows(
    model: &BoardModel,
    form: &BoardForm,
    task: &crate::domain::Task,
    width: usize,
) -> (
    Vec<render::BlockPageRow>,
    Vec<(crate::ui::board::BlockTarget, usize)>,
) {
    use crate::ui::board::BlockTarget;
    use render::{BlockPageRow, BlockRowKind, QueueHitTarget};

    let width = width.max(8);
    let now = model.now();
    let entries = crate::activity::paper_trail(task);
    if entries.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let expanded = form.block.trail_expanded;
    // Selection belongs to the page form; another seat's form never paints it.
    let selected = model.block_target().filter(|_| {
        model
            .form
            .as_ref()
            .is_some_and(|own| std::ptr::eq(own, form))
    });
    let mut rows: Vec<BlockPageRow> = Vec::new();
    let mut stops = vec![(BlockTarget::TrailHeading, 0)];
    let push = |rows: &mut Vec<BlockPageRow>,
                lead: &str,
                text: &str,
                selected: bool,
                target: Option<QueueHitTarget>| {
        for (index, text) in beside_lead(lead, 0, text, width).into_iter().enumerate() {
            rows.push(BlockPageRow {
                text,
                kind: BlockRowKind::Dim,
                selected: index == 0 && selected,
                target,
            });
        }
    };
    push(
        &mut rows,
        "",
        &format!(
            "PAPER TRAIL · {} {}",
            entries.len(),
            if expanded { "▾" } else { "▸" }
        ),
        selected == Some(BlockTarget::TrailHeading),
        Some(QueueHitTarget::TrailHeading),
    );
    if !expanded {
        return (rows, stops);
    }
    for entry in &entries {
        let line = entry.line(now);
        let Some(index) = entry.record else {
            push(&mut rows, "", &line, false, None);
            continue;
        };
        let stop = BlockTarget::Trail(index);
        let open = form.block.trail_open.contains(&index);
        stops.push((stop, rows.len()));
        push(
            &mut rows,
            "",
            &format!("{line} {}", if open { "▾" } else { "▸" }),
            selected == Some(stop),
            Some(QueueHitTarget::TrailRecord(index)),
        );
        if open {
            if let Some(block) = task.past_blocks.get(index) {
                for detail in crate::activity::record_lines(block, now) {
                    push(
                        &mut rows,
                        &format!("  {}", detail.lead),
                        &detail.text,
                        false,
                        Some(QueueHitTarget::TrailRecord(index)),
                    );
                }
            }
        }
    }
    (rows, stops)
}

/// `text` wrapped beside `lead`, the lead padded to `column` cells (at least its own width) and
/// continuations aligned under the text. A lead that would leave the text fewer than a dozen
/// columns takes its own rows instead, and the text continues under it at a small indent.
fn beside_lead(lead: &str, column: usize, text: &str, width: usize) -> Vec<String> {
    const MIN_TEXT: usize = 12;
    let lead = terminal_text(lead);
    let text = terminal_text(text);
    let column = column.max(render::display_width(&lead));
    if lead.trim().is_empty() || width.saturating_sub(column) >= MIN_TEXT {
        let pad = " ".repeat(column - render::display_width(&lead));
        return wrap_text(&text, width.saturating_sub(column).max(1))
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                if index == 0 {
                    format!("{lead}{pad}{}", row.text)
                } else {
                    format!("{}{}", " ".repeat(column), row.text)
                }
            })
            .collect();
    }
    let margin = lead.len() - lead.trim_start().len();
    let indent = (margin + 2).min(width / 4);
    wrap_text(lead.trim(), width.saturating_sub(margin).max(1))
        .into_iter()
        .map(|part| format!("{}{}", " ".repeat(margin), part.text))
        .chain(
            wrap_text(&text, width.saturating_sub(indent).max(1))
                .into_iter()
                .map(|part| format!("{}{}", " ".repeat(indent), part.text)),
        )
        .collect()
}

/// The BLOCKED or REVIEW section's rows, each ring stop's row, and the reply box caret.
type BlockPageRows = (
    Vec<render::BlockPageRow>,
    Vec<(crate::ui::board::BlockTarget, usize)>,
    Option<(usize, u16)>,
);

/// Gap between the thread's `name · age` column and the reply text.
const THREAD_GAP: usize = 3;

/// The BLOCKED section of a blocked task's page or the REVIEW section of a task in review: the
/// top line (the row's live line), the body (why and needs, or done and next) as plain text, the
/// numbered options or the checks, the thread with the reply box when open, then the dim action
/// line and the closing rule. Also returns each ring stop's row and the reply box caret. Every
/// text wraps at `width`.
fn block_page_rows(
    model: &BoardModel,
    form: &BoardForm,
    task: &crate::domain::Task,
    width: usize,
) -> BlockPageRows {
    use crate::ui::board::BlockTarget;
    use render::{BlockPageRow, BlockRowKind};

    let Some(block) = task.block.as_ref() else {
        return (Vec::new(), Vec::new(), None);
    };
    let width = width.max(8);
    let now = model.now();
    // Selection belongs to the page form; another seat's form never paints it.
    let own = model
        .form
        .as_ref()
        .is_some_and(|own| std::ptr::eq(own, form));
    let selected = model.block_target().filter(|_| own);
    let mut rows: Vec<BlockPageRow> = Vec::new();
    let mut stops = Vec::new();
    let row = |text: String, kind: BlockRowKind, selected: bool| BlockPageRow {
        text,
        kind,
        selected,
        target: None,
    };
    // A click on a check cycles it; on the `N passed` line it shows or folds them.
    let click = |stop: Option<BlockTarget>| match stop {
        Some(BlockTarget::Check(index)) => Some(render::QueueHitTarget::PageCheck(index)),
        Some(BlockTarget::PassedFold) => Some(render::QueueHitTarget::PassedFold),
        _ => None,
    };
    // `lead` pads to `column` on the first wrapped row; continuations align under the text.
    let mut push = |rows: &mut Vec<BlockPageRow>,
                    lead: &str,
                    column: usize,
                    text: &str,
                    kind: BlockRowKind,
                    stop: Option<BlockTarget>| {
        if let Some(stop) = stop {
            stops.push((stop, rows.len()));
        }
        for (index, text) in beside_lead(lead, column, text, width)
            .into_iter()
            .enumerate()
        {
            rows.push(BlockPageRow {
                target: click(stop),
                ..row(text, kind, index == 0 && stop.is_some() && stop == selected)
            });
        }
    };
    let blank =
        |rows: &mut Vec<BlockPageRow>| rows.push(row(String::new(), BlockRowKind::Plain, false));

    let review = block.is_review();
    let top = render::status_line(task, &model.tasks, now)
        .unwrap_or_else(|| if review { "needs review" } else { "blocked" }.to_string());
    push(
        &mut rows,
        "",
        0,
        &top,
        BlockRowKind::Bold,
        Some(BlockTarget::Heading),
    );
    let body: Vec<&str> = if review {
        [block.done.as_deref(), block.next.as_deref()]
    } else {
        [block.why.as_deref(), block.needs.as_deref()]
    }
    .into_iter()
    .flatten()
    .collect();
    if !body.is_empty() {
        blank(&mut rows);
        for text in body {
            push(&mut rows, "", 0, text, BlockRowKind::Plain, None);
        }
    }
    if review {
        let (shown, passed) =
            crate::ui::board::block::review_check_order(block, form.block.fold.as_ref());
        if !block.checks.is_empty() {
            blank(&mut rows);
            push(&mut rows, "", 0, "Check", BlockRowKind::Dim, None);
        }
        for index in shown {
            let check = &block.checks[index];
            push(
                &mut rows,
                &format!(" {} ", render::check_glyph(check.state)),
                0,
                &check.text,
                BlockRowKind::Plain,
                Some(BlockTarget::Check(index)),
            );
        }
        if !passed.is_empty() {
            let open = form.block.passed_open;
            push(
                &mut rows,
                " ",
                0,
                &format!("{} passed {}", passed.len(), if open { "▾" } else { "▸" }),
                BlockRowKind::Dim,
                Some(BlockTarget::PassedFold),
            );
            if open {
                for index in passed {
                    push(
                        &mut rows,
                        &format!("   {} ", render::check_glyph(block.checks[index].state)),
                        0,
                        &block.checks[index].text,
                        BlockRowKind::Dim,
                        Some(BlockTarget::Check(index)),
                    );
                }
            }
        }
    } else if !block.options.is_empty() {
        blank(&mut rows);
        for (index, option) in block.options.iter().enumerate() {
            push(
                &mut rows,
                &format!("{:>2}  ", index + 1),
                0,
                option,
                BlockRowKind::Plain,
                Some(BlockTarget::Option(index)),
            );
        }
    }

    // The thread: `name · age` in one column wide enough for every lead, the text beside it.
    // Your replies carry your `[board] me` name, as the paper trail does.
    let leads: Vec<String> = block
        .replies
        .iter()
        .map(|reply| {
            let mut lead = format!(
                "{} · {}",
                crate::domain::display_name(&reply.by),
                render::format_age(now, reply.at)
            );
            if reply.edited && !reply.deleted {
                lead.push_str(" · edited");
            }
            lead
        })
        .collect();
    let editor = form.block.reply.as_ref().filter(|_| own);
    let editor_lead = editor.map(|editor| {
        let owner = crate::domain::owner_name();
        if editor.edit.is_some() {
            format!("{owner} · edit")
        } else {
            owner
        }
    });
    let column = leads
        .iter()
        .chain(editor_lead.iter())
        .map(|lead| render::display_width(&terminal_text(lead)))
        .max()
        .unwrap_or(0)
        + THREAD_GAP;
    if !leads.is_empty() || editor.is_some() {
        blank(&mut rows);
    }
    for ((index, reply), lead) in block.replies.iter().enumerate().zip(&leads) {
        if reply.deleted {
            push(&mut rows, lead, column, "deleted", BlockRowKind::Dim, None);
        } else {
            push(
                &mut rows,
                lead,
                column,
                &reply.text,
                BlockRowKind::Plain,
                Some(BlockTarget::Reply(index)),
            );
        }
    }
    let mut caret = None;
    if let (Some(editor), Some(lead)) = (editor, editor_lead) {
        // Too narrow for the column: the name takes its own row and the draft follows under it.
        let beside = width.saturating_sub(column) >= THREAD_MIN_TEXT;
        let indent = if beside { column } else { 2 };
        let field_width = width.saturating_sub(indent).max(1);
        editor.width.set(field_width);
        let (draft_rows, cursor_row, cursor_col) = wrapped_edit_rows(&editor.buffer, field_width);
        if !beside {
            rows.push(row(lead.clone(), BlockRowKind::Bold, false));
        }
        let first = rows.len();
        // The empty draft's hint wraps at the draft's width too, never truncated.
        let shown: Vec<String> = if editor.buffer.value().is_empty() {
            wrap_text(&editor.placeholder, field_width)
                .into_iter()
                .map(|row| row.text)
                .collect()
        } else {
            draft_rows
        };
        for (index, text) in shown.iter().enumerate() {
            let lead = if index == 0 && beside {
                format!(
                    "{lead}{}",
                    " ".repeat(column - render::display_width(&lead))
                )
            } else {
                " ".repeat(indent)
            };
            rows.push(row(format!("{lead}{text}"), BlockRowKind::Bold, false));
        }
        if model.input_mode() == BoardInputMode::EditReply {
            caret = Some((
                first + cursor_row,
                u16::try_from(2 + indent + cursor_col).unwrap_or(u16::MAX),
            ));
        }
        if let Some(refusal) = editor.refusal.as_deref() {
            push(
                &mut rows,
                &" ".repeat(indent),
                0,
                refusal,
                BlockRowKind::Dim,
                None,
            );
        }
    }

    blank(&mut rows);
    let keys = section_action_keys(model, form, task, block, selected, own);
    for line in pack_keys(&keys, width) {
        rows.push(row(line, BlockRowKind::Dim, false));
    }
    rows.push(row(String::new(), BlockRowKind::Rule, false));
    blank(&mut rows);
    (rows, stops, caret)
}

/// Narrowest reply text worth keeping beside the thread's name column.
const THREAD_MIN_TEXT: usize = 12;

/// The section's action line: only keys that work right now. It follows the cursor (a check,
/// a step, the top line, your reply…) and gives way to the reply box's own keys while it is open.
fn section_action_keys(
    model: &BoardModel,
    form: &BoardForm,
    task: &crate::domain::Task,
    block: &crate::domain::Block,
    selected: Option<crate::ui::board::BlockTarget>,
    own: bool,
) -> Vec<String> {
    use crate::domain::OWNER;
    use crate::ui::board::BlockTarget;
    use crate::ui::queue::{block_wait, BlockWait};

    let review = block.is_review();
    if own && form.block.reply.is_some() {
        return super::chrome::reply_box_keys(review)
            .split(" · ")
            .map(str::to_string)
            .collect();
    }
    let mut keys: Vec<String> = Vec::new();
    if review {
        keys.push("tab next field".to_string());
    }
    let cursor = if !own {
        None
    } else if form.steps.cursor.is_some() {
        Some("enter check / uncheck")
    } else if form.steps.add_selected {
        Some("enter add step")
    } else {
        match selected {
            Some(BlockTarget::Heading) => Some("ctrl+e edit"),
            Some(BlockTarget::Option(_)) => Some("enter choose"),
            Some(BlockTarget::Check(_)) => Some("enter cycle check"),
            Some(BlockTarget::PassedFold) if form.block.passed_open => Some("enter hide passed"),
            Some(BlockTarget::PassedFold) => Some("enter show passed"),
            Some(BlockTarget::Reply(index))
                if block
                    .replies
                    .get(index)
                    .is_some_and(|reply| reply.by == OWNER) =>
            {
                Some("ctrl+e edit · ctrl+x delete")
            }
            Some(BlockTarget::TrailHeading) if form.block.trail_expanded => Some("enter collapse"),
            Some(BlockTarget::Trail(index)) if form.block.trail_open.contains(&index) => {
                Some("enter collapse")
            }
            Some(BlockTarget::TrailHeading | BlockTarget::Trail(_)) => Some("enter expand"),
            _ => None,
        }
    };
    keys.extend(
        cursor
            .into_iter()
            .flat_map(|keys| keys.split(" · "))
            .map(str::to_string),
    );
    if !review {
        match block.options.len() {
            0 => {}
            1 => keys.push("1 choose".to_string()),
            count => keys.push(format!("1-{} choose", count.min(9))),
        }
    }
    keys.push(if review { "r feedback" } else { "r reply" }.to_string());
    if review {
        keys.push("ctrl+s send back".to_string());
        keys.push("ctrl+d approve".to_string());
    } else if block_wait(task, &model.tasks) == Some(BlockWait::You) {
        keys.push("ctrl+s reply and unblock".to_string());
    } else {
        keys.push("ctrl+b unblock".to_string());
    }
    keys.push("w waiting on".to_string());
    keys
}

/// Pack key legends into rows joined by ` · `, a new row where the next would not fit. A legend
/// wider than the row wraps on its own.
fn pack_keys(keys: &[String], width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for key in keys {
        let joined = if line.is_empty() {
            key.clone()
        } else {
            format!("{line} · {key}")
        };
        if render::display_width(&joined) <= width {
            line = joined;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if render::display_width(key) <= width {
            line = key.clone();
        } else {
            lines.extend(wrap_text(key, width.max(1)).into_iter().map(|row| row.text));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The reply box open under a blocked board row, with the block's why and needs above it; on a
/// review row, the feedback box with done and next above it.
fn row_reply_paint(model: &BoardModel) -> Option<render::RowReplyPaint<'_>> {
    let row = model.row_reply.as_ref()?;
    let block = model
        .tasks
        .iter()
        .find(|task| task.id == row.task)
        .and_then(|task| task.block.as_ref());
    let context = match block {
        Some(block) if block.is_review() => [
            block.done.as_deref().map(|done| ("done   ", done)),
            block.next.as_deref().map(|next| ("next   ", next)),
        ],
        _ => [
            block
                .and_then(|block| block.why.as_deref())
                .map(|why| ("why    ", why)),
            block
                .and_then(|block| block.needs.as_deref())
                .map(|needs| ("needs  ", needs)),
        ],
    };
    Some(render::RowReplyPaint {
        task: row.task,
        context,
        placeholder: &row.editor.placeholder,
        draft: &row.editor.buffer,
        width: &row.editor.width,
        refusal: row.editor.refusal.as_deref(),
        caret: model.input_mode() == BoardInputMode::EditReply,
    })
}

/// Idle status-row context for the active lens. Stored paths reach the
/// renderer only through its `present_line` path before they are painted.
fn status_idle(model: &BoardModel, surface: BoardSurface, has_message: bool) -> String {
    if !has_message {
        if let Some(notice) = model.update_notice() {
            return format!(" {notice}");
        }
    }
    footer_context(model, surface)
}

fn footer_context(model: &BoardModel, surface: BoardSurface) -> String {
    let scope = match surface {
        BoardSurface::All => " all".to_string(),
        BoardSurface::Desk => " desk".to_string(),
        BoardSurface::Project => {
            let name = model
                .active_project()
                .map(|path| render::short_project(&path.to_string_lossy()).to_string())
                .unwrap_or_else(|| "desk".to_string());
            format!(" {name}")
        }
    };
    format!("{scope} · {}", model.machine_range_label())
}

/// The persistent navigation row's paint for this model: the board and questions tabs.
fn nav_paint(model: &BoardModel) -> NavPaint {
    NavPaint {
        active: model.nav_tab(),
        questions_tab: crate::ui::questions::tab_texts(&model.questions.feed).1,
        reports_tab: crate::ui::reports::tab_texts(&model.reports.feed),
        check: model.stall.enabled,
    }
}

/// Row marks: a stale task's `↻` first, then its question marks (`!`, `?n`).
fn row_marks(model: &BoardModel) -> std::collections::BTreeMap<uuid::Uuid, String> {
    let mut marks = crate::ui::questions::task_marks(&model.questions.feed);
    for id in model.stale_ids() {
        let mark = match marks.remove(&id) {
            Some(question) => format!("{} {question}", crate::stall::REFRESH_MARK),
            None => crate::stall::REFRESH_MARK.to_string(),
        };
        marks.insert(id, mark);
    }
    marks
}

/// Which surface the list paints, from the scope.
fn board_surface(model: &BoardModel) -> BoardSurface {
    match model.effective_lens() {
        crate::ui::queue::BoardLens::All => BoardSurface::All,
        crate::ui::queue::BoardLens::Desk => BoardSurface::Desk,
        crate::ui::queue::BoardLens::Project(_)
        | crate::ui::queue::BoardLens::ArchivedProject(_) => BoardSurface::Project,
    }
}

/// Project order paints a project heading wherever the project changes, on the scopes that
/// mix projects.
fn project_groups(model: &BoardModel, surface: BoardSurface) -> bool {
    model.sort() == crate::ui::queue::BoardSort::Project
        && matches!(surface, BoardSurface::All | BoardSurface::Desk)
}

/// Draw the board into any ratatui frame (live TTY or [`ratatui::backend::TestBackend`]).
///
/// the paints the queue frame via [`render::draw_queue_frame`]. Classic master-detail
/// chrome is retired; overlays that still need the classic layout (edit band, save
/// recovery banner via message) are layered lightly on top where session mode requires it.
pub fn draw_board(frame: &mut Frame, model: &BoardModel) -> render::QueueHitMap {
    let hits = draw_board_impl(frame, model);
    if let Some(selection) = model.text_selection() {
        crate::ui::text_select::paint_selection(frame, &selection, &hits.copyable);
    }
    hits
}

/// The mouse hit-map for one board frame, without a live terminal.
///
/// [`draw_board`] is the one painter every board size uses; this renders the identical
/// frame into a scratch buffer purely to recover the hit-map [`draw_board_impl`] builds
/// beside the paint, so the live mouse loop and the screen the user is looking at can
/// never disagree about where a control is -- there is no second, hand-maintained copy of
/// the geometry to drift out of step with a renderer change.
pub fn board_hit_map(area: ratatui::layout::Rect, model: &BoardModel) -> render::QueueHitMap {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let width = area.width.max(1);
    let height = area.height.max(1);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("scratch terminal");
    let mut hits = render::QueueHitMap::default();
    let _ = terminal.draw(|frame| {
        hits = draw_board_impl(frame, model);
    });
    hits
}

/// Board-level surfaces whose payloads must outlive the overlay borrowing them.
struct OverlayPayloads {
    help_lines: Vec<String>,
    palette_commands: Vec<PaletteCommandRow>,
    scope_options: Vec<String>,
    list_picker_options: Vec<String>,
    list_picker_query: Option<String>,
    list_picker_selected: usize,
    scope_selected: usize,
}

impl OverlayPayloads {
    fn collect(model: &BoardModel) -> Self {
        let help_lines = if model.input_mode() == BoardInputMode::Help {
            help_card_lines_for_query(model.help_query())
        } else {
            Vec::new()
        };
        let palette_commands: Vec<PaletteCommandRow> =
            if model.command_surface() == CommandSurface::Palette {
                let visible = model.visible_commands();
                let selected = model.command_selected();
                visible
                    .iter()
                    .enumerate()
                    .map(|(i, cmd)| PaletteCommandRow {
                        label: cmd.label.clone(),
                        selected: Some(i) == selected,
                    })
                    .collect()
            } else {
                Vec::new()
            };
        let scope_options: Vec<String> = if model.input_mode() == BoardInputMode::FormDropdown {
            match model.form.as_ref().map(|form| form.focus) {
                Some(CaptureField::Scope) => model
                    .form_scope_options()
                    .iter()
                    .map(|scope| match scope {
                        TaskScope::Global => "desk".to_string(),
                        TaskScope::Project { path } => render::short_project(path).to_string(),
                    })
                    .collect(),
                Some(CaptureField::Assignee) => model
                    .form
                    .as_ref()
                    .map(|form| {
                        form.assignee_options
                            .iter()
                            .map(|option| match option {
                                Some(assignee) if assignee.is_person() => {
                                    crate::domain::display_name(&assignee.name)
                                }
                                Some(assignee) => assignee.name.clone(),
                                None => "none".to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                Some(CaptureField::Base) | None => Vec::new(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let active = model.list_picker_active();
        let list_picker_options = model
            .visible_list_picker_options()
            .into_iter()
            .map(|(index, option)| {
                if option.value == super::model::ListPickerValue::Unavailable {
                    format!("({})", option.label)
                } else {
                    let mut row = match option.count {
                        Some(count) => format!("{}  {count}", option.label),
                        None => option.label,
                    };
                    // The tabbed filter pickers mark the choice the board applies.
                    if active == Some(index) {
                        row.push_str("  \u{2713}");
                    }
                    row
                }
            })
            .collect();
        let list_picker_query = model.list_picker_query().map(str::to_string);
        let list_picker_selected = model.list_picker_selected();
        let scope_selected = model
            .form
            .as_ref()
            .map(|form| match form.focus {
                CaptureField::Scope => form.scope_selected,
                CaptureField::Assignee => form.assignee_selected,
                _ => 0,
            })
            .unwrap_or(0);
        Self {
            help_lines,
            palette_commands,
            scope_options,
            list_picker_options,
            list_picker_query,
            list_picker_selected,
            scope_selected,
        }
    }

    /// The board-level modal or capture surface that outranks the list and the page, if any.
    fn modal<'a>(
        &'a self,
        model: &'a BoardModel,
        geo: &tier::TierGeometry,
    ) -> Option<QueueOverlay<'a>> {
        if model.input_mode() == BoardInputMode::Search {
            let input_width = (geo.row_width as usize).saturating_sub(2);
            let query = EditBuffer::new(model.search_query(), model.search_query().chars().count());
            let (text, cursor_col) = escaped_line_window(&query, input_width);
            return Some(QueueOverlay::Search {
                input: crate::ui::render::BottomInputSlot {
                    text,
                    cursor_col,
                    placeholder: "search tasks…",
                    refusal: None,
                    message: model.message(),
                    above_rows: Vec::new(),
                    cursor_row_offset: 0,
                },
            });
        }
        if let Some(quick_add) = model.quick_add.as_ref().filter(|_| {
            matches!(
                model.input_mode(),
                BoardInputMode::QuickAdd | BoardInputMode::SaveRecovery
            )
        }) {
            let input_width = (geo.row_width as usize).saturating_sub(2);
            // A long title wraps instead of scrolling sideways. Exactly ONE row above
            // the input is reserved (the message row: the shifted rule sits two up),
            // so the draft may span at most two painted rows, windowed by the
            // minimum that keeps the caret's wrapped row visible. Longer drafts
            // window vertically rather than touching chrome.
            const QUICK_ADD_MAX_ROWS: usize = 2;
            let (all_rows, cursor_row, cursor_column) =
                wrapped_edit_rows(&quick_add.title, input_width);
            let shown = all_rows.len().clamp(1, QUICK_ADD_MAX_ROWS);
            let start = cursor_row
                .min(all_rows.len().saturating_sub(1))
                .saturating_sub(shown - 1);
            let window: Vec<String> = all_rows[start..(start + shown).min(all_rows.len())].to_vec();
            let caret_index = cursor_row.saturating_sub(start);
            let title = window.last().cloned().unwrap_or_default();
            // Continuations paint top-first (the painter stacks them upward by index).
            let above_rows: Vec<String> = window[..window.len().saturating_sub(1)].to_vec();
            let multiline = window.len() > 1;
            // The row names its destination so Enter never has to move the user's
            // view to prove where the task went.
            let destination = match &quick_add.scope {
                TaskScope::Project { path } => project_option_label(Path::new(path)).to_string(),
                TaskScope::Global => "desk".to_string(),
            };
            return Some(QueueOverlay::QuickAdd {
                input: crate::ui::render::BottomInputSlot {
                    text: title,
                    cursor_col: u16::try_from(cursor_column).unwrap_or(u16::MAX),
                    placeholder: "title…   !p project · !a assignee · !b base",
                    refusal: None,
                    // Save recovery owns the verb row; ordinary quick-add refusals
                    // use the shared slot's reserved row above the cursor. A wrapped
                    // draft owns those rows itself, so the message yields while it is
                    // up and returns when the draft is back under the cap.
                    message: (!multiline && model.input_mode() != BoardInputMode::SaveRecovery)
                        .then(|| model.message())
                        .flatten(),
                    above_rows,
                    cursor_row_offset: u16::try_from(
                        window.len().saturating_sub(1).saturating_sub(caret_index),
                    )
                    .unwrap_or(0),
                },
                destination,
                recovery: model.input_mode() == BoardInputMode::SaveRecovery,
            });
        }
        if model.input_mode() == BoardInputMode::Help {
            return Some(QueueOverlay::Help {
                query: model.help_query(),
                lines: &self.help_lines,
                scroll: model.help_scroll(),
            });
        }
        if model.command_surface() == CommandSurface::Palette {
            return Some(QueueOverlay::Palette {
                query: model.command_query(),
                commands: &self.palette_commands,
            });
        }
        if let Some(prompt) = model.cleanup_prompt() {
            return Some(match model.cleanup_run() {
                Some(run) => cleanup_run_overlay(prompt, &run),
                None => cleanup_overlay(prompt, home_dir().as_deref()),
            });
        }
        if let Some(card) = model.block_card() {
            return Some(block_card_overlay(model, card));
        }
        if let Some(prompt) = model.dispatch_prompt() {
            return Some(dispatch_overlay(prompt, |project| {
                model.default_branch_name(project)
            }));
        }
        if let Some(prompt) = model.launch_card.as_ref() {
            return Some(launch_overlay(prompt));
        }
        if matches!(
            model.input_mode(),
            BoardInputMode::ParentPicker | BoardInputMode::ListPicker
        ) {
            if let Some(title) = model.after_picker_title() {
                return Some(QueueOverlay::AfterPicker(render::AfterPickerPaint {
                    title,
                    query: self.list_picker_query.as_deref().unwrap_or(""),
                    rows: &self.list_picker_options,
                    pinned_rows: model.after_picker_pinned_rows(),
                    selected: self.list_picker_selected,
                }));
            }
            let title = match model.list_picker_kind() {
                Some(crate::ui::board::ListPickerKind::Assignee) => "assignee",
                Some(crate::ui::board::ListPickerKind::WaitingOn) => "waiting on",
                Some(crate::ui::board::ListPickerKind::Base) => "base",
                Some(crate::ui::board::ListPickerKind::After) => "after",
                Some(crate::ui::board::ListPickerKind::Parent { .. }) | None => "parent",
            };
            return Some(QueueOverlay::ScopeDropdown {
                options: &self.list_picker_options,
                selected: self
                    .list_picker_selected
                    .min(self.list_picker_options.len().saturating_sub(1)),
                title: Some(title),
                query: self.list_picker_query.as_deref(),
                wrap: model.list_picker_rows(),
                removable: model.assignee_picker_person().is_some(),
            });
        }
        None
    }

    /// The open task page, painted from the retained form at `geo`.
    fn task_page<'a>(
        &'a self,
        model: &'a BoardModel,
        form: &'a BoardForm,
        geo: &tier::TierGeometry,
        column: bool,
    ) -> QueueOverlay<'a> {
        let scope_dropdown =
            (model.input_mode() == BoardInputMode::FormDropdown).then_some(FormDropdown {
                options: &self.scope_options,
                selected: self.scope_selected,
                field: form.focus,
                anchor_x: 0,
            });
        build_task_page_overlay(model, form, geo, scope_dropdown, column)
    }
}

/// Status row content: delete recovery notice (title + u undo hint) when armed; otherwise
/// the last action message, otherwise counts. When both channels are set (stale undo
/// refusal after delete), compose notice first then message so the refusal is visible.
fn status_row_content(model: &BoardModel) -> (Option<String>, Option<usize>, Option<usize>) {
    let editing_on_page = model.open_field_edit().is_some() && model.form.is_some();
    let bulk_count = model.visible_delete_notice_count();
    let notice = model.visible_delete_notice().map(|title| match bulk_count {
        Some(count) => format!(
            "deleted {count} {} · {DELETE_NOTICE_UNDO}",
            if count == 1 { "task" } else { "tasks" }
        ),
        None => notice_framed(model.delete_notice_label(), title, true),
    });
    let status_owned = match (notice.as_deref(), model.message()) {
        (Some(notice), Some(msg)) => Some(format!("{notice}  ·  {msg}")),
        (Some(notice), None) => Some(notice.to_string()),
        (None, Some(msg)) => Some(msg.to_string()),
        (None, None) if editing_on_page => Some("editing…".to_string()),
        // Under a bulk cleanup card, Esc cancels the card and keeps the marks.
        (None, None)
            if model.mark_mode_active()
                && (model.cleanup_prompt().is_some() || model.dispatch_prompt().is_some()) =>
        {
            Some(format!("multi-select · {} selected", model.marked_count()))
        }
        (None, None) if model.mark_mode_active() && model.marked_count() > 0 => Some(format!(
            "multi-select · {} selected · esc clear",
            model.marked_count()
        )),
        (None, None) if model.mark_mode_active() => {
            Some("multi-select · space/click mark · esc exit".to_string())
        }
        _ => None,
    };
    // Compute the clickable control from the exact notice text, never by searching user text.
    let undo_control = notice.as_ref().map(|_| DELETE_NOTICE_UNDO);
    let status_undo_offset = notice
        .as_deref()
        .zip(undo_control)
        .map(|(notice, control)| row_width(notice).saturating_sub(row_width(control)));
    let status_undo_width = undo_control.map(row_width);
    (status_owned, status_undo_offset, status_undo_width)
}

/// Field named in the task header's `editing <field>` state slot while an editor is active.
fn editing_field(model: &BoardModel) -> Option<&'static str> {
    match model.input_mode() {
        BoardInputMode::EditTitle => Some("title"),
        BoardInputMode::EditNotes => Some("notes"),
        BoardInputMode::EditScope => Some("scope"),
        BoardInputMode::EditAssignee => Some("assignee"),
        BoardInputMode::FormDropdown => match model.form_focus() {
            Some(CaptureField::Assignee) => Some("assignee"),
            _ => Some("scope"),
        },
        BoardInputMode::EditStep => Some("step"),
        _ => None,
    }
}

/// Header state slot for the task column: `status · project`, `editing <field>`, or
/// `unsaved` while a dirty draft waits with no editor active.
fn task_header_state(model: &BoardModel, form: &BoardForm, task: &crate::domain::Task) -> String {
    if let Some(field) = editing_field(model).filter(|_| form.task_id() == Some(task.id)) {
        return format!("editing {field}");
    }
    let is_retained = model
        .form
        .as_ref()
        .is_some_and(|retained| retained.task_id() == Some(task.id));
    if is_retained && model.task_session_dirty() {
        return "unsaved".to_string();
    }
    let status = if task.archived {
        "archived"
    } else {
        match task.status {
            HumanStatus::Open => "todo",
            HumanStatus::Ready => "ready",
            HumanStatus::Started => "doing",
            HumanStatus::Blocked => "blocked",
            HumanStatus::Review => "review",
            HumanStatus::Done => "done",
        }
    };
    let project = match &task.scope {
        TaskScope::Project { path } => render::short_project(path).to_string(),
        TaskScope::Global => "desk".to_string(),
    };
    format!("{status} · {project}")
}

/// The dim focus crumb and key hints for the wide status row.
fn wide_status_hint(model: &BoardModel) -> (Option<&'static str>, &'static str) {
    // Save/cancel keys apply only while a field editor is actually open or the parked draft
    // is dirty; a clean session shows the focus keys again.
    let field_editor =
        model.open_field_edit().is_some() || model.input_mode() == BoardInputMode::FormDropdown;
    let editing = field_editor || model.task_session_dirty();
    // Focus navigation only: the verb row already carries the surface's keys, so the hint
    // never repeats them.
    match model.wide_stage() {
        tier::WideStage::Board => (None, "→ detail"),
        tier::WideStage::Task if editing => (Some("detail"), ""),
        tier::WideStage::Task => (Some("detail"), "← list · J/K next/prev"),
    }
}

fn draw_board_impl(frame: &mut Frame, model: &BoardModel) -> render::QueueHitMap {
    let hits = draw_board_hits(frame, model);
    // Renderer-recorded horizon for the help card, like the list's max scroll: the reducer
    // clamps with it so the offset never runs past the last page.
    if let Some(max_scroll) = hits.help_max_scroll {
        model.help_max_scroll.set(max_scroll);
    }
    if let Some(max_scroll) = hits.cleanup_max_scroll {
        model.cleanup_max_scroll.set(max_scroll);
    }
    if let Some(layout) = &hits.cleanup_layout {
        model.cleanup_layout.replace(layout.clone());
    }
    hits
}

fn draw_board_hits(frame: &mut Frame, model: &BoardModel) -> render::QueueHitMap {
    let area = frame.area();
    // Tab 2 owns the frame unless help or the palette is open over it.
    if model.questions.active
        && !matches!(
            model.input_mode(),
            BoardInputMode::Help | BoardInputMode::Palette
        )
    {
        let message = model.message().map(str::to_string);
        return crate::ui::questions::draw(
            frame,
            &crate::ui::questions::QuestionsFrame {
                state: &model.questions,
                reports_tab: &crate::ui::reports::tab_texts(&model.reports.feed),
                tasks: &model.tasks,
                message: message.as_deref(),
            },
            area,
        );
    }
    // Tab 3 owns the frame unless help or the palette is open over it.
    if model.reports.active
        && !matches!(
            model.input_mode(),
            BoardInputMode::Help | BoardInputMode::Palette
        )
    {
        let message = model.message().map(str::to_string);
        return crate::ui::reports::draw(
            frame,
            &crate::ui::reports::ReportsFrame {
                state: &model.reports,
                questions_tab: &crate::ui::questions::tab_texts(&model.questions.feed).1,
                tasks: &model.tasks,
                message: message.as_deref(),
            },
            area,
        );
    }
    // A capture draft (quick-add expanded with Tab) owns the whole frame at every width; the
    // wide task column paints task forms only. `responsive_geometry` folds that rule in.
    let responsive = model.responsive_geometry(area);
    if responsive.presentation == tier::ResponsivePresentation::WideSplit {
        return draw_wide_board(frame, model, area, responsive);
    }
    let geo = tier::resolve(area.width, area.height);
    let queue_view = model.queue_view();
    let selection_id = model.saved_task.or(model.selection_id);
    let surface = board_surface(model);
    let (status_owned, status_undo_offset, status_undo_width) = status_row_content(model);
    // The verb bar entries: computed from the selection and the open surface so the label
    // is true for the row it describes, and drawn from this one function -- the same one
    // the goldens call -- so a later edit to either side cannot silently re-open wording
    // drift.
    let verbs = board_verb_items(model);
    let payloads = OverlayPayloads::collect(model);
    let overlay = match payloads.modal(model, &geo) {
        Some(modal) => modal,
        None => match model.form.as_ref() {
            // A parked task page is not painted while the board owns the frame.
            Some(form)
                if !(model.focused_surface() == tier::FocusedSurface::Board && form.is_task()) =>
            {
                payloads.task_page(model, form, &geo, false)
            }
            _ => QueueOverlay::None,
        },
    };
    let frame_model = QueueFrameModel {
        tasks: &model.tasks,
        view: &queue_view,
        selection_id,
        marked_ids: model.marked_ids.clone(),
        nav: nav_paint(model),
        surface,
        project_groups: project_groups(model, surface),
        assignee_labels: model.assignee_label_shown(),
        messages_hidden: model.messages_hidden(),
        search_query: model.search_query(),
        search_pinned: model.search_pinned(),
        summary: None,
        context: status_idle(model, surface, status_owned.is_some()),
        status_message: status_owned.as_deref(),
        status_undo_offset,
        status_undo_width,
        verb_items: &verbs,
        now: model.now(),
        overlay,
        row_reply: row_reply_paint(model),
        list_scroll: model.list_scroll.get(),
        follow_list: model.follow_list.get(),
        archived_collapsed: model.archived_collapsed,
        archived_header_selected: model.archived_header_selected(),
        inbox_collapsed: model.inbox_collapsed,
        inbox_header_selected: model.inbox_header_selected(),
        rows_dim: false,
        waiting: crate::ui::questions::waiting_line(&model.questions.feed),
        task_marks: row_marks(model),
        show_status_lines: model.show_status_lines,
        report_marks: crate::ui::reports::task_marks(&model.reports.feed),
    };
    let (hits, painted_list_scroll) = render::draw_queue_frame(frame, &frame_model, &geo, area);
    if let Some((scroll, max_scroll)) = painted_list_scroll {
        model.list_scroll.set(scroll);
        model.list_max_scroll.set(max_scroll);
    }
    paint_board_form_toast(frame, model, &geo, area);
    hits
}

/// Board-form edits use the task page's status and verb rows rather than an inline rule row.
fn paint_board_form_toast(
    frame: &mut Frame,
    model: &BoardModel,
    geo: &tier::TierGeometry,
    area: ratatui::layout::Rect,
) {
    if model.open_field_edit().is_some() && model.form.is_none() {
        if let Some(row) = geo.rule_row {
            let toast_area =
                ratatui::layout::Rect::new(area.x, area.y.saturating_add(row), area.width, 1);
            // Use the mono-only helpers so no product frame emits foreground or background
            // color SGR codes.
            frame.render_widget(
                Paragraph::new(present_line(
                    &model.edit_chrome_row(toast_area.width as usize),
                    toast_area.width as usize,
                ))
                .style(render::style_bold()),
                toast_area,
            );
        }
    }
}

/// The wide layout: the list and the task detail side by side, one rule column, one shared
/// footer.
///
/// Focus moves between the two columns; the footer (rule, status row with its focus crumb,
/// verb bar) is painted once across the frame.
fn draw_wide_board(
    frame: &mut Frame,
    model: &BoardModel,
    area: ratatui::layout::Rect,
    responsive: tier::ResponsiveGeometry,
) -> render::QueueHitMap {
    let task_focus = model.focused_surface() == tier::FocusedSurface::Task;
    let density = responsive.density;
    let queue_view = model.queue_view();
    let selection_id = model.saved_task.or(model.selection_id);
    let selected_task = selection_id.and_then(|id| model.tasks.iter().find(|task| task.id == id));
    let surface = board_surface(model);
    let (status_owned, status_undo_offset, status_undo_width) = status_row_content(model);
    let verbs = board_verb_items(model);
    let payloads = OverlayPayloads::collect(model);
    let frame_geo = tier::resolve_density(area.width, area.height, density);
    let modal = payloads.modal(model, &frame_geo);

    // The footer's owner decides its rows: a bottom input, the palette query, or the verbs
    // of the focused surface. Column heights follow the footer's (possibly shifted) rule.
    let footer_needs_input = modal.as_ref().is_some_and(render::has_bottom_input);
    let footer_geo = render::bottom_input_geometry(frame_geo, footer_needs_input);
    let column_height = footer_geo.rule_row.unwrap_or(area.height);
    let column_geo = |width: u16| tier::resolve_column(width, column_height, area.height, density);
    let column_rect = |rect: ratatui::layout::Rect| {
        ratatui::layout::Rect::new(rect.x, rect.y, rect.width, column_height.min(rect.height))
    };

    // The task column paints the retained page when it is bound to the selection (or owns
    // focus), otherwise a fresh view of the selected task. Nothing here touches the model.
    let retained = model
        .form
        .as_ref()
        .filter(|form| form.is_task() && (task_focus || form.task_id() == selection_id));
    let preview_form = (!task_focus && retained.is_none())
        .then(|| {
            selected_task.map(|task| {
                BoardForm::task(
                    task,
                    model.this_repo.as_deref(),
                    &model.tasks,
                    CaptureField::Title,
                    &model.archived_projects,
                    &model.agent_names,
                )
            })
        })
        .flatten();
    let task_form = retained.or(preview_form.as_ref());
    let task_area = responsive.task_content();
    let task_geo = (task_area.width > 0).then(|| column_geo(task_area.width));
    let task_overlay = match (task_geo.as_ref(), task_form) {
        (Some(geo), Some(form)) => payloads.task_page(model, form, geo, true),
        (Some(_), None) => QueueOverlay::None,
        (None, _) => QueueOverlay::None,
    };
    let header_task = task_form.and_then(|form| {
        form.task_id()
            .and_then(|id| model.tasks.iter().find(|task| task.id == id))
            .map(|task| (form, task))
    });
    let header_identifier = header_task.and_then(|(_, task)| task.board_identifier());
    let header_state = header_task.map(|(form, task)| task_header_state(model, form, task));
    // The page builder wrapped the title for this column (and mapped the caret while the
    // title is edited), so the header and the body layout count the same rows.
    let header_title = match &task_overlay {
        QueueOverlay::TaskPage {
            header_rows,
            title_cursor,
            ..
        } => Some((header_rows.as_slice(), *title_cursor)),
        _ => None,
    };
    let header = header_task
        .zip(header_title)
        .map(
            |((form, task), (title_rows, title_cursor))| render::TaskColumnHeader {
                glyph: render::task_status_glyph(task, &model.tasks),
                identifier: header_identifier.as_deref(),
                identifier_task: form.task_id(),
                title_rows,
                title_cursor,
                state: header_state.as_deref().unwrap_or_default(),
                bold: task_focus,
            },
        );

    let board_frame = QueueFrameModel {
        tasks: &model.tasks,
        view: &queue_view,
        selection_id,
        marked_ids: model.marked_ids.clone(),
        nav: nav_paint(model),
        surface,
        project_groups: project_groups(model, surface),
        assignee_labels: model.assignee_label_shown(),
        messages_hidden: model.messages_hidden(),
        search_query: model.search_query(),
        search_pinned: model.search_pinned(),
        summary: None,
        context: status_idle(model, surface, status_owned.is_some()),
        status_message: status_owned.as_deref(),
        status_undo_offset,
        status_undo_width,
        verb_items: &verbs,
        now: model.now(),
        overlay: if task_focus {
            QueueOverlay::None
        } else {
            modal.clone().unwrap_or(QueueOverlay::None)
        },
        row_reply: row_reply_paint(model),
        list_scroll: model.list_scroll.get(),
        follow_list: model.follow_list.get(),
        archived_collapsed: model.archived_collapsed,
        archived_header_selected: model.archived_header_selected(),
        inbox_collapsed: model.inbox_collapsed,
        inbox_header_selected: model.inbox_header_selected(),
        rows_dim: false,
        waiting: crate::ui::questions::waiting_line(&model.questions.feed),
        task_marks: row_marks(model),
        show_status_lines: model.show_status_lines,
        report_marks: crate::ui::reports::task_marks(&model.reports.feed),
    };
    let task_frame = QueueFrameModel {
        overlay: task_overlay.clone(),
        list_scroll: 0,
        follow_list: false,
        ..board_frame.clone()
    };
    let footer_frame = QueueFrameModel {
        overlay: match (&modal, task_focus) {
            (Some(modal), _) => modal.clone(),
            (None, true) => task_overlay.clone(),
            (None, false) => QueueOverlay::None,
        },
        ..board_frame.clone()
    };

    let mut hits = render::QueueHitMap::default();
    let board_area = responsive.board;
    if board_area.width > 0 {
        let board_geo = column_geo(board_area.width);
        let (mut board_hits, painted_list_scroll) =
            render::draw_queue_frame(frame, &board_frame, &board_geo, column_rect(board_area));
        hits.regions.append(&mut board_hits.regions);
        hits.copyable.append(&mut board_hits.copyable);
        hits.help_max_scroll = hits.help_max_scroll.or(board_hits.help_max_scroll);
        hits.cleanup_max_scroll = hits.cleanup_max_scroll.or(board_hits.cleanup_max_scroll);
        hits.cleanup_layout = hits
            .cleanup_layout
            .take()
            .or(board_hits.cleanup_layout.take());
        if let Some((scroll, max_scroll)) = painted_list_scroll {
            model.list_scroll.set(scroll);
            model.list_max_scroll.set(max_scroll);
        }
    }
    if responsive.rule.width > 0 {
        let rule = column_rect(responsive.rule);
        for y in rule.top()..rule.bottom() {
            frame.render_widget(
                Paragraph::new(render::paint_bounded_line("│", 1, render::style_dim())),
                ratatui::layout::Rect::new(rule.x, y, 1, 1),
            );
        }
    }
    if let Some(task_geo) = task_geo.as_ref() {
        let mut task_hits = render::draw_task_column(
            frame,
            &task_frame,
            task_geo,
            column_rect(task_area),
            header,
            modal.as_ref().filter(|_| task_focus),
        );
        // A board-owned preview keeps its control hits, but only the focus router reads
        // them: a click there moves the stage first, then dispatches against this frame.
        hits.regions.append(&mut task_hits.regions);
        hits.copyable.append(&mut task_hits.copyable);
    }
    let (crumb, keys) = wide_status_hint(model);
    let mut footer_hits = render::draw_queue_footer(
        frame,
        &footer_frame,
        &footer_geo,
        area,
        Some(render::StatusHint { crumb, keys }),
    );
    hits.regions.append(&mut footer_hits.regions);
    hits.copyable.append(&mut footer_hits.copyable);
    hits.footer = footer_hits.footer;
    paint_board_form_toast(frame, model, &footer_geo, area);
    hits
}
