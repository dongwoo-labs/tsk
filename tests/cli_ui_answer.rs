//! `tsk ui-answer`: the user-input entrypoint records origin `user_ui` through the same
//! question store, revision CAS, and approval checks as the board. Uses temp dirs only.

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
        let dir = std::env::temp_dir().join(format!("tsk-cli-ui-answer-{label}-{nanos}-{seq}"));
        fs::create_dir_all(&dir).expect("create temp state dir");
        Self(dir)
    }

    fn state(&self) -> String {
        self.0.to_string_lossy().into_owned()
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

fn cli_stdin(args: &[&str], stdin: &[u8]) -> CliOutput {
    let mut argv = vec!["tsk".to_string()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    run_with(argv, Cursor::new(stdin.to_vec()), false)
}

fn cli(args: &[&str]) -> CliOutput {
    cli_stdin(args, b"")
}

fn stdout_json(output: &CliOutput) -> Value {
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON stdout, got {:?} / {:?}", output.stdout, output.stderr))
}

fn task_id(dir: &TempDir) -> String {
    let added = cli(&[
        "add",
        "-t",
        "ask here",
        "--desk",
        "--state-dir",
        &dir.state(),
    ]);
    assert_eq!(added.code, 0, "{}", added.stderr);
    let listed = cli(&["list", "T1", "--json", "--state-dir", &dir.state()]);
    assert_eq!(listed.code, 0, "{}", listed.stderr);
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

/// Ask one question and return (Q UUID, revision).
fn ask(dir: &TempDir, kind: &str, audience: Option<&str>) -> (String, String) {
    let task = task_id(dir);
    let mut question = json!({
        "task_id": task,
        "kind": kind,
        "summary": "pick one",
        "origin_thread": "thr-a",
        "recipient": "lead-a",
        "text": "Which one?",
        "options": ["red", "blue"],
        "recommendation": null,
        "impact": null,
        "blocking_scope": null,
        "approval": null,
        "finding": null,
        "source": null
    });
    if kind == "approval" {
        question["options"] = json!([]);
        question["approval"] = terms(H);
    }
    if let Some(audience) = audience {
        question["audience"] = json!(audience);
    }
    let file = dir.write("question.json", &question);
    let asked = cli(&["ask", "--file", &file, "--state-dir", &dir.state()]);
    assert_eq!(asked.code, 0, "ask: {}", asked.stderr);
    let saved = stdout_json(&asked);
    let reference = &saved["saved"]["record"]["reference"];
    (
        reference["id"].as_str().expect("Q id").to_string(),
        reference["revision"]
            .as_str()
            .expect("revision")
            .to_string(),
    )
}

fn ui_answer(dir: &TempDir, address: &str, body: &Value) -> CliOutput {
    let file = dir.write("answer.json", body);
    cli(&[
        "ui-answer",
        address,
        "--file",
        &file,
        "--state-dir",
        &dir.state(),
    ])
}

fn record(dir: &TempDir, id: &str) -> Value {
    let read = cli(&["questions", id, "--state-dir", &dir.state()]);
    assert_eq!(read.code, 0, "{}", read.stderr);
    stdout_json(&read)
}

#[test]
fn a_choice_is_saved_as_the_option_text_with_user_ui_origin() {
    let dir = TempDir::new("choice");
    let (id, revision) = ask(&dir, "question", Some("user"));
    let out = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "choice": 1}),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let saved = stdout_json(&out);
    assert_eq!(saved["question_id"], json!(id));
    assert_eq!(saved["number"], json!(1));
    assert_eq!(saved["status"], json!("durable"));
    assert!(saved["task_id"].is_string());
    assert_ne!(saved["revision"], json!(revision));
    assert_eq!(out.stdout.lines().count(), 1, "one JSON object on one line");

    let record = record(&dir, &id);
    let answer = &record["answers"][0];
    assert_eq!(answer["text"], json!("blue"));
    assert_eq!(answer["origin"], json!("user_ui"));
    assert_eq!(answer["actor"], json!("user"));
    assert_eq!(answer["approval"], Value::Null);
    assert_eq!(record["reference"]["revision"], saved["revision"]);
}

#[test]
fn free_text_reads_from_stdin_and_a_correction_appends() {
    let dir = TempDir::new("stdin");
    let (id, revision) = ask(&dir, "question", Some("user"));
    let body = json!({"expected_revision": revision, "text": "  green  "}).to_string();
    let out = cli_stdin(
        &["ui-answer", &id, "--file", "-", "--state-dir", &dir.state()],
        body.as_bytes(),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let first = stdout_json(&out)["revision"].clone();

    let corrected = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": first, "choice": 0, "corrects_revision": first}),
    );
    assert_eq!(corrected.code, 0, "{}", corrected.stderr);
    let record = record(&dir, &id);
    assert_eq!(record["answers"][0]["text"], json!("green"));
    assert_eq!(record["answers"][1]["text"], json!("red"));
    assert_eq!(record["answers"][1]["corrects_revision"], first);
}

#[test]
fn a_stale_revision_is_refused_and_saves_nothing() {
    let dir = TempDir::new("stale");
    let (id, revision) = ask(&dir, "question", Some("user"));
    let out = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": "00000000-0000-4000-8000-000000000000", "choice": 0}),
    );
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty());
    assert!(
        out.stderr.starts_with("tsk ui-answer: stale-revision: "),
        "{}",
        out.stderr
    );
    let record = record(&dir, &id);
    assert_eq!(record["answers"], json!([]));
    assert_eq!(record["reference"]["revision"], json!(revision));

    // The first answer moves the revision; a second pick on the old one is stale.
    assert_eq!(
        ui_answer(
            &dir,
            "Q1",
            &json!({"expected_revision": revision, "choice": 0})
        )
        .code,
        0
    );
    let again = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "choice": 1}),
    );
    assert_eq!(again.code, 1);
    assert!(again.stderr.contains("stale-revision"), "{}", again.stderr);
}

