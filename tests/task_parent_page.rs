//! Task page parent relationship: `P` chooses a parent, `U` clears it, the page shows the
//! parent line and direct children, and the list shows `⊂done/total`. Nothing here changes a
//! child's status or scope, and a missing parent keeps its stored id.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use uuid::Uuid;

use tsk_tui::app::{apply_board_intent_with_save_recovery, BoardSaveContext};
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::save_recovery::SaveRecovery;
use tsk_tui::ui::input::{map_key, route_responsive_key, ResponsiveKeyRoute};
use tsk_tui::ui::mouse::{left_click, map_responsive_board_mouse};
use tsk_tui::ui::render::{assert_buffer_mono, QueueHitMap, QueueHitTarget};
use tsk_tui::ui::tier::{ResponsivePresentation, WideStage};
use tsk_tui::ui::{
    apply_intent, draw_board, BoardInputMode, BoardIntent, BoardModel, IntentOutcome,
};

const REPO: &str = "/repos/tsk";

struct Board {
    domain: DomainState,
    model: BoardModel,
    parent: Uuid,
    child: Uuid,
    other: Uuid,
}

/// T10 is a desk task; T11 (project) and T12 (desk) are not related yet. Numbers are set the
/// way a save would, so identifiers paint.
fn board() -> Board {
    let mut domain = DomainState::new();
    let parent = domain
        .create(
            "Ship the relationship view",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create parent");
    let child = domain
        .create(
            "Paint the parent line",
            Some("child notes".to_string()),
            TaskScope::Project {
                path: REPO.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create child");
    domain
        .set_status(child, HumanStatus::Started)
        .expect("start child");
    let other = domain
        .create(
            "Unrelated desk task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create other");
    let domain = numbered(&domain, &[(parent, 10), (child, 11), (other, 12)]);
    let model = BoardModel::from_domain(&domain, None);
    Board {
        domain,
        model,
        parent,
        child,
        other,
    }
}

/// Assign display numbers through the store document, as a save would.
fn numbered(domain: &DomainState, numbers: &[(Uuid, u64)]) -> DomainState {
    let mut value = serde_json::to_value(domain).expect("encode");
    for task in value["tasks"].as_array_mut().expect("tasks") {
        let id: Uuid = serde_json::from_value(task["id"].clone()).expect("id");
        if let Some((_, number)) = numbers.iter().find(|(task_id, _)| *task_id == id) {
            task["number"] = serde_json::json!(number);
        }
    }
    let mut next = value["next_task_number"].as_u64().unwrap_or(1);
    next = next.max(numbers.iter().map(|(_, n)| n + 1).max().unwrap_or(1));
    value["next_task_number"] = serde_json::json!(next);
    serde_json::from_value(value).expect("decode")
}

fn apply(board: &mut Board, intent: BoardIntent) -> IntentOutcome {
    let outcome =
        apply_intent(&mut board.domain, &mut board.model, intent, None).expect("apply intent");
    if outcome == IntentOutcome::Persist {
        board.model.sync_from_domain(&board.domain);
    }
    outcome
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shifted(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::SHIFT)
}

/// Map one key for the current mode (as a single-pane frame does) and apply it.
fn press(board: &mut Board, event: KeyEvent) -> Option<IntentOutcome> {
    let intent = map_key(board.model.input_mode(), event)?;
    Some(apply(board, intent))
}

fn type_text(board: &mut Board, text: &str) {
    for character in text.chars() {
        press(board, key(KeyCode::Char(character))).expect("query key maps");
    }
}

fn open_page(board: &mut Board, id: Uuid) {
    let index = board
        .model
        .visible_ids()
        .iter()
        .position(|visible| *visible == id)
        .expect("task is on the board");
    apply(board, BoardIntent::SelectIndex(index));
    apply(board, BoardIntent::OpenTaskPage);
    assert_eq!(board.model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(board.model.edit_target(), Some(id));
}

fn render(model: &BoardModel, width: u16, height: u16) -> (String, QueueHitMap) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let mut hits = QueueHitMap::default();
    terminal
        .draw(|frame| hits = draw_board(frame, model))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    assert_buffer_mono(&buffer);
    let rows = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (rows, hits)
}

#[test]
fn p_chooses_a_parent_by_search_and_enter_saves_it() {
    let mut board = board();
    let id = board.child;
    open_page(&mut board, id);
    let (page, _) = render(&board.model, 80, 24);
    assert!(page.contains("parent  none · P to set"), "{page}");

    assert_eq!(
        map_key(BoardInputMode::TaskPage, shifted('P')),
        Some(BoardIntent::OpenParentPicker),
        "a terminal that reports Shift still opens the chooser"
    );
    press(&mut board, key(KeyCode::Char('P'))).expect("P maps");
    assert_eq!(board.model.input_mode(), BoardInputMode::ParentPicker);
    let labels: Vec<String> = board
        .model
        .visible_list_picker_options()
        .into_iter()
        .map(|(_, option)| option.label)
        .collect();
    assert_eq!(
        labels,
        [
            "no parent",
            "T10 Ship the relationship view",
            "T12 Unrelated desk task"
        ],
        "every other task in number order; never the task itself"
    );
    let (picker, _) = render(&board.model, 80, 24);
    assert!(
        picker.contains("T10 Ship the relationship view"),
        "{picker}"
    );

    // Letters (here including `h`) type into the query while the chooser owns input.
    type_text(&mut board, "ship");
    assert_eq!(board.model.list_picker_query(), Some("ship"));
    assert_eq!(board.model.input_mode(), BoardInputMode::ParentPicker);
    let before_parent = board.domain.get(board.parent).expect("parent").clone();
    let before_child = board.domain.get(board.child).expect("child").clone();
    assert_eq!(
        press(&mut board, key(KeyCode::Enter)),
        Some(IntentOutcome::Persist)
    );

    let child = board.domain.get(board.child).expect("child");
    assert_eq!(child.parent_task_id, Some(board.parent));
    assert_eq!(child.status, before_child.status, "status untouched");
    assert_eq!(child.scope, before_child.scope, "scope never moves");
    assert_ne!(child.revision, before_child.revision, "one journaled edit");
    let parent = board.domain.get(board.parent).expect("parent");
    assert_eq!(
        parent.revision, before_parent.revision,
        "the parent is untouched"
    );
    assert_eq!(board.model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(board.model.message(), Some("parent set to T10"));

    let (page, _) = render(&board.model, 80, 24);
    assert!(
        page.contains("parent  ↑ T10 Ship the relationship view · open"),
        "{page}"
    );

    // The parent's page lists its direct child; the board row carries the count.
    apply(&mut board, BoardIntent::CloseLayer);
    let (list, _) = render(&board.model, 80, 24);
    let parent_row = list
        .lines()
        .find(|row| row.contains("T10 Ship the relationship view"))
        .expect("parent row");
    assert!(parent_row.trim_end().ends_with("⊂0/1"), "{parent_row:?}");
    let id = board.parent;
    open_page(&mut board, id);
    let (page, _) = render(&board.model, 80, 24);
    assert!(page.contains("children 0/1 done"), "{page}");
    assert!(page.contains("T11 Paint the parent line"), "{page}");
}

#[test]
fn esc_closes_the_chooser_without_saving() {
    let mut board = board();
    let id = board.child;
    open_page(&mut board, id);
    press(&mut board, key(KeyCode::Char('P'))).expect("P maps");
    let before = board.domain.clone();
    assert_eq!(
        press(&mut board, key(KeyCode::Esc)),
        Some(IntentOutcome::None)
    );
    assert_eq!(board.model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(
        board.domain.get(board.child).expect("child").revision,
        before.get(board.child).expect("child").revision
    );
}

#[test]
fn u_clears_the_parent_and_a_second_u_says_there_is_none() {
    let mut board = board();
    board
        .domain
        .set_parent(board.child, Some(board.parent))
        .expect("link");
    board.model.sync_from_domain(&board.domain);
    let id = board.child;
    open_page(&mut board, id);
    let parent_before = board.domain.get(board.parent).expect("parent").clone();

    assert_eq!(
        press(&mut board, key(KeyCode::Char('U'))),
        Some(IntentOutcome::Persist)
    );
    assert_eq!(
        board.domain.get(board.child).expect("child").parent_task_id,
        None
    );
    assert_eq!(
        board.domain.get(board.parent).expect("parent").revision,
        parent_before.revision,
        "clearing never touches the parent"
    );
    assert_eq!(board.model.message(), Some("parent cleared"));

    assert_eq!(
        press(&mut board, key(KeyCode::Char('U'))),
        Some(IntentOutcome::None)
    );
    assert_eq!(board.model.message(), Some("this task has no parent"));
}

#[test]
fn a_task_under_this_one_is_listed_but_refused() {
    let mut board = board();
    board
        .domain
        .set_parent(board.child, Some(board.parent))
        .expect("link");
    board.model.sync_from_domain(&board.domain);
    let id = board.parent;
    open_page(&mut board, id);
    press(&mut board, key(KeyCode::Char('P'))).expect("P maps");
    let (picker, _) = render(&board.model, 80, 24);
    assert!(
        picker.contains("T11 Paint the parent line · under this task"),
        "{picker}"
    );
    type_text(&mut board, "paint");
    let before = board.domain.get(board.parent).expect("parent").clone();
    assert_eq!(
        press(&mut board, key(KeyCode::Enter)),
        Some(IntentOutcome::None)
    );
    assert_eq!(
        board.model.input_mode(),
        BoardInputMode::ParentPicker,
        "the chooser stays open for another pick"
    );
    assert_eq!(
        board.model.message(),
        Some("T11 is under this task · choosing it would form a cycle")
    );
    let after = board.domain.get(board.parent).expect("parent");
    assert_eq!(after.parent_task_id, None);
    assert_eq!(after.revision, before.revision);
}

#[test]
fn a_dirty_draft_refuses_p_and_u_and_keeps_the_draft() {
    let mut board = board();
    board
        .domain
        .set_parent(board.child, Some(board.parent))
        .expect("link");
    board.model.sync_from_domain(&board.domain);
    let id = board.child;
    open_page(&mut board, id);
    apply(&mut board, BoardIntent::BeginEditTitle);
    // While a text field owns the cursor, capitals are text.
    assert_eq!(
        map_key(board.model.input_mode(), key(KeyCode::Char('P'))),
        Some(BoardIntent::EditInsert('P'))
    );
    apply(&mut board, BoardIntent::EditInsert('!'));
    apply(&mut board, BoardIntent::FormFocusNext);
    apply(&mut board, BoardIntent::FormFocusNext);
    assert_eq!(
        board.model.input_mode(),
        BoardInputMode::TaskPage,
        "Tab out of Notes keeps the edit session on the page"
    );
    assert!(board.model.task_session_dirty());
    let before = board.domain.clone();

    for character in ['P', 'U'] {
        assert_eq!(
            press(&mut board, key(KeyCode::Char(character))),
            Some(IntentOutcome::None)
        );
        assert_eq!(
            board.model.message(),
            Some("save or cancel edits before changing the parent")
        );
        assert_eq!(board.model.input_mode(), BoardInputMode::TaskPage);
        assert!(board.model.task_session_dirty(), "the draft survives");
    }
    assert_eq!(
        board.domain.get(board.child).expect("child").revision,
        before.get(board.child).expect("child").revision
    );
}

#[test]
fn a_deleted_parent_stays_on_the_child_as_not_found_and_other_edits_keep_it() {
    let mut board = board();
    board
        .domain
        .set_parent(board.child, Some(board.parent))
        .expect("link");
    board
        .domain
        .soft_delete(board.parent)
        .expect("delete parent");
    board.model.sync_from_domain(&board.domain);
    let child_before = board.domain.get(board.child).expect("child").clone();
    assert_eq!(
        child_before.parent_task_id,
        Some(board.parent),
        "deleting a parent never detaches or deletes its child"
    );
    assert_eq!(child_before.status, HumanStatus::Started);

    let id = board.child;
    open_page(&mut board, id);
    let (page, _) = render(&board.model, 80, 24);
    let flat: String = page.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains(&format!("↑ {}", board.parent)),
        "the stored id stays visible: {page}"
    );
    assert!(flat.contains("not found · link kept"), "{page}");

    // Saving another field keeps the unresolved link exactly as stored.
    apply(&mut board, BoardIntent::BeginEditTitle);
    apply(&mut board, BoardIntent::EditInsert('!'));
    assert_eq!(
        apply(&mut board, BoardIntent::ConfirmEdit),
        IntentOutcome::Persist
    );
    let child = board.domain.get(board.child).expect("child");
    assert!(child.title.ends_with('!'));
    assert_eq!(child.parent_task_id, Some(board.parent));
}

#[test]
fn list_badge_counts_direct_children_in_every_scope_and_status() {
    let mut board = board();
    for id in [board.child, board.other] {
        board
            .domain
            .set_parent(id, Some(board.parent))
            .expect("link");
    }
    board.domain.complete(board.other).expect("finish other");
    board
        .domain
        .archive_task(board.other)
        .expect("archive other");
    board.model.sync_from_domain(&board.domain);
    let (list, _) = render(&board.model, 80, 24);
    let row = list
        .lines()
        .find(|row| row.contains("T10 Ship the relationship view"))
        .expect("parent row");
    assert!(row.trim_end().ends_with("⊂1/2"), "{row:?}");
    let child_row = list
        .lines()
        .find(|row| row.contains("T11 Paint the parent line"))
        .expect("child row");
    assert!(!child_row.contains('⊂'), "a childless row has no badge");

    // A parent whose children are all done says so, but is never completed for the user.
    board.domain.complete(board.child).expect("finish child");
    board.model.sync_from_domain(&board.domain);
    let id = board.parent;
    open_page(&mut board, id);
    let (page, _) = render(&board.model, 80, 24);
    assert!(page.contains("children 2/2 done"), "{page}");
    assert!(
        page.contains("all children done · ctrl+d completes this task"),
        "{page}"
    );
    assert_eq!(
        board.domain.get(board.parent).expect("parent").status,
        HumanStatus::Open
    );
}

#[test]
fn wide_detail_shows_the_relationship_and_a_picker_click_saves() {
    let mut board = board();
    let area = Rect::new(0, 0, 130, 24);
    let index = board
        .model
        .visible_ids()
        .iter()
        .position(|visible| *visible == board.child)
        .expect("child on board");
    apply(&mut board, BoardIntent::SelectIndex(index));
    apply(&mut board, BoardIntent::FocusDetail);
    assert_eq!(board.model.wide_stage(), WideStage::Task);
    let (frame, _) = render(&board.model, 130, 24);
    assert!(frame.contains("parent  none · P to set"), "{frame}");

    let route = route_responsive_key(
        board.model.input_mode(),
        board.model.wide_stage(),
        ResponsivePresentation::WideSplit,
        key(KeyCode::Char('P')),
    );
    assert_eq!(route, ResponsiveKeyRoute::Surface);
    press(&mut board, key(KeyCode::Char('P'))).expect("P maps");
    assert_eq!(board.model.input_mode(), BoardInputMode::ParentPicker);
    // `l` is query text here, not a focus key.
    let route = route_responsive_key(
        board.model.input_mode(),
        board.model.wide_stage(),
        ResponsivePresentation::WideSplit,
        key(KeyCode::Char('l')),
    );
    assert_eq!(route, ResponsiveKeyRoute::Surface);

    let (frame, hits) = render(&board.model, 130, 24);
    assert!(frame.contains("T10 Ship the relationship view"), "{frame}");
    let rect = hits
        .regions
        .iter()
        .rev()
        .find(|hit| hit.target == QueueHitTarget::ListPickerOption(1))
        .map(|hit| hit.area)
        .expect("first task option is painted");
    let intent = map_responsive_board_mouse(&board.model, &hits, area, left_click(rect.x, rect.y))
        .expect("click maps");
    assert_eq!(intent, BoardIntent::SelectParentOption(1));
    assert_eq!(apply(&mut board, intent), IntentOutcome::Persist);
    assert_eq!(
        board.domain.get(board.child).expect("child").parent_task_id,
        Some(board.parent)
    );
    let (frame, _) = render(&board.model, 130, 24);
    assert!(
        frame.contains("parent  ↑ T10 Ship the relationship view · open"),
        "{frame}"
    );
}

#[test]
fn a_failed_parent_save_enters_recovery_and_cancel_restores_the_baseline() {
    let mut board = board();
    let id = board.child;
    open_page(&mut board, id);
    press(&mut board, key(KeyCode::Char('P'))).expect("P maps");
    type_text(&mut board, "ship");
    let baseline = board.domain.clone();
    let mut recovery = SaveRecovery::new();
    let outcome = apply_board_intent_with_save_recovery(
        &mut board.domain,
        &mut board.model,
        &mut recovery,
        BoardSaveContext {
            baseline: baseline.clone(),
            intent: BoardIntent::ConfirmParentPicker,
            snapshot: None,
        },
        |_| Err("disk full".to_string()),
    )
    .expect("recovery owns the failure");
    assert_eq!(outcome, IntentOutcome::None);
    assert!(recovery.is_pending());
    assert_eq!(board.model.input_mode(), BoardInputMode::SaveRecovery);

    let outcome = apply_board_intent_with_save_recovery(
        &mut board.domain,
        &mut board.model,
        &mut recovery,
        BoardSaveContext {
            baseline: DomainState::new(),
            intent: BoardIntent::CancelSave,
            snapshot: None,
        },
        |_| Ok(()),
    )
    .expect("cancel");
    assert_eq!(outcome, IntentOutcome::None);
    assert!(!recovery.is_pending());
    assert_eq!(
        board.domain.get(board.child).expect("child").parent_task_id,
        None
    );
    assert_eq!(board.model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(board.model.edit_target(), Some(board.child));
}
