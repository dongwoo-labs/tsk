//! App-side reference states for site/parity. These are TestBackend frames, not terminal screenshots.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tsk_tui::{
    domain::DomainState,
    ui::{
        apply_intent, draw_board,
        input::{map_key, route_responsive_key, ResponsiveKeyRoute},
        BoardIntent, BoardModel,
    },
};

fn key(
    state: &mut DomainState,
    model: &mut BoardModel,
    code: KeyCode,
    mods: KeyModifiers,
    width: u16,
) {
    let event = KeyEvent::new(code, mods);
    let intent = match route_responsive_key(
        model.input_mode(),
        model.wide_stage(),
        tsk_tui::ui::tier::resolve_responsive(width, 40, model.wide_stage()).presentation,
        event,
    ) {
        ResponsiveKeyRoute::Intent(intent) => Some(intent),
        ResponsiveKeyRoute::Inert => None,
        ResponsiveKeyRoute::Surface => map_key(model.input_mode(), event),
    };
    if let Some(intent) = intent {
        apply_intent(state, model, intent, None).unwrap();
    }
}
fn capture(model: &BoardModel, width: u16, _name: &str) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
    terminal
        .draw(|f| {
            draw_board(f, model);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text = (0..40)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    text
}
fn export_reference(text: &str, width: u16, name: &str) {
    if let Ok(dir) = std::env::var("TSK_PARITY_OUTPUT") {
        fs::create_dir_all(&dir).unwrap();
        let dir = PathBuf::from(dir);
        fs::write(dir.join(format!("app-{width}-{name}.txt")), text).unwrap();
        if name == "page" {
            // Wide frames carry the task page right of the `│` rule; narrow frames end in a
            // scrollbar column. Keep only the step text.
            // Step content and frame-edge scrollbar chrome are separate contracts.
            // Export both, rather than treating a painted thumb as part of a step.
            let scrollbar_rows: Vec<_> = text
                .lines()
                .enumerate()
                .filter_map(|(row, line)| line.ends_with('▌').then_some(row))
                .collect();
            fs::write(
                dir.join(format!("page-scrollbar-{width}.json")),
                serde_json::to_string_pretty(&scrollbar_rows).unwrap(),
            )
            .unwrap();
            let lines: Vec<_> = text
                .lines()
                .map(|line| line.split_once('│').map_or(line, |(_, detail)| detail))
                .skip_while(|line| !line.contains("✓ Check"))
                .take_while(|line| !line.contains("+ step"))
                .map(|line| {
                    let gutter = line.chars().take_while(|c| *c == ' ').count() + 2;
                    let gutter = if line.contains('✓') {
                        gutter
                    } else {
                        gutter - 2
                    };
                    line.chars()
                        .skip(gutter)
                        .collect::<String>()
                        .trim_end_matches(['▌', ' '])
                        .to_string()
                })
                .collect();
            fs::write(
                dir.join(format!("steps-{width}.json")),
                serde_json::to_string_pretty(&lines).unwrap(),
            )
            .unwrap();
        }
        if name == "project-peek-or-split" && width < 110 {
            write_json(
                &dir,
                &format!("peek-{width}.json"),
                &under_row(text, "T12 ", 0),
            );
            // The open peek keeps the row's gutter on its wrapped title.
            write_json(
                &dir,
                &format!("peek-row-{width}.json"),
                &row_lines(text, "T12 ", 0),
            );
        }
        if name.starts_with("preview-") {
            // The project preview is the right column: drop the index column and its rule.
            let column = text
                .lines()
                .find(|line| line.contains("T12 "))
                .and_then(|line| line.chars().position(|c| c == '│'))
                .expect("preview divider")
                + 1;
            write_json(
                &dir,
                &format!("{name}-{width}.json"),
                &under_row(text, "T12 ", column),
            );
            if name == "preview-live" || name == "preview-peek" {
                // The row's wrapped title, with the live line's gutter beside it.
                write_json(
                    &dir,
                    &format!("{}-row-{width}.json", name.trim_end_matches("-live")),
                    &row_lines(text, "T12 ", column),
                );
            }
        }
        if name == "blocked-page" {
            // The BLOCKED section from its top line through the options, without the gutter.
            // The action line differs on purpose: the demo answers nothing.
            let lines: Vec<String> = text
                .lines()
                .skip_while(|line| !line.contains("blocked on you"))
                .map(|line| {
                    line.chars()
                        .skip(2)
                        .collect::<String>()
                        .trim_end_matches(['▌', ' '])
                        .to_string()
                })
                .take_while(|line| !line.contains("choose"))
                .collect();
            let end = lines.iter().rposition(|line| !line.is_empty()).unwrap_or(0);
            write_json(&dir, &format!("blocked-page-{width}.json"), &lines[..=end]);
        }
        if name == "initial" {
            // The blocked row's live line, at the fixture's pinned clock.
            write_json(
                &dir,
                &format!("live-{width}.json"),
                &under_row(text, "T12 ", 0),
            );
            // The blocked row's wrapped title, with the live line's gutter beside it.
            write_json(
                &dir,
                &format!("row-{width}.json"),
                &row_lines(text, "T12 ", 0),
            );
            let lines: Vec<_> = text
                .lines()
                .map(|line| line.split_once('│').map_or(line, |(list, _)| list))
                .skip_while(|line| !line.contains("T13 "))
                // Rows have no status glyph: a 3-cell gutter, then `T13 `. Continuation
                // rows indent 7 cells under the title; the next task row does not.
                .take_while(|line| {
                    !line.trim_start().is_empty()
                        && (line.contains("T13") || line.starts_with("       "))
                })
                .map(|line| {
                    let line = line.chars().skip(7).collect::<String>();
                    let line = line.trim_end_matches(['▌', ' ']);
                    // The demo paints the right-edge marks (`✓1/2`, `⊂n/m`) outside the
                    // title lines; keep only the title text.
                    match line.rsplit_once("  ") {
                        Some((title, tail)) if tail.starts_with(['✓', '⊂', '!', '?']) => {
                            title.trim_end().to_string()
                        }
                        _ => line.to_string(),
                    }
                })
                .collect();
            fs::write(
                dir.join(format!("title-{width}.json")),
                serde_json::to_string_pretty(&lines).unwrap(),
            )
            .unwrap();
        }
    }
}

/// The fixture's clock, matching the browser harness's `now`, so painted ages agree.
fn fixture_clock() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_200)
}