#[test]
fn approval_needs_the_exact_terms_and_a_plain_answer_never_becomes_one() {
    let dir = TempDir::new("approval");
    let (id, revision) = ask(&dir, "approval", Some("user"));
    let other = "f".repeat(40);
    for body in [
        json!({"expected_revision": revision, "approve": true, "approval": terms(&other)}),
        json!({"expected_revision": revision, "approve": true}),
        json!({"expected_revision": revision, "reject": true, "approval": terms(H)}),
    ] {
        let out = ui_answer(&dir, "Q1", &body);
        assert_eq!(out.code, 1, "{body}");
        assert!(
            out.stderr.starts_with("tsk ui-answer: approval-refused: "),
            "{}",
            out.stderr
        );
    }
    let text = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "text": "yes"}),
    );
    assert_eq!(text.code, 2);
    assert!(text.stderr.contains("invalid-input"), "{}", text.stderr);
    assert_eq!(record(&dir, &id)["answers"], json!([]));

    let out = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "approve": true, "approval": terms(H)}),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let answer = record(&dir, &id)["answers"][0].clone();
    assert_eq!(answer["text"], json!("approved"));
    assert_eq!(answer["origin"], json!("user_ui"));
    assert_eq!(answer["approval"], terms(H));
}

#[test]
fn a_comment_rides_the_approval_decision_but_is_never_itself_one() {
    let dir = TempDir::new("comment");
    let (id, revision) = ask(&dir, "approval", Some("user"));

    // Comment is refused on anything but an approval question.
    let (plain_id, plain_revision) = ask(&dir, "question", Some("user"));
    let plain_with_comment = ui_answer(
        &dir,
        "Q2",
        &json!({"expected_revision": plain_revision, "choice": 0, "comment": "note"}),
    );
    assert_eq!(plain_with_comment.code, 2, "{}", plain_with_comment.stderr);
    assert!(
        plain_with_comment.stderr.contains("invalid-input"),
        "{}",
        plain_with_comment.stderr
    );
    assert_eq!(record(&dir, &plain_id)["answers"], json!([]));

    // A blank comment on an approval is refused the same way.
    let blank = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "reject": true, "comment": "   "}),
    );
    assert_eq!(blank.code, 2, "{}", blank.stderr);
    assert_eq!(record(&dir, &id)["answers"], json!([]));

    // Rejecting with a comment saves the comment beside the rejection, and the decision
    // is still exactly `rejected` with no approval terms attached.
    let rejected = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "reject": true, "comment": "not ready yet"}),
    );
    assert_eq!(rejected.code, 0, "{}", rejected.stderr);
    let answer = record(&dir, &id)["answers"][0].clone();
    assert_eq!(answer["text"], json!("rejected"));
    assert_eq!(answer["comment"], json!("not ready yet"));
    assert_eq!(answer["approval"], Value::Null);
}

