//! Public report state transitions and CLI flow, with isolated state only.
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::store::{ReportError, ReportStore, TaskStore, TrashTarget};
use uuid::Uuid;

static SEQ: AtomicU64 = AtomicU64::new(0);
fn seed() -> (PathBuf, Uuid) {
    let path = std::env::temp_dir().join(format!(
        "tsk-completion-reports-{}-{}-{}",
        std::process::id(),
        Uuid::new_v4(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = run_with(
        [
            "tsk",
            "add",
            "-t",
            "completion task",
            "--desk",
            "--state-dir",
            path.to_str().unwrap(),
        ],
        Cursor::new(b""),
        true,
    );
    assert_eq!(result.code, 0, "{}", result.stderr);
    let id = TaskStore::new(&path).load().unwrap().tasks()[0].id;
    (path, id)
}

#[test]
fn identical_markdown_keeps_revision_and_read_state() {
    let (path, id) = seed();
    let store = ReportStore::new(&path);
    let first = store
        .register(TrashTarget::Id(id), "# Done\n\nExact text.\n".into())
        .unwrap();
    assert_eq!(first.revision, 1);
    store.mark_read(id, 1).unwrap();
    let before = store.read(id).unwrap();
    assert_eq!(
        store.register(TrashTarget::Id(id), first.markdown).unwrap(),
        before
    );
}

#[test]
fn changed_markdown_preserves_oldest_first_history() {
    let (path, id) = seed();
    let store = ReportStore::new(path);
    for markdown in ["first", "second", "first"] {
        store
            .register(TrashTarget::Id(id), markdown.into())
            .unwrap();
    }
    let report = store.read(id).unwrap();
    assert_eq!(report.revision, 3);
    assert_eq!(
        report
            .revisions
            .iter()
            .map(|r| (r.revision, r.markdown.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "first"), (2, "second"), (3, "first")]
    );
}

#[test]
fn stale_mark_read_cannot_consume_a_new_revision() {
    let (path, id) = seed();
    let store = ReportStore::new(path);
    store.register(TrashTarget::Id(id), "first".into()).unwrap();
    store
        .register(TrashTarget::Id(id), "second".into())
        .unwrap();
    assert_eq!(store.mark_read(id, 1), Err(ReportError::StaleRevision));
    assert_eq!(store.read(id).unwrap().read_revision, None);
    assert!(store.mark_read(id, 2).unwrap().read_revision == Some(2));
}

#[test]
fn corrupt_document_refuses_without_replacing_bytes() {
    let (path, id) = seed();
    fs::write(path.join("reports.json"), b"{broken").unwrap();
    let store = ReportStore::new(&path);
    assert_eq!(store.list(), Err(ReportError::InvalidDocument));
    assert_eq!(
        store.register(TrashTarget::Id(id), "new".into()),
        Err(ReportError::InvalidDocument)
    );
    assert_eq!(fs::read(path.join("reports.json")).unwrap(), b"{broken");
}

#[test]
fn future_document_refuses_without_replacing_bytes() {
    let (path, id) = seed();
    let bytes = b"{\"format_version\":2,\"reports\":[]}";
    fs::write(path.join("reports.json"), bytes).unwrap();
    let store = ReportStore::new(&path);
    assert_eq!(store.list(), Err(ReportError::InvalidDocument));
    assert_eq!(
        store.register(TrashTarget::Id(id), "new".into()),
        Err(ReportError::InvalidDocument)
    );
    assert_eq!(fs::read(path.join("reports.json")).unwrap(), bytes);
}

#[test]
fn invalid_record_invariants_refuse_all_mutations() {
    let (path, id) = seed();
    let store = ReportStore::new(&path);
    store.register(TrashTarget::Id(id), "first".into()).unwrap();
    let baseline: serde_json::Value =
        serde_json::from_slice(&fs::read(path.join("reports.json")).unwrap()).unwrap();
    let mut variants = Vec::new();
    for (field, value) in [
        ("revision", serde_json::json!(0)),
        ("markdown", serde_json::json!(" ")),
        ("markdown", serde_json::json!("not the last revision")),
        ("read_revision", serde_json::json!(2)),
        ("unread", serde_json::json!(false)),
        ("task_number", serde_json::json!(0)),
    ] {
        let mut value_doc = baseline.clone();
        value_doc["reports"][0][field] = value;
        variants.push(value_doc);
    }
    let mut bad_history = baseline.clone();
    bad_history["reports"][0]["revisions"][0]["revision"] = serde_json::json!(2);
    variants.push(bad_history);
    let mut duplicate = baseline.clone();
    duplicate["reports"]
        .as_array_mut()
        .unwrap()
        .push(baseline["reports"][0].clone());
    variants.push(duplicate);
    let mut accepted = Vec::new();
    for (index, document) in variants.into_iter().enumerate() {
        let bytes = serde_json::to_vec(&document).unwrap();
        fs::write(path.join("reports.json"), &bytes).unwrap();
        if store.list() != Err(ReportError::InvalidDocument) {
            accepted.push(index);
        }
        assert_eq!(store.mark_read(id, 1), Err(ReportError::InvalidDocument));
        assert_eq!(
            store.register(TrashTarget::Id(id), "new".into()),
            Err(ReportError::InvalidDocument)
        );
        assert_eq!(fs::read(path.join("reports.json")).unwrap(), bytes);
    }
    assert!(
        accepted.is_empty(),
        "accepted corrupt variants: {accepted:?}"
    );
}

fn cli(path: &std::path::Path, args: &[&str], input: &[u8]) -> CliOutput {
    let mut argv = vec!["tsk".to_owned()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    argv.push(format!("--state-dir={}", path.display()));
    run_with(argv, Cursor::new(input), false)
}

#[test]
fn cli_registration_read_history_noop_and_input_refusals() {
    let (path, id) = seed();
    let before = fs::read(path.join("tsk.json")).unwrap();
    let first = cli(
        &path,
        &["report", "T1", "--file=-", "--json"],
        b"\xef\xbb\xbf# Done\n",
    );
    assert_eq!(first.code, 0, "{}", first.stderr);
    let first: serde_json::Value = serde_json::from_str(&first.stdout).unwrap();
    assert_eq!(first["markdown"], "# Done\n");
    assert_eq!(first["task_id"], id.to_string());
    assert_eq!(first["task_number"], 1);
    assert_eq!(first["read_revision"], serde_json::Value::Null);
    assert_eq!(
        cli(&path, &["report", &id.to_string(), "--json"], b"").code,
        0
    );
    assert!(ReportStore::new(&path).read(id).unwrap().unread());
    let changed = cli(
        &path,
        &["report", "T1", "--file", "-", "--json"],
        b"Second\n",
    );
    assert_eq!(changed.code, 0, "{}", changed.stderr);
    let repeated = cli(&path, &["report", "T1", "--file=-", "--json"], b"Second\n");
    assert_eq!(repeated.code, 0, "{}", repeated.stderr);
    assert_eq!(repeated.stdout, changed.stdout);
    let all = cli(&path, &["reports", "--unread", "--json"], b"");
    assert_eq!(all.code, 0, "{}", all.stderr);
    let all: serde_json::Value = serde_json::from_str(&all.stdout).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 1);
    assert_eq!(all[0]["revision"], 2);
    assert_eq!(all[0]["revisions"].as_array().unwrap().len(), 2);
    ReportStore::new(&path).mark_read(id, 2).unwrap();
    assert_eq!(
        cli(&path, &["reports", "--unread", "--json"], b"")
            .stdout
            .trim(),
        "[]"
    );
    assert_eq!(cli(&path, &["report", "T999", "--json"], b"").code, 1);
    assert_eq!(
        cli(&path, &["report", "T999", "--file=-"], b"result").code,
        1
    );
    for input in [b" \n".as_slice(), b"\xff".as_slice()] {
        assert_eq!(cli(&path, &["report", "T1", "--file=-"], input).code, 2);
    }
    assert_eq!(fs::read(path.join("tsk.json")).unwrap(), before);
}

#[test]
fn done_history_registration_and_orphan_read_preserve_independent_data() {
    use tsk_tui::domain::HumanStatus;
    use tsk_tui::store::DoneLine;
    let (path, id) = seed();
    let state = TaskStore::new(&path).load().unwrap();
    let mut task = state.tasks()[0].clone();
    task.status = HumanStatus::Done;
    let done = DoneLine {
        task,
        done_at: std::time::SystemTime::now(),
    };
    fs::create_dir(path.join("done")).unwrap();
    let done_path = path.join("done/2026-10-21.jsonl");
    let done_bytes = format!("{}\n", serde_json::to_string(&done).unwrap());
    fs::write(&done_path, &done_bytes).unwrap();
    let mut live = serde_json::to_value(&state).unwrap();
    live["tasks"] = serde_json::json!([]);
    let live_bytes = serde_json::to_vec(&live).unwrap();
    fs::write(path.join("tsk.json"), &live_bytes).unwrap();
    assert_eq!(TaskStore::new(&path).load_done_history().unwrap().len(), 1);
    let output = cli(
        &path,
        &["report", "T1", "--file=-", "--json"],
        b"Done history result\n",
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    let store = ReportStore::new(&path);
    assert_eq!(store.read(id).unwrap().markdown, "Done history result\n");
    assert_eq!(fs::read(path.join("tsk.json")).unwrap(), live_bytes);
    assert_eq!(fs::read_to_string(&done_path).unwrap(), done_bytes);
    // Simulate a Task whose live, trash, and dated history no longer retain it.
    fs::write(&done_path, b"").unwrap();
    assert_eq!(cli(&path, &["report", "T1", "--json"], b"").code, 0);
    assert_eq!(
        cli(&path, &["report", &id.to_string(), "--json"], b"").code,
        0
    );
    assert_eq!(store.list().unwrap().len(), 1);
    assert_eq!(
        store.register(TrashTarget::Id(id), "replacement".into()),
        Err(ReportError::UnknownTask)
    );
    assert_eq!(
        store.register(TrashTarget::Id(id), "Done history result\n".into()),
        Err(ReportError::UnknownTask)
    );
    assert!(!store.mark_read(id, 1).unwrap().unread());
}

#[test]
fn deleted_and_notice_tasks_refuse_registration() {
    use tsk_tui::domain::{HumanStatus, TaskScope};
    let (path, id) = seed();
    let task_store = TaskStore::new(&path);
    let mut state = task_store.load().unwrap();
    state.soft_delete(id).unwrap();
    let notice = state
        .create_notice(
            "report-test",
            "human only",
            None,
            HumanStatus::Todo,
            TaskScope::Global,
            Vec::new(),
        )
        .unwrap();
    task_store.save(&state).unwrap();
    let before = fs::read(path.join("tsk.json")).unwrap();
    let reports = ReportStore::new(&path);
    for task_id in [id, notice] {
        assert_eq!(
            reports.register(TrashTarget::Id(task_id), "result".into()),
            Err(ReportError::UnknownTask)
        );
    }
    assert!(reports.list().unwrap().is_empty());
    assert_eq!(fs::read(path.join("tsk.json")).unwrap(), before);
}

#[test]
fn binary_report_journey_uses_selected_environment_and_file_input() {
    use std::process::{Command, Stdio};
    let (path, _) = seed();
    let file = path.join("completion.md");
    let markdown = format!("# 완료\n\n{}\n", "Long Markdown stays whole. ".repeat(4096));
    fs::write(&file, &markdown).unwrap();
    let binary = std::env::var("CARGO_BIN_EXE_tsk").expect("Cargo provides binary path");
    let run = |args: &[&str]| {
        Command::new(&binary)
            .args(args)
            .env("TSK_STATE_DIR", &path)
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .env_remove("LOCALAPPDATA")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let first = run(&["report", "T1", "--file", file.to_str().unwrap(), "--json"]);
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let record: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(record["markdown"], markdown);
    assert_eq!(run(&["report", "T1", "--json"]).stdout, first.stdout);
    let listed = run(&["reports", "--unread", "--json"]);
    assert_eq!(listed.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&listed.stdout).unwrap()[0],
        record
    );
    assert_eq!(run(&["report", "T999", "--json"]).status.code(), Some(1));
    assert_eq!(run(&["help", "report"]).status.code(), Some(0));
    assert_eq!(run(&["reports", "--help"]).status.code(), Some(0));
    for args in [
        vec!["report"],
        vec!["reports", "T1"],
        vec!["reports", "--file=-"],
        vec!["report", "T1", "--unread"],
        vec!["report", "T1", "--file="],
        vec!["report", "T1", "--unknown"],
    ] {
        assert_eq!(run(&args).status.code(), Some(2), "{args:?}");
    }
    fs::write(path.join("reports.json"), b"{broken").unwrap();
    assert_eq!(run(&["reports", "--json"]).status.code(), Some(1));
    assert_eq!(
        run(&["report", "T1", "--file", file.to_str().unwrap()])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run(&["report", "T1", "--file=missing.md"]).status.code(),
        Some(3)
    );
}
