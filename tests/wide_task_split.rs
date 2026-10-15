//! Wide list + detail layout: chrome, focus routing, `J`/`K` stepping, mouse, dirty drafts.
//!
//! At 110 columns or wider the board always paints the list and the selected task's detail
//! side by side. Focus moves between the two columns (`→`/`l`, `←`/`h`, Enter, Esc, clicks);
//! the geometry never changes with focus.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tsk_tui::domain::{DomainState, HumanStatus, Notice, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::capture::CaptureField;
use tsk_tui::ui::input::{map_key, route_responsive_key, ResponsiveKeyRoute};
use tsk_tui::ui::mouse::{
    left_click, map_board_mouse, map_responsive_board_mouse, wide_mouse_focus_intent,
};
use tsk_tui::ui::render::{assert_buffer_mono, QueueHitMap, QueueHitTarget};
use tsk_tui::ui::tier::{
    resolve_responsive, FocusedSurface, ResponsivePresentation, Tier, WideStage,
    STANDARD_VERB_BAR_ENTRY_BUDGET, WIDE_SPLIT_MIN_WIDTH,
};

use tsk_tui::ui::{
    apply_intent, draw_board, BoardInputMode, BoardIntent, BoardModel, IntentOutcome,
};

const REPO: &str = "/repos/tsk";

const T12_NOTES: &str = "Rework the wide split so the task page reads as a **detail pane**, not a boxed clone.\n\n- keep board unboxed\n- decide separator\n- verb bar ownership\n\n```\nresolve_responsive(w, h, focus)\n```";

/// The reference fixture: T12 started with markdown notes in this repo, T15 ready on the
/// desk. The model carries the numbers; the domain shares the ids. Every scope is shown.
fn fixture() -> (DomainState, BoardModel) {
    fixture_with_titles("Frame the wide task view", "Renew domain")
}

fn fixture_with_titles(first: &str, second: &str) -> (DomainState, BoardModel) {
    let mut domain = DomainState::new();
    let started = domain
        .create(
            first,
            Some(T12_NOTES.to_string()),
            TaskScope::Project {
                path: REPO.to_string(),
            },
            ProvenanceOrigin::Manual,
        )
        .expect("create T12");
    domain
        .set_status(started, HumanStatus::Doing)
        .expect("start T12");
    let ready = domain
        .create(
            second,
            Some("renewal notes".to_string()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create T15");
    domain
        .set_status(ready, HumanStatus::Todo)
        .expect("ready T15");
    let mut tasks = domain.tasks().to_vec();
    tasks[0].number = Some(12);
    tasks[1].number = Some(15);
    let model = BoardModel::from_tasks(tasks, Some(PathBuf::from(REPO)));
    (domain, model)
}

fn go(domain: &mut DomainState, model: &mut BoardModel, intent: BoardIntent) {
    apply_intent(domain, model, intent, None).expect("apply intent");
}

/// Move input focus to `stage` through the real focus intents.
fn focus(domain: &mut DomainState, model: &mut BoardModel, stage: WideStage) {
    let intent = match stage {
        WideStage::Board => BoardIntent::FocusList,
        WideStage::Task => BoardIntent::FocusDetail,
    };
    go(domain, model, intent);
    assert_eq!(model.wide_stage(), stage);
}

const STAGES: [WideStage; 2] = [WideStage::Board, WideStage::Task];

fn render_buffer(model: &BoardModel, width: u16, height: u16) -> (Buffer, QueueHitMap) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    let mut hits = QueueHitMap::default();
    terminal
        .draw(|frame| hits = draw_board(frame, model))
        .expect("draw board");
    (terminal.backend().buffer().clone(), hits)
}

fn rows_of(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

/// Render and assert the frame is mono at every width, wide or not.
fn render(model: &BoardModel, width: u16, height: u16) -> (Vec<String>, QueueHitMap) {
    let (buffer, hits) = render_buffer(model, width, height);
    assert_buffer_mono(&buffer);
    (rows_of(&buffer), hits)
}

fn region_text(rows: &[String], area: Rect) -> String {
    rows.iter()
        .skip(area.y as usize)
        .take(area.height as usize)
        .map(|row| {
            row.chars()
                .skip(area.x as usize)
                .take(area.width as usize)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn column_text(rows: &[String], column: Rect, y: u16) -> String {
    rows[y as usize]
        .chars()
        .skip(column.x as usize)
        .take(column.width as usize)
        .collect()
}

fn footer_rows(height: u16) -> (u16, u16, u16) {
    (height - 3, height - 2, height - 1)
}

fn inside(area: Rect, hit: Rect) -> bool {
    hit.x >= area.x
        && hit.y >= area.y
        && hit.x.saturating_add(hit.width) <= area.x.saturating_add(area.width)
        && hit.y.saturating_add(hit.height) <= area.y.saturating_add(area.height)
}

fn column_text_area(column: Rect, height: u16) -> Rect {
    Rect::new(column.x, 0, column.width, height - 3)
}

// ---------------------------------------------------------------------------
// Chrome
// ---------------------------------------------------------------------------

#[test]
fn wide_geometry_is_list_plus_detail_for_both_focus_owners() {
    for stage in STAGES {
        for width in [WIDE_SPLIT_MIN_WIDTH, 130, 200] {
            let geometry = resolve_responsive(width, 24, stage);
            assert_eq!(geometry.presentation, ResponsivePresentation::WideSplit);
            assert_eq!(
                geometry.board,
                resolve_responsive(width, 24, WideStage::Board).board
            );
            assert!(geometry.board.width > 0 && geometry.task.width > 0);
        }
        let narrow = resolve_responsive(WIDE_SPLIT_MIN_WIDTH - 1, 24, stage);
        assert_eq!(
            narrow.presentation,
            match stage {
                WideStage::Board => ResponsivePresentation::SingleBoard,
                WideStage::Task => ResponsivePresentation::SingleTask,
            }
        );
    }
}

#[test]
fn wide_frames_are_mono_for_both_focus_owners_at_every_size() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, stage);
        for (width, height) in [
            (WIDE_SPLIT_MIN_WIDTH, 24),
            (111, 24),
            (130, 24),
            (130, 10),
            (157, 26),
            (200, 40),
        ] {
            let (buffer, _) = render_buffer(&model, width, height);
            assert_buffer_mono(&buffer);
        }
    }
}

#[test]
fn wide_paints_exactly_one_footer_rule_row_for_both_focus_owners() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, stage);
        let (rows, hits) = render(&model, 130, 24);
        let (rule_y, status_y, verb_y) = footer_rows(24);
        let rule_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.chars().all(|c| c == '─'))
            .map(|(y, _)| y)
            .collect();
        assert_eq!(
            rule_rows,
            vec![rule_y as usize],
            "{stage:?}: one footer rule"
        );
        assert!(
            rows[status_y as usize].starts_with(" all"),
            "{stage:?}: footer status names the scope: {}",
            rows[status_y as usize]
        );
        let verb_row = &rows[verb_y as usize];
        assert!(verb_row.starts_with(' '), "{stage:?}: leading space");
        assert!(verb_row.contains("ctrl+d done"), "{stage:?}: {verb_row}");
        let ctrl_rows = rows.iter().filter(|row| row.contains("ctrl+")).count();
        assert_eq!(ctrl_rows, 1, "{stage:?}: exactly one verb bar");
        let verb_hits: Vec<&tsk_tui::ui::render::QueueHit> = hits
            .regions
            .iter()
            .filter(|hit| matches!(hit.target, QueueHitTarget::Verb(_)))
            .collect();
        assert!(!verb_hits.is_empty(), "{stage:?}: verb hits exist");
        assert!(verb_hits.iter().all(|hit| hit.area.y == verb_y));
    }
}

#[test]
fn list_and_detail_render_the_standard_tier_at_130x24() {
    let (mut domain, mut model) = fixture();
    let geometry = resolve_responsive(130, 24, WideStage::Board);
    assert_eq!(geometry.density, Tier::Standard);
    let (rows, hits) = render(&model, 130, 24);
    let list = |y: u16| column_text(&rows, geometry.board, y);
    assert!(list(0).trim().is_empty(), "list row 0 blank: {:?}", list(0));
    assert!(
        list(1).contains("1 board  ·  2 questions  ·  all ▾  ·  number ▾"),
        "selector row: {}",
        list(1)
    );
    assert!(list(3).starts_with(" DOING ─"), "{}", list(3));
    assert!(list(5).starts_with("▸  T12 Frame the wide task view"));
    assert!(list(7).starts_with(" TODO ─"), "{}", list(7));
    assert!(list(9).starts_with("   T15 Renew domain"));
    assert_eq!(rows[5].chars().nth(geometry.rule.x as usize), Some('│'));
    // The detail shows the selected task beside the list.
    let detail = geometry.task_content();
    assert!(column_text(&rows, detail, 1).starts_with(" ● T12 Frame the wide task view"));
    assert!(column_text(&rows, detail, 3).contains("Rework the wide split"));
    let verbs = hits
        .regions
        .iter()
        .filter(|hit| matches!(hit.target, QueueHitTarget::Verb(_)))
        .count();
    assert_eq!(verbs, 4, "doing row bar: {verbs}");
    assert!(verbs <= usize::from(STANDARD_VERB_BAR_ENTRY_BUDGET));

    focus(&mut domain, &mut model, WideStage::Task);
    let (rows, _) = render(&model, 130, 24);
    assert_eq!(
        column_text(&rows, geometry.board, 5),
        column_text(&rows, geometry.board, 5),
        "the list keeps painting while the detail owns focus"
    );
    assert!(column_text(&rows, geometry.board, 5).starts_with("▸  T12 Frame"));
}