#[test]
fn an_approval_takes_a_comment_alone_as_an_unapproved_answer() {
    let dir = TempDir::new("comment-alone");
    let (id, revision) = ask(&dir, "approval", Some("user"));
    let (plain_id, plain_revision) = ask(&dir, "question", Some("user"));

    // Alone, a comment answers only an approval, and never when blank.
    let plain = ui_answer(
        &dir,
        "Q2",
        &json!({"expected_revision": plain_revision, "comment": "note"}),
    );
    assert_eq!(plain.code, 2, "{}", plain.stderr);
    assert_eq!(record(&dir, &plain_id)["answers"], json!([]));
    let blank = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "comment": "  "}),
    );
    assert_eq!(blank.code, 2, "{}", blank.stderr);
    assert_eq!(record(&dir, &id)["answers"], json!([]));

    let commented = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "comment": "scope를 줄여서 다시 올려줘"}),
    );
    assert_eq!(commented.code, 0, "{}", commented.stderr);
    let answer = record(&dir, &id)["answers"][0].clone();
    assert_eq!(answer["text"], json!("commented"));
    assert_eq!(answer["comment"], json!("scope를 줄여서 다시 올려줘"));
    assert_eq!(answer["origin"], json!("user_ui"));
    assert_eq!(answer["approval"], Value::Null);
}

#[test]
fn a_plain_question_refuses_approve_and_approval_terms() {
    let dir = TempDir::new("plain");
    let (id, revision) = ask(&dir, "question", Some("user"));
    for body in [
        json!({"expected_revision": revision, "approve": true, "approval": terms(H)}),
        json!({"expected_revision": revision, "choice": 0, "approval": terms(H)}),
    ] {
        let out = ui_answer(&dir, "Q1", &body);
        assert_eq!(out.code, 1, "{body}");
        assert!(out.stderr.contains("approval-refused"), "{}", out.stderr);
    }
    assert_eq!(record(&dir, &id)["answers"], json!([]));
}

#[test]
fn an_unknown_or_unrouted_question_is_refused() {
    let dir = TempDir::new("unknown");
    let (id, revision) = ask(&dir, "question", None);
    for address in ["Q9", "00000000-0000-4000-8000-000000000000"] {
        let out = ui_answer(
            &dir,
            address,
            &json!({"expected_revision": revision, "choice": 0}),
        );
        assert_eq!(out.code, 1, "{address}");
        assert!(
            out.stderr.starts_with("tsk ui-answer: unknown-question: "),
            "{}",
            out.stderr
        );
    }
    let out = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "choice": 0}),
    );
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("unrouted-question"), "{}", out.stderr);
    assert_eq!(record(&dir, &id)["answers"], json!([]));
}

#[test]
fn usage_errors_exit_2() {
    let dir = TempDir::new("usage");
    let (_, revision) = ask(&dir, "question", Some("user"));
    let both = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "choice": 0, "text": "x"}),
    );
    assert_eq!(both.code, 2);
    let unknown = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "choice": 0, "origin": "agent"}),
    );
    assert_eq!(unknown.code, 2);
    let blank = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "text": "   "}),
    );
    assert_eq!(blank.code, 2);
    assert!(blank.stderr.contains("invalid-input"), "{}", blank.stderr);
    let missing = cli(&["ui-answer", "Q1", "--state-dir", &dir.state()]);
    assert_eq!(missing.code, 2);
    assert!(missing.stderr.contains("usage"), "{}", missing.stderr);
}

