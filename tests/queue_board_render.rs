//! Queue Board Renderer frame goldens.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::{Frame, Terminal};
use tsk_tui::domain::{
    DomainState, HumanStatus, Notice, ProvenanceOrigin, Task, TaskEvent, TaskEventKind, TaskScope,
};
use tsk_tui::ui::input::map_key;
use tsk_tui::ui::queue::{self, BoardLens, BoardSort, NavTab, QueueView};
use tsk_tui::ui::render::{
    assert_buffer_mono, assert_no_color_sgr, draw_queue_frame, BottomInputSlot, NavPaint,
    PaletteCommandRow, QueueFrameModel, QueueOverlay, VerbEntry,
};
use tsk_tui::ui::tier::{self, Tier, TierGeometry};
use tsk_tui::ui::{
    apply_intent, board_verb_items, draw_board, BoardInputMode, BoardIntent, BoardModel,
};
use uuid::Uuid;

/// Fixed "now" for deterministic age labels in the fixture.
const NOW_SECS: u64 = 1_700_000_000;

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(NOW_SECS)
}

fn at_secs_ago(secs_ago: u64) -> SystemTime {
    now()
        .checked_sub(Duration::from_secs(secs_ago))
        .expect("fixture timestamp")
}

fn task(id: u128, title: &str, status: HumanStatus, scope: TaskScope, secs_ago: u64) -> Task {
    let at = at_secs_ago(secs_ago);
    Task {
        id: Uuid::from_u128(id),
        number: None,
        parent_task_id: None,
        notice: None,
        revision: Uuid::from_u128(id),
        merge_base_revision: None,
        title: title.to_string(),
        notes: None,
        status,
        scope,
        provenance: ProvenanceOrigin::Manual,
        history: vec![TaskEvent {
            kind: TaskEventKind::Created,
            at,
        }],
        steps: Vec::new(),
        soft_deleted: false,
        archived: false,
        created_at: at,
        updated_at: at,
    }
}

/// The same tasks with every timestamp shifted so the fixed fixture "now" lands on the wall
/// clock: relative ages stay as authored for painters that read `SystemTime::now()`.
fn tasks_rebased_on_wall_clock(tasks: &[Task]) -> Vec<Task> {
    let shift = SystemTime::now()
        .duration_since(now())
        .expect("wall clock after fixture now");
    tasks
        .iter()
        .cloned()
        .map(|mut task| {
            task.created_at += shift;
            task.updated_at += shift;
            for event in &mut task.history {
                event.at += shift;
            }
            task
        })
        .collect()
}

fn project(path: &str) -> TaskScope {
    TaskScope::Project {
        path: path.to_string(),
    }
}

/// Deterministic deck-only fixture: motion, multi-project deck, blocked/review glyphs, done.
fn inbox_golden_tasks() -> Vec<Task> {
    let mut tasks = vec![
        task(
            50,
            "Ready planning pass",
            HumanStatus::Todo,
            TaskScope::Global,
            20 * 60,
        ),
        task(
            51,
            "Ready release check",
            HumanStatus::Todo,
            TaskScope::Global,
            10 * 60,
        ),
        task(
            52,
            "Inbox capture one",
            HumanStatus::Todo,
            TaskScope::Global,
            5 * 60,
        ),
        task(
            53,
            "Inbox capture two",
            HumanStatus::Todo,
            TaskScope::Global,
            2 * 60,
        ),
    ];
    for (index, task) in tasks.iter_mut().enumerate() {
        task.number = Some((index + 50) as u64);
    }
    tasks
}

fn inbox_golden_verbs() -> &'static [VerbEntry<'static>] {
    static CACHE: OnceLock<Vec<VerbEntry<'static>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let model = BoardModel::from_tasks(inbox_golden_tasks(), Some(PathBuf::from("/repos/tsk")));
        board_verb_items(&model)
    })
}

fn fixture_tasks() -> Vec<Task> {
    let mut tasks = vec![
        task(
            1,
            "Smoke-test worktree dispatch",
            HumanStatus::Doing,
            project("/repos/tsk"),
            3 * 60,
        ),
        task(
            2,
            "Edit target binding pin",
            HumanStatus::Doing,
            project("/repos/herdr"),
            12 * 60,
        ),
        task(
            10,
            "Prototype the queue-style board UI",
            HumanStatus::Todo,
            project("/repos/tsk"),
            3600,
        ),
        task(
            11,
            "Cut rust-toolchain pin into CI docs",
            HumanStatus::Todo,
            project("/repos/tsk"),
            86400,
        ),
        task(
            20,
            "Wire dispatch cleanup receipts",
            HumanStatus::Doing,
            project("/repos/herdr"),
            41 * 60,
        ),
        task(
            21,
            "Docs refresh pass after F9 ships",
            HumanStatus::Doing,
            project("/repos/herdr"),
            3 * 3600,
        ),
        task(
            30,
            "Global backlog note",
            HumanStatus::Todo,
            TaskScope::Global,
            2 * 86400,
        ),
        task(
            40,
            "Ship the queue board milestone",
            HumanStatus::Done,
            project("/repos/tsk"),
            5 * 3600,
        ),
        task(
            41,
            "Retire classic board chrome",
            HumanStatus::Done,
            TaskScope::Global,
            6 * 3600,
        ),
    ];
    for (index, task) in tasks.iter_mut().enumerate() {
        task.number = Some((index + 1) as u64);
    }
    tasks
}

/// Board model matching this file's fixture: same tasks, `seed_selection` lands on the same
/// task 1 ("Smoke-test worktree dispatch", Doing) [`fixture_model`] hardcodes as its
/// selection.
fn base_board_model() -> BoardModel {
    let mut model = BoardModel::from_tasks(fixture_tasks(), Some(PathBuf::from("/repos/tsk")));
    // The desk's NEEDS YOU lane is global now, so seeding no longer lands on task 1
    // (the started /repos/tsk row): the render fixtures pin task 1's legend explicitly.
    let pinned_index = model
        .visible_ids()
        .iter()
        .position(|&id| id == Uuid::from_u128(1))
        .expect("fixture task 1 must be visible for the pinned selection");
    apply_intent(
        &mut DomainState::new(),
        &mut model,
        BoardIntent::SelectIndex(pinned_index),
        None,
    )
    .expect("pin task 1");
    assert_eq!(model.selected_id(), Some(Uuid::from_u128(1)));
    model
}

/// Imp-5 (round 2 regression): the goldens' verb array used to *coincide in wording* with
/// `draw_board`'s, but nothing bound them -- a later edit to either side could
/// silently re-open the divergence. Compute this fixture's verbs through the same
/// [`board_verb_items`] function `draw_board` calls, on a `BoardModel` built to match this
/// file's render fixture, so the two can never drift apart again.
fn fixture_verbs() -> &'static [VerbEntry<'static>] {
    static CACHE: OnceLock<Vec<VerbEntry<'static>>> = OnceLock::new();
    CACHE.get_or_init(|| board_verb_items(&base_board_model()))
}

/// Imp-A (round 3 finding): a **Todo**-selected model, so `board_verb_items` yields six
/// entries (`s start` plus the base five) instead of the Doing selection's five. Used
/// only where the trim-the-sixth path needs a budget-5 bar to actually drop an entry --
/// `fixture_verbs()`'s Doing selection is exactly 5 against a budget of 5, so `take(5)`
/// never drops anything and a test built on it can't fail.
fn todo_verbs() -> Vec<VerbEntry<'static>> {
    let mut model = base_board_model();
    let mut domain = DomainState::new();
    model.set_selected_project(Some(PathBuf::from("/repos/tsk")));
    let target = Uuid::from_u128(10);
    let visible = model.visible_ids();
    for _ in 0..visible.len() {
        if model.selected_id() == Some(target) {
            break;
        }
        apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None).expect("select next");
    }
    assert_eq!(
        model.selected_id(),
        Some(target),
        "fixture task 10 (Todo) must be reachable via SelectNext -- keep in sync with fixture_tasks"
    );
    board_verb_items(&model)
}

/// G-8 (gate MEDIUM): the palette golden used to hand-write its command rows and included
/// `dispatch agent…`, a label no code path can produce -- the excludes dispatch from the
/// palette outright. This is the same defect class fixed for the verb bar
/// (`fixture_verbs`/`accordion_verbs` above): compute the rows through the real
/// `BoardModel::visible_commands` catalog, driven by the actual `OpenCommandPalette` /
/// `CommandQueryInsert` / `CommandNext` intents `draw_board` itself drives, so the golden
/// can never show a command the product cannot produce.
///
/// Matches this file's palette scene: the fixture's selected task (1, Doing) with a "stat"
/// query narrowed to the status tail, second row (`set status: blocked`) highlighted.
fn palette_commands() -> Vec<PaletteCommandRow<'static>> {
    let mut model = base_board_model();
    let mut domain = DomainState::new();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open the palette");
    for character in "stat".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::CommandQueryInsert(character),
            None,
        )
        .expect("type palette query character");
    }
    assert_eq!(
        model.command_query(),
        "stat",
        "fixture query drifted from the scene's \"stat\" query"
    );
    let labels: Vec<&str> = model
        .visible_commands()
        .iter()
        .map(|command| command.label)
        .collect();
    assert_eq!(
        labels,
        vec!["set status: todo", "set status: doing", "set status: done",],
        "the \"stat\" query against the real M1 catalog must narrow to exactly the three \
         status commands (no dispatch, no other tail entry) -- if the product catalog \
         changed, this fixture must follow it, not be hand-patched"
    );
    for _ in 0..1 {
        apply_intent(&mut domain, &mut model, BoardIntent::CommandNext, None)
            .expect("command next");
    }
    assert_eq!(
        model.command_selected(),
        Some(1),
        "fixture must highlight \"set status: doing\""
    );
    let selected = model.command_selected();
    model
        .visible_commands()
        .iter()
        .enumerate()
        .map(|(i, command)| PaletteCommandRow {
            label: command.label,
            selected: Some(i) == selected,
        })
        .collect()
}

fn fixture_view(tasks: &[Task], drawer_open: bool) -> QueueView {
    queue::query_board(
        tasks,
        &std::collections::BTreeSet::new(),
        BoardLens::Desk,
        drawer_open,
        BoardSort::Number,
    )
}

fn fixture_view_projects(tasks: &[Task], drawer_open: bool) -> QueueView {
    queue::query_board(
        tasks,
        &std::collections::BTreeSet::new(),
        BoardLens::Project(Path::new("/repos/tsk")),
        drawer_open,
        BoardSort::Number,
    )
}

fn fixture_model<'a>(tasks: &'a [Task], view: &'a QueueView) -> QueueFrameModel<'a> {
    fixture_model_on_scope(tasks, view, false)
}

/// The fixture on the `/repos/tsk` project board (thread filter chip on the right).
fn fixture_model_on_project<'a>(tasks: &'a [Task], view: &'a QueueView) -> QueueFrameModel<'a> {
    fixture_model_on_scope(tasks, view, true)
}

fn fixture_model_on_scope<'a>(
    tasks: &'a [Task],
    view: &'a QueueView,
    project: bool,
) -> QueueFrameModel<'a> {
    QueueFrameModel {
        tasks,
        view,
        selection_id: Some(Uuid::from_u128(1)),
        marked_ids: std::collections::BTreeSet::new(),
        nav: NavPaint {
            questions_tab: " 2 questions ".to_string(),
            active: NavTab::Board,
            check: false,
        },
        surface: if project {
            tsk_tui::ui::render::BoardSurface::Project
        } else {
            tsk_tui::ui::render::BoardSurface::Desk
        },
        project_groups: false,
        search_query: "",
        search_pinned: false,
        summary: None,
        context: " desk".to_string(),
        has_update_notice: false,
        status_message: None,
        status_undo_offset: None,
        status_undo_width: None,
        verb_items: fixture_verbs(),
        now: now(),
        overlay: QueueOverlay::None,
        list_scroll: 0,
        follow_list: true,
        archived_collapsed: true,
        archived_header_selected: false,
        inbox_collapsed: false,
        inbox_header_selected: false,
        rows_dim: false,
        waiting: None,
        task_marks: Default::default(),
    }
}

fn paint(width: u16, height: u16, model: &QueueFrameModel<'_>) -> (Vec<String>, TierGeometry) {
    let geo = tier::resolve(width, height);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame: &mut Frame| {
            let _ = draw_queue_frame(frame, model, &geo, Rect::new(0, 0, width, height));
        })
        .expect("draw");
    let backend = terminal.backend();
    let buffer = backend.buffer();
    assert_buffer_mono(buffer);
    let rows: Vec<String> = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect();
    // Full-frame plain text must stay free of color SGR.
    let joined = rows.join("\n");
    assert_no_color_sgr(&joined);
    (rows, geo)
}

fn row_display_width(row: &str) -> usize {
    // Buffer symbols are already one cell each in the TestBackend grid; length == width budget.
    row.chars().count()
}