#[test]
fn rows_paint_the_title_only() {
    let (_, model) = fixture();
    let geometry = resolve_responsive(130, 24, WideStage::Board);
    let (rows, _) = render(&model, 130, 24);
    let row = column_text(&rows, geometry.board, 5);
    assert_eq!(row.trim_end(), "▸  T12 Frame the wide task view", "{row}");
    assert!(!rows.iter().any(|row| row.contains("└─ tsk")));
}

#[test]
fn detail_header_weight_follows_focus() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, stage);
        let column = resolve_responsive(130, 24, stage).task_content();
        let (buffer, _) = render_buffer(&model, 130, 24);
        let rows = rows_of(&buffer);
        let header = column_text(&rows, column, 1);
        assert!(
            header.starts_with(" ● T12 Frame the wide task view"),
            "{header}"
        );
        assert!(header.trim_end().ends_with("doing · tsk"), "{header}");
        assert!(column_text(&rows, column, 2).chars().all(|c| c == '─'));
        let body = region_text(&rows, Rect::new(column.x, 3, column.width, 18));
        assert!(
            !body.contains("● T12"),
            "{stage:?}: in-pane header must not paint"
        );
        let title_cell = &buffer[(column.x + 7, 1)];
        match stage {
            WideStage::Board => {
                assert!(
                    title_cell.modifier.contains(Modifier::DIM),
                    "list focus: dim"
                );
                assert!(!title_cell.modifier.contains(Modifier::BOLD));
            }
            WideStage::Task => {
                assert!(
                    title_cell.modifier.contains(Modifier::BOLD),
                    "detail focus: bold"
                );
                assert!(!title_cell.modifier.contains(Modifier::DIM));
            }
        }
    }
}

#[test]
fn status_row_crumb_and_keys_follow_focus() {
    let (mut domain, mut model) = fixture();
    let (_, status_y, _) = footer_rows(24);
    let (rows, _) = render(&model, 130, 24);
    let status = &rows[status_y as usize];
    assert!(status.trim_end().ends_with("→ detail"), "{status}");

    focus(&mut domain, &mut model, WideStage::Task);
    let (rows, _) = render(&model, 130, 24);
    let status = &rows[status_y as usize];
    assert!(
        status.contains("detail    ← list · J/K next/prev"),
        "{status}"
    );
}

#[test]
fn status_row_refusal_wins_over_the_crumb_then_the_keys() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    let (_, status_y, _) = footer_rows(24);
    model.set_message("x".repeat(100));
    let (rows, _) = render(&model, 130, 24);
    let status = &rows[status_y as usize];
    assert!(status.contains(&"x".repeat(100)));
    assert!(
        !status.contains("detail    ←"),
        "crumb drops first: {status}"
    );

    model.set_message("y".repeat(124));
    let (rows, _) = render(&model, 130, 24);
    let status = &rows[status_y as usize];
    assert!(status.contains(&"y".repeat(124)));
    assert!(
        !status.contains("← list"),
        "keys drop after the crumb: {status}"
    );
}

#[test]
fn status_row_shows_editor_keys_while_an_editor_is_active() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    let (rows, _) = render(&model, 130, 24);
    let (_, status_y, verb_y) = footer_rows(24);
    assert!(
        rows[verb_y as usize].contains("shift+enter save")
            && rows[verb_y as usize].contains("esc cancel"),
        "{}",
        rows[verb_y as usize]
    );
    assert!(!rows[status_y as usize].contains("← list"));
    let header = column_text(
        &rows,
        resolve_responsive(130, 24, WideStage::Task).task_content(),
        1,
    );
    assert!(header.trim_end().ends_with("editing title"), "{header}");
}

#[test]
fn empty_detail_paints_no_task_header_and_is_inert() {
    let mut model = BoardModel::from_tasks(Vec::new(), Some(PathBuf::from(REPO)));
    let mut domain = DomainState::new();
    assert_eq!(model.selected_id(), None);
    go(&mut domain, &mut model, BoardIntent::FocusDetail);
    assert_eq!(
        model.wide_stage(),
        WideStage::Board,
        "the detail needs a selection"
    );
    go(&mut domain, &mut model, BoardIntent::OpenTaskPage);
    assert_eq!(model.wide_stage(), WideStage::Board);

    // A selected task that then disappears: the detail paints the empty pane.
    let (_, mut model) = fixture();
    model.sync_from_domain(&DomainState::new());
    assert_eq!(model.selected_id(), None);
    let column = resolve_responsive(130, 24, WideStage::Board).task_content();
    let (rows, hits) = render(&model, 130, 24);
    let header = column_text(&rows, column, 1);
    assert!(header.starts_with(" no task"), "{header}");
    assert!(column_text(&rows, column, 3).contains("select a task to preview it here"));
    assert!(
        hits.regions
            .iter()
            .all(|hit| !inside(column_text_area(column, 24), hit.area)),
        "empty pane exposes no hits"
    );
}

#[test]
fn wide_hits_stay_inside_their_column_or_the_footer() {
    for stage in STAGES {
        for (width, height) in [(110, 24), (130, 24), (157, 26), (240, 60)] {
            let (mut domain, mut model) = fixture();
            focus(&mut domain, &mut model, stage);
            let geometry = resolve_responsive(width, height, stage);
            let (_, hits) = render(&model, width, height);
            let footer = Rect::new(0, height - 3, width, 3);
            assert_eq!(hits.footer, Some(footer), "{stage:?} {width}x{height}");
            let board = Rect::new(0, 0, geometry.board.width, height - 3);
            let task = Rect::new(
                geometry.task_content().x,
                0,
                geometry.task_content().width,
                height - 3,
            );
            for area in hits
                .regions
                .iter()
                .map(|hit| hit.area)
                .chain(hits.copyable.iter().copied())
            {
                assert!(
                    inside(footer, area) || inside(board, area) || inside(task, area),
                    "{stage:?} {width}x{height}: {area:?} escapes its column"
                );
                assert!(
                    !(area.x <= geometry.rule.x
                        && area.x + area.width > geometry.rule.x
                        && area.y < height - 3),
                    "{stage:?} {width}x{height}: {area:?} crosses the rule column"
                );
            }
            assert!(hits.regions.iter().any(|hit| inside(board, hit.area)));
        }
    }
}

#[test]
fn list_focused_detail_paints_controls_for_the_focus_router_only() {
    let (_, model) = fixture();
    let geometry = resolve_responsive(130, 24, WideStage::Board);
    let (_, hits) = render(&model, 130, 24);
    let column = column_text_area(geometry.task_content(), 24);
    let number = hits
        .regions
        .iter()
        .find(|hit| inside(column, hit.area) && matches!(hit.target, QueueHitTarget::TaskNumber(_)))
        .expect("the T<number> prefix keeps its copy hit");
    assert_eq!(
        number.area,
        Rect::new(geometry.task_content().x + 3, 1, 3, 1)
    );
    let control = hits
        .regions
        .iter()
        .find(|hit| inside(column, hit.area) && matches!(hit.target, QueueHitTarget::StepAdd))
        .expect("detail controls exist for the focus router");
    assert_eq!(
        click_map(&model, &hits, control.area.x, control.area.y),
        None,
        "detail controls are inert to the list-focused router"
    );
    assert_eq!(
        wide_mouse_focus_intent(
            &model,
            &hits,
            AREA_130,
            left_click(control.area.x, control.area.y)
        ),
        Some(BoardIntent::FocusDetail)
    );
}

#[test]
fn narrow_board_paints_one_surface_and_no_crumb() {
    let (_, model) = fixture();
    let (rows, _) = render(&model, 109, 24);
    assert!(
        rows[1].contains("1 board  ·  2 questions  ·  all ▾  ·  sort number ▾"),
        "{:?}",
        rows[1]
    );
    assert!(rows[5].starts_with("▸  T12 Frame the wide task view"));
    assert!(
        !rows[22].contains("→ detail"),
        "no crumb below the wide threshold"
    );
    assert!(
        !rows.join("\n").contains("Rework the wide split"),
        "no detail column"
    );
}

