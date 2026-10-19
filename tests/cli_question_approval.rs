//! `tsk ask` checks an approval's shape before anything is saved: every bad field comes
//! back in one refusal, and a refused input leaves no Q number, record, or notification.
//! Temp dirs only.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tsk_tui::cli::{run_with, CliOutput};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

const H: &str = "0123456789abcdef0123456789abcdef01234567";

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsk-cli-approval-{label}-{nanos}-{seq}"));
        fs::create_dir_all(dir.join("state")).expect("create temp state dir");
        Self(dir)
    }

    fn state(&self) -> String {
        self.0.join("state").to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, value: &Value) -> String {
        let path = self.0.join(name);
        fs::write(&path, serde_json::to_vec(value).expect("json")).expect("write input");
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cli(args: &[&str]) -> CliOutput {
    let mut argv = vec!["tsk".to_string()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    run_with(argv, Cursor::new(Vec::new()), false)
}

fn stdout_json(output: &CliOutput) -> Value {
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON stdout, got {:?} / {:?}", output.stdout, output.stderr))
}

fn task_id(dir: &TempDir) -> String {
    let added = cli(&["add", "-t", "gate", "--desk", "--state-dir", &dir.state()]);
    assert_eq!(added.code, 0, "{}", added.stderr);
    let listed = cli(&["list", "T1", "--json", "--state-dir", &dir.state()]);
    let value = stdout_json(&listed);
    let task = if value.is_array() {
        value[0].clone()
    } else {
        value
    };
    task["id"].as_str().expect("task id").to_string()
}

fn terms(h: &str) -> Value {
    json!({
        "target": "prod",
        "action": "deploy",
        "h": h,
        "scope": "web",
        "rollback": "redeploy previous"
    })
}

fn draft(task: &str, kind: &str, approval: Value) -> Value {
    json!({
        "task_id": task,
        "kind": kind,
        "summary": "deploy web",
        "origin_thread": "thr-a",
        "recipient": "lead-a",
        "text": "Deploy web to prod?",
        "options": [],
        "recommendation": null,
        "impact": null,
        "blocking_scope": null,
        "approval": approval,
        "finding": null,
        "source": null,
        "audience": "user"
    })
}

fn ask(dir: &TempDir, draft: &Value) -> CliOutput {
    let file = dir.write("question.json", draft);
    cli(&["ask", "--file", &file, "--state-dir", &dir.state()])
}

fn questions(dir: &TempDir) -> Value {
    let listed = cli(&["questions", "--state-dir", &dir.state()]);
    assert_eq!(listed.code, 0, "{}", listed.stderr);
    stdout_json(&listed)
}

#[test]
fn a_bad_approval_is_refused_with_every_field_at_once_and_saves_nothing() {
    let dir = TempDir::new("refuse");
    let task = task_id(&dir);

    let mut blanks = terms("zz");
    blanks["target"] = json!("  ");
    blanks["rollback"] = json!("");
    let out = ask(&dir, &draft(&task, "approval", blanks));
    assert_eq!(out.code, 1, "{}", out.stderr);
    for field in ["approval.target", "approval.rollback", "approval.h"] {
        assert!(
            out.stderr.contains(field),
            "{field} missing: {}",
            out.stderr
        );
    }
    assert!(!out.stderr.contains("approval.action"), "{}", out.stderr);
    assert!(!out.stderr.contains("approval.scope"), "{}", out.stderr);

    let out = ask(&dir, &draft(&task, "approval", Value::Null));
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("approval: required"), "{}", out.stderr);

    // Approval terms on the wrong kind, a bad agent name, and a blank summary: all named.
    let mut wrong = draft(&task, "question", terms(H));
    wrong["recipient"] = json!("Lead A");
    wrong["summary"] = json!(" ");
    let out = ask(&dir, &wrong);
    assert_eq!(out.code, 1);
    for field in ["summary", "recipient", "approval: only kind approval"] {
        assert!(
            out.stderr.contains(field),
            "{field} missing: {}",
            out.stderr
        );
    }

    // No Q number, record, or notification came out of any refusal.
    assert!(out.stdout.is_empty());
    assert_eq!(questions(&dir), json!([]));
}

#[test]
fn an_unknown_kind_or_field_names_itself_in_the_refusal() {
    let dir = TempDir::new("json");
    let task = task_id(&dir);
    let out = ask(&dir, &draft(&task, "decision", Value::Null));
    assert_eq!(out.code, 2);
    assert!(
        out.stderr.contains("unknown variant `decision`"),
        "{}",
        out.stderr
    );

    let mut extra = draft(&task, "question", Value::Null);
    extra["scope"] = json!("web");
    let out = ask(&dir, &extra);
    assert_eq!(out.code, 2);
    assert!(
        out.stderr.contains("unknown field `scope`"),
        "{}",
        out.stderr
    );

    let mut missing = draft(&task, "question", Value::Null);
    missing.as_object_mut().expect("object").remove("summary");
    let out = ask(&dir, &missing);
    assert_eq!(out.code, 2);
    assert!(
        out.stderr.contains("missing field `summary`"),
        "{}",
        out.stderr
    );
    assert_eq!(questions(&dir), json!([]));
}

#[test]
fn a_valid_approval_and_a_plain_question_save_as_before() {
    let dir = TempDir::new("valid");
    let task = task_id(&dir);
    for plain_h in [H.to_string(), "a".repeat(64)] {
        let out = ask(&dir, &draft(&task, "approval", terms(&plain_h)));
        assert_eq!(out.code, 0, "{}", out.stderr);
    }
    let out = ask(&dir, &draft(&task, "question", Value::Null));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(questions(&dir).as_array().expect("array").len(), 3);
}

#[test]
fn an_approval_with_options_is_refused_and_saves_nothing() {
    let dir = TempDir::new("approval-options");
    let task = task_id(&dir);
    let mut with_options = draft(&task, "approval", terms(H));
    with_options["options"] = json!(["yes", "no"]);
    let out = ask(&dir, &with_options);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr.contains("options: kind approval takes none"),
        "{}",
        out.stderr
    );
    assert_eq!(questions(&dir), json!([]));
}