/// Paint a whole [`BoardModel`] through `draw_board` -- the payload-builder path
/// (`build_task_page_overlay`) rather than a hand-composed `QueueFrameModel` -- so a
/// test can assert what the page payload consumed from a real stored task.
fn board_rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    assert_buffer_mono(buffer);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn inactive_selector_tab_is_dimmed() {
    let model = base_board_model();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    let (tab_y, row) = (0..24)
        .map(|y| {
            let row = (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>();
            (y, row)
        })
        .find(|(_, row)| row.contains("1 board") && row.contains("questions"))
        .expect("selector row with its tab digits");
    let label = "questions";
    let x = row[..row.find(label).expect("questions tab")]
        .chars()
        .count() as u16;
    assert!(
        buffer[(x, tab_y)]
            .style()
            .add_modifier
            .contains(Modifier::DIM),
        "inactive {label} tab must be dimmed"
    );
}

fn trimmed(row: &str) -> String {
    row.trim_end().to_string()
}

/// List content without the overflow scrollbar's track/thumb cell on the right edge.
fn list_body(row: &str) -> String {
    let mut body = trimmed(row);
    if body.ends_with('█') || body.ends_with('▌') || body.ends_with('│') {
        body.pop();
        body = body.trim_end().to_string();
    }
    body
}

fn assert_exact_header_spacing(
    rows: &[String],
    geo: TierGeometry,
    header: &str,
    first_content: &str,
    dimensions: &str,
) {
    let marker = format!("{header} ─");
    let header_row = rows
        .iter()
        .position(|line| {
            let body = list_body(line);
            if header == "inbox" {
                body.contains("inbox ·")
            } else {
                body.contains(&marker)
            }
        })
        .unwrap_or_else(|| panic!("{dimensions}: missing {header:?}:\n{:#?}", rows));
    let viewport_top = geo.viewport_top as usize;
    let viewport_bottom = viewport_top + geo.viewport_height as usize;
    assert!(
        header_row > viewport_top && header_row + 2 < viewport_bottom,
        "{dimensions}: {header:?} and its surrounding rows must be visible after selection scrolling:\n{:#?}",
        rows
    );
    if header == "inbox" {
        assert!(
            list_body(&rows[header_row - 1]).is_empty()
                && list_body(&rows[header_row + 1]).contains(first_content),
            "{dimensions}: inbox keeps its blank row above but its first task follows directly:\n{:#?}",
            rows
        );
        return;
    }
    assert!(
        list_body(&rows[header_row - 1]).is_empty() && list_body(&rows[header_row + 1]).is_empty(),
        "{dimensions}: {header:?} needs one blank row immediately above and below:\n{:#?}",
        rows
    );
    assert!(
        !list_body(&rows[header_row - 2]).is_empty()
            && list_body(&rows[header_row + 2]).contains(first_content),
        "{dimensions}: {header:?} must have exactly one surrounding blank row before {first_content:?}:\n{:#?}",
        rows
    );
}

#[test]
fn standard_78x24_fixture_has_selector_list_rule_status_verb_and_no_other_chrome_rows() {
    let tasks = fixture_tasks();
    let view = fixture_view(&tasks, true);
    let model = fixture_model(&tasks, &view);
    let (rows, geo) = paint(78, 24, &model);

    assert_eq!(geo.tier, Tier::Standard);
    assert_eq!(geo.selector_row, Some(1));
    assert_eq!(geo.rule_row, Some(21));
    assert_eq!(geo.status_row, Some(22));
    assert_eq!(geo.verb_row, Some(23));
    assert_eq!(geo.viewport_top, 2);
    assert_eq!(geo.viewport_height, 19);
    assert_eq!(geo.meta_column_width, 36);
    assert_eq!(geo.title_width, 42);

    assert!(trimmed(&rows[0]).is_empty(), "row above tabs stays blank");

    let selector = trimmed(&rows[1]);
    assert!(
        !selector.contains("queue"),
        "selector must not paint a view switcher: {selector:?}"
    );
    assert!(
        selector.contains("1 board") && selector.contains("2 questions"),
        "selector must show both numbered tabs: {selector:?}"
    );
    assert!(
        !selector.contains('▾') && !selector.contains("sort"),
        "selector paints no scope or sort control: {selector:?}"
    );

    let list: String = rows
        [geo.viewport_top as usize..(geo.viewport_top + geo.viewport_height) as usize]
        .iter()
        .map(|r| trimmed(r))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        list.contains("DOING"),
        "list must include IN MOTION header:\n{list}"
    );
    assert!(
        list.lines()
            .any(|line| line.trim_start().starts_with("TODO · desk ─")),
        "desk tab must include the desk ON DECK header:\n{list}"
    );
    assert!(
        !list.contains('●') && !list.contains('○'),
        "list rows carry no status glyphs:\n{list}"
    );
    assert!(
        list.contains("Smoke-test worktree dispatch"),
        "task titles must appear:\n{list}"
    );
    // Standard trailing meta keeps scope/thread identity without relative ages.
    assert!(
        !list.contains("└─"),
        "collapsed board must hide attribution:\n{list}"
    );
    assert!(
        !list.contains("ago"),
        "task rows must not paint relative ages:\n{list}"
    );
    // NEEDS YOU is the desk's global attention lane: the fixture's blocked/review
    // rows from /repos/herdr paint here with project attribution.
    assert!(
        list.contains("DOING") && list.contains("Wire dispatch cleanup receipts"),
        "desk NEEDS YOU must gather blocked/review from every live scope:\n{list}"
    );
    assert!(
        list.contains("Docs refresh pass after F9 ships"),
        "review rows join blocked rows in the global lane:\n{list}"
    );
    assert!(
        !list.contains("claude") && !list.contains("grok") && !list.contains("agent"),
        "M1 must not paint agent fields:\n{list}"
    );

    let rule = &rows[21];
    assert!(
        rule.chars().filter(|&c| c == '─').count() >= 40,
        "dim rule row expected, got {rule:?}"
    );

    let status = trimmed(&rows[22]);
    assert_eq!(
        status.trim(),
        "desk",
        "idle status line must name the active lens"
    );
    assert!(
        !status.contains("need") && !status.contains("claude") && !status.contains("grok"),
        "status must omit need-you and agent ticker: {status:?}"
    );

    let verbs = trimmed(&rows[23]);
    // Imp-3 (round 2): `space` is context-dependent -- this fixture's selection is a Doing
    // task, and `PrimaryVerb` is a silent no-op there, so a correct legend omits the entry
    // rather than advertise a no-op. `enter`/`?` are always present regardless of selection.
    assert!(
        verbs.contains("enter")
            && verbs.contains('?')
            && verbs.contains("ctrl+d done")
            && verbs.contains("ctrl+o todo")
            && !verbs.contains("+ add"),
        "in-motion bar keeps its status actions and help without a misleading add seat: {verbs:?}"
    );

    // Chrome is exactly selector + rule + status + verb. Viewport rows are list content only
    // (headers/tasks/spacers), never a second status/verb/rule band.
    for (idx, row) in rows.iter().enumerate() {
        let t = trimmed(row);
        if idx == 1 || idx == 21 || idx == 22 || idx == 23 {
            continue;
        }
        assert!(
            !t.contains("all projects ▾") && !t.starts_with(" space "),
            "non-chrome row {idx} must not repeat selector/verb chrome: {t:?}"
        );
        if idx != 21 && idx != 0 {
            let rule_like = t.chars().filter(|&c| c == '─').count() >= 60
                && !t.contains("DOING")
                && !t.contains("DONE")
                && !t.chars().any(|c| c.is_ascii_digit());
            assert!(
                !rule_like,
                "extra full-width rule chrome at row {idx}: {t:?}"
            );
        }
    }

    for row in &rows {
        assert_eq!(row_display_width(row), 78);
    }
}

#[test]
fn desk_and_project_focus_paint_needs_you_above_in_motion() {
    let mut domain = DomainState::new();
    let started = domain
        .create(
            "started work",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("doing");
    domain
        .set_status(started, HumanStatus::Doing)
        .expect("doing");
    let blocked = domain
        .create(
            "blocked desk",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("doing");
    domain
        .set_status(blocked, HumanStatus::Doing)
        .expect("block");
    let review = domain
        .create(
            "review desk",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("doing");
    domain
        .set_status(review, HumanStatus::Doing)
        .expect("doing");
    let desk = BoardModel::from_domain(&domain, None);
    let desk_rows = board_rows(&desk, 78, 24).join("\n");
    assert_eq!(desk_rows.matches("DOING").count(), 1, "{desk_rows}");
    assert!(!desk_rows.contains("NEEDS YOU"), "{desk_rows}");
    assert!(desk_rows.contains("started work"), "{desk_rows}");
    assert!(desk_rows.contains("blocked desk") && desk_rows.contains("review desk"));

    let mut domain = DomainState::new();
    let project = TaskScope::Project {
        path: "/repos/tsk".into(),
    };
    let started = domain
        .create(
            "started project",
            None,
            project.clone(),
            ProvenanceOrigin::Manual,
        )
        .expect("doing");
    domain
        .set_status(started, HumanStatus::Doing)
        .expect("doing");
    let blocked = domain
        .create(
            "blocked project",
            None,
            project.clone(),
            ProvenanceOrigin::Manual,
        )
        .expect("doing");
    domain
        .set_status(blocked, HumanStatus::Doing)
        .expect("block");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/tsk")));
    model.set_selected_project(Some(PathBuf::from("/repos/tsk")));
    let project_rows = board_rows(&model, 78, 24).join("\n");
    assert!(
        project_rows.contains("DOING"),
        "project focus must paint NEEDS YOU:\n{project_rows}"
    );
    assert!(project_rows.contains("blocked project"));
}

fn assert_visible_chrome(rows: &[String], geo: TierGeometry, dimensions: &str) {
    let status = trimmed(&rows[geo.status_row.expect("status row") as usize]);
    assert!(
        !status.is_empty() && !status.contains("done"),
        "{dimensions}: lens context must remain reachable: {status:?}"
    );
    let verbs = trimmed(&rows[geo.verb_row.expect("verb row") as usize]);
    assert!(
        !verbs.is_empty(),
        "{dimensions}: verb controls must remain reachable"
    );
    for row in rows {
        assert_eq!(
            row_display_width(row),
            geo.row_width as usize,
            "{dimensions}: row exceeds the frame width"
        );
    }
}

/// every section heading has one, and only one, blank row above and below it in both
/// tiers. Selecting its first task must scroll that header and task into the list viewport.
#[test]
fn every_section_header_has_symmetric_spacing_and_scrolls_with_its_selected_task() {
    let tasks = fixture_tasks();
    let desk_view = fixture_view(&tasks, true);
    let projects_view = fixture_view_projects(&tasks, true);
    let desk_cases = [
        ("DOING", Uuid::from_u128(1), "Smoke-test worktree"),
        ("TODO · desk", Uuid::from_u128(30), "Global backlog note"),
        ("DONE", Uuid::from_u128(40), "Ship the queue board"),
    ];
    let project_cases = [
        ("DOING", Uuid::from_u128(1), "Smoke-test worktree"),
        (
            "TODO",
            Uuid::from_u128(10),
            "Prototype the queue-style board UI",
        ),
    ];

    for &(width, height) in &[(78u16, 24u16), (77u16, 24u16), (40u16, 10u16)] {
        let dimensions = format!("{width}x{height}");
        for &(header, selected_id, selected_title) in &desk_cases {
            let mut model = fixture_model(&tasks, &desk_view);
            model.selection_id = Some(selected_id);
            let (rows, geo) = paint(width, height, &model);
            assert_exact_header_spacing(&rows, geo, header, selected_title, &dimensions);

            let top = geo.viewport_top as usize;
            let bottom = top + geo.viewport_height as usize;
            let viewport = rows[top..bottom]
                .iter()
                .map(|row| trimmed(row))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                viewport.contains(selected_title),
                "{dimensions}: selected {selected_title:?} must remain visible through normal list scrolling:\n{viewport}"
            );
            assert_visible_chrome(&rows, geo, &dimensions);
        }
        for &(header, selected_id, selected_title) in &project_cases {
            if width == 40 {
                // At 40 columns the project board's thread filter wraps onto its own
                // selector row (pinned by the full-thread selector test), and the long
                // project titles wrap too: a 10-row frame keeps no room for both blank
                // rows around a header, which is the compact floor's own trade-off.
                continue;
            }
            let mut model = fixture_model_on_project(&tasks, &projects_view);
            model.selection_id = Some(selected_id);
            let (rows, geo) = paint(width, height, &model);
            assert_exact_header_spacing(&rows, geo, header, selected_title, &dimensions);

            let top = geo.viewport_top as usize;
            let bottom = top + geo.viewport_height as usize;
            let viewport = rows[top..bottom]
                .iter()
                .map(|row| trimmed(row))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                viewport.contains(selected_title),
                "{dimensions}: selected {selected_title:?} must remain visible through normal list scrolling:\n{viewport}"
            );
            assert_visible_chrome(&rows, geo, &dimensions);
        }
    }
}

#[test]
fn empty_board_hint_advertises_the_live_quick_add_key() {
    let tasks = fixture_tasks();
    let view = queue::query_lens(&tasks, BoardLens::Project(Path::new("/no-tasks")), false);
    let model = fixture_model(&tasks, &view);
    let (rows, _) = paint(80, 24, &model);
    let frame = rows.join("\n");

    assert!(frame.contains("+ add"), "empty-board hint: {frame}");
    assert!(!frame.contains("a capture"), "empty-board hint: {frame}");
}

#[test]
fn quick_add_refusal_message_uses_the_reserved_blank_row_without_color_or_overflow() {
    let tasks = fixture_tasks();
    let view = fixture_view(&tasks, true);

    for &(width, height) in &[(80, 24), (40, 10)] {
        let mut model = fixture_model(&tasks, &view);
        model.overlay = QueueOverlay::QuickAdd {
            input: BottomInputSlot {
                text: String::new(),
                cursor_col: 0,
                placeholder: "title…",
                refusal: None,
                message: Some("Title required"),
                above_rows: Vec::new(),
                cursor_row_offset: 0,
            },
            destination: "desk".to_string(),
            recovery: false,
        };
        let (rows, geo) = paint(width, height, &model);
        // Quick-add reserves two rows by shifting its input up one from ordinary status
        // chrome, leaving the former status row blank below it and this row above it.
        let input_row = (geo.status_row.expect("quick-add input row") - 1) as usize;
        let message_row = input_row.checked_sub(1).expect("reserved blank row");

        assert!(
            trimmed(&rows[message_row]).contains("Title required"),
            "{width}x{height}: refusal must be visible above quick-add input: {rows:#?}"
        );
        assert!(
            trimmed(&rows[input_row]).starts_with('▎'),
            "{width}x{height}: refusal must not replace the input cursor row: {rows:#?}"
        );
        for row in &rows {
            assert_eq!(row_display_width(row), width as usize, "{width}x{height}");
        }
    }
}

/// An empty hint and capture form are first list content after a destination heading. The
/// B4 regression: overlays must pad every painted cell to their width so base frame content
/// (status line, section rules+counts, task meta) cannot bleed through.
#[test]
fn overlay_rows_are_padded_exact_no_base_bleed() {
    let tasks = fixture_tasks();
    let view = fixture_view(&tasks, true);
    // Distinctive status so any bleed is obvious (e.g. `:re` + status tail must not happen).
    let status = " 3 in motion · 42 done";

    // --- Palette query paints on status row; must be exact padded query, no status tail ---
    {
        let mut model = fixture_model(&tasks, &view);
        model.status_message = Some(status);
        model.overlay = QueueOverlay::Palette {
            query: "re",
            commands: &[
                PaletteCommandRow {
                    label: "reopen",
                    selected: true,
                },
                PaletteCommandRow {
                    label: "delete",
                    selected: false,
                },
            ],
        };
        let (rows, geo) = paint(80, 24, &model);
        let qrow = geo.status_row.expect("status row at 80x24");
        let row = &rows[qrow as usize];
        assert_eq!(row.chars().count(), 80, "row must fill width");
        // Painted query is ":re" (with leading space from paint), bold but symbols identical.
        assert!(
            row.starts_with(" :re"),
            "palette query row must start with query: {row:?}"
        );
        // Everything after the visible query content must be spaces; no "n motion" etc.
        let after = &row[4..]; // " :re".len() == 4
        assert!(
            after.chars().all(|c| c == ' '),
            "palette query row must pad to width with spaces, got tail: {after:?} in {row:?}"
        );
        assert!(
            !row.contains("motion") && !row.contains("done"),
            "status tail must not bleed into palette query: {row:?}"
        );
    }

    // --- Help card rows must be full-width padded, no meta bleed into card area ---
    {
        let mut model = fixture_model(&tasks, &view);
        model.status_message = Some(status);
        let help_lines: Vec<String> = vec![
            "the queue - keys".to_string(),
            String::new(),
            "j/k move | ↑/↓ move".to_string(),
            "enter detail | a capture".to_string(),
            ": palette | ? help".to_string(),
            "u undo | z drawer".to_string(),
            String::new(),
            "any key to close".to_string(),
        ];
        model.overlay = QueueOverlay::Help {
            query: "",
            lines: &help_lines,
            scroll: 0,
        };
        let (rows, _geo) = paint(80, 24, &model);
        // Help centers; ensure painted rows are width-padded and carry no status/meta fragments.
        // The body width is min(width-2,62), padded on left; trailing must fill to row_width.
        let body_rows: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.contains("queue")
                    || r.contains("move")
                    || r.contains("detail")
                    || r.contains("palette")
                    || r.contains("drawer")
                    || r.contains("close")
            })
            .collect();
        assert!(!body_rows.is_empty(), "help must paint body rows");
        for r in &body_rows {
            assert_eq!(r.chars().count(), 80, "help row must be full width: {r:?}");
            // After centering pad, body ends and we must have trailing spaces to 80.
            // We only care no bleed of status/meta.
            assert!(
                !r.contains("motion") && !r.contains("done") && !r.contains("3 in"),
                "base status/meta must not bleed into help: {r:?}"
            );
        }
    }

    // --- Scope dropdown is a centered modal card; its rows must be padded full width ---
    {
        let mut model = fixture_model(&tasks, &view);
        model.status_message = Some(status);
        let scope_opts: Vec<String> = vec!["desk".to_string(), "tsk".to_string()];
        model.overlay = QueueOverlay::ScopeDropdown {
            options: &scope_opts,
            selected: 0,
            title: None,
            query: None,
            wrap: None,
        };
        let (rows, _geo) = paint(80, 24, &model);
        let desk_row = rows
            .iter()
            .position(|row| row.contains("▸ desk"))
            .expect("scope dropdown must paint the selected desk option");
        assert_eq!(
            rows[desk_row].chars().count(),
            80,
            "scope dropdown option row must fill width: {:?}",
            rows[desk_row]
        );
        // The `tsk` option must be painted as its own row, not merely satisfied by task
        // titles or meta elsewhere on the frame.
        assert!(
            rows.get(desk_row + 1)
                .is_some_and(|option_row| option_row.contains("tsk")),
            "scope dropdown must paint each option on its own row"
        );
        assert!(
            !rows[desk_row].contains("motion") && !rows[desk_row].contains("done"),
            "base status/meta must not bleed into scope dropdown: {:?}",
            rows[desk_row]
        );
    }
}

