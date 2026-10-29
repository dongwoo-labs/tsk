//! The app boundary of the reports tab: `ReportStore` reads and the one write it owns
//! (`mark_read`). The pure tab state lives in [`crate::ui::reports`].
//!
//! Opening a document acknowledges it once a frame has shown it whole: `mark_read` names the
//! exact revision the page snapshotted (never whatever the feed holds later), so a newer
//! registration that lands meanwhile is refused as stale rather than silently marked read.
//! Selecting a row, a list preview, a feed refresh, and a document too long to display never
//! write.

use std::path::Path;

use uuid::Uuid;

use crate::store::{ReportError, ReportStore};
use crate::ui::board::BoardModel;
use crate::ui::reports::{MarkReadFailure, ReportFeed, ReportIntent};

/// Read the board's completion reports once from the store at `state_dir`. A read failure is
/// an unavailable feed, never an empty one.
pub fn load_feed(state_dir: &Path) -> ReportFeed {
    match ReportStore::new(state_dir).list() {
        Ok(records) => ReportFeed::Loaded(records),
        Err(error) => ReportFeed::Unavailable(error.to_string()),
    }
}

/// One reports-tab action against the store at `state_dir`.
pub fn handle(model: &mut BoardModel, state_dir: &Path, intent: ReportIntent) {
    match intent {
        ReportIntent::Move(forward) => model.reports_mut().move_cursor(forward),
        ReportIntent::Scroll(forward) => model.reports().scroll_page(forward),
        ReportIntent::ScrollLine(forward) => model.reports().scroll_line(forward),
        ReportIntent::Open => {
            if let Some(id) = model.reports().cursor {
                model.reports_mut().open(id);
            }
        }
        ReportIntent::Row(id) => model.reports_mut().open(id),
        ReportIntent::Back => model.reports_mut().back(),
        ReportIntent::RetryMarkRead => acknowledge(model, &ReportStore::new(state_dir)),
    }
}

/// After a paint: acknowledge the open page's snapshot if that paint showed the whole
/// document. A document past the page's display limit stays unread (the page says why), and
/// so does a page no frame has painted yet.
pub fn acknowledge_painted(model: &mut BoardModel, state_dir: &Path) {
    let reports = model.reports();
    if !reports.active || !reports.ack_pending || reports.page_fits.get() != Some(true) {
        return;
    }
    model.reports_mut().ack_pending = false;
    acknowledge(model, &ReportStore::new(state_dir));
}

/// Mark exactly the open page's snapshot revision read, then refresh the feed so the tab
/// count and board markers follow. An already-read snapshot writes nothing. A stale revision
/// (a newer registration landed since the page opened) or an I/O failure stays on the page
/// -- never reported as success, never retried silently; `Enter` retries it explicitly.
fn acknowledge(model: &mut BoardModel, store: &ReportStore) {
    let Some(snapshot) = model.reports().page_snapshot.clone() else {
        return;
    };
    if !snapshot.unread() {
        model.reports_mut().mark_read_failure = None;
        return;
    }
    let task_id = snapshot.task_id;
    match store.mark_read(task_id, snapshot.revision) {
        Ok(updated) => {
            model.reports_mut().page_snapshot = Some(updated);
            model.reports_mut().mark_read_failure = None;
        }
        Err(error) => {
            model.reports_mut().mark_read_failure = Some(MarkReadFailure {
                task_id,
                message: mark_read_message(error),
            });
        }
    }
    model.set_report_feed(match store.list() {
        Ok(records) => ReportFeed::Loaded(records),
        Err(error) => ReportFeed::Unavailable(error.to_string()),
    });
}

fn mark_read_message(error: ReportError) -> String {
    match error {
        ReportError::StaleRevision => {
            "a newer revision landed \u{b7} esc and reopen to read it".to_string()
        }
        other => other.to_string(),
    }
}

/// Open a task's report from the board or its clean task page (`r`). It changes tab exactly
/// the way `3` does, so an unsaved edit refuses it with nothing opened or acknowledged; the
/// shown revision is then acknowledged after paint, like the tab's own `Enter`.
pub fn open_from_task(model: &mut BoardModel, state_dir: &Path, task_id: Uuid) {
    model.set_report_feed(load_feed(state_dir));
    open_from_task_with_feed(model, task_id);
}