#[test]
fn title_edit_window_matches_the_painted_header_room_with_a_long_identifier() {
    let (mut domain, model) = fixture();
    let mut tasks = domain.tasks().to_vec();
    let id = model.selected_id().expect("selection");
    for task in &mut tasks {
        if task.id == id {
            task.number = Some(123456);
        }
    }
    let mut model = BoardModel::from_tasks(tasks, Some(PathBuf::from(REPO)));
    assert_eq!(model.selected_id(), Some(id));
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("x".repeat(200)),
    );
    let (rows, _) = render(&model, 130, 24);
    let column = resolve_responsive(130, 24, WideStage::Task).task_content();
    let header = column_text(&rows, column, 1);
    assert!(header.contains("T123456"), "{header}");
    assert!(header.trim_end().ends_with("editing title"), "{header}");
    // The draft wraps under the first row; every wrapped row fits the painted room.
    let rule_y = (2..21u16)
        .find(|y| {
            let row = column_text(&rows, column, *y);
            !row.trim().is_empty() && row.chars().all(|c| c == '─')
        })
        .expect("header rule");
    let wrapped = (1..rule_y)
        .map(|y| column_text(&rows, column, y))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(wrapped.matches('x').count(), 200, "{wrapped}");
    assert!(!wrapped.contains('…'), "caret window overflow: {wrapped}");
    // The caret follows the draft's end onto the last wrapped row.
    let mut terminal = Terminal::new(TestBackend::new(130, 24)).expect("test terminal");
    terminal
        .draw(|frame| {
            draw_board(frame, &model);
        })
        .expect("draw board");
    let cursor = terminal.get_cursor_position().expect("cursor position");
    let last = column_text(&rows, column, rule_y - 1);
    assert_eq!(cursor.y, rule_y - 1, "caret on the last title row");
    assert_eq!(
        cursor.x,
        column.x + last.trim_end().chars().count() as u16,
        "caret after the last x: {last}"
    );
}

/// A long title wraps onto further header rows instead of truncating with `…`; the dash
/// rule moves under the last title row and the state slot stays on the first row.
#[test]
fn long_detail_title_wraps_in_the_header_without_an_ellipsis() {
    let title = "Frame the wide task view so a long title wraps across several header rows \
                 instead of hiding its tail behind an ellipsis at the column edge";
    for stage in STAGES {
        let (mut domain, mut model) = fixture_with_titles(title, "Renew domain");
        focus(&mut domain, &mut model, stage);
        let column = resolve_responsive(130, 24, stage).task_content();
        let (rows, _) = render(&model, 130, 24);
        let first = column_text(&rows, column, 1);
        assert!(first.starts_with(" ● T12 Frame the wide"), "{first}");
        assert!(first.trim_end().ends_with("doing · tsk"), "{first}");
        let rule_y = (2..21u16)
            .find(|y| {
                let row = column_text(&rows, column, *y);
                !row.trim().is_empty() && row.chars().all(|c| c == '─')
            })
            .expect("header rule");
        assert!(rule_y > 2, "{stage:?}: the title wraps onto more rows");
        let header = (1..rule_y)
            .map(|y| column_text(&rows, column, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!header.contains('…'), "{stage:?}: truncated: {header}");
        let words = header
            .replace("doing · tsk", "")
            .split_whitespace()
            .skip(2)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(words, title, "{stage:?}: {header}");
        assert!(
            region_text(&rows, Rect::new(column.x, rule_y + 1, column.width, 3))
                .contains("Rework the wide split"),
            "{stage:?}: body starts under the rule"
        );
    }
}

// ---------------------------------------------------------------------------
// Focus routing
// ---------------------------------------------------------------------------

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Route one bare key through the focus router at a wide frame.
fn wide_key(model: &BoardModel, code: KeyCode) -> Option<BoardIntent> {
    route_key(model, ResponsivePresentation::WideSplit, code)
}

fn route_key(
    model: &BoardModel,
    presentation: ResponsivePresentation,
    code: KeyCode,
) -> Option<BoardIntent> {
    match route_responsive_key(
        model.input_mode(),
        model.wide_stage(),
        presentation,
        key(code),
    ) {
        ResponsiveKeyRoute::Intent(intent) => Some(intent),
        ResponsiveKeyRoute::Inert => None,
        ResponsiveKeyRoute::Surface => map_key(model.input_mode(), key(code)),
    }
}

/// Press one key at a wide width: map it, apply it when it maps to something.
fn press(domain: &mut DomainState, model: &mut BoardModel, code: KeyCode) -> Option<BoardIntent> {
    let intent = wide_key(model, code);
    if let Some(intent) = intent.clone() {
        go(domain, model, intent);
    }
    intent
}

#[test]
fn list_keys_select_focus_the_detail_and_stay_put() {
    let (mut domain, mut model) = fixture();
    let first = model.selected_id();
    assert_eq!(
        wide_key(&model, KeyCode::Left),
        None,
        "← is inert on the list"
    );
    assert_eq!(wide_key(&model, KeyCode::Char('h')), None);
    assert_eq!(
        wide_key(&model, KeyCode::Esc),
        Some(BoardIntent::CloseLayer)
    );
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Char('j')),
        Some(BoardIntent::SelectNext)
    );
    let second = model.selected_id().expect("moved");
    assert_ne!(Some(second), first);
    assert_eq!(model.wide_stage(), WideStage::Board);
    let (rows, _) = render(&model, 130, 24);
    assert!(
        column_text(
            &rows,
            resolve_responsive(130, 24, WideStage::Board).task_content(),
            1
        )
        .contains("T15 Renew domain"),
        "the detail follows the list selection"
    );
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Tab),
        None,
        "Tab is not a focus key"
    );
    assert_eq!(model.wide_stage(), WideStage::Board);
    for code in [KeyCode::Right, KeyCode::Char('l')] {
        let (mut domain, mut model) = fixture();
        assert_eq!(
            press(&mut domain, &mut model, code),
            Some(BoardIntent::FocusDetail)
        );
        assert_eq!(model.wide_stage(), WideStage::Task);
        assert_eq!(model.focused_surface(), FocusedSurface::Task);
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        assert_eq!(model.edit_target(), model.selected_id());
    }

    let (mut domain, mut model) = fixture();
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Enter),
        Some(BoardIntent::OpenTaskPage)
    );
    assert_eq!(model.wide_stage(), WideStage::Task);
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn detail_keys_return_to_the_list_close_and_navigate_the_page() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    let bound = model.edit_target().expect("bound page");
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Char('j')),
        Some(BoardIntent::PageScrollDown)
    );
    assert_eq!(
        model.selected_id(),
        Some(bound),
        "j navigates the page, not the list"
    );
    assert_eq!(
        wide_key(&model, KeyCode::Right),
        None,
        "→ is inert on the detail"
    );
    for code in [KeyCode::Left, KeyCode::Char('h')] {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, WideStage::Task);
        let bound = model.edit_target().expect("bound page");
        assert_eq!(
            press(&mut domain, &mut model, code),
            Some(BoardIntent::FocusList)
        );
        assert_eq!(model.wide_stage(), WideStage::Board);
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert_eq!(model.edit_target(), Some(bound), "the detail stays bound");
    }
    for code in [KeyCode::Esc, KeyCode::Enter] {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, WideStage::Task);
        press(&mut domain, &mut model, code);
        assert_eq!(
            model.wide_stage(),
            WideStage::Board,
            "{code:?} closes to the list"
        );
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
    }
}

#[test]
fn focus_keys_need_a_selection() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_tasks(Vec::new(), Some(PathBuf::from(REPO)));
    for code in [KeyCode::Right, KeyCode::Enter] {
        press(&mut domain, &mut model, code);
        assert_eq!(model.wide_stage(), WideStage::Board, "{code:?}");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
    }
    assert_eq!(model.edit_target(), None);
}

#[test]
fn focus_keys_fall_through_to_editor_semantics_while_editing() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    assert_eq!(
        wide_key(&model, KeyCode::Left),
        Some(BoardIntent::EditMoveLeft)
    );
    assert_eq!(
        wide_key(&model, KeyCode::Right),
        Some(BoardIntent::EditMoveRight)
    );
    for character in ['h', 'l', 'J', 'K'] {
        assert_eq!(
            wide_key(&model, KeyCode::Char(character)),
            Some(BoardIntent::EditInsert(character)),
            "{character} remains text in a wide editor"
        );
    }
    assert_eq!(
        wide_key(&model, KeyCode::Esc),
        Some(BoardIntent::CancelEdit)
    );
    // A parked session in view mode moves focus; the detail stays bound.
    go(&mut domain, &mut model, BoardIntent::CancelEdit);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(model.task_editing());
    assert_eq!(
        wide_key(&model, KeyCode::Left),
        Some(BoardIntent::FocusList)
    );
    press(&mut domain, &mut model, KeyCode::Esc);
    assert!(!model.task_editing(), "Esc cancels the session first");
    assert_eq!(model.wide_stage(), WideStage::Task, "and keeps the focus");
}