#[test]
fn compact_77x24_and_48x19_and_40x10_paint_glyph_title_only_rows_and_leq_5_verb_entries() {
    let tasks = fixture_tasks();
    let view = fixture_view(&tasks, false);
    // Imp-A: a Todo selection gives `board_verb_items` six entries (`s start` plus the
    // base five), so the budget-5 trim below is genuinely exercised -- the Doing selection
    // `fixture_model` otherwise uses yields exactly five, which `take(5)` never trims.
    let verbs = todo_verbs();
    let model = QueueFrameModel {
        verb_items: &verbs,
        ..fixture_model(&tasks, &view)
    };

    for &(w, h) in &[(77, 24), (48, 19), (40, 10)] {
        let (rows, geo) = paint(w, h, &model);
        assert_eq!(geo.tier, Tier::Compact, "{w}x{h}");
        assert_eq!(geo.meta_column_width, 0, "{w}x{h}");
        assert!(
            geo.verb_bar_entry_budget <= 5,
            "{w}x{h} verb budget {}",
            geo.verb_bar_entry_budget
        );

        let body = rows
            .iter()
            .map(|r| trimmed(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("Prototype the queue-style board UI")
                || body.contains("Smoke-test")
                || body.contains("queue"),
            "{w}x{h} must paint titles or chrome:\n{body}"
        );

        // Glyph+title only: known age tokens from the fixture must not trail as meta.
        // (Titles in the fixture contain none of these standalone age tokens.)
        for age in ["3m", "12m", "41m", "1h", "1d", "2d", "3h", "5h", "6h"] {
            // Compact may still show counts like "2" on headers; ages are multi-char with unit.
            if let Some(verb_row) = geo.verb_row {
                let list_and_status: String = rows
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i as u16 != verb_row && Some(*i as u16) != geo.status_row)
                    .map(|(_, r)| trimmed(r))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    !list_and_status.split_whitespace().any(|tok| tok == age),
                    "{w}x{h} compact must drop trailing age meta token {age}:\n{list_and_status}"
                );
            }
        }

        if let Some(verb_row) = geo.verb_row {
            let painted = trimmed(&rows[verb_row as usize]);
            // Count key tokens from the fixture set that actually appear.
            let shown = todo_verbs()
                .iter()
                .filter(|v| {
                    if v.key == "+" {
                        painted.contains("+ capture")
                    } else {
                        painted.contains(v.key)
                    }
                })
                .count();
            assert!(
                shown <= 5,
                "{w}x{h} verb bar shows {shown} entries (>5): {painted:?}"
            );
            assert!(
                shown <= geo.verb_bar_entry_budget as usize,
                "{w}x{h} showed {shown} verbs over budget {}",
                geo.verb_bar_entry_budget
            );
            assert!(
                !painted.contains("+ capture"),
                "{w}x{h} compact keeps its existing verbs instead of displacing one for capture: {painted:?}"
            );
        }

        for row in &rows {
            assert_eq!(row_display_width(row), w as usize, "{w}x{h} row overflow");
        }
    }
}

/// The task page paints a full-height takeover in BOTH tiers: header (glyph + title +
/// status word), notes body, and meta footer. The selector row stays hidden, the base list
/// never bleeds through, and every row is width-bounded at the 40x10 floor.
#[test]
fn board_row_leads_title_with_uppercase_t_identifier() {
    let mut tasks = fixture_tasks();
    tasks[0].number = Some(12);
    let view = fixture_view(&tasks, false);
    let model = fixture_model(&tasks, &view);
    let body = paint(80, 24, &model).0.join("\n");

    assert!(
        body.contains("T12 Smoke-test worktree dispatch"),
        "identifier must lead the title:\n{body}"
    );
    assert!(
        !body.contains("12 · tsk · 3m"),
        "trailing meta must not repeat the identifier:\n{body}"
    );
}

#[test]
fn notice_row_leads_title_with_uppercase_n_identifier() {
    let mut tasks = fixture_tasks();
    tasks[0].notice = Some(Notice {
        catalog_id: "welcome".into(),
        number: Some(3),
    });
    let view = fixture_view(&tasks, false);
    let model = fixture_model(&tasks, &view);
    let body = paint(80, 24, &model).0.join("\n");

    assert!(
        body.contains("N3 Smoke-test worktree dispatch"),
        "notice identifier must lead the title:\n{body}"
    );
    assert!(
        !body.contains("T3 "),
        "a notice never paints a T number:\n{body}"
    );
}

/// The page header derives its identifier through the real payload builder, not the
/// literal fixture above: a persisted notice must paint `N<n>` and register the
/// click-to-copy hit region, or the guide flow the page header teaches loses both.
#[test]
fn notice_task_page_header_derives_the_n_identifier_and_its_copy_hit() {
    let mut domain = DomainState::new();
    let id = domain
        .create_notice(
            "guide.page",
            "Guide page render",
            None,
            HumanStatus::Doing,
            TaskScope::Global,
            Vec::new(),
        )
        .expect("seed notice");
    // Persistence assigns the notice's `N` number exactly as the board sees it.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "tsk-render-notice-page-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let store = tsk_tui::store::TaskStore::new(&dir);
    store.save(&domain).expect("persist notice");
    let mut domain = store.load().expect("reload numbered notice");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);

    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open task page");
    let rows = board_rows(&model, 80, 24);
    let body = rows.join("\n");
    assert!(
        body.contains("N1 Guide page render"),
        "the page header must derive the notice identifier:\n{body}"
    );
    assert!(
        !body.contains("T1 "),
        "a notice never paints a T number:\n{body}"
    );

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
    let mut hit_map = tsk_tui::ui::render::QueueHitMap::default();
    terminal
        .draw(|frame| hit_map = draw_board(frame, &model))
        .expect("draw board");
    let hit = hit_map
        .regions
        .iter()
        .find(|hit| hit.target == tsk_tui::ui::render::QueueHitTarget::TaskNumber(id))
        .expect("the header identifier registers a copy hit");
    let painted: String = rows[hit.area.y as usize]
        .chars()
        .skip(hit.area.x as usize)
        .take(hit.area.width as usize)
        .collect();
    assert_eq!(painted, "N1", "the hit region must cover the identifier");
}

#[test]
fn standard_rows_paint_the_title_only_without_project_or_age() {
    let mut tasks = fixture_tasks();
    tasks[0].number = Some(7);
    tasks[0].scope = project("/src/customer-portal-api");
    tasks[0].updated_at = at_secs_ago(12 * 60);
    let view = fixture_view(&tasks, false);
    let model = fixture_model(&tasks, &view);
    let rows = paint(80, 24, &model).0;
    let row = rows
        .iter()
        .find(|row| row.contains("Smoke-test worktree dispatch"))
        .expect("started fixture row");
    assert!(row.contains('7'), "number must remain visible:\n{row}");
    assert!(
        !rows.iter().any(|row| row.contains("customer-portal-api")),
        "rows must not paint a project label:\n{row}"
    );
    assert!(
        !row.contains("12m"),
        "task rows must not paint relative ages:\n{row}"
    );
}

#[test]
fn task_page_header_shows_identifier_not_footer() {
    let tasks = fixture_tasks();
    let view = fixture_view(&tasks, false);
    let mut model = fixture_model(&tasks, &view);
    model.overlay = QueueOverlay::TaskPage {
        header_rows: vec!["○ numbered page".to_string()],
        header_identifier: Some("T12".to_string()),
        header_identifier_task: Some(Uuid::from_u128(1)),
        title_cursor: None,
        status_word: "todo",
        notes_rows: Vec::new(),
        notes_lines: Vec::new(),
        notes_cursor: None,
        more_lines: 0,
        relation_rows: Vec::new(),
        step_views: Vec::new(),
        stored_step_count: 0,
        step_cursor: None,
        step_add_selected: false,
        step_scroll: 0,
        step_marked: None,
        inline_step_editor: None,
        bottom_input: None,
        meta: "desk · created 1m ago · updated 1m ago".to_string(),
        meta_scope_x: 0,
        meta_scope_width: 4,
        focus: None,
        scope_dropdown: None,
    };
    let body = paint(80, 24, &model).0.join("\n");

    assert!(
        body.contains("T12 numbered page"),
        "header must lead with T12:\n{body}"
    );
    assert!(
        !body.contains("12 · desk"),
        "footer must not repeat the identifier:\n{body}"
    );
}

#[test]
fn done_drawer_rows_lead_with_identifiers() {
    let mut tasks = fixture_tasks();
    tasks[7].number = Some(12);
    let view = fixture_view(&tasks, true);
    let mut model = fixture_model(&tasks, &view);
    model.selection_id = Some(tasks[7].id);
    let body = paint(80, 24, &model).0.join("\n");

    assert!(
        body.contains("T12"),
        "done row must show a leading identifier:\n{body}"
    );
    assert!(
        !body.contains("12 · tsk · 5h"),
        "done row must not retain trailing number chrome:\n{body}"
    );
}

#[test]
fn numbered_board_at_40x10_paints_in_bounds_wraps_titles_and_keeps_tasks_reachable() {
    let mut tasks = (0..4)
        .map(|index| {
            let mut task = task(
                500 + index,
                &format!("item {index} title wraps across the compact forty column boundary"),
                HumanStatus::Todo,
                TaskScope::Global,
                index as u64,
            );
            task.number = Some(1000 + index as u64);
            task
        })
        .collect::<Vec<_>>();
    let mut model = BoardModel::from_tasks(std::mem::take(&mut tasks), None);
    let ids = model.visible_ids();
    let mut domain = DomainState::new();

    for (position, id) in ids.iter().copied().enumerate() {
        // The deck's order is the board's own rule (oldest created first), so derive
        // each row's expectations from the visited task, not from loop position.
        let index = (id.as_u128() - 500) as u64;
        if position > 0 {
            apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None)
                .expect("select next numbered task");
        }
        assert_eq!(model.selected_id(), Some(id));
        let rows = board_rows(&model, 40, 10);
        let body = rows.join("\n");
        assert!(
            body.contains(&format!("100{index}")),
            "number must paint at 40x10:\n{body}"
        );
        assert!(
            body.contains(&format!("item {index}")) && body.contains("boundary"),
            "title must wrap rather than truncate:\n{body}"
        );
        assert!(
            rows.iter().all(|row| row_display_width(row) == 40),
            "row exceeded 40 columns"
        );
    }
}

#[test]
fn task_page_renders_header_notes_and_meta_as_a_full_takeover_in_both_tiers() {
    let tasks = fixture_tasks();
    let view = fixture_view(&tasks, false);
    let mut model = fixture_model(&tasks, &view);
    model.overlay = QueueOverlay::TaskPage {
        header_rows: vec!["\u{25cb} Rename this task".to_string()],
        header_identifier: None,
        header_identifier_task: None,
        title_cursor: None,
        status_word: "todo",
        notes_rows: vec![
            "first draft note".to_string(),
            "second draft note".to_string(),
        ],
        notes_lines: Vec::new(),
        notes_cursor: None,
        more_lines: 0,
        relation_rows: Vec::new(),
        step_views: Vec::new(),
        stored_step_count: 0,
        step_cursor: None,
        step_add_selected: false,
        step_scroll: 0,
        step_marked: None,
        inline_step_editor: None,
        bottom_input: None,
        meta: "tsk \u{b7} created 1h ago \u{b7} updated 1h ago".to_string(),
        meta_scope_x: 0,
        meta_scope_width: 11,
        focus: None,
        scope_dropdown: None,
    };

    for &(width, height) in &[(78u16, 24u16), (77u16, 24u16), (40u16, 10u16)] {
        let (rows, geo) = paint(width, height, &model);
        let body = rows
            .iter()
            .map(|row| trimmed(row))
            .collect::<Vec<_>>()
            .join("\n");
        // The meta footer clips at the 40x10 floor, so "updated" is only required at the
        // 78-column standard width where the full line fits.
        let mut expected = vec!["Rename this task", "todo", "first draft note", "created"];
        if width >= 78 {
            expected.push("updated");
        }
        for expected in expected {
            assert!(
                body.contains(expected),
                "{width}x{height} task page omitted {expected:?}:\n{body}"
            );
        }
        // The selector row is hidden and the base list never bleeds through the page.
        assert!(
            !body.contains("all projects"),
            "{width}x{height} task page must hide the selector row:\n{body}"
        );
        assert!(
            !body.contains("DOING"),
            "{width}x{height} task page must erase the base list in both tiers:\n{body}"
        );
        assert!(
            rows.iter()
                .all(|row| row_display_width(row) == width as usize),
            "{width}x{height} task page exceeded the frame width"
        );
        assert_visible_chrome(&rows, geo, &format!("{width}x{height}"));
    }
}

