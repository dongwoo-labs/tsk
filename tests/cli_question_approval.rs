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
fn inventory_keeps_unavailable_questions_visible_while_default_reads_stay_strict() {
    for damage in ["missing", "corrupt", "unsupported", "mismatch", "not-file"] {
        let dir = TempDir::new(damage);
        let task = task_id(&dir);
        let saved = stdout_json(&ask(&dir, &draft(&task, "approval", terms(H))));
        let unavailable = &saved["saved"]["record"];
        let healthy = stdout_json(&ask(&dir, &draft(&task, "question", Value::Null)));
        let healthy = &healthy["saved"]["record"];
        let private = dir.0.join("state/questions-private").join(format!(
            "{}.json",
            unavailable["reference"]["revision"].as_str().unwrap()
        ));
        match damage {
            "missing" => fs::remove_file(&private).unwrap(),
            "corrupt" => fs::write(&private, b"not JSON").unwrap(),
            "not-file" => {
                fs::remove_file(&private).unwrap();
                fs::create_dir(&private).unwrap();
            }
            _ => {
                let mut record = unavailable.clone();
                if damage == "unsupported" {
                    record["future_field"] = json!(true);
                } else {
                    record["reference"]["revision"] = json!("00000000-0000-0000-0000-000000000001");
                }
                fs::write(&private, serde_json::to_vec(&record).unwrap()).unwrap();
            }
        }
        let shared_before = fs::read(dir.0.join("state/tsk.json")).unwrap();
        let healthy_file = dir.0.join("state/questions-private").join(format!(
            "{}.json",
            healthy["reference"]["revision"].as_str().unwrap()
        ));
        let healthy_before = fs::read(&healthy_file).unwrap();

        for filter in [vec![], vec!["--task", "T1"], vec!["--task", task.as_str()]] {
            let mut args = vec!["questions", "--state-dir"];
            let state = dir.state();
            args.push(&state);
            args.extend(filter);
            let strict = cli(&args);
            assert_eq!(strict.code, 1, "{damage}: {}", strict.stderr);
            assert!(strict.stdout.is_empty());
            args.push("--inventory");
            let listed = cli(&args);
            assert_eq!(listed.code, 0, "{damage}: {}", listed.stderr);
            assert!(listed.stderr.contains("unavailable"), "{}", listed.stderr);
            assert_eq!(
                stdout_json(&listed),
                json!([
                    {"reference": unavailable["reference"], "unavailable": true}, healthy
                ])
            );
        }
        let strict_number = cli(&["questions", "Q2", "--state-dir", &dir.state()]);
        assert_eq!(strict_number.code, 1);
        for address in [healthy["reference"]["id"].as_str().unwrap()] {
            let read = cli(&[
                "questions",
                address,
                "--task",
                "T1",
                "--state-dir",
                &dir.state(),
            ]);
            assert_eq!(read.code, 0, "{damage}: {}", read.stderr);
            assert!(read.stderr.is_empty());
            assert_eq!(stdout_json(&read), *healthy);
        }
        for address in ["Q1", unavailable["reference"]["id"].as_str().unwrap()] {
            let read = cli(&["questions", address, "--state-dir", &dir.state()]);
            assert_eq!(read.code, 1, "{damage}: {}", read.stderr);
            assert!(read.stdout.is_empty());
            assert!(read.stderr.contains("missing or corrupt"));
            for (verb, input) in [
                (
                    "route",
                    json!({"expected_revision": unavailable["reference"]["revision"], "audience": "lead"}),
                ),
                (
                    "answer",
                    json!({"expected_revision": unavailable["reference"]["revision"], "text": "approved", "actor": "lead-a", "origin": "agent"}),
                ),
                (
                    "ui-answer",
                    json!({"expected_revision": unavailable["reference"]["revision"], "approve": true, "approval": terms(H)}),
                ),
            ] {
                let file = dir.write("unavailable-input.json", &input);
                let refused = cli(&[verb, address, "--file", &file, "--state-dir", &dir.state()]);
                assert_eq!(refused.code, 1, "{damage} {verb}: {}", refused.stderr);
            }
        }
        assert_eq!(
            fs::read(dir.0.join("state/tsk.json")).unwrap(),
            shared_before
        );
        assert_eq!(fs::read(&healthy_file).unwrap(), healthy_before);
        // UUID access to a healthy target keeps the existing mutation path.
        let file = dir.write(
            "healthy-route.json",
            &json!({
                "expected_revision": healthy["reference"]["revision"], "audience": "lead"
            }),
        );
        let routed = cli(&[
            "route",
            healthy["reference"]["id"].as_str().unwrap(),
            "--file",
            &file,
            "--state-dir",
            &dir.state(),
        ]);
        assert_eq!(routed.code, 0, "{damage}: {}", routed.stderr);
    }
}