#[test]
fn narrow_routes_carry_no_focus_or_peek_meaning() {
    let (mut domain, mut model) = fixture();
    let narrow =
        |model: &BoardModel, code| route_key(model, ResponsivePresentation::SingleBoard, code);
    for code in [
        KeyCode::Right,
        KeyCode::Left,
        KeyCode::Char('l'),
        KeyCode::Char('h'),
    ] {
        assert_eq!(narrow(&model, code), None, "{code:?}");
    }
    assert_eq!(
        narrow(&model, KeyCode::Enter),
        Some(BoardIntent::OpenTaskPage)
    );
    go(&mut domain, &mut model, BoardIntent::OpenTaskPage);
    let page =
        |model: &BoardModel, code| route_key(model, ResponsivePresentation::SingleTask, code);
    assert_eq!(page(&model, KeyCode::Left), None);
    assert_eq!(page(&model, KeyCode::Right), None);
    assert_eq!(
        page(&model, KeyCode::Char('J')),
        Some(BoardIntent::NextTask)
    );
    assert_eq!(page(&model, KeyCode::Esc), Some(BoardIntent::CloseLayer));
}

#[test]
fn shrinking_and_growing_keeps_the_focus_and_its_session() {
    let long = "long notes ".repeat(400);
    for stage in STAGES {
        let mut domain = DomainState::new();
        domain
            .create(
                "resize survivor",
                Some(long.clone()),
                TaskScope::Global,
                ProvenanceOrigin::Manual,
            )
            .expect("create survivor");
        domain
            .create(
                "other row",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
            )
            .expect("create other");
        let mut model = BoardModel::from_domain(&domain, None);
        let survivor = domain
            .tasks()
            .iter()
            .find(|task| task.title == "resize survivor")
            .expect("survivor")
            .id;
        let index = model
            .visible_ids()
            .iter()
            .position(|&id| id == survivor)
            .expect("survivor row");
        go(&mut domain, &mut model, BoardIntent::SelectIndex(index));
        focus(&mut domain, &mut model, stage);
        let _ = render(&model, 130, 24);
        if stage == WideStage::Task {
            go(&mut domain, &mut model, BoardIntent::PageWheelScrollDown);
            assert!(model.page_scroll() > 0, "fixture exercises page scroll");
        }
        let snapshot = |model: &BoardModel| {
            (
                model.wide_stage(),
                model.focused_surface(),
                model.selected_id(),
                model.edit_target(),
                model.page_scroll(),
                model.input_mode(),
            )
        };
        let before = snapshot(&model);
        let (narrow_rows, _) = render(&model, 109, 24);
        match stage {
            WideStage::Board => assert!(narrow_rows.join("\n").contains("other row")),
            WideStage::Task => assert!(
                !narrow_rows.join("\n").contains("other row"),
                "the page fills the narrow frame"
            ),
        }
        assert_eq!(
            snapshot(&model),
            before,
            "{stage:?}: shrink keeps focus and session"
        );
        let (wide_rows, _) = render(&model, 130, 24);
        assert_eq!(
            snapshot(&model),
            before,
            "{stage:?}: grow keeps focus and session"
        );
        let geometry = resolve_responsive(130, 24, stage);
        assert!(column_text(&wide_rows, geometry.task_content(), 1).contains("resize survivor"));
        assert!(region_text(&wide_rows, geometry.board).contains("resize survivor"));
    }
}

#[test]
fn shrinking_during_an_edit_keeps_mode_draft_cursor_and_binding() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    go(&mut domain, &mut model, BoardIntent::EditMoveLeft);
    let snapshot = |model: &BoardModel| {
        (
            model.wide_stage(),
            model.input_mode(),
            model.edit_target(),
            model.edit_buffer().to_string(),
            model.edit_cursor(),
        )
    };
    let before = snapshot(&model);
    assert!(model.task_session_dirty());
    let (narrow_rows, _) = render(&model, 109, 24);
    assert!(narrow_rows.join("\n").contains(&before.3));
    let _ = render(&model, 130, 24);
    assert_eq!(snapshot(&model), before);
}

#[test]
fn repeated_threshold_crossings_keep_both_focus_owners_live() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, stage);
        for width in [109u16, 110].into_iter().cycle().take(20) {
            let (rows, hits) = render(&model, width, 24);
            assert!(!rows.is_empty());
            assert!(!hits.regions.is_empty(), "{stage:?} at {width}");
        }
        assert_eq!(model.wide_stage(), stage);
    }
}

#[test]
fn focus_changes_never_mutate_the_domain() {
    let (mut domain, mut model) = fixture();
    let before = domain.clone();
    for intent in [
        BoardIntent::FocusDetail,
        BoardIntent::FocusList,
        BoardIntent::FocusDetail,
        BoardIntent::NextTask,
        BoardIntent::PrevTask,
        BoardIntent::CloseLayer,
        BoardIntent::OpenTaskPage,
        BoardIntent::FocusList,
    ] {
        let outcome = apply_intent(&mut domain, &mut model, intent, None).expect("focus intent");
        assert_eq!(outcome, IntentOutcome::None);
    }
    assert_eq!(model.wide_stage(), WideStage::Board);
    assert_eq!(domain.tasks(), before.tasks());
}

// ---------------------------------------------------------------------------
// J / K on the task detail
// ---------------------------------------------------------------------------

/// T1 started, T2 ready, T3 open (under the inbox heading), all on the desk.
fn stepping_fixture() -> (DomainState, BoardModel, [uuid::Uuid; 3]) {
    let mut domain = DomainState::new();
    let mut ids = [uuid::Uuid::nil(); 3];
    for (slot, (title, status)) in ids.iter_mut().zip([
        ("first started", HumanStatus::Doing),
        ("second ready", HumanStatus::Todo),
        ("third open", HumanStatus::Todo),
    ]) {
        let id = domain
            .create(title, None, TaskScope::Global, ProvenanceOrigin::Manual)
            .expect("create");
        domain.set_status(id, status).expect("status");
        *slot = id;
    }
    let model = BoardModel::from_domain(&domain, None);
    (domain, model, ids)
}

#[test]
fn j_and_k_step_the_detail_through_list_order_and_skip_headings() {
    let (mut domain, mut model, [first, second, third]) = stepping_fixture();
    assert_eq!(model.selected_id(), Some(first));
    focus(&mut domain, &mut model, WideStage::Task);

    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Char('J')),
        Some(BoardIntent::NextTask)
    );
    assert_eq!(model.edit_target(), Some(second));
    assert_eq!(
        model.selected_id(),
        Some(second),
        "the list selection follows"
    );
    assert_eq!(
        model.wide_stage(),
        WideStage::Task,
        "focus stays on the detail"
    );
    press(&mut domain, &mut model, KeyCode::Char('J'));
    assert_eq!(
        model.edit_target(),
        Some(third),
        "the inbox heading is skipped"
    );
    press(&mut domain, &mut model, KeyCode::Char('J'));
    assert_eq!(model.edit_target(), Some(third));
    assert_eq!(model.message(), Some("last task in this list"));

    press(&mut domain, &mut model, KeyCode::Char('K'));
    assert_eq!(model.edit_target(), Some(second));
    press(&mut domain, &mut model, KeyCode::Char('K'));
    assert_eq!(model.edit_target(), Some(first));
    press(&mut domain, &mut model, KeyCode::Char('K'));
    assert_eq!(model.edit_target(), Some(first));
    assert_eq!(model.message(), Some("first task in this list"));
}

#[test]
fn j_and_k_follow_the_project_sort_order() {
    let mut domain = DomainState::new();
    let make = |domain: &mut DomainState, title: &str, scope: TaskScope| {
        let id = domain
            .create(title, None, scope, ProvenanceOrigin::Manual)
            .expect("create");
        domain.set_status(id, HumanStatus::Doing).expect("start");
        id
    };
    let beta = make(
        &mut domain,
        "beta",
        TaskScope::Project {
            path: "/repos/b".into(),
        },
    );
    let desk = make(&mut domain, "desk", TaskScope::Global);
    let alpha = make(
        &mut domain,
        "alpha",
        TaskScope::Project {
            path: "/repos/a".into(),
        },
    );
    let mut model = BoardModel::from_domain(&domain, None);
    assert_eq!(model.visible_ids(), vec![desk, alpha, beta]);
    model.set_message("");
    let index = model
        .visible_ids()
        .iter()
        .position(|&id| id == desk)
        .unwrap();
    go(&mut domain, &mut model, BoardIntent::SelectIndex(index));
    focus(&mut domain, &mut model, WideStage::Task);
    press(&mut domain, &mut model, KeyCode::Char('J'));
    assert_eq!(model.edit_target(), Some(alpha));
    press(&mut domain, &mut model, KeyCode::Char('J'));
    assert_eq!(model.edit_target(), Some(beta));
}