#[test]
fn an_approval_record_saved_with_options_before_the_rule_stays_readable() {
    // The options-takes-none rule for kind approval only refuses new `ask` input
    // (tested above). It must not also apply on read: an approval record an older
    // binary saved with options present stays readable, and a sibling record in the
    // same store is not dragged down with it (D3826).
    let dir = TempDir::new("legacy-options");
    let task = task_id(&dir);
    let saved = stdout_json(&ask(&dir, &draft(&task, "approval", terms(H))));
    let revision = saved["saved"]["record"]["reference"]["revision"]
        .as_str()
        .expect("revision")
        .to_string();
    let other = stdout_json(&ask(&dir, &draft(&task, "question", Value::Null)));
    assert_eq!(other["saved"]["record"]["reference"]["number"], json!(2));

    let private_file = dir
        .0
        .join("state")
        .join("questions-private")
        .join(format!("{revision}.json"));
    let mut record: Value =
        serde_json::from_slice(&fs::read(&private_file).expect("read private record"))
            .expect("parse private record");
    record["draft"]["options"] = json!(["yes", "no"]);
    fs::write(&private_file, serde_json::to_vec_pretty(&record).unwrap())
        .expect("rewrite private record with legacy options");

    let records = questions(&dir);
    let records = records.as_array().expect("array");
    assert_eq!(records.len(), 2, "{records:?}");
    let approval = records
        .iter()
        .find(|record| record["reference"]["number"] == json!(1))
        .expect("approval record present");
    assert_eq!(
        approval["draft"]["options"],
        json!(["yes", "no"]),
        "{approval:?}"
    );

    let by_q1 = cli(&["questions", "Q1", "--state-dir", &dir.state()]);
    assert_eq!(by_q1.code, 0, "{}", by_q1.stderr);
    assert_eq!(
        stdout_json(&by_q1)["draft"]["options"],
        json!(["yes", "no"])
    );
}

#[test]
fn a_repeat_ask_is_a_new_record_and_never_inherits_the_earlier_approval() {
    let dir = TempDir::new("reask");
    let task = task_id(&dir);
    let first = stdout_json(&ask(&dir, &draft(&task, "approval", terms(H))));
    let revision = first["saved"]["record"]["reference"]["revision"].clone();
    let approved = {
        let file = dir.write(
            "answer.json",
            &json!({"expected_revision": revision, "approve": true, "approval": terms(H)}),
        );
        cli(&[
            "ui-answer",
            "Q1",
            "--file",
            &file,
            "--state-dir",
            &dir.state(),
        ])
    };
    assert_eq!(approved.code, 0, "{}", approved.stderr);

    // A plain duplicate (same terms) and a real re-approval (changed h) each get their own
    // Q number and revision, open and unanswered: approval never carries between records.
    let duplicate = stdout_json(&ask(&dir, &draft(&task, "approval", terms(H))));
    let changed_h = "f".repeat(40);
    let reapproval = stdout_json(&ask(&dir, &draft(&task, "approval", terms(&changed_h))));
    for (saved, number) in [(&duplicate, 2), (&reapproval, 3)] {
        assert_eq!(
            saved["saved"]["record"]["reference"]["number"],
            json!(number)
        );
        assert_eq!(saved["saved"]["record"]["answers"], json!([]));
        assert_ne!(saved["saved"]["record"]["reference"]["revision"], revision);
    }
    // The old approval for H cannot answer the changed-h question.
    let file = dir.write(
        "stale.json",
        &json!({
            "expected_revision": reapproval["saved"]["record"]["reference"]["revision"],
            "approve": true,
            "approval": terms(H)
        }),
    );
    let refused = cli(&[
        "ui-answer",
        "Q3",
        "--file",
        &file,
        "--state-dir",
        &dir.state(),
    ]);
    assert_eq!(refused.code, 1);
    assert!(
        refused.stderr.contains("approval-refused"),
        "{}",
        refused.stderr
    );
}