/// T-2 (AC-5/AC-7): the task page of a task with steps steps paints a steps
/// section between the notes block and the meta footer. The fixture drives the real
/// payload-builder path (`BoardModel` + `draw_board`) from stored steps (add + toggle),
/// so the assertions cross the storage -> page-payload -> paint boundary: the label's
/// done/total counts and the per-step glyphs must come from the extracted step views.
#[test]
fn task_page_paints_steps_section_between_notes_and_footer() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Page task with steps",
            Some("the notes body".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let first = domain.add_step(id, "first step").expect("step 1");
    let second = domain.add_step(id, "second step").expect("step 2");
    domain.add_step(id, "third step").expect("step 3");
    domain.toggle_step(id, first).expect("toggle step 1");
    domain.toggle_step(id, second).expect("toggle step 2");

    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open task page");

    let rows = board_rows(&model, 78, 24);
    let shown: Vec<String> = rows.iter().map(|row| trimmed(row)).collect();
    let find = |needle: &str, shown: &[String]| {
        shown
            .iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} missing from page:\n{rows:#?}"))
    };
    let notes_row = find("the notes body", &shown);
    let parent_row = find("parent  none · P to set", &shown);
    let label_row = find("steps 2/3", &shown);
    let meta_row = find("created", &shown);
    assert_eq!(
        (parent_row, label_row),
        (notes_row + 2, notes_row + 4),
        "the parent line sits between the notes and the steps label, a blank row each side:\n{}",
        shown.join("\n")
    );
    assert!(shown[notes_row + 1].is_empty() && shown[notes_row + 3].is_empty());
    assert!(
        label_row < meta_row,
        "steps section must sit before the meta footer:\n{}",
        shown.join("\n")
    );
    // Steps render one per line below the label, in storage order: done `✓`, open `▪`.
    let first_step = find("✓ first step", &shown);
    let second_step = find("✓ second step", &shown);
    let third_step = find("▪ third step", &shown);
    assert!(
        label_row < first_step && first_step < second_step && second_step < third_step,
        "steps must paint one per line below the label in storage order:\n{}",
        shown.join("\n")
    );

    // The compact tier stays operable: the notes, a blank row, and the parent line remain
    // visible at the head, while the steps section can be reached by scrolling. Every
    // scroll position must preserve the fixed chrome and the frame width.
    let compact = board_rows(&model, 40, 10);
    let compact_shown: Vec<String> = compact.iter().map(|row| trimmed(row)).collect();
    let compact_note = compact_shown
        .iter()
        .position(|row| row.contains("the notes body"))
        .expect("compact page omitted the notes");
    assert!(
        list_body(&compact[compact_note + 1]).is_empty()
            && compact_shown[compact_note + 2].contains("parent  none · P to set"),
        "compact page must keep the blank row and the parent line before steps:\n{}",
        compact_shown.join("\n")
    );

    let mut saw_label = false;
    let mut saw_first = false;
    let mut saw_second = false;
    let mut saw_third = false;
    for scroll in 0..=5 {
        let rows = board_rows(&model, 40, 10);
        let geo = tier::resolve(40, 10);
        let body = rows
            .iter()
            .map(|row| trimmed(row))
            .collect::<Vec<_>>()
            .join("\n");
        saw_label |= body.contains("steps 2/3");
        saw_first |= body.contains("✓ first step");
        saw_second |= body.contains("✓ second step");
        saw_third |= body.contains("▪ third step");
        let rule_row = geo.rule_row.expect("compact rule row");
        let status_row = geo.status_row.expect("compact status row");
        let verb_row = geo.verb_row.expect("compact verb row");
        assert!(
            rows[rule_row as usize].contains('─')
                && !trimmed(&rows[status_row as usize]).is_empty()
                && !trimmed(&rows[verb_row as usize]).is_empty(),
            "compact fixed chrome must remain intact at scroll {scroll}:\n{body}"
        );
        assert!(
            rows.iter().all(|row| row_display_width(row) == 40),
            "compact steps rows exceeded the frame width at scroll {scroll}:\n{body}"
        );
        if scroll < 5 {
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::PageWheelScrollDown,
                None,
            )
            .expect("scroll compact task page");
        }
    }
    assert!(saw_label, "compact scrolling never reached the steps label");
    assert!(saw_first, "compact scrolling never reached the first step");
    assert!(
        saw_second,
        "compact scrolling never reached the second step"
    );
    assert!(saw_third, "compact scrolling never reached the third step");
}

/// A task with no stored steps can scroll all the way to its trailing add target.
#[test]
fn task_page_without_steps_reaches_its_trailing_add_target() {
    let notes = (0..20)
        .map(|index| format!("overflow line {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    for &(width, height) in &[(78u16, 24u16), (40u16, 10u16)] {
        let mut domain = DomainState::new();
        domain
            .create(
                "Notes-only page task",
                Some(notes.clone()),
                TaskScope::Global,
                ProvenanceOrigin::Manual,
            )
            .expect("create task");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
            .expect("open task page");

        let mut body = String::new();
        for _ in 0..64 {
            let rows = board_rows(&model, width, height);
            body = rows
                .iter()
                .map(|row| trimmed(row))
                .collect::<Vec<_>>()
                .join("\n");
            if body.contains("steps 0/0") && body.contains("+ step") {
                break;
            }
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::PageWheelScrollDown,
                None,
            )
            .expect("scroll toward add target");
        }

        assert!(
            body.contains("steps 0/0") && body.contains("+ step"),
            "{width}x{height} empty-steps page never reached its add target:\n{body}"
        );
        // `✓`/`▪` are step glyphs (the task is ready, so the header glyph is `○`).
        assert!(
            !body.contains('✓') && !body.contains('▪'),
            "{width}x{height} empty-steps page must paint no step glyphs:\n{body}"
        );
    }
}

/// At the 40x10 compact floor, a notes edit keeps its draft row visible and preserves
/// the blank row and parent line before the steps section. The notes editor owns the viewport, so
/// below-fold steps become reachable after Esc returns to page view.
#[test]
fn task_page_notes_edit_keeps_a_visible_row_and_spacing_at_the_compact_floor() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Editing notes beside steps",
            Some("draft line under edit".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let first = domain.add_step(id, "first step").expect("step 1");
    domain.add_step(id, "second step").expect("step 2");
    domain.add_step(id, "third step").expect("step 3");
    domain.toggle_step(id, first).expect("toggle step 1");

    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open task page");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None)
        .expect("begin notes edit");

    let rows = board_rows(&model, 40, 10);
    let shown: Vec<String> = rows.iter().map(|row| trimmed(row)).collect();
    let body = shown.join("\n");
    assert!(
        body.contains("draft line under edit"),
        "a notes edit must paint at least one draft row at 40x10:\n{body}"
    );
    let note_row = shown
        .iter()
        .position(|row| row.contains("draft line under edit"))
        .expect("notes edit row missing");
    assert!(
        list_body(&rows[note_row + 1]).is_empty()
            && shown[note_row + 2].contains("parent  none · P to set"),
        "notes edit must keep the blank row and the parent line before steps:\n{body}"
    );
    assert!(
        rows.iter().all(|row| row_display_width(row) == 40),
        "compact edit page exceeded the frame width"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
        .expect("return to page view");
    let mut saw_label = false;
    let mut saw_step = false;
    for scroll in 0..=5 {
        let rows = board_rows(&model, 40, 10);
        let geo = tier::resolve(40, 10);
        let body = rows
            .iter()
            .map(|row| trimmed(row))
            .collect::<Vec<_>>()
            .join("\n");
        saw_label |= body.contains("steps 1/3");
        saw_step |= body.contains("✓ first step");
        let rule_row = geo.rule_row.expect("compact rule row");
        let status_row = geo.status_row.expect("compact status row");
        let verb_row = geo.verb_row.expect("compact verb row");
        assert!(
            rows[rule_row as usize].contains('─')
                && !trimmed(&rows[status_row as usize]).is_empty()
                && !trimmed(&rows[verb_row as usize]).is_empty(),
            "compact fixed chrome must remain intact after notes edit at scroll {scroll}:\n{body}"
        );
        if scroll < 5 {
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::PageWheelScrollDown,
                None,
            )
            .expect("scroll compact page after notes edit");
        }
    }
    assert!(saw_label, "steps label was not reachable after notes edit");
    assert!(saw_step, "steps were not reachable after notes edit");
}