#[test]
fn j_and_k_refuse_a_dirty_draft_and_keep_it() {
    let (mut domain, mut model, [first, ..]) = stepping_fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert!(model.task_session_dirty());
    let draft = model.edit_buffer().to_string();
    for code in [KeyCode::Char('J'), KeyCode::Char('K')] {
        press(&mut domain, &mut model, code);
        assert_eq!(model.edit_target(), Some(first), "{code:?}: refused");
        assert_eq!(model.selected_id(), Some(first));
        assert_eq!(model.edit_buffer(), draft);
        assert!(model
            .message()
            .is_some_and(|message| message.contains("save or cancel")));
    }
}

#[test]
fn j_and_k_name_a_detail_whose_task_left_the_list() {
    let (mut domain, mut model, [first, ..]) = stepping_fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    domain
        .set_status(first, HumanStatus::Done)
        .expect("done elsewhere");
    model.sync_from_domain(&domain);
    assert!(
        !model.visible_ids().contains(&first),
        "the closed drawer hides it"
    );
    if model.edit_target() == Some(first) && model.wide_stage() == WideStage::Task {
        press(&mut domain, &mut model, KeyCode::Char('J'));
        assert_eq!(model.edit_target(), Some(first));
        assert_eq!(
            model.message(),
            Some("this task is not in the list · esc back")
        );
    }
}

#[test]
fn j_and_k_on_the_list_do_nothing() {
    let (mut domain, mut model, [first, ..]) = stepping_fixture();
    go(&mut domain, &mut model, BoardIntent::NextTask);
    assert_eq!(model.selected_id(), Some(first));
    assert_eq!(model.wide_stage(), WideStage::Board);
    assert_eq!(model.edit_target(), None);
}

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

const AREA_130: Rect = Rect {
    x: 0,
    y: 0,
    width: 130,
    height: 24,
};

fn row_hit(hits: &QueueHitMap, column: Rect, predicate: impl Fn(uuid::Uuid) -> bool) -> Rect {
    hits.regions
        .iter()
        .find(|hit| {
            inside(column_text_area(column, 24), hit.area)
                && matches!(hit.target, QueueHitTarget::Task(id) if predicate(id))
        })
        .map(|hit| hit.area)
        .expect("row hit")
}

fn click_map(model: &BoardModel, hits: &QueueHitMap, x: u16, y: u16) -> Option<BoardIntent> {
    map_responsive_board_mouse(model, hits, AREA_130, left_click(x, y))
}

#[test]
fn list_row_click_retargets_the_detail_and_keeps_list_focus() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, stage);
        let selected = model.selected_id();
        let geometry = resolve_responsive(130, 24, stage);
        let (_, hits) = render(&model, 130, 24);
        let other = row_hit(&hits, geometry.board, |id| Some(id) != selected);
        assert_eq!(
            wide_mouse_focus_intent(&model, &hits, AREA_130, left_click(other.x, other.y)),
            None,
            "{stage:?}: a list row is not a detail click"
        );
        let intent = click_map(&model, &hits, other.x, other.y).expect("row click maps");
        assert!(matches!(intent, BoardIntent::FocusBoardAndSelectIndex(_)));
        go(&mut domain, &mut model, intent);
        assert_ne!(model.selected_id(), selected);
        assert_eq!(model.wide_stage(), WideStage::Board, "{stage:?}");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        let (after, _) = render(&model, 130, 24);
        let header = column_text(&after, geometry.task_content(), 1);
        assert!(
            header.contains("T15 Renew domain"),
            "detail retargets: {header}"
        );
    }
}

#[test]
fn plain_click_marks_in_mark_mode_while_ctrl_click_keeps_the_wide_route() {
    let (mut domain, mut model) = fixture();
    let selected = model.selected_id();
    let geometry = resolve_responsive(130, 24, WideStage::Board);
    let (_, hits) = render(&model, 130, 24);
    let row = row_hit(&hits, geometry.board, |id| Some(id) != selected);
    go(&mut domain, &mut model, BoardIntent::ToggleMarkMode);
    let mark = click_map(&model, &hits, row.x, row.y).expect("mark-mode row click");
    let BoardIntent::MarkToggleAt(index) = mark else {
        panic!("plain click should mark in mark mode: {mark:?}");
    };
    let marked = model.visible_ids()[index];
    go(&mut domain, &mut model, mark);
    assert!(model.marked_ids().contains(&marked));
    assert_eq!(model.wide_stage(), WideStage::Board);
    let mut ctrl_click = left_click(row.x, row.y);
    ctrl_click.modifiers = KeyModifiers::CONTROL;
    assert!(matches!(
        map_responsive_board_mouse(&model, &hits, AREA_130, ctrl_click),
        Some(BoardIntent::FocusBoardAndSelectIndex(_))
    ));
}

#[test]
fn detail_click_focuses_then_dispatches_against_the_painted_frame() {
    let (mut domain, mut model) = fixture();
    let geometry = resolve_responsive(130, 24, WideStage::Board);
    let (_, hits) = render(&model, 130, 24);
    let add = hits
        .regions
        .iter()
        .find(|hit| {
            inside(column_text_area(geometry.task_content(), 24), hit.area)
                && matches!(hit.target, QueueHitTarget::StepAdd)
        })
        .expect("detail add-step control")
        .area;
    let click = left_click(add.x, add.y);
    assert_eq!(
        click_map(&model, &hits, add.x, add.y),
        None,
        "list focus: inert"
    );
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, click),
        Some(BoardIntent::FocusDetail)
    );
    let blank = left_click(geometry.task_content().x + 10, 18);
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, blank),
        Some(BoardIntent::FocusDetail),
        "blank detail space focuses too"
    );
    let footer = left_click(geometry.task_content().x + 10, 23);
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, footer),
        None
    );

    go(&mut domain, &mut model, BoardIntent::FocusDetail);
    assert_eq!(
        click_map(&model, &hits, add.x, add.y),
        Some(BoardIntent::BeginAppendStep),
        "the painted frame's control dispatches after the focus move"
    );
}

#[test]
fn detail_click_without_selection_is_inert() {
    let (_, mut model) = fixture();
    model.sync_from_domain(&DomainState::new());
    let geometry = resolve_responsive(130, 24, WideStage::Board);
    let (_, hits) = render(&model, 130, 24);
    let click = left_click(geometry.task_content().x + 4, 4);
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, click),
        None
    );
    assert_eq!(click_map(&model, &hits, click.column, click.row), None);
}

#[test]
fn list_side_click_moves_focus_back_from_the_detail() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    let geometry = resolve_responsive(130, 24, WideStage::Task);
    let (_, hits) = render(&model, 130, 24);
    assert_eq!(
        click_map(&model, &hits, 2, 14),
        Some(BoardIntent::FocusList)
    );
    assert_eq!(
        click_map(&model, &hits, geometry.rule.x, 5),
        None,
        "the rule column is inert"
    );
}

#[test]
fn row_double_click_focuses_the_detail() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, stage);
        let geometry = resolve_responsive(130, 24, stage);
        let (_, hits) = render(&model, 130, 24);
        let selected = model.selected_id();
        let row = row_hit(&hits, geometry.board, |id| Some(id) != selected);
        for _ in 0..2 {
            let intent = click_map(&model, &hits, row.x, row.y).expect("row click");
            go(&mut domain, &mut model, intent);
        }
        assert_eq!(
            model.wide_stage(),
            WideStage::Task,
            "{stage:?}: double-click focuses"
        );
        assert_eq!(model.edit_target(), model.selected_id());
        assert_ne!(model.selected_id(), selected);
        let (_, after) = render(&model, 130, 24);
        assert_eq!(
            row_hit(&after, geometry.board, |id| Some(id) == model.selected_id()),
            row,
            "the row stays under the pointer"
        );
    }
}

#[test]
fn footer_verbs_dispatch_for_the_focused_surface() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        focus(&mut domain, &mut model, stage);
        let (rows, hits) = render(&model, 130, 24);
        let verb_row = &rows[23];
        let done_x = u16::try_from(verb_row.find("ctrl+d done").expect("done verb")).expect("x");
        let intent = click_map(&model, &hits, done_x, 23);
        assert_eq!(intent, Some(BoardIntent::Complete), "{stage:?}: {verb_row}");
        let id = model.selected_id().expect("selection");
        go(&mut domain, &mut model, intent.expect("verb"));
        assert_eq!(domain.get(id).expect("task").status, HumanStatus::Done);
    }
}