fn fixture_board() -> (DomainState, BoardModel) {
    let state: DomainState =
        serde_json::from_str(include_str!("fixtures/demo-parity/store.json")).unwrap();
    let mut model = BoardModel::from_domain(&state, Some(PathBuf::from("/tmp/tsk-parity")));
    model.set_clock(Some(fixture_clock()));
    (state, model)
}

/// The first row containing `row` as painted from column `skip` on: its head line, then its
/// own wrapped title lines with the live line's `│` gutter when it carries one.
fn row_lines(text: &str, row: &str, skip: usize) -> Vec<String> {
    let mut lines = text
        .lines()
        .skip_while(|line| !line.contains(row))
        .map(|line| line.chars().skip(skip).collect::<Vec<char>>());
    let Some(head) = lines.next() else {
        return Vec::new();
    };
    let row_cells: Vec<char> = row.chars().collect();
    let title_x = (0..head.len())
        .find(|at| head[*at..].starts_with(&row_cells))
        .map_or(0, |at| at + row_cells.len());
    std::iter::once(head)
        .chain(lines.take_while(|cells| {
            cells.len() > title_x
                && cells[title_x] != ' '
                && cells[..title_x].iter().all(|c| matches!(c, ' ' | '│'))
        }))
        .map(|cells| {
            let line: String = cells.into_iter().collect();
            let line = line.trim_end().trim_end_matches(['▌', '█']).trim_end();
            let line = if skip > 0 {
                line.strip_prefix(' ').unwrap_or(line)
            } else {
                line
            };
            line.to_string()
        })
        .collect()
}

