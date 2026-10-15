//! BoardModel cutover onto queue query, selection anchor, and session-only state.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::BTreeSet;
use tsk_tui::context::InvocationSnapshot;
use tsk_tui::domain::{
    DomainState, HumanStatus, ProvenanceOrigin, Task, TaskEvent, TaskEventKind, TaskScope,
};
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{apply_intent, draw_board, BoardModel};
use tsk_tui::ui::input::BoardIntent;
use tsk_tui::ui::queue::{
    query_board, query_lens, BoardLens, BoardSort, QueueView, SectionKind, INBOX_HEADER_ROW_ID,
};
use uuid::Uuid;

const THIS_REPO: &str = "/repos/app";

fn epoch_plus(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn task(id: u128, title: &str, status: HumanStatus, scope: TaskScope, updated_secs: u64) -> Task {
    let at = epoch_plus(updated_secs);
    Task {
        id: Uuid::from_u128(id),
        number: None,
        parent_task_id: None,
        notice: None,
        revision: Uuid::from_u128(id),
        merge_base_revision: None,
        title: title.into(),
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

fn project(path: &str) -> TaskScope {
    TaskScope::Project {
        path: path.to_string(),
    }
}

fn domain_with_tasks(tasks: Vec<Task>) -> DomainState {
    let mut document = serde_json::to_value(DomainState::new()).expect("serialize empty domain");
    document["tasks"] = serde_json::to_value(tasks).expect("serialize tasks");
    serde_json::from_value(document).expect("domain with fixture tasks")
}

fn section_ids(view: &QueueView, kind: SectionKind) -> Vec<Uuid> {
    view.sections
        .iter()
        .filter(|section| section.kind == kind)
        .flat_map(|section| section.task_ids.iter().copied())
        .collect()
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-queue-board-model-{tag}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

struct TempGuard(PathBuf);
impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn list_files_recursive(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// First open pins the first NEEDS YOU / IN MOTION / ON DECK row of the startup
/// destination: the invocation project board, or desk with no project context.
#[test]
fn from_domain_seeds_selection_on_first_needs_you_else_motion_else_deck_row() {
    // Case A: startup in a repo opens its project board and seeds by lane order.
    let motion_newer = task(1, "motion-new", HumanStatus::Doing, project(THIS_REPO), 100);
    let motion_older = task(2, "motion-old", HumanStatus::Doing, project(THIS_REPO), 50);
    let deck = task(3, "deck", HumanStatus::Todo, project(THIS_REPO), 200);
    let tasks = vec![motion_newer.clone(), motion_older.clone(), deck.clone()];
    let model = BoardModel::from_tasks(tasks.clone(), Some(PathBuf::from(THIS_REPO)));
    // The board orders unnumbered rows by capture order: the older motion task leads.
    let view = query_board(
        &tasks,
        &BTreeSet::new(),
        BoardLens::All,
        false,
        BoardSort::Number,
    );
    let first_motion = view
        .sections
        .iter()
        .find(|s| s.kind == SectionKind::InMotion)
        .and_then(|s| s.task_ids.first().copied());
    assert_eq!(first_motion, Some(Uuid::from_u128(2)));
    assert_eq!(
        model.selected_id(),
        first_motion,
        "open seeds the first IN MOTION row"
    );

    // from_domain shares the same seed rule.
    let mut domain = DomainState::new();
    let id_new = domain
        .create(
            "motion-new",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .unwrap();
    domain.set_status(id_new, HumanStatus::Doing).unwrap();
    let id_old = domain
        .create(
            "motion-old",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .unwrap();
    domain.set_status(id_old, HumanStatus::Doing).unwrap();
    let _deck = domain
        .create("deck", None, project(THIS_REPO), ProvenanceOrigin::Manual)
        .unwrap();
    // Domain create stamps wall-clock times; seed still lands on some IN MOTION id.
    let from_domain = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let selected = from_domain.selected_id().expect("selection");
    assert_eq!(
        domain.get(selected).unwrap().status,
        HumanStatus::Doing,
        "from_domain seeds an IN MOTION task when any exist"
    );

    // Case B: a ready-only project seeds its first ON DECK row.
    let deck_a = task(10, "a", HumanStatus::Todo, project("/repos/a"), 30);
    let deck_b = task(11, "b", HumanStatus::Todo, project(THIS_REPO), 40);
    let deck_tasks = vec![deck_a.clone(), deck_b.clone()];
    let view = query_lens(&deck_tasks, BoardLens::Project(Path::new(THIS_REPO)), false);
    let first_deck = view
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::OnDeck)
        .flat_map(|s| s.task_ids.iter().copied())
        .next();
    assert_eq!(first_deck, Some(Uuid::from_u128(11)));

    let mut domain = DomainState::new();
    let deck_id = domain
        .create("b", None, project(THIS_REPO), ProvenanceOrigin::Manual)
        .unwrap();
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(
        model.active_project(),
        None,
        "startup stays on every project"
    );
    assert_eq!(
        model.selected_id(),
        Some(deck_id),
        "startup seeds the invocation project's first row"
    );

    // Case C: empty → no selection.
    let empty = BoardModel::from_tasks(vec![], Some(PathBuf::from(THIS_REPO)));
    assert_eq!(empty.selected_id(), None);
}

/// Directory-aware startup: the board always opens on every project; a live project seeds
/// the cursor on its group, an archived project stays hidden behind the launch card,
/// absent project context means desk.
#[test]
fn from_domain_seeds_on_the_invocation_project_unless_archived() {
    let mut domain = DomainState::new();
    let live_id = domain
        .create("live", None, project(THIS_REPO), ProvenanceOrigin::Manual)
        .unwrap();
    domain
        .create(
            "elsewhere",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
        )
        .unwrap();

    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(
        model.selected_project(),
        Some(Path::new(THIS_REPO)),
        "launch inside a repo remembers that project"
    );
    assert_eq!(
        model.active_project(),
        None,
        "the board stays on every project"
    );
    assert_eq!(model.visible_ids().len(), 2, "every live project is listed");
    assert_eq!(
        model.selected_id(),
        Some(live_id),
        "the cursor starts on the invocation project's row"
    );

    // Empty project: same destination, useful empty state.
    let empty_repo = format!("{THIS_REPO}-empty");
    let model = BoardModel::from_domain(&DomainState::new(), Some(PathBuf::from(&empty_repo)));
    assert_eq!(
        model.selected_project(),
        Some(Path::new(empty_repo.as_str()))
    );
    assert!(model.visible_ids().is_empty());
    let view = model.queue_view();
    assert!(
        view.sections
            .iter()
            .any(|section| section.kind == SectionKind::OnDeck && section.empty_hint),
        "an empty project board keeps a hinted deck section"
    );

    // Archived invocation repo: the launch card owns it; the board stays on the desk.
    let mut domain = DomainState::new();
    let archived_id = domain
        .create(
            "archived repo task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .unwrap();
    domain.archive_project(THIS_REPO).expect("archive project");
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(model.selected_project(), None);
    assert_eq!(model.active_project(), None, "every scope opens instead");
    assert!(!model.visible_ids().contains(&archived_id));

    // A non-Git invocation remembers its directory but opens every scope.
    let mut domain = DomainState::new();
    let desk_id = domain
        .create("desk", None, TaskScope::Global, ProvenanceOrigin::Manual)
        .unwrap();
    let outside = PathBuf::from("/work/outside-git");
    let snapshot = InvocationSnapshot {
        default_scope: TaskScope::Global,
        this_repo: Some(outside.clone()),
        title_prefill: None,
        provenance: ProvenanceOrigin::Manual,
    };
    let model = BoardModel::from_domain_for_snapshot(&domain, &snapshot);
    assert_eq!(model.active_project(), None);
    assert_eq!(model.selected_project(), Some(outside.as_path()));
    assert_eq!(model.selected_id(), Some(desk_id));
}

/// Startup in a repo keeps every project on the board with the cursor on that project's
/// row; pinning another project's scope narrows the queue to it.
#[test]
fn pinning_a_project_scope_changes_visible_queue_sections() {
    let mut domain = DomainState::new();
    let app_id = domain
        .create(
            "app task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .unwrap();
    let other_id = domain
        .create(
            "other task",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
        )
        .unwrap();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let visible = model.visible_ids();
    assert!(
        visible.contains(&app_id) && visible.contains(&other_id),
        "startup keeps every project on the board: {visible:?}"
    );
    assert_eq!(
        model.selected_id(),
        Some(app_id),
        "startup seeds the cursor on the invocation project's row"
    );

    model.set_selected_project(Some(PathBuf::from("/repos/other")));

    assert_eq!(model.selected_project(), Some(Path::new("/repos/other")));
    assert_eq!(model.visible_ids(), vec![other_id]);
}

/// After a domain sync, selection stays on the same id when it remains visible.
#[test]
fn sync_from_domain_reanchors_by_id() {
    let mut domain = DomainState::new();
    let id_doing = domain
        .create("doing", None, project(THIS_REPO), ProvenanceOrigin::Manual)
        .unwrap();
    domain.set_status(id_doing, HumanStatus::Doing).unwrap();
    let id_todo = domain
        .create("todo", None, project(THIS_REPO), ProvenanceOrigin::Manual)
        .unwrap();

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let visible = model.visible_ids();
    assert!(visible.contains(&id_doing));
    assert!(visible.contains(&id_todo));
    let todo_idx = visible.iter().position(|&id| id == id_todo).unwrap();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(todo_idx),
        None,
    )
    .unwrap();
    assert_eq!(model.selected_id(), Some(id_todo));

    // Sync with the same snapshot: selection stays on todo by id.
    model.sync_from_domain(&domain);
    assert_eq!(
        model.selected_id(),
        Some(id_todo),
        "reanchor keeps the same id when still visible"
    );

    // Soft-delete the selected task; reanchor must leave todo and land on a survivor.
    domain.soft_delete(id_todo).unwrap();
    model.sync_from_domain(&domain);
    assert_eq!(
        model.selected_id(),
        Some(id_doing),
        "when the pinned id leaves the visible set, reanchor lands on a survivor"
    );
    assert!(!model.visible_ids().contains(&id_todo));
}

/// Completing the last ready row must not reanchor onto the inbox heading that follows it.
#[test]
fn ready_task_leaving_view_reanchors_to_neighbor_not_inbox_header() {
    let mut domain = DomainState::new();
    let first_ready = domain
        .create(
            "first ready",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .unwrap();
    domain.set_status(first_ready, HumanStatus::Todo).unwrap();
    let last_ready = domain
        .create(
            "last ready",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .unwrap();
    domain.set_status(last_ready, HumanStatus::Todo).unwrap();
    let open = domain
        .create(
            "inbox neighbor",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .unwrap();

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let visible = model.visible_ids();
    let last_ready_index = visible
        .iter()
        .position(|&id| id == last_ready)
        .expect("last ready row");
    assert!(!visible.contains(&INBOX_HEADER_ROW_ID));
    assert_eq!(visible[last_ready_index + 1], open);
    assert!(visible.contains(&open));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(last_ready_index),
        None,
    )
    .unwrap();

    apply_intent(&mut domain, &mut model, BoardIntent::Complete, None).unwrap();

    assert_eq!(model.selected_id(), Some(first_ready));
    assert_ne!(model.selected_id(), None);
    assert!(model.visible_ids().contains(&open));
    assert!(!model.visible_ids().contains(&last_ready));
}

#[test]
fn selection_stays_on_task_id_across_motion_reorder() {
    // Beta is numbered and alpha is not yet, so beta leads the number order.
    let mut alpha_task = task(1, "alpha", HumanStatus::Doing, project(THIS_REPO), 10);
    alpha_task.history.push(TaskEvent {
        kind: TaskEventKind::StatusSet,
        at: epoch_plus(100),
    });
    let alpha = alpha_task.id;
    let mut beta_task = task(2, "beta", HumanStatus::Doing, project(THIS_REPO), 20);
    beta_task.number = Some(5);
    let beta = beta_task.id;
    let mut domain = domain_with_tasks(vec![alpha_task.clone(), beta_task.clone()]);
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));

    let before = model.queue_view();
    assert_eq!(
        section_ids(&before, SectionKind::InMotion),
        vec![beta, alpha]
    );

    let beta_index = model
        .visible_ids()
        .iter()
        .position(|id| *id == beta)
        .expect("beta is visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(beta_index),
        None,
    )
    .unwrap();

    // The persistence boundary numbers alpha below beta, which moves it above beta;
    // selection stays pinned to beta's id, not to beta's row position.
    alpha_task.number = Some(2);
    alpha_task.updated_at = epoch_plus(300);
    let reordered = domain_with_tasks(vec![alpha_task, beta_task]);
    model.sync_from_domain(&reordered);

    let after = model.queue_view();
    assert_eq!(
        section_ids(&after, SectionKind::InMotion),
        vec![alpha, beta],
        "the lower task number leads IN MOTION"
    );
    assert_eq!(model.selected_id(), Some(beta));
    assert_eq!(
        model.visible_ids(),
        section_ids(&after, SectionKind::InMotion)
    );
    assert_eq!(model.selected_index(), Some(1));
    assert_eq!(
        section_ids(&after, SectionKind::InMotion)[model.selected_index().unwrap()],
        beta
    );
}

/// Two fresh models from the same store share no UI state and write no UI-state files.
fn board_rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn arrow_navigation_crosses_painted_header_task_to_task() {
    let mut domain = DomainState::new();
    let review = domain
        .create(
            "review task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .expect("create review");
    domain
        .set_status(review, HumanStatus::Doing)
        .expect("doing");
    let ready = domain
        .create(
            "ready task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
        )
        .expect("create ready");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    let ids = model.visible_ids();
    assert_eq!(
        ids,
        vec![review, ready],
        "needs-you first, then on deck and its inbox heading"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(0), None)
        .expect("select first task");

    let rows = board_rows(&model, 80, 24);
    let first_y = rows
        .iter()
        .position(|row| row.contains("review task"))
        .expect("first task paints");
    let header_y = rows
        .iter()
        .position(|row| row.contains("DOING"))
        .expect("the NEEDS YOU header paints");
    let second_y = rows
        .iter()
        .position(|row| row.contains("ready task"))
        .expect("second task paints");
    assert!(
        header_y < first_y && first_y < second_y,
        "a painted header must physically sit above its tasks:\n{}",
        rows.join("\n")
    );

    apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None)
        .expect("arrow navigation moves to the inbox heading");
    assert!(!model.inbox_header_selected());
    assert_eq!(
        model.selected_id(),
        Some(ready),
        "selection must land on the task below the inbox heading"
    );
}

#[test]
fn two_fresh_models_from_same_store_share_no_ui_state_and_no_ui_writes_under_state_or_config_dirs()
{
    let state_dir = temp_dir("state");
    let config_dir = temp_dir("config");
    let _g1 = TempGuard(state_dir.clone());
    let _g2 = TempGuard(config_dir.clone());

    let store = TaskStore::new(&state_dir);
    let mut domain = DomainState::new();
    let id = domain
        .create("shared", None, project(THIS_REPO), ProvenanceOrigin::Manual)
        .unwrap();
    store.save(&domain).expect("save domain");

    let before_state = list_files_recursive(&state_dir);
    let before_config = list_files_recursive(&config_dir);

    let mut a = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let b = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(a.selected_id(), Some(id));
    assert_eq!(b.selected_id(), Some(id));

    // Mutate session-only UI on A; a fresh B must not inherit it.
    a.set_message("only on A");
    let _ = apply_intent(&mut domain, &mut a, BoardIntent::ToggleDoneDrawer, None);

    let b_fresh = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(
        b_fresh.message(),
        None,
        "fresh model has no carried message"
    );
    assert_eq!(
        b_fresh.selected_id(),
        Some(id),
        "fresh model re-seeds; does not inherit A's session UI"
    );
    assert_eq!(a.message(), Some("only on A"));
    assert!(a.drawer_open(), "A's drawer mutation stays in this session");
    assert!(
        !b_fresh.drawer_open(),
        "fresh model starts with drawer closed"
    );
    assert_eq!(
        b_fresh.selected_project(),
        Some(Path::new(THIS_REPO)),
        "fresh model starts on the invocation project's board"
    );
    assert_eq!(
        b_fresh.visible_ids(),
        vec![id],
        "the startup board renders that project's rows and inbox heading"
    );

    let after_state = list_files_recursive(&state_dir);
    let after_config = list_files_recursive(&config_dir);
    assert_eq!(
        before_state, after_state,
        "BoardModel must not write UI-state files under the task store dir"
    );
    assert_eq!(
        before_config, after_config,
        "BoardModel must not write UI-state files under the config dir"
    );
}