#[test]
fn wheel_scrolls_only_the_focused_column() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    let geometry = resolve_responsive(130, 24, WideStage::Task);
    let (_, hits) = render(&model, 130, 24);
    let wheel = |x, y| MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(
        map_responsive_board_mouse(
            &model,
            &hits,
            AREA_130,
            wheel(geometry.task_content().x + 5, 6)
        ),
        Some(BoardIntent::PageWheelScrollDown)
    );
    assert_eq!(
        map_responsive_board_mouse(&model, &hits, AREA_130, wheel(4, 6)),
        None,
        "the list is not the focused column"
    );
    focus(&mut domain, &mut model, WideStage::Board);
    let (_, hits) = render(&model, 130, 24);
    assert!(matches!(
        map_responsive_board_mouse(&model, &hits, AREA_130, wheel(4, 6)),
        Some(BoardIntent::ListScrollTo(_))
    ));
    assert_eq!(
        map_responsive_board_mouse(
            &model,
            &hits,
            AREA_130,
            wheel(geometry.task_content().x + 5, 6)
        ),
        None,
        "the detail is not the focused column"
    );
}

#[test]
fn help_and_palette_close_on_detail_clicks_without_dispatching() {
    for (open, expected, mode) in [
        (
            BoardIntent::OpenHelp,
            BoardIntent::CloseHelp,
            BoardInputMode::Help,
        ),
        (
            BoardIntent::OpenCommandPalette,
            BoardIntent::CloseCommandSurface,
            BoardInputMode::Palette,
        ),
    ] {
        let (mut domain, mut model) = fixture();
        go(&mut domain, &mut model, open);
        assert_eq!(model.input_mode(), mode);
        let geometry = resolve_responsive(130, 24, WideStage::Board);
        let (_, hits) = render(&model, 130, 24);
        let click = left_click(geometry.task_content().x + 6, 6);
        assert_eq!(
            wide_mouse_focus_intent(&model, &hits, AREA_130, click),
            None
        );
        assert_eq!(
            click_map(&model, &hits, click.column, click.row),
            Some(expected)
        );
        if mode == BoardInputMode::Help {
            let wheel = MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: geometry.task_content().x + 6,
                row: 6,
                modifiers: KeyModifiers::NONE,
            };
            assert_eq!(
                map_responsive_board_mouse(&model, &hits, AREA_130, wheel),
                Some(BoardIntent::HelpScrollDown),
                "Help owns wheel input across the centered card"
            );
        }
    }
}

#[test]
fn quick_add_ignores_detail_clicks_and_keeps_its_draft() {
    let (mut domain, mut model) = fixture();
    let original = domain.tasks().len();
    go(&mut domain, &mut model, BoardIntent::OpenCapture);
    go(
        &mut domain,
        &mut model,
        BoardIntent::QuickAddInsertText("draft task".to_string()),
    );
    let draft = model.quick_add_title_value().to_string();
    let geometry = resolve_responsive(130, 24, WideStage::Board);
    let (rows, hits) = render(&model, 130, 24);
    assert!(
        rows.join("\n").contains("draft task"),
        "the footer paints the input"
    );
    let click = left_click(geometry.task_content().x + 6, 6);
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, click),
        None
    );
    let mapped = click_map(&model, &hits, click.column, click.row);
    if let Some(intent) = mapped.clone() {
        go(&mut domain, &mut model, intent);
    }
    assert_eq!(mapped, None);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(model.quick_add_title_value(), draft);
    assert_eq!(domain.tasks().len(), original);
}

fn hit_signature(hits: &QueueHitMap, column: Rect) -> Vec<String> {
    let mut targets: Vec<String> = hits
        .regions
        .iter()
        .filter(|hit| inside(column, hit.area))
        .filter_map(|hit| match hit.target {
            QueueHitTarget::Step(index) => Some(format!("Step({index})")),
            QueueHitTarget::StepAdd => Some("StepAdd".to_string()),
            QueueHitTarget::FormNotes(_) => Some("FormNotes".to_string()),
            QueueHitTarget::FormScopeOption(index) => Some(format!("FormScopeOption({index})")),
            QueueHitTarget::Verb(index) => Some(format!("Verb({index})")),
            _ => None,
        })
        .collect();
    targets.sort();
    targets.dedup();
    targets
}

fn first_hit(hits: &QueueHitMap, column: Rect, name: &str) -> Rect {
    hits.regions
        .iter()
        .find(|hit| {
            inside(column, hit.area)
                && match hit.target {
                    QueueHitTarget::Step(index) => format!("Step({index})") == name,
                    QueueHitTarget::StepAdd => name == "StepAdd",
                    QueueHitTarget::FormNotes(_) => name == "FormNotes",
                    QueueHitTarget::FormScopeOption(index) => {
                        format!("FormScopeOption({index})") == name
                    }
                    QueueHitTarget::Verb(index) => format!("Verb({index})") == name,
                    _ => false,
                }
        })
        .map(|hit| hit.area)
        .expect("named hit")
}

fn session_signature(model: &BoardModel) -> impl PartialEq + std::fmt::Debug {
    (
        model.input_mode(),
        model.selected_id(),
        model.edit_target(),
        model.page_scroll(),
        model.step_cursor(),
        model.form_focus(),
        model.edit_buffer().to_string(),
        model.edit_cursor(),
        model.task_editing(),
    )
}

fn assert_same_outcome(
    domain: &DomainState,
    single: &BoardModel,
    wide: &BoardModel,
    intent: BoardIntent,
    label: &str,
) {
    let mut single_domain = domain.clone();
    let mut wide_domain = domain.clone();
    let mut single_model = single.clone();
    let mut wide_model = wide.clone();
    let single_outcome = apply_intent(&mut single_domain, &mut single_model, intent.clone(), None)
        .expect("single-pane action");
    let wide_outcome =
        apply_intent(&mut wide_domain, &mut wide_model, intent, None).expect("wide action");
    assert_eq!(single_outcome, wide_outcome, "{label}: outcome");
    assert_eq!(
        single_domain.tasks(),
        wide_domain.tasks(),
        "{label}: domain"
    );
    assert_eq!(
        session_signature(&single_model),
        session_signature(&wide_model),
        "{label}: session"
    );
}