#[test]
fn notes_edit_shows_raw_markdown_markers_that_view_mode_strips() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Markdown notes",
            Some("see *em* and **strong** here".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("todo");
    let view = board_rows(&model, 80, 24).join("\n");
    assert!(
        !view.contains("*em*"),
        "view must strip em markers:\n{view}"
    );
    assert!(
        view.contains("em"),
        "view must still show the em text:\n{view}"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit");
    let edit = board_rows(&model, 80, 24).join("\n");
    assert!(
        edit.contains("*em*"),
        "notes edit must show raw *em* markers:\n{edit}"
    );
    assert!(
        edit.contains("**strong**"),
        "notes edit must show raw **strong** markers:\n{edit}"
    );
}

#[test]
fn task_page_view_renders_agent_markdown_and_edit_keeps_the_source() {
    let notes = "## Plan\n\
                 1. read [the spec](https://x.dev/spec)\n\
                 - [ ] write tests\n\
                 - [x] ship\n\
                 > keep it **simple**\n\
                 ---\n\
                 | key | value |\n\
                 |-----|-------|\n\
                 | a | b |";
    let mut domain = DomainState::new();
    domain
        .create(
            "Agent notes",
            Some(notes.into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
    for width in [80u16, 140] {
        let rows = board_rows(&model, width, 30);
        let view = rows.join("\n");
        for shown in [
            "Plan",
            "1. read the spec (https://x.dev/spec)",
            "☐ write tests",
            "☑ ship",
            "│ keep it simple",
            "key │ value",
            "a │ b",
            "─────",
        ] {
            assert!(
                view.contains(shown),
                "{width}: view must show {shown:?}:\n{view}"
            );
        }
        for hidden in ["## Plan", "[the spec]", "- [ ]", "|-----|", "**simple**"] {
            assert!(
                !view.contains(hidden),
                "{width}: view must hide {hidden:?}:\n{view}"
            );
        }
        assert!(
            rows.iter()
                .all(|row| row_display_width(row) == width as usize),
            "{width}: a rendered notes row overflowed the frame"
        );
    }
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit");
    let edit = board_rows(&model, 80, 30).join("\n");
    for raw in [
        "## Plan",
        "[the spec](https://x.dev/spec)",
        "- [ ] write tests",
        "|-----|",
    ] {
        assert!(edit.contains(raw), "notes edit must show {raw:?}:\n{edit}");
    }
}

#[test]
fn task_page_view_leaves_fence_body_unparsed() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Fenced",
            Some("```\n**not bold**\n```".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("todo");
    let view = board_rows(&model, 80, 24).join("\n");
    assert!(view.contains("```"), "page must show fence ticks:\n{view}");
    assert!(
        view.contains("**not bold**"),
        "fence body must not run inline markdown:\n{view}"
    );
}

#[test]
fn task_page_fence_stays_closed_after_the_opener_scrolls_off() {
    let mut notes = String::from("```\n");
    for i in 0..40 {
        notes.push_str(&format!("pad-line-{i}\n"));
    }
    notes.push_str("**still stars**\n```\n");
    let mut domain = DomainState::new();
    domain
        .create(
            "Long fence",
            Some(notes),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("todo");
    let _ = board_rows(&model, 80, 24);
    for _ in 0..30 {
        apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None).expect("scroll");
    }
    let view = board_rows(&model, 80, 24).join("\n");
    assert!(
        !view.contains("pad-line-0"),
        "fence opener should have left the viewport:\n{view}"
    );
    assert!(
        view.contains("**still stars**"),
        "scrolled fence body must still skip inline markdown:\n{view}"
    );
}

/// T-10 (AC-27): a Notes caret uses the same shared-stream offset as its rows.
#[test]
fn notes_edit_caret_accounts_for_shared_stream_scroll() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Caret stream offset",
            Some("first\nsecond\nthird".to_string()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    for index in 0..30 {
        domain
            .add_step(id, format!("step {index}"))
            .expect("add step");
    }
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    board_rows(&model, 80, 24);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::PageWheelScrollDown,
        None,
    )
    .expect("scroll shared stream");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit notes");

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw edit page");
    let cursor = terminal.backend().cursor_position();
    assert_eq!(
        cursor.y, 5,
        "entering Notes edit restores the cursor-windowed draft to the visible stream origin"
    );
}

/// AC-27: entering Notes after a deep shared-stream read restores the cursor-windowed
/// draft and keeps the terminal caret on its visible draft row.
#[test]
fn notes_edit_after_deep_stream_scroll_keeps_draft_and_caret_aligned() {
    let mut domain = DomainState::new();
    let notes = (0..20)
        .map(|line| format!("note line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let id = domain
        .create(
            "Deep Notes",
            Some(notes),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    for index in 0..30 {
        domain
            .add_step(id, format!("step {index}"))
            .expect("add step");
    }
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("todo");
    board_rows(&model, 80, 24);
    for _ in 0..20 {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::PageWheelScrollDown,
            None,
        )
        .expect("deep wheel");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit notes");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let text = board_rows(&model, 80, 24).join("\n");
    assert!(
        text.contains("note line 9"),
        "draft window must remain visible:\n{text}"
    );
    assert_eq!(
        terminal.backend().cursor_position().y,
        19,
        "caret must remain on the visible cursor-windowed draft row"
    );
}

/// AC-27 applies to form navigation too: Shift+Tab from Scope enters Notes at the
/// cursor-window origin, rather than preserving the reading position from the shared stream.
#[test]
fn shift_tab_from_scope_resets_notes_stream_origin_and_aligns_caret() {
    let mut domain = DomainState::new();
    let notes = (0..20)
        .map(|line| format!("note line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let _id = domain
        .create(
            "Shift tab Notes",
            Some(notes),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("todo");
    board_rows(&model, 80, 24);
    for _ in 0..20 {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::PageWheelScrollDown,
            None,
        )
        .expect("deep wheel");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditScope, None).expect("edit scope");
    let shift_tab = map_key(
        BoardInputMode::EditScope,
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
    );
    assert_eq!(shift_tab, Some(BoardIntent::FormFocusPrev));
    apply_intent(
        &mut domain,
        &mut model,
        shift_tab.clone().expect("Shift+Tab intent"),
        None,
    )
    .expect("Shift+Tab selects the trailing add target from Scope");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None)
        .expect("Shift+Tab from the add target enters Notes");
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let text = board_rows(&model, 80, 24).join("\n");
    assert!(
        text.contains("note line 9"),
        "Shift+Tab Notes draft must return to the visible cursor window:\n{text}"
    );
    assert_eq!(
        terminal.backend().cursor_position().y,
        19,
        "Shift+Tab Notes caret must land on its painted draft row"
    );
}

/// A long single-line note wraps onto continuation rows while the Notes editor is
/// open: both ends of the draft stay on the page instead of the cursor's row
/// scrolling sideways under a fixed-width window.
#[test]
fn notes_edit_wraps_a_long_line_instead_of_scrolling_horizontally() {
    let mut domain = DomainState::new();
    let note = format!("HEAD{}TAIL", "w".repeat(200));
    domain
        .create(
            "Long line",
            Some(note),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    board_rows(&model, 80, 24);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit notes");

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw edit page");
    let rows = board_rows(&model, 80, 24);
    assert!(
        rows.iter().any(|row| row.contains("HEAD")),
        "the note's head must stay visible while editing:\n{}",
        rows.join("\n")
    );
    let tail_row = rows
        .iter()
        .position(|row| row.contains("TAIL"))
        .unwrap_or_else(|| panic!("the note's tail must wrap into view:\n{}", rows.join("\n")))
        as u16;
    assert_eq!(
        terminal.backend().cursor_position().y,
        tail_row,
        "the caret must sit on the wrapped continuation row carrying the draft's end:\n{}",
        rows.join("\n")
    );
}

/// View mode already wraps stored notes (`wrapped_draft_rows`); this pins that
/// contract so a later presenter change cannot quietly reintroduce truncation.
#[test]
fn task_page_view_wraps_long_notes_instead_of_truncating() {
    let mut domain = DomainState::new();
    let note = format!("HEAD{}TAIL", "w".repeat(200));
    domain
        .create(
            "Long line",
            Some(note),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let rows = board_rows(&model, 80, 24);
    assert!(
        rows.iter().any(|row| row.contains("HEAD")),
        "view mode lost the note's head:\n{}",
        rows.join("\n")
    );
    assert!(
        rows.iter().any(|row| row.contains("TAIL")),
        "view mode truncated the note instead of wrapping it:\n{}",
        rows.join("\n")
    );
}

/// Long notes and steps form one scrollable page body. Steps follow the notes and the
/// parent line instead of being positioned at a viewport fraction. The header and
/// meta footer remain fixed while PageScroll reveals the deferred steps and scrollbar.
#[test]
fn task_page_scrolls_notes_and_steps_as_one_content_region() {
    let notes = (0..20)
        .map(|i| format!("N{i:02} filler line"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Scrollable page",
            Some(notes),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    domain.add_step(id, "only step").expect("add step");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open task page");

    let initial = board_rows(&model, 78, 24);
    let shown: Vec<String> = initial.iter().map(|row| trimmed(row)).collect();
    assert!(
        shown[1].contains("Scrollable page"),
        "header must stay fixed"
    );
    assert!(shown[20].contains("created"), "footer must stay fixed");
    assert!(
        shown.iter().any(|row| row.contains('▌')),
        "overflow needs a scrollbar"
    );
    assert!(shown.iter().any(|row| row.contains("N16 filler line")));
    assert!(
        !shown.iter().any(|row| row.contains("steps 0/1")),
        "long notes push the steps below the viewport:\n{}",
        shown.join("\n")
    );

    // Enough wheel steps to reach the end of the stream (the scroll clamps there).
    for _ in 0..10 {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::PageWheelScrollDown,
            None,
        )
        .expect("wheel scroll down");
    }
    let scrolled = board_rows(&model, 78, 24);
    let shown: Vec<String> = scrolled.iter().map(|row| trimmed(row)).collect();
    assert!(
        shown[1].contains("Scrollable page"),
        "header must stay fixed"
    );
    assert!(shown[20].contains("created"), "footer must stay fixed");
    assert!(shown.iter().any(|row| row.contains("steps 0/1")));
    let step_row = shown
        .iter()
        .position(|row| row.contains("▪ only step"))
        .expect("step visible after scrolling");
    assert!(
        shown[step_row + 1].contains("+ step"),
        "the trailing add target follows the last step:\n{}",
        shown.join("\n")
    );
    let note_row = initial
        .iter()
        .find(|row| row.contains("N00 filler line"))
        .expect("first note row");
    assert!(
        note_row
            .chars()
            .rev()
            .skip(1)
            .take(2)
            .all(|cell| cell == ' '),
        "content needs the same two-cell gutter before the right-edge scrollbar: {note_row:?}"
    );
}

/// #159: a positioned add row paints directly below its anchor step. It is not a stored step,
/// so the count still covers the last stored step and the selector stays on the anchor.
#[test]
fn positioned_add_row_paints_below_its_anchor() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Insert page",
            Some("the notes body".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    for text in ["first step", "second step", "third step"] {
        domain.add_step(id, text).expect("add step");
    }
    let third = domain.get(id).expect("task").steps[2].id;
    domain.toggle_step(id, third).expect("check third");
    let mut model = BoardModel::from_domain(&domain, None);
    for intent in [
        BoardIntent::OpenTaskPage,
        BoardIntent::SelectStep(0),
        BoardIntent::BeginAddStep,
        BoardIntent::EditInsert('x'),
    ] {
        apply_intent(&mut domain, &mut model, intent, None).expect("intent");
    }
    let shown: Vec<String> = board_rows(&model, 78, 24)
        .iter()
        .map(|row| trimmed(row))
        .collect();
    let label = shown
        .iter()
        .position(|row| row.contains("steps 1/3"))
        .unwrap_or_else(|| panic!("count must skip the draft row:\n{}", shown.join("\n")));
    let rows = &shown[label + 1..label + 5];
    assert!(
        rows[0].contains("▸ ▪ first step")
            && rows[1].contains("▪ x")
            && !rows[1].contains('▸')
            && rows[2].contains("▪ second step")
            && rows[3].contains("✓ third step"),
        "the draft sits between its anchor and the next step:\n{}",
        shown.join("\n")
    );
}

/// #159 review F-1: a move scrolls by the moved layout. A tall first step moved below a
/// one-row step must leave the selector painted, not scroll toward the old tall offset.
#[test]
fn moving_a_tall_step_keeps_its_selector_on_screen() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Tall move",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let tall = (0..40)
        .map(|word| format!("word{word:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    domain
        .add_step(id, format!("tall {tall}"))
        .expect("tall step");
    domain.add_step(id, "short").expect("short step");
    let mut model = BoardModel::from_domain(&domain, None);
    for intent in [BoardIntent::OpenTaskPage, BoardIntent::SelectStep(0)] {
        apply_intent(&mut domain, &mut model, intent, None).expect("select the tall step");
    }
    let _ = board_rows(&model, 40, 16);
    apply_intent(&mut domain, &mut model, BoardIntent::MoveStepDown, None).expect("move down");
    let shown: Vec<String> = board_rows(&model, 40, 16)
        .iter()
        .map(|row| trimmed(row))
        .collect();
    assert!(
        shown.iter().any(|row| row.contains("▸ ▪ tall")),
        "the moved step's first row and selector stay painted:\n{}",
        shown.join("\n")
    );
}

/// #159 review: `ctrl+a` below a step at the bottom of the viewport, then `Enter`, keeps the
/// reopened add row (below the step just saved) in view.
#[test]
fn positioned_add_rows_scroll_into_view() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Scroll add",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    for index in 0..20 {
        domain
            .add_step(id, format!("step {index:02}"))
            .expect("add step");
    }
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("todo");
    let _ = board_rows(&model, 40, 16);
    // Walk the cursor down until its step sits on the last painted steps row.
    apply_intent(&mut domain, &mut model, BoardIntent::SelectStep(0), None).expect("select");
    let mut shown = Vec::new();
    for _ in 0..19 {
        shown = board_rows(&model, 40, 16)
            .iter()
            .map(|row| trimmed(row))
            .collect::<Vec<_>>();
        let cursor = shown.iter().position(|row| row.starts_with("▸ ▪"));
        let next = cursor.and_then(|row| shown.get(row + 1));
        if next.is_some_and(|row| !row.contains("▪ step")) {
            break;
        }
        apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None).expect("down");
    }
    let anchor = shown
        .iter()
        .find(|row| row.starts_with("▸ ▪"))
        .cloned()
        .expect("a selected step");
    let visible_editor = |model: &BoardModel| {
        board_rows(model, 40, 16)
            .iter()
            .any(|row| row.contains("step…"))
    };
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None).expect("ctrl+a");
    assert!(
        visible_editor(&model),
        "the add row below {anchor:?} opens in view"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('x'), None).expect("type");
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("enter");
    // The app's save boundary syncs the persisted domain back, which releases the row.
    model.sync_from_domain(&domain);
    assert!(visible_editor(&model), "the reopened add row stays in view");
}

/// Steps stack from the top of their section, with step 1 directly under the section
/// label and each next step directly below the previous, never pinned to the footer.
#[test]
fn steps_stack_from_the_top_below_the_divider() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Stacking page",
            Some("the notes body".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    for text in ["first step", "second step", "third step"] {
        domain.add_step(id, text).expect("add step");
    }
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open task page");

    // 78x24: the note is on row 3, then a blank, the parent line on row 5, a blank, and the
    // label on row 7; steps 1..3 stack downward from it while the footer-side rows stay empty.
    let rows = board_rows(&model, 78, 24);
    let shown: Vec<String> = rows.iter().map(|row| trimmed(row)).collect();
    let label = shown
        .iter()
        .position(|row| row.contains("steps 0/3"))
        .unwrap_or_else(|| panic!("steps label missing:\n{}", shown.join("\n")));
    assert_eq!(
        label, 7,
        "the steps label follows the notes and the parent line"
    );
    assert!(
        shown[8].contains("▪ first step")
            && shown[9].contains("▪ second step")
            && shown[10].contains("▪ third step"),
        "steps must stack one directly under another from the top of the half:\n{}",
        shown.join("\n")
    );
    assert!(
        shown[19].is_empty(),
        "the row above the footer must stay empty — the block is not footer-anchored:\n{}",
        shown.join("\n")
    );

    // A fourth step lands directly below the third, still far from the footer.
    domain.add_step(id, "fourth step").expect("add step 4");
    model.sync_from_domain(&domain);
    let rows4 = board_rows(&model, 78, 24);
    let shown4: Vec<String> = rows4.iter().map(|row| trimmed(row)).collect();
    assert!(
        shown4[11].contains("▪ fourth step"),
        "the fourth step must land directly below the third:\n{}",
        shown4.join("\n")
    );
    assert!(
        shown4[19].is_empty(),
        "the fourth step must not be pinned to the footer:\n{}",
        shown4.join("\n")
    );
}

/// Imp-1 (round 2 regression): the compact palette windowed its command list off a raw
/// row *index* (`list_bottom`) instead of the rows actually available above chrome, so an
/// ordinary unfiltered the catalog (>= 6 commands) painted the `command` header over row 0 --
/// the selector row -- instead of stopping at `viewport_top`. Reproduce with 9 commands (the
/// review's repro count) at three compact sizes and assert the selector survives untouched
/// Imp-2 (round 2 regression): the title/notes editors reused `CAPTURE_VERBS`, which
/// advertised `tab notes/scope` (Tab is unbound in both single-field modes) and `enter save`
/// (Notes' plain Enter opens a line, not save). Each editor's verb bar must name only keys
/// a compact editor takeover must actually erase the base list underneath it. The prior
/// bug painted `Paragraph::new("")` over the viewport, which sets style but writes no symbols,
/// so the list content already painted there kept showing through every one of the three
/// an over-tall notes draft must stop at the takeover region, not paint into the rule or
/// status row below it. At 40x10 the prior bug's hardcoded 8-row request destroyed both.
#[test]
fn compact_notes_editor_never_paints_into_the_rule_or_status_row_at_40x10() {
    let tasks = fixture_tasks();
    let view = fixture_view(&tasks, false);
    let mut model = fixture_model(&tasks, &view);
    model.status_message = None;

    let rows_text: Vec<String> = (1..=8).map(|i| format!("NOTELINE{i}")).collect();
    model.overlay = QueueOverlay::EditNotes {
        rows: rows_text,
        cursor_row: 7,
        cursor_col: 0,
    };

    let (rows, geo) = paint(40, 10, &model);
    let rule_row = geo.rule_row.expect("rule row at 40x10");
    let status_row = geo.status_row.expect("status row at 40x10");

    assert!(
        !rows[rule_row as usize].contains("NOTELINE"),
        "notes draft must not overwrite the rule row: {:?}",
        rows[rule_row as usize]
    );
    assert!(
        !rows[status_row as usize].contains("NOTELINE"),
        "notes draft must not overwrite the status row: {:?}",
        rows[status_row as usize]
    );
    assert!(
        rows[rule_row as usize]
            .chars()
            .filter(|&c| c == '─')
            .count()
            > 0,
        "rule row lost its rule chrome: {:?}",
        rows[rule_row as usize]
    );
    assert_eq!(
        trimmed(&rows[status_row as usize]).trim(),
        "desk",
        "status row lost its desk context: {:?}",
        rows[status_row as usize]
    );
}

// ---------------------------------------------------------------------------
// accordion/takeover detail, and surface goldens for the reviewer.
// ---------------------------------------------------------------------------

/// G-2 (gate round 1, PR #11): with no expanded accordion open, the list once painted from
/// row 0, so on a deck longer than the viewport, moving the plain selection past the fold
/// walked it off-screen with no visual
/// feedback, and the mutating verbs (`space`/`d`/`x`) then acted on a row the user could not
/// see. Select the *last* task of a 40-task deck with nothing else open and require its own
/// title to be scrolled into view.
#[test]
fn plain_selection_on_a_task_below_the_fold_scrolls_it_into_view_in_both_tiers() {
    let tasks: Vec<Task> = (0..40u128)
        .map(|i| {
            task(
                5000 + i,
                &format!("Selection padding task {i}"),
                HumanStatus::Todo,
                project("/repos/tsk"),
                3600,
            )
        })
        .collect();
    let last_id = Uuid::from_u128(5000 + 39);
    let view = fixture_view_projects(&tasks, false);
    let mut model = fixture_model_on_project(&tasks, &view);
    model.selection_id = Some(last_id);

    for &(width, height) in &[(80u16, 24u16), (77u16, 24u16), (40u16, 10u16)] {
        let (rows, geo) = paint(width, height, &model);
        let top = geo.viewport_top as usize;
        let bottom = (geo.viewport_top + geo.viewport_height) as usize;
        let viewport: String = rows[top..bottom]
            .iter()
            .map(|r| trimmed(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            viewport.contains("Selection padding task 39"),
            "{width}x{height}: the plain-selected task's own row must be scrolled into view \
             even with no capture or accordion open:\n{viewport}"
        );
        assert_visible_chrome(&rows, geo, &format!("{width}x{height}"));
    }
}

/// On compact frames the field editors are full-screen takeovers; closing one restores the
/// base list and the prior selection.
#[test]
fn compact_editors_stay_full_screen_takeovers() {
    let tasks = fixture_tasks();
    let view = fixture_view(&tasks, false);
    let mut model = fixture_model(&tasks, &view);
    let selected = Uuid::from_u128(1);
    assert_eq!(model.selection_id, Some(selected));

    for &(w, h) in &[(48u16, 19u16), (40u16, 10u16)] {
        let geo = tier::resolve(w, h);
        assert_eq!(geo.tier, Tier::Compact, "{w}x{h}");
        let top = geo.viewport_top as usize;
        let bottom = (geo.viewport_top + geo.viewport_height) as usize;

        // A field editor (title) is a full takeover that erases the base list.
        model.overlay = QueueOverlay::EditTitle {
            draft: "x".to_string(),
            cursor_col: 1,
        };
        let (rows2, _geo2) = paint(w, h, &model);
        let viewport2: String = rows2[top..bottom]
            .iter()
            .map(|r| trimmed(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !viewport2.contains("DOING"),
            "{w}x{h}: title editor takeover must erase the base list:\n{viewport2}"
        );

        // Close it: the base list, and the prior selection, are back.
        model.overlay = QueueOverlay::None;
        let (rows3, _geo3) = paint(w, h, &model);
        let viewport3: String = rows3[top..bottom]
            .iter()
            .map(|r| trimmed(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            viewport3.contains("Smoke-test worktree dispatch") && viewport3.contains("DOING"),
            "{w}x{h}: closing must restore the base list:\n{viewport3}"
        );
        assert_eq!(
            model.selection_id,
            Some(selected),
            "{w}x{h}: closing must not have touched the selection"
        );
    }
}

/// One named scene painted for the reviewer's side-by-side against
/// `python3 design/prototype/main.py --dump` (the-relevant scenes only).
struct GoldenScene {
    name: &'static str,
    rows: Vec<String>,
    width: u16,
}

fn golden_scenes() -> Vec<GoldenScene> {
    let tasks = fixture_tasks();

    // Wide scenes stay on the real board painter: the list + detail columns and their
    // shared footer are not representable by a standalone QueueFrameModel.
    // The real painter reads the wall clock for the detail's "created/updated … ago" footer,
    // so these scenes rebase the fixture onto it; the golden then pins a fixed age.
    let mut wide_domain = DomainState::new();
    let mut wide_model = BoardModel::from_tasks(
        tasks_rebased_on_wall_clock(&tasks),
        Some(PathBuf::from("/repos/tsk")),
    );
    let board_wide_rows = board_rows(&wide_model, 110, 30);
    apply_intent(
        &mut wide_domain,
        &mut wide_model,
        BoardIntent::FocusDetail,
        None,
    )
    .expect("focus the wide detail for golden");
    let board_wide_detail_rows = board_rows(&wide_model, 110, 30);
    // The board always groups by project: desk first, then each project under its heading.
    let sorted_model = BoardModel::from_tasks(tasks.clone(), Some(PathBuf::from("/repos/tsk")));
    let board_project_order_rows = board_rows(&sorted_model, 80, 30);

    let mut search_domain = DomainState::new();
    let mut search_model = BoardModel::from_tasks(tasks.clone(), Some(PathBuf::from("/repos/tsk")));
    apply_intent(
        &mut search_domain,
        &mut search_model,
        BoardIntent::FocusSearch,
        None,
    )
    .expect("focus board search for golden");
    apply_intent(
        &mut search_domain,
        &mut search_model,
        BoardIntent::SearchQueryInsertText("worktree".into()),
        None,
    )
    .expect("type board search for golden");
    let board_search_rows = board_rows(&search_model, 80, 24);
    apply_intent(
        &mut search_domain,
        &mut search_model,
        BoardIntent::PinSearch,
        None,
    )
    .expect("pin board search for golden");
    let board_search_pinned_rows = board_rows(&search_model, 80, 24);

    let mut empty_search_model =
        BoardModel::from_tasks(tasks.clone(), Some(PathBuf::from("/repos/tsk")));
    apply_intent(
        &mut search_domain,
        &mut empty_search_model,
        BoardIntent::FocusSearch,
        None,
    )
    .expect("focus empty board search for golden");
    apply_intent(
        &mut search_domain,
        &mut empty_search_model,
        BoardIntent::SearchQueryInsertText("nothing-here".into()),
        None,
    )
    .expect("type empty board search for golden");
    let board_search_empty_rows = board_rows(&empty_search_model, 80, 24);

    let board_view = fixture_view(&tasks, false);
    let board_model = fixture_model(&tasks, &board_view);
    let (base_rows, _) = paint(80, 24, &board_model);
    let mut marked_model = base_board_model();
    let mut marked_domain = DomainState::new();
    apply_intent(
        &mut marked_domain,
        &mut marked_model,
        BoardIntent::ToggleMarkMode,
        None,
    )
    .expect("enter mark mode");
    apply_intent(
        &mut marked_domain,
        &mut marked_model,
        BoardIntent::MarkExtend(tsk_tui::ui::input::MarkDirection::Down),
        None,
    )
    .expect("mark selected row and move");
    apply_intent(
        &mut marked_domain,
        &mut marked_model,
        BoardIntent::MarkToggle,
        None,
    )
    .expect("mark destination row");
    let marked_rows = board_rows(&marked_model, 80, 24);
    let (default_split_rows, default_split_geo) = paint(78, 24, &board_model);
    assert_eq!(
        default_split_geo.tier,
        Tier::Standard,
        "the default 78x24 Herdr split must render the designed tier"
    );

    let mut palette_model = fixture_model(&tasks, &board_view);
    let palette_commands_owned = palette_commands();
    palette_model.overlay = QueueOverlay::Palette {
        query: "stat",
        commands: &palette_commands_owned,
    };
    let (palette_rows, _) = paint(80, 24, &palette_model);

    // C1: the help golden must be the help card the product actually paints, not a
    // hand-written stand-in. `help_card_lines()` is the same function `draw_board` feeds
    // into `QueueOverlay::Help`, derived from the live keymap.
    //
    // this card deliberately diverges from the prototype's help scene --
    // the prototype lays its bindings out as a two-column card with an em dash and a few
    // prototype-only keys (triage/dispatch/lens navigation) that the does not have. That is
    // intentional for the (deck-only, no lenses/dispatch keyboard surface yet), not a gap;
    // recorded here so a reviewer does not re-litigate the divergence as a bug.
    let mut help_model = fixture_model(&tasks, &board_view);
    let help_lines: Vec<String> = tsk_tui::ui::input::help_card_lines();
    help_model.overlay = QueueOverlay::Help {
        query: "",
        lines: &help_lines,
        scroll: 0,
    };
    let (help_rows, _) = paint(80, 24, &help_model);

    // `done_drawer` has no prototype counterpart -- the prototype's own
    // `done` scene pushes the DONE section below the fold and dumps a frame that never shows
    // it. This scene's check is therefore structural only (DONE painted, drawer open),
    // not a byte-for-byte comparison against a prototype dump the way the other five scenes
    // are; recorded here rather than left implicit.
    let done_view = fixture_view(&tasks, true);
    let done_model = fixture_model(&tasks, &done_view);
    let (done_rows, _) = paint(80, 24, &done_model);

    // `inbox`: desk rows with picked work followed by an expanded inbox. This keeps the
    // new nested section visible in a reviewer-sized frame, including each open row's glyph.
    let inbox_tasks = inbox_golden_tasks();
    let inbox_view = fixture_view(&inbox_tasks, false);
    let mut inbox_model = fixture_model(&inbox_tasks, &inbox_view);
    inbox_model.selection_id = Some(Uuid::from_u128(50));
    inbox_model.verb_items = inbox_golden_verbs();
    let (inbox_rows, _) = paint(80, 24, &inbox_model);

    // `done_drawer_archived`: the drawer open with an expanded archived group below the
    // DONE rows -- the dim header with its count and the dim rows (glyph + T<n> kept).
    let mut archived_tasks = fixture_tasks();
    let mut archived_one = task(
        7301,
        "Archived receipt sweep",
        HumanStatus::Todo,
        project("/repos/tsk"),
        45 * 60,
    );
    archived_one.number = Some(31);
    archived_one.archived = true;
    let mut archived_two = task(
        7302,
        "Archived vendored spike",
        HumanStatus::Doing,
        TaskScope::Global,
        50 * 60,
    );
    archived_two.number = Some(32);
    archived_two.archived = true;
    archived_tasks.push(archived_one);
    archived_tasks.push(archived_two);
    let archived_view = fixture_view(&archived_tasks, true);
    let mut archived_model = fixture_model(&archived_tasks, &archived_view);
    archived_model.archived_collapsed = false;
    let (archived_rows, _) = paint(80, 24, &archived_model);

    vec![
        GoldenScene {
            name: "board",
            rows: base_rows,
            width: 80,
        },
        GoldenScene {
            name: "board_default_split_78",
            rows: default_split_rows,
            width: 78,
        },
        GoldenScene {
            name: "board_marked",
            rows: marked_rows,
            width: 80,
        },
        GoldenScene {
            name: "board_search",
            rows: board_search_rows,
            width: 80,
        },
        GoldenScene {
            name: "board_search_pinned",
            rows: board_search_pinned_rows,
            width: 80,
        },
        GoldenScene {
            name: "board_search_empty",
            rows: board_search_empty_rows,
            width: 80,
        },
        GoldenScene {
            name: "palette",
            rows: palette_rows,
            width: 80,
        },
        GoldenScene {
            name: "help",
            rows: help_rows,
            width: 80,
        },
        GoldenScene {
            name: "done_drawer",
            rows: done_rows,
            width: 80,
        },
        GoldenScene {
            name: "inbox",
            rows: inbox_rows,
            width: 80,
        },
        GoldenScene {
            name: "done_drawer_archived",
            rows: archived_rows,
            width: 80,
        },
        GoldenScene {
            name: "board_wide_110x30",
            rows: board_wide_rows,
            width: 110,
        },
        GoldenScene {
            name: "board_wide_detail_110x30",
            rows: board_wide_detail_rows,
            width: 110,
        },
        GoldenScene {
            name: "board_project_order_80x30",
            rows: board_project_order_rows,
            width: 80,
        },
    ]
}

/// Boxed plain-text dump matching the shape of the prototype's own `show()` dump, so a
/// reviewer can diff this file against `python3 design/prototype/main.py --dump` by eye.
fn dump_frame(rows: &[String], width: u16) -> String {
    let mut out = String::new();
    out.push('┌');
    out.push_str(&"─".repeat(width as usize));
    out.push_str("┐\n");
    for row in rows {
        out.push('│');
        out.push_str(row);
        out.push('│');
        out.push('\n');
    }
    out.push('└');
    out.push_str(&"─".repeat(width as usize));
    out.push_str("┘\n");
    out
}

fn golden_fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/queue_board")
}

fn golden_path(name: &str) -> std::path::PathBuf {
    golden_fixtures_dir().join(format!("{name}.txt"))
}

/// Regeneration helper (not part of the suite): run with `--ignored` after a deliberate,
/// reviewed change to what a scene paints, to rewrite the checked-in goldens from the
/// renderer's own output. Never hand-edit a `.txt` fixture.
#[test]
#[ignore]
fn regenerate_golden_fixtures() {
    for scene in golden_scenes() {
        let dump = dump_frame(&scene.rows, scene.width);
        std::fs::write(golden_path(scene.name), dump).expect("write golden");
    }
}

/// the the-relevant scenes the reviewer compares against the frozen prototype dump
/// (board at 80×24, board at the default 78×24 split, the wide list + detail, palette,
/// help, done drawer) each have a checked-in golden frame, and every fresh paint still matches
/// its golden byte-for-byte.
#[test]
fn surface_goldens_board_wide_palette_help_drawer_exist_for_reviewer_side_by_side() {
    for scene in golden_scenes() {
        let dump = dump_frame(&scene.rows, scene.width);
        let path = golden_path(scene.name);
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "missing golden fixture for scene {:?} at {path:?} ({e}); \
                 generate it from this same `dump_frame` output and check it in",
                scene.name
            )
        });
        assert_eq!(
            dump, expected,
            "golden frame for scene {:?} drifted from {path:?}",
            scene.name
        );
    }
}

/// every checked-in golden frame is itself scanned for forbidden color SGR, the same
/// scan every in-test paint already runs, so a hand-edited or stale fixture cannot smuggle
/// color back in unnoticed.
///
/// this scan cannot itself detect a colored cell -- the `.txt` goldens are built from
/// `buffer[(x,y)].symbol()` (see `paint()` above), symbols only, no `Style`, so no SGR byte
/// can ever reach a golden file regardless of what the renderer painted. The real color
/// oracle is [`assert_buffer_mono`], run on every painted `Buffer` inside `paint()` before a
/// single row is ever turned into golden text; this scan's job is narrower and stated
/// plainly here: it only guards the on-disk fixture itself against a hand edit or drift that
/// would smuggle a raw escape sequence into text a reviewer diffs by eye.
/// G-8 (gate MEDIUM): guards the binding `palette_commands()` establishes -- a future edit
/// that reverts the palette golden scene to a hand-rolled `PaletteCommandRow` array (as
/// opposed to deriving it from `BoardModel::visible_commands`) must fail here rather than
/// silently reintroducing a command the the catalog does not offer (: no park, resume,
/// link, or dispatch entries).
#[test]
fn palette_golden_scene_commands_are_bound_to_the_real_m1_catalog_and_exclude_dispatch() {
    let commands = palette_commands();
    let labels: Vec<&str> = commands.iter().map(|command| command.label).collect();
    assert_eq!(
        labels,
        vec!["set status: todo", "set status: doing", "set status: done",],
        "the palette golden scene's command rows must be exactly what \
         `BoardModel::visible_commands` produces for the fixture's selection and query, not \
         a hand-rolled stand-in"
    );
    assert!(
        commands
            .iter()
            .all(|command| !command.label.contains("dispatch")),
        "AC-17 excludes dispatch from the M1 palette outright; the golden scene must never \
         show it: {commands:?}"
    );
}

/// T-7 (AC-22): the task page's footer verb bar lists the step-add verb while the
/// bound task has steps steps (view mode). A task with no steps keeps the
/// pre-T-7 verb bar exactly: the with-steps bar is the without-steps bar plus the
/// one step-add entry — modifier implied by the bar's prefix convention — and
/// nothing else. The full listing is asserted at a width the whole bar fits. At
#[test]
fn footer_lists_the_step_add_verb() {
    let page_verb_row_with = |steps: &[&str], width: u16| -> String {
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "Verb bar witness",
                Some("the notes body".into()),
                TaskScope::Global,
                ProvenanceOrigin::Manual,
            )
            .expect("create task");
        for text in steps {
            domain.add_step(id, text).expect("add step");
        }
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
            .expect("open task page");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("enter task edit mode");
        apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
            .expect("return to task page");
        let rows = board_rows(&model, width, 24);
        let verb_row = tier::resolve(width, 24).verb_row.expect("verb row");
        trimmed(&rows[verb_row as usize])
    };

    let with = page_verb_row_with(&["only step"], 100);
    let without = page_verb_row_with(&[], 100);

    for row in [&with, &without] {
        assert!(
            row.contains("ctrl+a step"),
            "task edit mode must list the step-add verb, including an empty checklist:\n{row}"
        );
    }
    assert_eq!(
        with, without,
        "step availability belongs to task edit mode, not to whether a step already exists"
    );

    let floor = page_verb_row_with(&["only step"], 78);
    assert!(
        floor.contains("ctrl+a step") && floor.contains("esc cancel"),
        "the editing bar fits the standard floor without clipping:\n{floor}"
    );
}

#[test]
fn all_golden_frames_pass_no_color_sgr_scan() {
    let dir = golden_fixtures_dir();
    let mut scanned = 0usize;
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        assert_no_color_sgr(&text);
        scanned += 1;
    }
    assert_eq!(
        scanned, 14,
        "expected the fourteen board surface goldens (board, board_marked, \
         board_default_split_78, board_search, board_search_pinned, board_search_empty, \
         palette, help, done_drawer, inbox, done_drawer_archived, board_wide_110x30, \
         board_wide_detail_110x30, board_project_order_80x30) in {dir:?}"
    );
}

/// The main list wraps a long title onto continuation lines indented under the
/// task's own first row: nothing is cut, and no row ends in an omission marker.
#[test]
fn board_list_wraps_a_long_title_onto_a_continuation_row() {
    let mut domain = DomainState::new();
    let title =
        "alpha bravo charlie delta echo foxtrot golf india juliet kilo lima mike hotel".to_string();
    domain
        .create(&title, None, TaskScope::Global, ProvenanceOrigin::Manual)
        .expect("create task");
    let model = BoardModel::from_domain(&domain, None);

    let rows = board_rows(&model, 80, 24);
    // The head row is the gutter then the title: the section already names the status.
    assert!(
        rows.iter().any(|row| row.contains("▸  alpha")),
        "head row must keep the gutter + title shape:\n{}",
        rows.join("\n")
    );
    // A continuation line indents exactly three cells into the title column (the gutter).
    let continuation = rows
        .iter()
        .find(|row| row.contains("hotel"))
        .unwrap_or_else(|| panic!("no wrapped continuation row:\n{}", rows.join("\n")));
    assert!(
        continuation.starts_with("   ") && !continuation.starts_with("    "),
        "continuation rows indent three cells under the title column: {continuation:?}"
    );
    // Neither of the task's own rows may carry an omission marker (the verb bar's
    // own tier budget is a different surface).
    let head = rows
        .iter()
        .find(|row| row.contains("▸  alpha"))
        .expect("head row");
    assert!(
        !head.contains('…') && !continuation.contains('…'),
        "a wrapped task row carried an omission marker"
    );
}

/// Quick-add wraps its draft into the reserved blank rows instead of scrolling
/// sideways: every typed word stays visible above or on the input line.
#[test]
fn quick_add_wraps_a_long_title_into_the_reserved_rows() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None).expect("open quick add");
    let title: String = (0..40).map(|index| format!("w{index} ")).collect();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::QuickAddInsertText(title),
        None,
    )
    .expect("type long title");

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw quick add");
    let rows = board_rows(&model, 80, 24);
    let prompt_rows: Vec<&String> = rows.iter().filter(|row| row.contains("▎")).collect();
    assert_eq!(
        prompt_rows.len(),
        2,
        "exactly one continuation row fits the reserved blank above the input:\n{}",
        rows.join("\n")
    );
    let text = rows.join("\n");
    // Reading order survives wrapping: an earlier segment paints above the tail,
    // and the rule row two above the input stays chrome, never draft text.
    let tail = rows
        .iter()
        .position(|row| row.contains("w39"))
        .expect("tail segment painted");
    let above = rows
        .iter()
        .position(|row| row.contains("▎") && !row.contains("w39"))
        .expect("continuation row painted");
    assert!(
        above < tail,
        "the draft reads top-down across its wrapped rows:\n{text}"
    );
    assert!(
        rows[tail - 2].starts_with('─'),
        "the rule row stays a rule row:\n{}",
        rows.join("\n")
    );
}

