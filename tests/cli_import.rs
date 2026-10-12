//! `tsk import linear` through the real binary, against an isolated `TSK_STATE_DIR` and
//! the `TSK_LINEAR_FIXTURE` test hook in place of Linear's HTTP API.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tsk_tui::domain::HumanStatus;
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tsk-cli-import-{label}-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Two pages, so the binary follows a cursor.
const FIXTURE: &str = r#"[
  {"data": {"issues": {
    "nodes": [
      {"identifier": "ENG-2654", "title": "tsk fork: import Linear issues",
       "url": "https://linear.app/acme/issue/ENG-2654/import",
       "description": "Pull assigned issues onto the board.",
       "state": {"name": "In Progress", "type": "started"}, "team": {"key": "ENG"}},
      {"identifier": "ENG-2655", "title": "Review importer",
       "url": "https://linear.app/acme/issue/ENG-2655/review", "description": null,
       "state": {"name": "In Review", "type": "started"}, "team": {"key": "ENG"}}
    ],
    "pageInfo": {"hasNextPage": true, "endCursor": "page-2"}}}},
  {"data": {"issues": {
    "nodes": [
      {"identifier": "ENG-2656", "title": "Dropped",
       "url": "https://linear.app/acme/issue/ENG-2656/dropped", "description": "",
       "state": {"name": "Canceled", "type": "canceled"}, "team": {"key": "ENG"}},
      {"identifier": "ENG-2657", "title": "Backlog item",
       "url": "https://linear.app/acme/issue/ENG-2657/backlog", "description": "Later.",
       "state": {"name": "Backlog", "type": "backlog"}, "team": {"key": "ENG"}}
    ],
    "pageInfo": {"hasNextPage": false, "endCursor": null}}}}
]"#;

fn tsk(state: &Path, fixture: Option<&Path>, key: bool, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tsk"));
    command
        .args(args)
        .env("TSK_STATE_DIR", state)
        .env_remove("LINEAR_API_KEY")
        .env_remove("TSK_LINEAR_FIXTURE");
    if key {
        command.env("LINEAR_API_KEY", "lin_api_test");
    }
    if let Some(fixture) = fixture {
        command.env("TSK_LINEAR_FIXTURE", fixture);
    }
    command.output().expect("run tsk")
}

fn setup(label: &str) -> (TempDir, PathBuf, PathBuf) {
    let root = TempDir::new(label);
    let state = root.0.join("state");
    fs::create_dir_all(&state).unwrap();
    let fixture = root.0.join("linear.json");
    fs::write(&fixture, FIXTURE).unwrap();
    (root, state, fixture)
}

#[test]
fn import_creates_once_and_reports_json() {
    let (_root, state, fixture) = setup("create");
    let output = tsk(
        &state,
        Some(&fixture),
        true,
        &["import", "linear", "--desk", "--json"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("json report");
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["fetched"], 4);
    assert_eq!(report["created"].as_array().unwrap().len(), 3);
    assert_eq!(report["skipped"][0]["identifier"], "ENG-2656");
    assert_eq!(report["skipped"][0]["state"], "canceled");
    assert_eq!(report["created"][0]["identifier"], "ENG-2654");
    assert_eq!(report["created"][0]["number"], 1);
    assert_eq!(report["created"][0]["status"], "started");
    assert_eq!(report["created"][1]["status"], "review");
    assert_eq!(report["created"][2]["status"], "open");

    let tasks = TaskStore::new(&state).load().unwrap();
    let first = tasks
        .tasks()
        .iter()
        .find(|task| task.title == "ENG-2654 tsk fork: import Linear issues")
        .expect("imported task");
    assert_eq!(first.status, HumanStatus::Started);
    assert_eq!(
        first.notes.as_deref(),
        Some("Linear: ENG-2654 https://linear.app/acme/issue/ENG-2654/import\n\nPull assigned issues onto the board.")
    );

    let again = tsk(
        &state,
        Some(&fixture),
        true,
        &["import", "linear", "--desk"],
    );
    assert_eq!(again.status.code(), Some(0));
    let human = String::from_utf8(again.stdout).unwrap();
    assert!(
        human.starts_with("fetched 4 · created 0 · updated 0 · unchanged 3 · skipped 1\n"),
        "{human}"
    );
    assert_eq!(TaskStore::new(&state).load().unwrap().tasks().len(), 3);
}

#[test]
fn dry_run_reports_without_writing() {
    let (_root, state, fixture) = setup("dry");
    let output = tsk(
        &state,
        Some(&fixture),
        true,
        &["import", "linear", "--desk", "--dry-run"],
    );
    assert_eq!(output.status.code(), Some(0));
    let human = String::from_utf8(output.stdout).unwrap();
    assert!(human.starts_with("dry run, nothing written\n"), "{human}");
    assert!(human.contains("would create 3"), "{human}");
    assert!(!state.join("tsk.json").exists());
}

#[test]
fn missing_key_refuses_with_exit_one_and_writes_nothing() {
    let (_root, state, fixture) = setup("nokey");
    let output = tsk(
        &state,
        Some(&fixture),
        false,
        &["import", "linear", "--desk"],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with("tsk import: missing-api-key: "),
        "{stderr}"
    );
    assert!(!state.join("tsk.json").exists());
}

#[test]
fn graphql_error_refuses_with_exit_one() {
    let (root, state, _) = setup("gqlerr");
    let fixture = root.0.join("error.json");
    fs::write(&fixture, r#"{"errors": [{"message": "Invalid filter"}]}"#).unwrap();
    let output = tsk(
        &state,
        Some(&fixture),
        true,
        &["import", "linear", "--desk"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "tsk import: linear-error: Invalid filter\n"
    );
    assert!(!state.join("tsk.json").exists());
}

#[test]
fn bad_flags_are_usage_errors() {
    let (_root, state, fixture) = setup("usage");
    for args in [
        &["import"][..],
        &["import", "linear", "--state", "done"],
        &["import", "linear", "--limit", "nope"],
    ] {
        let output = tsk(&state, Some(&fixture), true, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .starts_with("tsk import: "));
    }
}