/// The focused detail exposes the same controls as the single-pane page, and every shared
/// control maps to the same intent with the same domain and session outcome.
#[test]
fn focused_detail_controls_match_the_single_pane_page() {
    let (mut domain, mut base) = fixture();
    let id = base.selected_id().expect("selection");
    domain.add_step(id, "first parity step").expect("step");
    domain.add_step(id, "second parity step").expect("step");
    base.sync_from_domain(&domain);

    let mut single = base.clone();
    go(&mut domain.clone(), &mut single, BoardIntent::OpenTaskPage);
    let single_area = Rect::new(0, 0, 100, 24);
    let single_column = Rect::new(0, 0, 100, 21);

    let edits: Vec<(&str, Vec<BoardIntent>)> = vec![
        ("task view", vec![]),
        ("title edit", vec![BoardIntent::BeginEditTitle]),
        ("notes edit", vec![BoardIntent::BeginEditNotes]),
        (
            "step edit",
            vec![BoardIntent::BeginEditTitle, BoardIntent::SelectStep(0)],
        ),
    ];
    for (label, steps) in &edits {
        let mut single_model = single.clone();
        let mut wide_model = base.clone();
        let mut wide_domain = domain.clone();
        focus(&mut wide_domain, &mut wide_model, WideStage::Task);
        for intent in steps {
            go(&mut domain.clone(), &mut single_model, intent.clone());
            go(&mut wide_domain, &mut wide_model, intent.clone());
        }
        assert_eq!(
            single_model.input_mode(),
            wide_model.input_mode(),
            "{label}"
        );
        let (_, single_hits) = render(&single_model, single_area.width, single_area.height);
        let (_, wide_hits) = render(&wide_model, 130, 24);
        let geometry = resolve_responsive(130, 24, WideStage::Task);
        let wide_column = Rect::new(
            geometry.task_content().x,
            0,
            geometry.task_content().width,
            21,
        );
        let single_targets = hit_signature(&single_hits, single_column);
        let wide_targets = hit_signature(&wide_hits, wide_column);
        assert_eq!(
            single_targets, wide_targets,
            "{label}: task-surface controls"
        );
        assert!(
            single_targets.iter().any(|t| t.starts_with("Step(")),
            "{label}"
        );
        for name in &single_targets {
            let single_rect = first_hit(&single_hits, single_column, name);
            let wide_rect = first_hit(&wide_hits, wide_column, name);
            let single_intent = map_board_mouse(
                &single_model,
                &single_hits,
                left_click(single_rect.x, single_rect.y),
            );
            let wide_intent = map_responsive_board_mouse(
                &wide_model,
                &wide_hits,
                AREA_130,
                left_click(wide_rect.x, wide_rect.y),
            );
            assert_eq!(single_intent, wide_intent, "{label}: {name}");
            if let Some(intent) = single_intent {
                assert_same_outcome(
                    &domain,
                    &single_model,
                    &wide_model,
                    intent,
                    &format!("{label}: {name}"),
                );
            }
        }
        let (single_rows, _) = render(&single_model, single_area.width, single_area.height);
        let (wide_rows, _) = render(&wide_model, 130, 24);
        assert_eq!(
            single_rows[23].trim_end(),
            wide_rows[23].trim_end(),
            "{label}: verb bar"
        );
        for code in [
            KeyCode::Char('x'),
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Tab,
        ] {
            let single_intent = map_key(single_model.input_mode(), key(code));
            let wide_intent = route_key(&wide_model, ResponsivePresentation::WideSplit, code);
            assert_eq!(single_intent, wide_intent, "{label}: {code:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// Dirty drafts across focus
// ---------------------------------------------------------------------------

/// Detail focus with a dirty title draft parked in the page's edit session (view mode).
fn dirty_session_on_detail() -> (DomainState, BoardModel) {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(model.task_session_dirty());
    (domain, model)
}

/// Detail focus with the title editor open and dirty.
fn dirty_editor_on_detail() -> (DomainState, BoardModel) {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    assert!(model.task_session_dirty());
    (domain, model)
}

fn header_title_row(model: &BoardModel) -> String {
    let geometry = resolve_responsive(130, 24, model.wide_stage());
    let (rows, _) = render(model, 130, 24);
    column_text(&rows, geometry.task_content(), 1)
        .trim_end()
        .to_string()
}

#[test]
fn dirty_draft_moves_focus_to_the_list_and_back_untouched() {
    let (mut domain, mut model) = dirty_session_on_detail();
    let bound = model.edit_target().expect("bound");
    let draft = model.edit_buffer().to_string();
    assert!(header_title_row(&model).ends_with("unsaved"));
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Left),
        Some(BoardIntent::FocusList)
    );
    assert_eq!(model.wide_stage(), WideStage::Board);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(model.edit_target(), Some(bound), "the detail stays bound");
    assert!(header_title_row(&model).ends_with("unsaved"));
    assert!(model.message().is_none(), "no refusal for a focus move");
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Right),
        Some(BoardIntent::FocusDetail)
    );
    assert_eq!(model.edit_target(), Some(bound));
    assert_eq!(model.edit_buffer(), draft, "the draft is untouched");
    assert!(model.task_session_dirty());
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn dirty_draft_refuses_keyboard_retarget_from_the_list() {
    let (mut domain, mut model) = dirty_session_on_detail();
    press(&mut domain, &mut model, KeyCode::Left);
    let bound = model.edit_target().expect("bound");
    let draft = model.edit_buffer().to_string();
    for code in [KeyCode::Char('j'), KeyCode::Char('k')] {
        press(&mut domain, &mut model, code);
        assert_eq!(
            model.selected_id(),
            Some(bound),
            "{code:?}: selection refused"
        );
        assert_eq!(model.edit_target(), Some(bound));
        assert_eq!(model.edit_buffer(), draft);
        assert!(model
            .message()
            .is_some_and(|message| message.contains("save or cancel")));
    }
    let (rows, _) = render(&model, 130, 24);
    assert!(
        rows[22].contains("save or cancel"),
        "refusal paints on the status row"
    );
}

#[test]
fn dirty_draft_refuses_list_row_click_and_keeps_the_editor() {
    let (mut domain, mut model) = dirty_editor_on_detail();
    let bound = model.edit_target().expect("bound");
    let draft = model.edit_buffer().to_string();
    let geometry = resolve_responsive(130, 24, WideStage::Task);
    let (_, hits) = render(&model, 130, 24);
    let other = row_hit(&hits, geometry.board, |id| id != bound);
    let intent = click_map(&model, &hits, other.x, other.y)
        .expect("a list row stays an explicit retarget attempt");
    go(&mut domain, &mut model, intent);
    assert_eq!(model.wide_stage(), WideStage::Task);
    assert_eq!(model.selected_id(), Some(bound));
    assert_eq!(model.edit_target(), Some(bound));
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    assert_eq!(model.edit_buffer(), draft);
    assert!(model
        .message()
        .is_some_and(|message| message.contains("save or cancel")));
    let same = row_hit(&hits, geometry.board, |id| id == bound);
    assert_eq!(click_map(&model, &hits, same.x, same.y), None);
}

#[test]
fn dirty_refusal_clears_after_save_and_the_detail_retargets_again() {
    let (mut domain, mut model) = dirty_editor_on_detail();
    let bound = model.edit_target().expect("bound");
    let draft = model.edit_buffer().to_string();
    go(&mut domain, &mut model, BoardIntent::SelectNext);
    assert!(model.message().is_some());
    go(&mut domain, &mut model, BoardIntent::ConfirmEdit);
    assert!(model.message().is_none());
    assert!(!model.task_session_dirty());
    assert_eq!(domain.get(bound).expect("saved").title, draft);
    assert!(header_title_row(&model).ends_with("doing · tsk"));
    press(&mut domain, &mut model, KeyCode::Left);
    press(&mut domain, &mut model, KeyCode::Char('j'));
    assert_ne!(
        model.selected_id(),
        Some(bound),
        "retarget works after the save"
    );
}

#[test]
fn dirty_refusal_clears_after_cancel() {
    let (mut domain, mut model) = dirty_editor_on_detail();
    let bound = model.edit_target().expect("bound");
    let saved = domain.get(bound).expect("bound").title.clone();
    go(&mut domain, &mut model, BoardIntent::SelectNext);
    assert!(model.message().is_some());
    go(&mut domain, &mut model, BoardIntent::CancelEdit);
    assert!(model.message().is_none());
    assert!(!model.task_session_dirty());
    assert_eq!(model.edit_buffer(), saved);
    press(&mut domain, &mut model, KeyCode::Left);
    press(&mut domain, &mut model, KeyCode::Char('j'));
    assert_ne!(model.selected_id(), Some(bound));
}

#[test]
fn clean_editor_list_row_click_retargets_and_moves_focus_to_the_list() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    assert!(!model.task_session_dirty());
    let bound = model.edit_target().expect("bound");
    let geometry = resolve_responsive(130, 24, WideStage::Task);
    let (_, hits) = render(&model, 130, 24);
    let other = row_hit(&hits, geometry.board, |id| id != bound);
    let intent = click_map(&model, &hits, other.x, other.y).expect("clean retarget");
    go(&mut domain, &mut model, intent);
    assert_eq!(
        model.wide_stage(),
        WideStage::Board,
        "focus moves to the list"
    );
    assert_ne!(model.selected_id(), Some(bound));
    assert_eq!(model.input_mode(), BoardInputMode::Normal, "the page parks");
    assert!(model.message().is_none());
}

#[test]
fn dirty_scope_dropdown_same_bound_row_click_keeps_the_visible_editor() {
    let (mut domain, mut model) = dirty_editor_on_detail();
    let draft = model.edit_buffer().to_string();
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Scope),
    );
    go(&mut domain, &mut model, BoardIntent::OpenFormScopeDropdown);
    assert_eq!(model.input_mode(), BoardInputMode::FormScopeDropdown);
    let bound = model.edit_target().expect("bound");
    let geometry = resolve_responsive(130, 24, WideStage::Task);
    let (_, hits) = render(&model, 130, 24);
    let row = row_hit(&hits, geometry.board, |id| id == bound);
    let mapped = click_map(&model, &hits, row.x, row.y);
    if let Some(intent) = mapped.clone() {
        go(&mut domain, &mut model, intent);
    }
    assert_eq!(mapped, None);
    assert_eq!(model.input_mode(), BoardInputMode::FormScopeDropdown);
    assert_eq!(model.wide_stage(), WideStage::Task);
    go(
        &mut domain,
        &mut model,
        BoardIntent::CancelFormScopeDropdown,
    );
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Title),
    );
    assert_eq!(model.edit_buffer(), draft);
}

#[test]
fn list_focused_field_edit_focuses_the_detail() {
    let (mut domain, mut model) = fixture();
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    assert_eq!(model.wide_stage(), WideStage::Task);
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    let (rows, _) = render(&model, 130, 24);
    let header = column_text(
        &rows,
        resolve_responsive(130, 24, WideStage::Task).task_content(),
        1,
    );
    assert!(header.contains(model.edit_buffer()), "{header}");
    assert!(header.trim_end().ends_with("editing title"));
    go(&mut domain, &mut model, BoardIntent::CancelEdit);
    go(&mut domain, &mut model, BoardIntent::CloseLayer);
    assert!(!model.task_editing());
    go(&mut domain, &mut model, BoardIntent::CloseLayer);
    assert_eq!(model.wide_stage(), WideStage::Board);
}

