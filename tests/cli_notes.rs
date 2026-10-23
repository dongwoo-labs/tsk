use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct Board(PathBuf);

impl Board {
    fn new(notes: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let board = Self(std::env::temp_dir().join(format!("tsk-cli-notes-{nanos}-{seq}")));
        board.ok(&["add", "--desk", "-t", "notes target", "--notes", notes]);
        board
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tsk"));
        command.args(args).arg("--state-dir").arg(&self.0);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run tsk")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        String::from_utf8(output.stdout).unwrap()
    }

    fn detail(&self) -> Value {
        serde_json::from_str::<Value>(&self.ok(&["list", "T1", "--json"])).unwrap()[0].clone()
    }
}

impl Drop for Board {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn notes_set_remove_append_preserve_other_lines_and_repeat_without_writes() {
    let board = Board::new(
        "Goal: ship\r\n  - now : old\r\n\r\nNowhere: keep\nNOW: duplicate\nThread: agent w t p",
    );
    assert_eq!(
        board.ok(&["notes", "t1", "set", "Now:", "  running CI  "]),
        "notes T1 notes target\n"
    );
    assert_eq!(
        board.detail()["notes"],
        "Goal: ship\r\nNow: running CI\r\n\r\nNowhere: keep\nThread: agent w t p"
    );
    let before = fs::read(board.0.join("tsk.json")).unwrap();
    board.ok(&["notes", "1", "set", "Now:", "running CI"]);
    board.ok(&["notes", "T1", "remove", "Blocked:"]);
    assert_eq!(fs::read(board.0.join("tsk.json")).unwrap(), before);
    board.ok(&["notes", "T1", "remove", "now:"]);
    board.ok(&["notes", "T1", "set", "Next:", "merge"]);
    // `--` must leave dash-leading text as data, not flags. State dir goes before it.
    let output = board
        .command(&["notes", "T1", "append"])
        .args(["--", "- keep\tthis"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    board.ok(&["notes", "T1", "append", "reference"]);
    board.ok(&["notes", "T1", "append", "reference"]);
    assert_eq!(board.detail()["notes"], "Goal: ship\r\n\r\nNowhere: keep\nThread: agent w t p\nNext: merge\n- keep\tthis\nreference\nreference");
    let uuid = board.detail()["id"].as_str().unwrap().to_string();
    board.ok(&["notes", &uuid, "set", "Done when:", "all checks pass"]);
    assert_eq!(
        board.ok(&["list", "T1", "--note", "Done when:", "--json"]),
        "\"all checks pass\"\n"
    );
}

#[test]
fn notes_match_only_one_optional_bullet() {
    let original = "Goal: keep\n- - Now: literal\nNow: old";
    let board = Board::new(original);
    let projected = board.ok(&["list", "T1", "--note", "Now:"]);
    board.ok(&["notes", "T1", "set", "Now:", "new"]);
    let after_set = board.detail()["notes"].clone();
    board.ok(&["edit", "T1", "--notes", original]);
    board.ok(&["notes", "T1", "remove", "Now:"]);
    let after_remove = board.detail()["notes"].clone();
    assert_eq!(
        (projected, after_set, after_remove),
        (
            "old\n".to_string(),
            json!("Goal: keep\n- - Now: literal\nNow: new"),
            json!("Goal: keep\n- - Now: literal\n"),
        )
    );
}

#[test]
fn notes_empty_and_trailing_newline_boundaries() {
    let board = Board::new("");
    board.ok(&["notes", "T1", "set", "Now:", "one"]);
    assert_eq!(board.detail()["notes"], "Now: one");
    board.ok(&["notes", "T1", "remove", "Now:"]);
    assert_eq!(board.detail()["notes"], Value::Null);
    board.ok(&["edit", "T1", "--notes", "Now: one\r\n"]);
    board.ok(&["notes", "T1", "set", "Now:", "two"]);
    assert_eq!(board.detail()["notes"], "Now: two\r\n");
    board.ok(&["notes", "T1", "append", "Next: three"]);
    assert_eq!(board.detail()["notes"], "Now: two\r\nNext: three");
    board.ok(&["notes", "T1", "remove", "Now:"]);
    board.ok(&["notes", "T1", "remove", "Next:"]);
    assert_eq!(board.detail()["notes"], Value::Null);
}

#[test]
fn list_projects_one_field_or_note_without_task_array() {
    let board = Board::new(
        "  - nOw : first value\nNow: second\nEmpty:   \nPR: https://example.test/pull/1",
    );
    let detail = board.detail();
    for (field, expected) in detail.as_object().unwrap() {
        let output = board.ok(&["list", "T1", "--field", field, "--json"]);
        assert_eq!(
            serde_json::from_str::<Value>(&output).unwrap(),
            *expected,
            "{field}"
        );
    }
    assert_eq!(
        board.ok(&["list", "T1", "--field=id"]),
        format!("{}\n", detail["id"].as_str().unwrap())
    );
    assert_eq!(board.ok(&["list", "T1", "--field", "number"]), "1\n");
    assert_eq!(board.ok(&["list", "T1", "--field", "project"]), "\n");
    assert_eq!(
        board.ok(&["list", "T1", "--field", "waiting_for", "--json"]),
        "[]\n"
    );
    assert_eq!(board.ok(&["list", "T1", "--field", "steps"]), "[]\n");
    assert_eq!(
        board.ok(&["list", "T1", "--field", "notes"]),
        format!("{}\n", detail["notes"].as_str().unwrap())
    );
    assert_eq!(board.ok(&["list", "T1", "--note=NOW:"]), "first value\n");
    assert_eq!(
        board.ok(&["list", "T1", "--note", "PR:", "--json"]),
        "\"https://example.test/pull/1\"\n"
    );
    assert_eq!(
        board.ok(&["list", "T1", "--note", "Absent:", "--json"]),
        "null\n"
    );
    assert_eq!(
        board.ok(&["list", "T1", "--note", "Empty:", "--json"]),
        "\"\"\n"
    );
    assert_eq!(board.ok(&["list", "T1", "--note", "Absent:"]), "\n");
    assert_eq!(board.detail(), detail, "read-only projections");
    let unknown = board.run(&["list", "T999", "--field", "id"]);
    assert_eq!(unknown.status.code(), Some(2));
}

#[test]
fn notes_and_projection_reject_bad_inputs_without_writing() {
    let board = Board::new("Now: keep");
    let before = fs::read(board.0.join("tsk.json")).unwrap();
    for args in [
        vec!["notes", "T1", "set", "Now:", ""],
        vec!["notes", "T1", "append", "\nline"],
        vec!["notes", "T1", "append", "line\r"],
        vec!["notes", "T1", "append", "\u{1b}x"],
        vec!["notes", "T1", "append", "line\u{2028}x"],
        vec!["notes", "T1", "set", " :", "x"],
        vec!["notes", "T1", "set", "Now", "x"],
        vec!["notes", "T1", "set", "Now: other:", "x"],
        vec!["notes", "T1", "set", "Now:\n", "x"],
        vec!["notes", "T1", "remove", "Now:", "extra"],
        vec!["notes", "T1", "unknown", "Now:"],
        vec!["list", "--field", "id"],
        vec!["list", "T1", "--field", "secret"],
        vec!["list", "T1", "--field", "id", "--note", "Now:"],
        vec!["list", "T1", "--field", "id", "--field", "title"],
        vec!["list", "T1", "--note", "Now:", "--note", "Next:"],
        vec!["list", "T1", "--note", "Now"],
        vec!["list", "T1", "--field", "id", "--all"],
        vec!["list", "T1", "--field", "id", "--doing"],
    ] {
        let output = board.run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!output.stderr.is_empty());
        assert_eq!(
            fs::read(board.0.join("tsk.json")).unwrap(),
            before,
            "{args:?}"
        );
    }
    let output = board.run(&["notes", "T999", "append", "line"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown-task"));
    assert_eq!(fs::read(board.0.join("tsk.json")).unwrap(), before);
}

#[test]
fn concurrent_notes_processes_preserve_all_other_updates() {
    let board = Board::new("Goal: preserve\nThread: agent w t p");
    let mut children = Vec::new();
    for index in 0..16 {
        let prefix = format!("Worker{index}:");
        children.push(
            board
                .command(&["notes", "T1", "set", &prefix, "kept"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let line = format!("reference {index}");
        children.push(
            board
                .command(&["notes", "T1", "append", &line])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let detail = board.detail();
    let lines = detail["notes"]
        .as_str()
        .unwrap()
        .lines()
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 34);
    assert_eq!(&lines[..2], &["Goal: preserve", "Thread: agent w t p"]);
    for index in 0..16 {
        assert!(lines.contains(&format!("Worker{index}: kept").as_str()));
        assert!(lines.contains(&format!("reference {index}").as_str()));
    }
    assert_eq!(detail["status"], json!("todo"));
}

#[test]
fn notes_help_routes_through_binary_and_help_command() {
    let direct = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["notes", "--help"])
        .output()
        .unwrap();
    let indirect = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["help", "notes"])
        .output()
        .unwrap();
    assert!(direct.status.success());
    assert!(indirect.status.success());
    assert_eq!(direct.stdout, indirect.stdout);
    for word in ["set", "remove", "append", "--state-dir", "Exit:"] {
        assert!(String::from_utf8_lossy(&direct.stdout).contains(word));
    }
}