/// The lines painted under the first row containing `row`, from column `skip` on: its live
/// line or its open peek, up to the peek's `└` footer or the first line of anything else.
/// The row's own wrapped title lines ([`row_lines`]) are not part of it.
fn under_row(text: &str, row: &str, skip: usize) -> Vec<String> {
    let mut out = Vec::new();
    let lines = text
        .lines()
        .skip_while(|line| !line.contains(row))
        .skip(row_lines(text, row, skip).len());
    for line in lines {
        let line: String = line.chars().skip(skip).collect();
        let line = line.trim_end().trim_end_matches(['▌', '█']).trim_end();
        let line = if skip > 0 {
            line.strip_prefix(' ').unwrap_or(line)
        } else {
            line
        };
        if line.starts_with("    │ ") || line.starts_with("    └") {
            out.push(line.to_string());
            // A peek ends at its `└` footer; a live line goes on to its continuations.
            if line.starts_with("    └") && out.iter().any(|line| line.starts_with("    │ ")) {
                break;
            }
        } else if line.starts_with("       ") && !line.trim().is_empty() && !out.is_empty() {
            out.push(line.to_string());
        } else {
            break;
        }
    }
    out
}

fn write_json(dir: &Path, name: &str, lines: &[String]) {
    fs::write(dir.join(name), serde_json::to_string_pretty(lines).unwrap()).unwrap();
}

/// Put the cursor on task `number` in the current view, as a click would.
fn select_task(state: &mut DomainState, model: &mut BoardModel, number: u64) {
    let id = state
        .tasks()
        .iter()
        .find(|task| task.number == Some(number))
        .unwrap()
        .id;
    let index = model
        .visible_ids()
        .iter()
        .position(|visible| *visible == id)
        .unwrap();
    apply_intent(state, model, BoardIntent::SelectIndex(index), None).unwrap();
}

#[test]
fn shared_fixture_first_flow_and_detail_references() {
    fixture_flow(false);
}

#[test]
#[ignore = "regenerates browser reference artifacts"]
fn regenerate_parity_references() {
    fixture_flow(true);
}