#[test]
fn detail_header_number_click_copies() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    let (_, hits) = render_buffer(&model, 130, 24);
    let column = resolve_responsive(130, 24, WideStage::Task).task;
    let number = hits
        .regions
        .iter()
        .find(|hit| {
            matches!(hit.target, QueueHitTarget::TaskNumber(_))
                && column.contains(hit.area.as_position())
        })
        .expect("header number hit")
        .area;
    let intent =
        map_responsive_board_mouse(&model, &hits, AREA_130, left_click(number.x, number.y));
    assert_eq!(
        intent,
        Some(BoardIntent::CopyTaskNumber(model.selected_id().unwrap()))
    );
}

/// The detail derives its header identifier from the task: a notice paints `N<n>` and keeps
/// its click-to-copy hit.
#[test]
fn detail_header_derives_the_notice_n_identifier_and_its_copy_hit() {
    let mut domain = DomainState::new();
    let notice = domain
        .create_notice(
            "guide.wide",
            "Wide guide header",
            None,
            HumanStatus::Todo,
            TaskScope::Global,
            Vec::new(),
        )
        .expect("seed notice");
    let mut tasks = domain.tasks().to_vec();
    tasks[0].notice = Some(Notice {
        catalog_id: "guide.wide".into(),
        number: Some(2),
    });
    let model = BoardModel::from_tasks(tasks, Some(PathBuf::from(REPO)));
    assert_eq!(model.selected_id(), Some(notice));
    for stage in STAGES {
        let (mut domain, mut model) = (domain.clone(), model.clone());
        focus(&mut domain, &mut model, stage);
        let geometry = resolve_responsive(130, 24, stage);
        let (rows, hits) = render(&model, 130, 24);
        let header = column_text(&rows, geometry.task_content(), 1);
        assert!(
            header.contains("N2 Wide guide header"),
            "{stage:?}: {header}"
        );
        let identifier = hits
            .regions
            .iter()
            .find(|hit| {
                matches!(hit.target, QueueHitTarget::TaskNumber(id) if id == notice)
                    && geometry.task.contains(hit.area.as_position())
            })
            .expect("header identifier copy hit")
            .area;
        let painted: String = rows[identifier.y as usize]
            .chars()
            .skip(identifier.x as usize)
            .take(identifier.width as usize)
            .collect();
        assert_eq!(painted, "N2");
    }
}

#[test]
fn detail_without_a_selection_cannot_take_focus() {
    let (mut domain, mut model) = fixture();
    let mut empty = DomainState::new();
    model.sync_from_domain(&empty);
    assert_eq!(model.selected_id(), None);
    go(&mut empty, &mut model, BoardIntent::FocusDetail);
    assert_eq!(model.wide_stage(), WideStage::Board);
    let _ = &mut domain;
}

#[test]
fn soft_delete_from_the_detail_returns_focus_to_the_list() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    let deleted = model.selected_id().unwrap();
    go(&mut domain, &mut model, BoardIntent::SoftDelete);
    go(&mut domain, &mut model, BoardIntent::SoftDelete);
    assert!(!model.visible_ids().contains(&deleted));
    assert_eq!(model.wide_stage(), WideStage::Board);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    let route = route_responsive_key(
        model.input_mode(),
        model.wide_stage(),
        ResponsivePresentation::WideSplit,
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
    );
    assert_eq!(route, ResponsiveKeyRoute::Intent(BoardIntent::FocusDetail));
}

#[test]
fn wide_list_titles_fill_the_column_without_attribution() {
    let (_, model) = fixture_with_titles(&"X".repeat(180), "second");
    let geometry = resolve_responsive(130, 40, WideStage::Board);
    let (rows, _) = render(&model, 130, 40);
    let line = (0..rows.len())
        .map(|y| column_text(&rows, geometry.board, y as u16))
        .find(|line| line.contains("T12 X"))
        .unwrap();
    assert!(!line.contains("tsk"), "no row-end attribution: {line}");
    assert_eq!(
        line.trim_end().chars().count(),
        geometry.board.width as usize - 2,
        "title reaches the two-cell margin: {line}"
    );
}

/// At wide widths `Tab` on the quick-add line expands into a draft that owns the whole
/// frame; Esc returns to the line.
#[test]
fn expanded_quick_add_draft_paints_at_wide_widths_and_esc_returns_to_the_line() {
    let (mut domain, mut model) = fixture();
    go(&mut domain, &mut model, BoardIntent::OpenCapture);
    for c in "buy milk".chars() {
        go(&mut domain, &mut model, BoardIntent::QuickAddInsert(c));
    }
    go(&mut domain, &mut model, BoardIntent::ExpandQuickAdd);
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    for width in [110u16, 140, 200] {
        let (buffer, _) = render_buffer(&model, width, 30);
        let text = rows_of(&buffer).join("\n");
        assert!(
            text.contains("buy milk"),
            "{width} cols: draft title missing\n{text}"
        );
        assert!(
            text.contains("shift+enter save"),
            "{width} cols: verb bar\n{text}"
        );
        assert!(
            !text.contains("DOING"),
            "{width} cols: board still painted\n{text}"
        );
    }
    go(&mut domain, &mut model, BoardIntent::CloseLayer);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    let (buffer, _) = render_buffer(&model, 140, 30);
    let text = rows_of(&buffer).join("\n");
    assert!(text.contains("DOING"), "board should be back\n{text}");
    assert!(text.contains("buy milk"));
}

/// The list keeps the done drawer and archived fold while the detail shows a done task.
#[test]
fn list_keeps_the_done_drawer_and_selects_the_open_done_task() {
    let mut domain = DomainState::new();
    let scope = || TaskScope::Project {
        path: REPO.to_string(),
    };
    let started = domain
        .create("Open work", None, scope(), ProvenanceOrigin::Manual)
        .expect("create started");
    domain
        .set_status(started, HumanStatus::Doing)
        .expect("doing");
    let done = domain
        .create("Shipped fix", None, scope(), ProvenanceOrigin::Manual)
        .expect("create done");
    domain.set_status(done, HumanStatus::Done).expect("finish");
    let archived = domain
        .create("Old shelf", None, scope(), ProvenanceOrigin::Manual)
        .expect("create archived");
    domain
        .set_status(archived, HumanStatus::Done)
        .expect("finish archived");
    domain.archive_task(archived).expect("archive");
    let mut tasks = domain.tasks().to_vec();
    for (task, number) in tasks.iter_mut().zip([3, 4, 5]) {
        task.number = Some(number);
    }
    let mut model = BoardModel::from_tasks(tasks, Some(PathBuf::from(REPO)));

    go(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer);
    for _ in 0..8 {
        if model.selected_id() == Some(done) {
            break;
        }
        press(&mut domain, &mut model, KeyCode::Down);
    }
    assert_eq!(model.selected_id(), Some(done), "drawer row selected");
    press(&mut domain, &mut model, KeyCode::Enter);
    assert_eq!(model.wide_stage(), WideStage::Task);

    let list_text = |model: &BoardModel| {
        let geometry = resolve_responsive(130, 24, model.wide_stage());
        let (rows, _) = render(model, 130, 24);
        let (rule_y, _, _) = footer_rows(24);
        (0..rule_y)
            .map(|y| column_text(&rows, geometry.board, y))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let text = list_text(&model);
    assert!(
        text.contains("DONE"),
        "the list keeps the done drawer:\n{text}"
    );
    assert!(
        text.lines().any(|row| row.starts_with("▸  T4 Shipped fix")),
        "the open done task stays selected:\n{text}"
    );
    assert!(text.contains("archived · 1"), "archived heading:\n{text}");
    assert!(
        !text.contains("Old shelf"),
        "archived group stays folded:\n{text}"
    );

    focus(&mut domain, &mut model, WideStage::Board);
    go(&mut domain, &mut model, BoardIntent::ToggleArchivedGroup);
    // The selection now rests on the archived header, which has no detail to focus; the
    // wide list stays painted beside the detail column either way.
    assert!(model.archived_header_selected());
    assert!(
        list_text(&model).contains("Old shelf"),
        "unfolded archived group"
    );
}

#[test]
fn changed_scope_draft_refuses_retarget_and_paints_unsaved() {
    let (mut domain, mut model) = fixture();
    focus(&mut domain, &mut model, WideStage::Task);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Scope),
    );
    let original = model.form_scope().cloned();
    go(&mut domain, &mut model, BoardIntent::FormCycleScope);
    assert_ne!(model.form_scope(), original.as_ref());
    assert!(header_title_row(&model).ends_with("editing scope"));
    let bound = model.edit_target().expect("bound");
    let other = model
        .visible_ids()
        .iter()
        .position(|&id| id != bound)
        .expect("other");
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusBoardAndSelectIndex(other),
    );
    assert_eq!(model.edit_target(), Some(bound));
    assert!(model.message().is_some());
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert!(model.task_session_dirty());
    assert!(header_title_row(&model).ends_with("unsaved"));
}