/// Arrow keys move the Notes caret across WRAPPED rows on the task page: down
/// crosses logical lines and wrapped continuations alike; up returns it.
#[test]
fn notes_edit_arrows_move_across_logical_and_wrapped_rows() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Arrow nav",
            Some("one\n\ntwo".to_string()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    board_rows(&model, 80, 24);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit notes");

    let caret_at = |model: &BoardModel, width: u16, height: u16| {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| {
                let _ = draw_board(frame, model);
            })
            .expect("draw");
        terminal.backend().cursor_position().y
    };
    let caret_y = |model: &BoardModel| caret_at(model, 80, 24);

    // Seeded at the end of "two" (row 2). Up twice walks back over the blank row.
    assert_eq!(caret_y(&model), 5);
    apply_intent(&mut domain, &mut model, BoardIntent::EditMoveUp, None).expect("up to blank row");
    assert_eq!(caret_y(&model), 4, "up lands on the blank middle row");
    apply_intent(&mut domain, &mut model, BoardIntent::EditMoveUp, None).expect("up to first row");
    assert_eq!(caret_y(&model), 3, "up reaches the first note row");
    apply_intent(&mut domain, &mut model, BoardIntent::EditMoveDown, None).expect("down again");
    assert_eq!(caret_y(&model), 4);

    // Wrapped rows: at the compact floor the same draft wraps at word boundaries,
    // and Up from the tail row climbs onto the wrapped head row.
    let mut domain = DomainState::new();
    domain
        .create(
            "Wrapped arrows",
            Some("aaaaa bbbbb ccccc ddddd eeeee fffff ggggg".to_string()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    board_rows(&model, 40, 10);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit notes");
    let before = caret_at(&model, 40, 10);
    apply_intent(&mut domain, &mut model, BoardIntent::EditMoveUp, None)
        .expect("up across the wrap");
    let after = caret_at(&model, 40, 10);
    assert!(
        after < before,
        "Up must climb from the wrapped tail row ({before}) onto the head row, got {after}"
    );
}

/// A pathological title cannot eat the page: at the compact floor the wrapped
/// header stops inside the page body, the last shown row carries the omission
/// marker, and the rule/status/verb chrome survives untouched.
#[test]
fn task_page_caps_a_wrapped_header_inside_the_page_body() {
    let mut domain = DomainState::new();
    let title = "word ".repeat(120);
    let id = domain
        .create(&title, None, TaskScope::Global, ProvenanceOrigin::Manual)
        .expect("create task");
    // Doing retains the five-column status geometry this boundary test needs.
    domain
        .set_status(id, HumanStatus::Doing)
        .expect("keep the five-column header layout");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let rows = board_rows(&model, 40, 10);
    // Chrome rows keep their own content: rule dashes at 7, status at 8, verbs at 9.
    assert!(
        rows[7].starts_with('─'),
        "the rule row was overwritten by the header:\n{}",
        rows.join("\n")
    );
    assert!(
        rows[8].trim() == "all",
        "the status row was overwritten by the header:\n{}",
        rows.join("\n")
    );
    assert!(
        rows[9].contains("ctrl+e"),
        "the verb row was overwritten by the header:\n{}",
        rows.join("\n")
    );
    // The header itself is bounded and honest about what it hides.
    assert!(
        (1..7).any(|y| rows[y].contains("wor…")),
        "a capped header names the rows it cannot show:\n{}",
        rows.join("\n")
    );
}

/// Step navigation must not re-scroll the whole page on every cursor move: the
/// viewport height the renderer records (`window_rows`) bounds the scroll math,
/// and losing it collapses the window to one row.
#[test]
fn step_cursor_moves_do_not_rescroll_the_page_when_steps_fit() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Steps page",
            Some("n".to_string()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create task");
    for index in 0..20 {
        domain
            .add_step(id, format!("step {index}"))
            .expect("add step");
    }
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    // One painted frame records the viewport; then walk the step cursor down.
    board_rows(&model, 80, 24);
    for _ in 0..4 {
        apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
            .expect("advance step cursor");
    }
    let rows = board_rows(&model, 80, 24);
    let text = rows.join("\n");
    assert!(
        text.contains("step 0"),
        "a fitting steps window must not scroll its head away:\\n{text}"
    );
    assert!(
        text.contains("step 3"),
        "the walked-to step must be visible:\\n{text}"
    );
}

/// A title-edit caret hidden below the capped header parks at the END of the
/// last shown row, never at its hidden column on the ellipsis row.
#[test]
fn edit_title_caret_parks_at_the_capped_headers_end() {
    let mut domain = DomainState::new();
    let title = "word ".repeat(120);
    let id = domain
        .create(&title, None, TaskScope::Global, ProvenanceOrigin::Manual)
        .expect("create task");
    // Doing retains the five-column status geometry this boundary test needs.
    domain
        .set_status(id, HumanStatus::Doing)
        .expect("keep the five-column header layout");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    board_rows(&model, 40, 10);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("edit title");

    let mut terminal = Terminal::new(TestBackend::new(40, 10)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw edit page");
    let rows = board_rows(&model, 40, 10);
    // The capped header's last row carries the marker; the caret must sit at
    // that row's past-end column, immediately after the "…".
    let last_header = (1..7)
        .rev()
        .find(|&y| rows[y].starts_with("    ") && rows[y].contains("wor…"))
        .expect("capped header row");
    let cursor = terminal.backend().cursor_position();
    assert_eq!(
        cursor.y, last_header as u16,
        "caret left the last header row"
    );
    let painted = rows[last_header].trim_end();
    assert_eq!(
        cursor.x as usize,
        painted.chars().count(),
        "caret must sit immediately after the marker, not at a hidden column:\n{}",
        rows.join("\n")
    );
}

/// A deck longer than the viewport paints a dim track+thumb on the right edge and
/// reserves a gap so wrapped titles never sit under the thumb.
#[test]
fn overflowing_board_list_paints_a_scrollbar() {
    let tasks: Vec<Task> = (0..40u128)
        .map(|i| {
            task(
                7000 + i,
                &format!("Scrollbar padding task {i}"),
                HumanStatus::Todo,
                project("/repos/tsk"),
                3600,
            )
        })
        .collect();
    let last_id = Uuid::from_u128(7000 + 39);
    let view = fixture_view_projects(&tasks, false);
    let mut model = fixture_model_on_project(&tasks, &view);
    model.selection_id = Some(last_id);

    let (rows, geo) = paint(80, 24, &model);
    let top = geo.viewport_top as usize;
    let bottom = (geo.viewport_top + geo.viewport_height) as usize;
    let viewport = &rows[top..bottom];
    assert!(
        viewport.iter().any(|row| row.contains('▌')),
        "overflowing list needs a thumb:\n{}",
        viewport.join("\n")
    );
    assert!(
        viewport
            .iter()
            .all(|row| { !matches!(row.chars().last(), Some('│')) }),
        "scrollbar gutter is blank, not a │ track:\n{}",
        viewport.join("\n")
    );
    assert!(
        viewport
            .iter()
            .any(|row| trimmed(row).contains("Scrollbar padding task 39")),
        "selection-follow still keeps the selected task visible:\n{}",
        viewport.join("\n")
    );
}

#[test]
fn overflowing_board_list_does_not_clip_task_rows_to_ellipsis() {
    let tasks: Vec<Task> = (0..40u128)
        .map(|i| {
            task(
                7100 + i,
                &format!("ScrollbarNoBreakTitleThatMustWrapNotEllipsize{i:02}XXXXXXXXXXXXXXXX"),
                HumanStatus::Todo,
                project("/repos/tsk"),
                3600,
            )
        })
        .collect();
    let view = fixture_view_projects(&tasks, false);
    let model = fixture_model_on_project(&tasks, &view);
    let (rows, geo) = paint(80, 24, &model);
    let top = geo.viewport_top as usize;
    let bottom = (geo.viewport_top + geo.viewport_height) as usize;
    let mut saw_title = false;
    for row in &rows[top..bottom] {
        if trimmed(row).contains("ScrollbarNoBreakTitle") {
            saw_title = true;
            assert!(
                !row.contains('…'),
                "scrollbar must not clip task rows into ellipsis:\n{row}"
            );
        }
    }
    assert!(
        saw_title,
        "fixture title must actually paint:\n{}",
        rows.join("\n")
    );
}

#[test]
fn scrolled_list_pins_the_section_header_under_the_tabs() {
    let tasks: Vec<Task> = (0..40u128)
        .map(|i| {
            task(
                7200 + i,
                &format!("Sticky header task {i}"),
                HumanStatus::Todo,
                TaskScope::Global,
                3600,
            )
        })
        .collect();
    let view = fixture_view(&tasks, false);
    let mut model = fixture_model(&tasks, &view);
    model.selection_id = Some(Uuid::from_u128(7200));
    model.list_scroll = 6;
    model.follow_list = false;
    let (rows, geo) = paint(80, 24, &model);
    let top = geo.viewport_top as usize;
    assert!(
        trimmed(&rows[top]).is_empty(),
        "pinned header needs a blank row under the tabs:\n{}",
        rows.join("\n")
    );
    let header = trimmed(&rows[top + 1]);
    assert!(
        header.contains("desk"),
        "desk header must pin under the tabs after scrolling past it:\n{header}\n{}",
        rows.join("\n")
    );
}

#[test]
fn scrolled_done_section_pins_done_under_the_tabs() {
    let mut tasks: Vec<Task> = (0..8u128)
        .map(|i| {
            task(
                7300 + i,
                &format!("Open sticky {i}"),
                HumanStatus::Todo,
                TaskScope::Global,
                3600,
            )
        })
        .collect();
    tasks.extend((0..40u128).map(|i| {
        task(
            7400 + i,
            &format!("Done sticky {i}"),
            HumanStatus::Done,
            TaskScope::Global,
            3600,
        )
    }));
    let view = fixture_view(&tasks, true);
    let mut model = fixture_model(&tasks, &view);
    model.selection_id = Some(Uuid::from_u128(7300));
    model.list_scroll = 20;
    model.follow_list = false;
    let (rows, geo) = paint(80, 24, &model);
    let top = geo.viewport_top as usize;
    assert!(
        trimmed(&rows[top]).is_empty(),
        "pinned header needs a blank row under the tabs:\n{}",
        rows.join("\n")
    );
    let header = trimmed(&rows[top + 1]);
    assert!(
        header.contains("DONE"),
        "DONE header must pin under the tabs once that section reaches the top:\n{header}\n{}",
        rows.join("\n")
    );
}

#[test]
fn archived_task_paints_in_no_working_lens_in_any_status_at_any_tier() {
    for status in [
        HumanStatus::Todo,
        HumanStatus::Doing,
        HumanStatus::Doing,
        HumanStatus::Doing,
        HumanStatus::Done,
    ] {
        let mut domain = DomainState::new();
        let project = TaskScope::Project {
            path: "/repos/lens".into(),
        };
        domain
            .create(
                "visible row task",
                None,
                project.clone(),
                ProvenanceOrigin::Manual,
            )
            .expect("create live task");
        let archived_title = format!("archived-{status:?} row");
        let archived_id = domain
            .create(
                archived_title.clone(),
                None,
                project,
                ProvenanceOrigin::Manual,
            )
            .expect("create archived task");
        domain.archive_task(archived_id).expect("archive it");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/lens")));

        // Scope setups: every scope (grouped by project), the desk, and the project board.
        let lens_names = ["all", "desk", "project board"];
        for (lens_index, lens) in lens_names.iter().enumerate() {
            match lens_index {
                0 => {}
                1 => model.set_selected_project(None),
                _ => model.set_selected_project(Some(PathBuf::from("/repos/lens"))),
            }

            for (width, height) in [(80u16, 24u16), (40u16, 10u16)] {
                let rows = board_rows(&model, width, height);
                assert!(
                    !rows.iter().any(|row| row.contains(&archived_title)),
                    "archived {status:?} task painted at {width}x{height} in {lens}:\n{}",
                    rows.join("\n")
                );
            }

            // Wide list + detail with the detail focused.
            apply_intent(&mut domain, &mut model, BoardIntent::FocusDetail, None)
                .expect("focus detail");
            let rows = board_rows(&model, 130, 24);
            assert!(
                !rows.iter().any(|row| row.contains(&archived_title)),
                "archived {status:?} task painted in the wide split in {lens}:\n{}",
                rows.join("\n")
            );
            apply_intent(&mut domain, &mut model, BoardIntent::FocusList, None)
                .expect("focus list");
        }
    }
}

#[test]
fn archived_group_paints_below_done_with_its_count_and_no_header_when_empty() {
    let mut domain = DomainState::new();
    let project = TaskScope::Project {
        path: "/repos/tsk".into(),
    };
    domain
        .create(
            "done row task",
            None,
            project.clone(),
            ProvenanceOrigin::Manual,
        )
        .expect("create done task");
    let done_id = domain.tasks()[0].id;
    domain.complete(done_id).expect("complete it");
    let a = domain
        .create(
            "archived alpha",
            None,
            project.clone(),
            ProvenanceOrigin::Manual,
        )
        .expect("create a");
    let b = domain
        .create("archived beta", None, project, ProvenanceOrigin::Manual)
        .expect("create b");
    domain.archive_task(a).expect("archive a");
    domain.archive_task(b).expect("archive b");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/tsk")));
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");

    let rows = board_rows(&model, 80, 24);
    let done_row = rows
        .iter()
        .position(|row| row.contains("DONE"))
        .expect("DONE header paints with the drawer open");
    let archived_row = rows
        .iter()
        .position(|row| row.contains("archived") && row.contains('2'))
        .expect("an `archived 2` header row must paint below DONE");
    assert!(
        archived_row > done_row,
        "the archived group paints below DONE"
    );
    assert!(
        !rows.iter().any(|row| row.contains("archived alpha")),
        "collapsed: archived titles stay hidden:\n{}",
        rows.join("\n")
    );
    assert!(!rows.iter().any(|row| row.contains("archived beta")));

    // Zero archived: no archived header row at all.
    domain.unarchive_task(a).expect("unarchive a");
    domain.unarchive_task(b).expect("unarchive b");
    model.sync_from_domain(&domain);
    let rows = board_rows(&model, 80, 24);
    assert!(
        !rows.iter().any(|row| row.contains("archived ·")),
        "no archived group header with zero archived tasks (the unarchived rows are \
         ordinary ON DECK rows now):\n{}",
        rows.join("\n")
    );
}

#[test]
fn expanded_archived_rows_are_dim_keep_identifier_and_are_selectable_and_hit_testable() {
    use tsk_tui::ui::board::board_hit_map;
    use tsk_tui::ui::render::QueueHitTarget;

    let mut domain = DomainState::new();
    domain
        .create(
            "live open task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create live task");
    let archived_id = domain
        .create(
            "archived deep work",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create archived task");
    domain.archive_task(archived_id).expect("archive it");
    // Numbers are assigned at the persistence boundary: round-trip through a store so
    // the archived row carries its `T<n>` like a real session's rows do.
    let dir = std::env::temp_dir().join(format!("tsk-render-archived-{}", Uuid::new_v4()));
    let store = tsk_tui::store::TaskStore::new(&dir);
    store.save(&domain).expect("save for numbers");
    domain = store.load().expect("reload with numbers");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(domain
        .get(archived_id)
        .and_then(|task| task.number)
        .is_some());
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    // The toggle intent selects the header row and expands the group.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleArchivedGroup,
        None,
    )
    .expect("expand archived group");

    // Paint BEFORE selecting the row: unselected archived rows are all-dim.
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    assert_buffer_mono(buffer);
    let row_y = (0..24)
        .map(|y| {
            let row: String = (0..80).map(|x| buffer[(x, y)].symbol()).collect();
            (y, row)
        })
        .find(|(_, row)| row.contains("archived deep work"))
        .expect("expanded archived row paints")
        .0;
    let row_text: String = (0..80).map(|x| buffer[(x, row_y)].symbol()).collect();
    assert!(
        row_text.contains("T2"),
        "identifier prefix paints: {row_text}"
    );
    assert!(
        !row_text.contains('○'),
        "list rows carry no status glyph: {row_text}"
    );
    for x in 0..80 {
        let symbol = buffer[(x, row_y)].symbol();
        if symbol == " " {
            continue;
        }
        assert!(
            buffer[(x, row_y)]
                .style()
                .add_modifier
                .contains(Modifier::DIM),
            "every cell of the archived row is dim (cell {x}): {row_text}"
        );
    }

    // Selectable: SelectNext from the header lands on the archived row.
    apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None).expect("select next");
    assert_eq!(
        model.selected_id(),
        Some(archived_id),
        "SelectNext must land on the archived row"
    );

    // Hit-testable: the painted row carries the task's hit target.
    let hits = board_hit_map(ratatui::layout::Rect::new(0, 0, 80, 24), &model);
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::Task(t) if t == archived_id)),
        "the archived row is hit-testable: {hits:?}"
    );
}