/// The testable half of [`open_from_task`]: assumes the feed is already loaded, so a test can
/// seed it directly. Returns whether a page opened.
fn open_from_task_with_feed(model: &mut BoardModel, task_id: Uuid) -> bool {
    let Some(record) = model.task_report(task_id).cloned() else {
        model.set_message("no completion report on this task");
        return false;
    };
    if !model.enter_reports_tab() {
        return false;
    }
    let reports = model.reports_mut();
    reports.return_task = Some(task_id);
    reports.open_record(record);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::run_with;
    use crate::store::TaskStore;
    use std::io;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn seeded_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "tsk-report-actions-{}-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn mark_read_refuses_a_stale_snapshot_without_marking_the_newer_revision() {
        let dir = seeded_dir();
        let output = run_with(
            [
                "tsk",
                "add",
                "-t",
                "report task",
                "--desk",
                "--state-dir",
                dir.to_str().unwrap(),
            ],
            io::empty(),
            true,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        let id = TaskStore::new(&dir).load().unwrap().tasks()[0].id;
        let store = ReportStore::new(&dir);
        let first = store
            .register(crate::store::TrashTarget::Id(id), "first".into())
            .unwrap();
        // A second revision lands after the page would have opened on the first.
        store
            .register(crate::store::TrashTarget::Id(id), "second".into())
            .unwrap();

        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.reports_mut().page = Some(id);
        model.reports_mut().page_snapshot = Some(first);
        acknowledge(&mut model, &store);

        assert!(
            model.reports().mark_read_failure.is_some(),
            "stale mark_read must surface a failure, not silent success"
        );
        assert!(
            store.read(id).unwrap().unread(),
            "the newer revision stays unread"
        );
    }

    #[test]
    fn opening_acknowledges_the_shown_revision_and_selecting_does_not() {
        let dir = seeded_dir();
        for title in ["first task", "second task"] {
            let output = run_with(
                [
                    "tsk",
                    "add",
                    "-t",
                    title,
                    "--desk",
                    "--state-dir",
                    dir.to_str().unwrap(),
                ],
                io::empty(),
                true,
            );
            assert_eq!(output.code, 0, "{}", output.stderr);
        }
        let ids: Vec<Uuid> = TaskStore::new(&dir)
            .load()
            .unwrap()
            .tasks()
            .iter()
            .map(|task| task.id)
            .collect();
        let store = ReportStore::new(&dir);
        for id in &ids {
            store
                .register(crate::store::TrashTarget::Id(*id), "# Done\n\nbody".into())
                .unwrap();
        }
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.set_report_feed(load_feed(&dir));
        model.reports_mut().active = true;

        handle(&mut model, &dir, ReportIntent::Move(true));
        let selected = model.reports().cursor.unwrap();
        assert!(
            ids.iter().all(|id| store.read(*id).unwrap().unread()),
            "a selection alone acknowledges nothing"
        );

        handle(&mut model, &dir, ReportIntent::Open);
        acknowledge_painted(&mut model, &dir);
        assert!(
            store.read(selected).unwrap().unread(),
            "nothing is acknowledged before a frame has shown the page"
        );
        paint(&model, 110, 30);
        acknowledge_painted(&mut model, &dir);
        let opened = store.read(selected).unwrap();
        assert_eq!(opened.read_revision, Some(1), "opening acknowledges rev 1");
        assert!(!opened.unread);
        assert_eq!(
            crate::ui::reports::tab_count(&model.reports().feed),
            Some(1),
            "the tab count follows the acknowledgement without a reload key"
        );
        assert!(
            model.reports().page_failure().is_none(),
            "a successful acknowledgement shows no failure"
        );
    }

    fn paint(model: &BoardModel, width: u16, height: u16) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                crate::ui::board::draw_board(frame, model);
            })
            .unwrap();
    }

    /// A document that wraps past `tui-scrollview`'s `u16` row canvas at the page width must
    /// say so and stay unread, instead of wrapping its row count around, cutting the body,
    /// and acknowledging it. Blank-separated paragraphs cross the limit with a source line
    /// count just above it, so the check has to be on wrapped rows.
    #[test]
    fn a_document_past_the_display_limit_stays_unread_and_says_why() {
        let dir = seeded_dir();
        let output = run_with(
            [
                "tsk",
                "add",
                "-t",
                "huge",
                "--desk",
                "--state-dir",
                dir.to_str().unwrap(),
            ],
            io::empty(),
            true,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        let id = TaskStore::new(&dir).load().unwrap().tasks()[0].id;
        let mut markdown = String::new();
        for index in 0..33_000 {
            markdown.push_str(&format!("p {index:05}\n\n"));
        }
        let store = ReportStore::new(&dir);
        store
            .register(crate::store::TrashTarget::Id(id), markdown.clone())
            .unwrap();
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.set_report_feed(load_feed(&dir));
        model.reports_mut().active = true;
        handle(&mut model, &dir, ReportIntent::Open);

        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                crate::ui::board::draw_board(frame, &model);
            })
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("65535"), "{screen}");
        assert!(screen.contains("stays unread"), "{screen}");
        acknowledge_painted(&mut model, &dir);
        let record = store.read(id).unwrap();
        assert!(
            record.unread,
            "a document the page could not show stays unread"
        );
        assert_eq!(
            record.markdown, markdown,
            "the stored document is untouched"
        );
    }

    #[test]
    fn open_from_task_opens_the_tab_on_that_tasks_report_and_remembers_the_return() {
        let id = Uuid::new_v4();
        let record = crate::store::ReportRecord {
            task_id: id,
            task_number: 1,
            task_title: "report task".into(),
            project: None,
            revision: 1,
            markdown: "done".into(),
            read_revision: None,
            unread: true,
            revisions: vec![],
        };
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.set_report_feed(ReportFeed::Loaded(vec![record]));
        open_from_task_with_feed(&mut model, id);
        assert!(
            model.reports().active,
            "opening from a task activates tab 3"
        );
        assert_eq!(model.reports().page, Some(id));
        assert_eq!(model.reports().return_task, Some(id));
    }

    #[test]
    fn open_from_task_with_no_report_leaves_the_tab_closed_and_says_so() {
        let id = Uuid::new_v4();
        let mut model = BoardModel::from_tasks(Vec::new(), None);
        model.set_report_feed(ReportFeed::Loaded(Vec::new()));
        open_from_task_with_feed(&mut model, id);
        assert!(!model.reports().active);
        assert_eq!(model.reports().page, None);
        assert_eq!(model.message(), Some("no completion report on this task"));
    }
}