fn fixture_flow(export: bool) {
    let capture = |model: &BoardModel, width: u16, name: &str| {
        let text = capture(model, width, name);
        if export {
            export_reference(&text, width, name);
        }
        text
    };
    for width in [40, 78, 109, 110] {
        let (mut state, mut model) = fixture_board();
        let initial = capture(&model, width, "initial");
        // The v7 fixture already holds a doing task; ctrl+s below adds T13 to it.
        assert!(initial.contains("DOING"));
        if width >= 110 {
            // Wide frames always pair the list with the selected task's detail.
            assert!(initial.contains("Confirm the wording before release."));
            key(
                &mut state,
                &mut model,
                KeyCode::Right,
                KeyModifiers::NONE,
                width,
            );
            assert_eq!(
                model.input_mode(),
                tsk_tui::ui::board::BoardInputMode::TaskPage
            );
            capture(&model, width, "detail-focus");
            key(
                &mut state,
                &mut model,
                KeyCode::Left,
                KeyModifiers::NONE,
                width,
            );
            assert_eq!(
                model.input_mode(),
                tsk_tui::ui::board::BoardInputMode::Normal
            );
        }
        key(
            &mut state,
            &mut model,
            KeyCode::Down,
            KeyModifiers::NONE,
            width,
        );
        key(
            &mut state,
            &mut model,
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
            width,
        );
        assert!(capture(&model, width, "doing").contains("DOING"));
        key(
            &mut state,
            &mut model,
            KeyCode::Enter,
            KeyModifiers::NONE,
            width,
        );
        let page = capture(&model, width, "page");
        assert!(page.contains("plain note"));
        assert!(page.contains("steps 1/2"));
        apply_intent(&mut state, &mut model, BoardIntent::FormFocusNext, None).unwrap();
        // The app keyboard boundary resolves Enter on a stored step to ToggleStep.
        apply_intent(&mut state, &mut model, BoardIntent::ToggleStep, None).unwrap();
        assert!(capture(&model, width, "step-toggled").contains("steps 2/2"));
        let task = state
            .tasks()
            .iter()
            .find(|task| task.number == Some(13))
            .unwrap();
        assert_eq!(task.status, tsk_tui::domain::HumanStatus::Doing);
        apply_intent(&mut state, &mut model, BoardIntent::BeginAddStep, None).unwrap();
        for ch in "New step".chars() {
            apply_intent(&mut state, &mut model, BoardIntent::EditInsert(ch), None).unwrap();
        }
        key(
            &mut state,
            &mut model,
            KeyCode::Enter,
            KeyModifiers::NONE,
            width,
        );
        assert_eq!(
            state
                .tasks()
                .iter()
                .find(|task| task.number == Some(13))
                .unwrap()
                .steps
                .len(),
            3
        );
        capture(&model, width, "step-added");
        key(
            &mut state,
            &mut model,
            KeyCode::Esc,
            KeyModifiers::NONE,
            width,
        );
        key(
            &mut state,
            &mut model,
            KeyCode::Esc,
            KeyModifiers::NONE,
            width,
        );
        assert!(capture(&model, width, "back").contains("DOING"));
        // The board always groups rows by project; the reference keeps the frame's name.
        let grouped = capture(&model, width, "project-order");
        assert!(grouped.contains("tsk-parity"), "{grouped}");
    }
}

#[test]
fn selection_uses_arrow_and_underlined_tab_without_reverse_fill() {
    use ratatui::style::Modifier;
    let mut state: DomainState =
        serde_json::from_str(include_str!("fixtures/demo-parity/store.json")).unwrap();
    let mut model = BoardModel::from_domain(&state, Some(PathBuf::from("/tmp/tsk-parity")));
    model.set_selected_project(None);
    let mut terminal = Terminal::new(TestBackend::new(78, 24)).unwrap();
    terminal
        .draw(|f| {
            draw_board(f, &model);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert!(
        buffer
            .content
            .iter()
            .all(|cell| !cell.modifier.contains(Modifier::REVERSED)),
        "board and tabs must not use reverse fill"
    );
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.symbol() == "b" && cell.modifier.contains(Modifier::UNDERLINED)),
        "active board tab must be underlined"
    );
    let frame = capture(&model, 78, "selection-style");
    assert!(
        frame.contains("▸  T12"),
        "selection arrow must retain the number: {frame}"
    );
    let tab_row = buffer
        .content
        .chunks(78)
        .find(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .contains("board")
        })
        .unwrap();
    let text = tab_row.iter().map(|cell| cell.symbol()).collect::<String>();
    for (label, active) in [("board", true), ("questions", false)] {
        let start = text[..text.find(label).unwrap()].chars().count();
        for cell in &tab_row[start..start + label.len()] {
            assert_eq!(
                cell.modifier.contains(Modifier::UNDERLINED),
                active,
                "{label}"
            );
            assert_eq!(cell.modifier.contains(Modifier::BOLD), active, "{label}");
            assert_eq!(cell.modifier.contains(Modifier::DIM), !active, "{label}");
        }
    }
    select_task(&mut state, &mut model, 13);
    terminal
        .draw(|f| {
            draw_board(f, &model);
        })
        .unwrap();
    assert!(capture(&model, 78, "wrapped-selection").contains("▸  T13"));
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| !cell.modifier.contains(Modifier::REVERSED)),
        "wrapped selected rows must not use reverse fill"
    );
}