#[test]
fn ordinary_answer_still_refuses_user_ui_origin() {
    let dir = TempDir::new("answer");
    let (id, revision) = ask(&dir, "question", Some("user"));
    let file = dir.write(
        "plain.json",
        &json!({
            "expected_revision": revision,
            "text": "red",
            "actor": "user",
            "origin": "user_ui",
            "corrects_revision": null,
            "approval": null,
            "user_chat": null,
            "source": null
        }),
    );
    let out = cli(&["answer", &id, "--file", &file, "--state-dir", &dir.state()]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("UserUi"), "{}", out.stderr);
    assert_eq!(record(&dir, &id)["answers"], json!([]));
}

/// Ask one secret question (global fnox target, audience user) and return (Q id, revision).
fn ask_secret(dir: &TempDir) -> (String, String) {
    let task = task_id(dir);
    let question = json!({
        "task_id": task,
        "kind": "secret",
        "summary": "staging key",
        "origin_thread": "thr-a",
        "recipient": "lead-a",
        "text": "Paste the staging key",
        "options": [],
        "recommendation": null,
        "impact": null,
        "blocking_scope": null,
        "approval": null,
        "finding": null,
        "source": null,
        "audience": "user",
        "secret": {"key": "STAGING_KEY", "profile": null, "config": null}
    });
    let file = dir.write("secret-question.json", &question);
    let asked = cli(&["ask", "--file", &file, "--state-dir", &dir.state()]);
    assert_eq!(asked.code, 0, "ask: {}", asked.stderr);
    let saved = stdout_json(&asked);
    let reference = &saved["saved"]["record"]["reference"];
    (
        reference["id"].as_str().expect("Q id").to_string(),
        reference["revision"]
            .as_str()
            .expect("revision")
            .to_string(),
    )
}

#[test]
fn ui_answer_rejects_a_secret_without_a_value_but_still_refuses_a_set_attempt() {
    let dir = TempDir::new("secret-reject");
    let (id, revision) = ask_secret(&dir);

    // Anything but `reject` still refuses with secret-needs-board: no text, no choice, no
    // approve, and a plain reject-shaped pick combined with another field is still one pick.
    let attempt = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "text": "hunter2"}),
    );
    assert_eq!(attempt.code, 1, "{}", attempt.stderr);
    assert!(
        attempt
            .stderr
            .starts_with("tsk ui-answer: secret-needs-board: "),
        "{}",
        attempt.stderr
    );
    assert_eq!(record(&dir, &id)["answers"], json!([]));

    // A correction attempt on a secret reject is refused before any write.
    let corrected = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "reject": true, "corrects_revision": revision}),
    );
    assert_eq!(corrected.code, 1, "{}", corrected.stderr);
    assert!(
        corrected.stderr.contains("invalid-correction"),
        "{}",
        corrected.stderr
    );
    assert_eq!(record(&dir, &id)["answers"], json!([]));

    // `reject: true` closes it with no value and no fnox call.
    let rejected = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "reject": true}),
    );
    assert_eq!(rejected.code, 0, "{}", rejected.stderr);
    let answer = record(&dir, &id)["answers"][0].clone();
    assert_eq!(answer["text"], json!("rejected"));
    assert_eq!(answer["origin"], json!("user_ui"));

    // Once rejected, the question stays closed; nothing else can answer it again.
    let again = ui_answer(
        &dir,
        "Q1",
        &json!({"expected_revision": revision, "reject": true}),
    );
    assert_eq!(again.code, 1, "{}", again.stderr);
    assert!(again.stderr.contains("stale-revision"), "{}", again.stderr);
}