#[test]
fn task_page_header_slot_reads_archived_for_an_archived_task() {
    let mut domain = DomainState::new();
    let live_id = domain
        .create(
            "live row task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create live");
    let archived_id = domain
        .create(
            "archived page task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create archived");
    domain.archive_task(archived_id).expect("archive it");

    let open_page = |domain: &mut DomainState| -> BoardModel {
        let mut model = BoardModel::from_domain(domain, None);
        apply_intent(domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
        apply_intent(domain, &mut model, BoardIntent::ToggleArchivedGroup, None)
            .expect("expand archived group");
        let idx = model
            .visible_ids()
            .iter()
            .position(|&visible| visible == archived_id)
            .expect("archived row visible");
        apply_intent(domain, &mut model, BoardIntent::SelectIndex(idx), None)
            .expect("select the archived row");
        apply_intent(domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
        model
    };

    // Single-pane 80x24: the header slot reads `archived`, not the status word.
    let model = open_page(&mut domain);
    let rows = board_rows(&model, 80, 24);
    assert!(
        rows.iter().any(|row| row.contains("archived")),
        "header slot must read archived:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows.iter().any(|row| row.contains("todo")),
        "the status word must give way to archived:\n{}",
        rows.join("\n")
    );

    // Wide 130x24 stage F: same header contract.
    let model = open_page(&mut domain);
    let rows = board_rows(&model, 130, 24);
    assert!(
        rows.iter().any(|row| row.contains("archived")),
        "wide header slot must read archived:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows.iter().any(|row| row.contains("todo")),
        "wide status word must give way to archived:\n{}",
        rows.join("\n")
    );

    // An unarchived task still shows its status word.
    let mut model = BoardModel::from_domain(&domain, None);
    let live_index = model
        .visible_ids()
        .iter()
        .position(|&visible| visible == live_id)
        .expect("live task visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(live_index),
        None,
    )
    .expect("select live");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let rows = board_rows(&model, 80, 24);
    assert!(
        rows.iter().any(|row| row.contains("todo")),
        "unarchived task keeps its status word:\n{}",
        rows.join("\n")
    );
}

#[test]
fn archived_project_paints_nowhere_on_every_scope() {
    let mut domain = DomainState::new();
    domain
        .create(
            "gone started task",
            None,
            TaskScope::Project {
                path: "/repos/gone".into(),
            },
            ProvenanceOrigin::Manual,
        )
        .expect("create");
    domain
        .set_status(domain.tasks()[0].id, HumanStatus::Doing)
        .expect("doing");
    domain
        .create(
            "here task",
            None,
            TaskScope::Project {
                path: "/repos/here".into(),
            },
            ProvenanceOrigin::Manual,
        )
        .expect("create");
    domain.archive_project("/repos/gone").expect("archive gone");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/here")));

    for desk in [false, true] {
        // The board starts on every live project, grouped; `desk` pins the desk scope.
        if desk {
            model.set_selected_project(None);
        }
        let rows = board_rows(&model, 80, 24);
        assert!(
            !rows.iter().any(|row| row.contains("gone started task")),
            "desk={desk}: the archived project's task paints:\n{}",
            rows.join("\n")
        );
        // Not even as a project group heading.
        assert!(
            !rows.iter().any(|row| row.contains("gone")),
            "desk={desk}: the archived project paints:\n{}",
            rows.join("\n")
        );
    }
    model.set_selected_project(Some(PathBuf::from("/repos/here")));
    let rows = board_rows(&model, 80, 24);
    assert!(
        rows.iter().any(|row| row.contains("here task")),
        "the live project keeps its task:\n{}",
        rows.join("\n")
    );
}

#[test]
fn archived_header_selection_follows_the_viewport() {
    use ratatui::style::Modifier;

    let mut domain = DomainState::new();
    for i in 0..30u128 {
        let id = domain
            .create(
                format!("done filler {i}"),
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
            )
            .expect("create done filler");
        domain.complete(id).expect("complete filler");
    }
    let archived_id = domain
        .create(
            "archived below the fold",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create archived");
    domain.archive_task(archived_id).expect("archive it");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleArchivedGroup,
        None,
    )
    .expect("expand archived group");

    // The toggle intent selects the header while the group expands; the header sits
    // below 30 done rows, so without viewport follow it stays below the fold.
    assert!(
        model.archived_header_selected(),
        "selection rests on the archived header"
    );

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    assert_buffer_mono(buffer);
    let header_visible = (0..24).any(|y| {
        let row: String = (0..80).map(|x| buffer[(x, y)].symbol()).collect();
        if !(row.contains("archived") && row.contains('\u{25be}')) {
            return false;
        }
        // Selected paint: the word `archived` is bold (AC-13 amended), never reverse.
        (0..20).any(|x| {
            buffer[(x, y)].symbol() == "a"
                && buffer[(x, y)].style().add_modifier.contains(Modifier::BOLD)
        })
    });
    assert!(
        header_visible,
        "the selected archived header must scroll into view, word bold:\n{}",
        (0..24)
            .map(|y| {
                let row: String = (0..80).map(|x| buffer[(x, y)].symbol()).collect();
                format!("{y:02} {row}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn archived_header_reads_chevron_word_dot_count_and_selection_is_bold_not_reverse() {
    use ratatui::style::Modifier;

    let mut domain = DomainState::new();
    let archived_id = domain
        .create(
            "archived header paint",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
        )
        .expect("create archived");
    domain.archive_task(archived_id).expect("archive it");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleArchivedGroup,
        None,
    )
    .expect("expand and select the header");

    let paint = |model: &BoardModel| {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let _ = draw_board(frame, model);
            })
            .expect("draw board");
        terminal.backend().buffer().clone()
    };

    // Selected: `▾ archived · 1`, the word bold, nothing reversed.
    let buffer = paint(&model);
    assert_buffer_mono(&buffer);
    let header_y = (0..24)
        .map(|y| {
            let row: String = (0..80).map(|x| buffer[(x, y)].symbol()).collect();
            (y, row)
        })
        .find(|(_, row)| row.contains("archived") && row.contains('▾'))
        .expect("header row paints")
        .0;
    let row: String = (0..80).map(|x| buffer[(x, header_y)].symbol()).collect();
    assert!(
        row.contains("▾ archived · 1"),
        "header reads chevron, word, dot, count: {row:?}"
    );
    let mut rule_cells = 0;
    let mut word_bold = false;
    let mut any_reverse = false;
    for x in 0..80 {
        let cell = buffer[(x, header_y)].clone();
        let symbol = cell.symbol().to_string();
        if symbol == "─" {
            rule_cells += 1;
        }
        if cell.style().add_modifier.contains(Modifier::REVERSED) {
            any_reverse = true;
        }
        let style = cell.style().add_modifier;
        if style.contains(Modifier::BOLD) && symbol == "a" {
            word_bold = true;
        }
    }
    assert_eq!(rule_cells, 0, "the header has no rule row cells: {row:?}");
    assert!(!any_reverse, "selected header never paints a reverse block");
    assert!(
        word_bold,
        "the word `archived` is bold when selected: {row:?}"
    );

    // Unselected: all dim, no bold.
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(1), None)
        .expect("select the archived task row (index 0 is the header)");
    let buffer = paint(&model);
    assert_buffer_mono(&buffer);
    let header_y = (0..24)
        .map(|y| {
            let row: String = (0..80).map(|x| buffer[(x, y)].symbol()).collect();
            (y, row)
        })
        .find(|(_, row)| row.contains("archived") && row.contains('▾'))
        .expect("header row still paints when unselected")
        .0;
    let row: String = (0..80).map(|x| buffer[(x, header_y)].symbol()).collect();
    assert!(
        row.contains("▾ archived · 1"),
        "unselected header keeps its text: {row:?}"
    );
    for x in 0..80 {
        let cell = buffer[(x, header_y)].clone();
        if cell.symbol() == " " {
            continue;
        }
        let style = cell.style().add_modifier;
        assert!(
            !style.contains(Modifier::BOLD),
            "unselected header has no bold (cell {x}): {row:?}"
        );
        assert!(
            !style.contains(Modifier::REVERSED),
            "unselected header has no reverse (cell {x}): {row:?}"
        );
    }
}

#[test]
fn real_quick_add_paints_its_destination() {
    let mut domain = DomainState::new();
    domain
        .create(
            "existing",
            None,
            TaskScope::Project {
                path: "/repos/beta".into(),
            },
            ProvenanceOrigin::Manual,
        )
        .expect("task");
    // Launched inside the project: the every-project board's quick-add goes there.
    let snapshot = tsk_tui::context::InvocationSnapshot {
        default_scope: TaskScope::Project {
            path: "/repos/beta".into(),
        },
        this_repo: Some(PathBuf::from("/repos/beta")),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };
    let mut model = BoardModel::from_domain_for_snapshot(&domain, &snapshot);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCapture,
        Some(&snapshot),
    )
    .expect("quick add");
    let text = board_rows(&model, 162, 43).join("\n");
    assert!(
        text.contains("add to beta"),
        "quick-add destination missing:\n{text}"
    );
}

/// Two projects that share a basename keep separate group headings: the run boundary is
/// the stored path, only the label is short.
#[test]
fn projects_sharing_a_basename_each_get_a_heading() {
    let mut domain = DomainState::new();
    for path in ["/work/a/app", "/work/b/app"] {
        domain
            .create(
                format!("task in {path}"),
                None,
                TaskScope::Project {
                    path: path.to_string(),
                },
                ProvenanceOrigin::Manual,
            )
            .expect("create task");
    }
    let model = BoardModel::from_domain(&domain, None);
    let rows = board_rows(&model, 80, 24);
    let headings = rows.iter().filter(|row| row.trim() == "app").count();
    assert_eq!(headings, 2, "{}", rows.join("\n"));
}

/// A project heading paints in its palette colour on the real board frame; the desk stays
/// plain. (Bounded-line sanitising once stripped the colour before it reached the buffer.)
#[test]
fn a_project_heading_paints_its_palette_colour_on_the_board() {
    if std::env::var_os("NO_COLOR").is_some() {
        return;
    }
    let mut domain = DomainState::new();
    for scope in [
        TaskScope::Global,
        TaskScope::Project {
            path: "/work/app".to_string(),
        },
    ] {
        domain
            .create("task", None, scope, ProvenanceOrigin::Manual)
            .expect("create task");
    }
    let model = BoardModel::from_domain(&domain, None);
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    assert_buffer_mono(buffer);
    let row_of = |label: &str| {
        (0..24u16)
            .find(|&y| {
                (0..80u16)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim()
                    == label
            })
            .unwrap_or_else(|| panic!("{label} heading"))
    };
    let app = row_of("app");
    let x = (0..80u16)
        .find(|&x| buffer[(x, app)].symbol() == "a")
        .expect("label cell");
    assert!(
        tsk_tui::ui::render::GROUP_PALETTE.contains(&buffer[(x, app)].fg),
        "project heading colour: {:?}",
        buffer[(x, app)].fg
    );
    let desk = row_of("desk");
    assert!((0..80u16).all(|x| buffer[(x, desk)].fg == ratatui::style::Color::Reset));
}