#[test]
fn inventory_is_list_only_filters_by_task_and_does_not_hide_shared_store_failures() {
    let dir = TempDir::new("inventory-scope");
    let task = task_id(&dir);
    assert_eq!(
        cli(&["add", "-t", "other", "--desk", "--state-dir", &dir.state()]).code,
        0
    );
    let out = ask(&dir, &draft(&task, "question", Value::Null));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let normal = cli(&["questions", "--state-dir", &dir.state()]);
    let inventory = cli(&["questions", "--inventory", "--state-dir", &dir.state()]);
    assert_eq!(inventory.code, 0, "{}", inventory.stderr);
    assert!(inventory.stderr.is_empty());
    assert_eq!(inventory.stdout, normal.stdout);
    let empty = cli(&[
        "questions",
        "--inventory",
        "--task",
        "T2",
        "--state-dir",
        &dir.state(),
    ]);
    assert_eq!(empty.code, 0);
    assert_eq!(stdout_json(&empty), json!([]));

    let shared_before = fs::read(dir.0.join("state/tsk.json")).unwrap();
    let file = dir.write("input.json", &json!({}));
    for verb in ["ask", "answer", "route", "ui-answer", "secret-result"] {
        let refused = cli(&[
            verb,
            "--inventory",
            "--file",
            &file,
            "--state-dir",
            &dir.state(),
        ]);
        assert_eq!(refused.code, 2, "{verb}: {}", refused.stderr);
    }
    for extra in [vec!["Q1"], vec!["--file", file.as_str()]] {
        let state = dir.state();
        let mut args = vec!["questions", "--inventory", "--state-dir", &state];
        args.extend(extra);
        assert_eq!(cli(&args).code, 2);
    }
    assert_eq!(
        fs::read(dir.0.join("state/tsk.json")).unwrap(),
        shared_before
    );
    fs::write(dir.0.join("state/tsk.json"), b"broken shared store").unwrap();
    let refused = cli(&["questions", "--inventory", "--state-dir", &dir.state()]);
    assert_eq!(refused.code, 1);
    assert!(refused.stdout.is_empty());
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
fn permission_context_survives_private_save_and_cli_readback_when_present() {
    let dir = TempDir::new("permission-context");
    let task = task_id(&dir);
    let contexts = [
        json!({
            "cwd": "/work/project",
            "tool_name": "Bash",
            "tool_input": {"command": "cargo test", "nested": {"flags": [true, 3, null]}},
            "task_number": 12,
            "task_scope": "질문 보존 확인\nGoal: permission metadata 보존\nDone when: readback 일치",
            "holds": ["배포 보류", "Keep data unchanged"]
        }),
        json!({
            "cwd": null, "tool_name": "Read", "tool_input": null,
            "task_number": 12, "task_scope": null, "holds": null
        }),
        Value::Null,
    ];
    for context in contexts {
        let mut input = draft(&task, "question", Value::Null);
        input["permission_context"] = context.clone();
        let out = ask(&dir, &input);
        assert_eq!(out.code, 0, "{}", out.stderr);
        let saved = stdout_json(&out);
        assert_eq!(saved["saved"]["durability"], "durable");
        let number = saved["saved"]["record"]["reference"]["number"]
            .as_u64()
            .expect("question number");
        let read = cli(&[
            "questions",
            &format!("Q{number}"),
            "--json",
            "--state-dir",
            &dir.state(),
        ]);
        assert_eq!(read.code, 0, "{}", read.stderr);
        let record = stdout_json(&read);
        assert_eq!(record["draft"]["permission_context"], context);
        assert_eq!(record["draft"]["text"], input["text"]);
        assert_eq!(record["answers"], json!([]));
        let shared: Value =
            serde_json::from_slice(&fs::read(dir.0.join("state/tsk.json")).expect("shared store"))
                .expect("shared JSON");
        assert!(!shared.to_string().contains("permission_context"));
    }
    let out = ask(&dir, &draft(&task, "question", Value::Null));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let records = questions(&dir);
    assert_eq!(records.as_array().expect("records").len(), 4);
    for record in &records.as_array().expect("records")[2..] {
        assert!(record["draft"].get("permission_context").is_none());
    }

    let number = "123456789012345678901234567890";
    let mut input = draft(&task, "question", Value::Null);
    input["permission_context"] = json!({
        "tool_name": "Bash", "task_number": 12,
        "tool_input": {"number": "NUMBER_LITERAL"}
    });
    let path = dir.0.join("large-number.json");
    fs::write(
        &path,
        serde_json::to_string(&input)
            .expect("input JSON")
            .replace("\"NUMBER_LITERAL\"", number),
    )
    .expect("raw numeric input");
    let out = cli(&[
        "ask",
        "--file",
        path.to_str().expect("input path"),
        "--state-dir",
        &dir.state(),
    ]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains(number), "{}", out.stdout);
    let read = cli(&["questions", "Q5", "--json", "--state-dir", &dir.state()]);
    assert_eq!(read.code, 0, "{}", read.stderr);
    assert!(read.stdout.contains(number), "{}", read.stdout);
    let context = &stdout_json(&read)["draft"]["permission_context"];
    for field in ["cwd", "task_scope", "holds"] {
        assert_eq!(context.get(field), Some(&Value::Null));
    }
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
