//! Secret questions from the CLI side: `tsk ask` validates the fnox target, readback shows
//! only the target, and no CLI answer path (`answer`, `ui-answer`) or route can take one.
//! The board alone hands the value to fnox (unit tests in `question_actions`). Temp dirs only.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tsk_tui::cli::{run_with, CliOutput};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsk-cli-secret-{label}-{nanos}-{seq}"));
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
    let added = cli(&[
        "add",
        "-t",
        "needs a key",
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

fn secret_draft(task: &str, secret: Value) -> Value {
    json!({
        "task_id": task,
        "kind": "secret",
        "summary": "staging API key",
        "origin_thread": "thr-a",
        "recipient": "lead-a",
        "text": "Enter the staging API key",
        "options": [],
        "recommendation": null,
        "impact": null,
        "blocking_scope": null,
        "approval": null,
        "finding": null,
        "source": null,
        "audience": "user",
        "secret": secret
    })
}

fn ask(dir: &TempDir, draft: &Value) -> CliOutput {
    let file = dir.write("question.json", draft);
    cli(&["ask", "--file", &file, "--state-dir", &dir.state()])
}

fn global() -> Value {
    json!({"key": "STAGING_API_KEY", "profile": null, "config": null})
}

#[test]
fn a_secret_question_saves_its_target_and_reads_back_without_a_value() {
    let dir = TempDir::new("ok");
    let task = task_id(&dir);
    let asked = ask(&dir, &secret_draft(&task, global()));
    assert_eq!(asked.code, 0, "{}", asked.stderr);

    let read = cli(&["questions", "Q1", "--state-dir", &dir.state()]);
    assert_eq!(read.code, 0, "{}", read.stderr);
    let record = stdout_json(&read);
    assert_eq!(record["reference"]["kind"], json!("secret"));
    assert_eq!(record["draft"]["secret"], global());
    assert_eq!(record["answers"], json!([]));

    // A project config: an absolute path to an existing regular file, with a profile.
    let config = dir.0.join("fnox.toml");
    fs::write(&config, "").expect("config");
    let target = json!({"key": "_K2", "profile": "stage-1", "config": config.to_string_lossy()});
    let asked = ask(&dir, &secret_draft(&task, target.clone()));
    assert_eq!(asked.code, 0, "{}", asked.stderr);
    let listed = stdout_json(&cli(&["questions", "--state-dir", &dir.state()]));
    assert_eq!(listed[1]["draft"]["secret"], target);
}

#[test]
fn ask_refuses_a_malformed_secret_question_with_the_target_rules() {
    let dir = TempDir::new("invalid");
    let task = task_id(&dir);
    let config = dir.0.join("fnox.toml");
    fs::write(&config, "").expect("config");
    let link = dir.0.join("link.toml");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&config, &link).expect("symlink");
    let mut cases: Vec<(&str, Value)> = vec![
        (
            "key starts with a digit",
            secret_draft(
                &task,
                json!({"key": "1KEY", "profile": null, "config": null}),
            ),
        ),
        (
            "key has a dash",
            secret_draft(
                &task,
                json!({"key": "MY-KEY", "profile": null, "config": null}),
            ),
        ),
        (
            "profile with a space",
            secret_draft(&task, json!({"key": "K", "profile": "a b", "config": null})),
        ),
        (
            "relative config",
            secret_draft(
                &task,
                json!({"key": "K", "profile": null, "config": "fnox.toml"}),
            ),
        ),
        (
            "missing config",
            secret_draft(
                &task,
                json!({"key": "K", "profile": null, "config": dir.0.join("nope.toml").to_string_lossy()}),
            ),
        ),
        (
            "directory config",
            secret_draft(
                &task,
                json!({"key": "K", "profile": null, "config": dir.0.to_string_lossy()}),
            ),
        ),
        ("no target", secret_draft(&task, Value::Null)),
    ];
    if cfg!(unix) {
        cases.push((
            "symlinked config",
            secret_draft(
                &task,
                json!({"key": "K", "profile": null, "config": link.to_string_lossy()}),
            ),
        ));
    }
    let mut lead = secret_draft(&task, global());
    lead["audience"] = json!("lead");
    cases.push(("lead audience", lead));
    let mut unrouted = secret_draft(&task, global());
    unrouted.as_object_mut().expect("object").remove("audience");
    cases.push(("no audience", unrouted));
    let mut options = secret_draft(&task, global());
    options["options"] = json!(["a"]);
    cases.push(("options", options));
    let mut plain = secret_draft(&task, global());
    plain["kind"] = json!("question");
    cases.push(("secret on a plain question", plain));

    for (label, draft) in cases {
        let out = ask(&dir, &draft);
        assert_eq!(out.code, 1, "{label}: {}", out.stderr);
        assert!(
            out.stderr.contains("a secret question needs audience user"),
            "{label}: {}",
            out.stderr
        );
    }
    let listed = stdout_json(&cli(&["questions", "--state-dir", &dir.state()]));
    assert_eq!(listed, json!([]), "nothing was saved");
}

#[test]
fn no_cli_answer_or_route_takes_a_secret_question() {
    let dir = TempDir::new("refuse");
    let task = task_id(&dir);
    let asked = ask(&dir, &secret_draft(&task, global()));
    assert_eq!(asked.code, 0, "{}", asked.stderr);
    let revision = stdout_json(&asked)["saved"]["record"]["reference"]["revision"].clone();

    let answer = dir.write(
        "answer.json",
        &json!({
            "expected_revision": revision,
            "text": "set",
            "actor": "thr-a",
            "origin": "agent",
            "corrects_revision": null,
            "approve": false,
            "confirmed": false,
            "approval": null,
            "user_chat": null,
            "source": null
        }),
    );
    let out = cli(&[
        "answer",
        "Q1",
        "--file",
        &answer,
        "--state-dir",
        &dir.state(),
    ]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr
            .starts_with("tsk questions: secret-needs-board: a secret is entered only in the tsk board questions tab"),
        "{}",
        out.stderr
    );

    let pick = dir.write(
        "pick.json",
        &json!({"expected_revision": revision, "text": "hunter2"}),
    );
    let out = cli(&[
        "ui-answer",
        "Q1",
        "--file",
        &pick,
        "--state-dir",
        &dir.state(),
    ]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr
            .starts_with("tsk ui-answer: secret-needs-board: "),
        "{}",
        out.stderr
    );

    let route = dir.write(
        "route.json",
        &json!({"expected_revision": revision, "audience": "lead"}),
    );
    let out = cli(&["route", "Q1", "--file", &route, "--state-dir", &dir.state()]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("audience user"), "{}", out.stderr);

    let record = stdout_json(&cli(&["questions", "Q1", "--state-dir", &dir.state()]));
    assert_eq!(record["answers"], json!([]), "the question stays open");
    assert_eq!(record["reference"]["revision"], revision);
}
