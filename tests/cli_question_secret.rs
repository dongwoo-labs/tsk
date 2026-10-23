//! Secret questions from the CLI side: `tsk ask` validates the fnox target, readback shows
//! only the target. `answer` and `ui-answer` cannot take a value; `secret-result` records
//! only a fixed result after a name-only fnox check. Temp dirs only.

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

#[test]
fn tsk_answer_cannot_reject_a_secret_even_with_user_ui_shaped_reject_text() {
    let dir = TempDir::new("reject-agent");
    let task = task_id(&dir);
    let asked = ask(&dir, &secret_draft(&task, global()));
    assert_eq!(asked.code, 0, "{}", asked.stderr);
    let revision = stdout_json(&asked)["saved"]["record"]["reference"]["revision"].clone();

    // `tsk answer` refuses every secret question outright, before the draft is even
    // parsed: an agent has no way to close a secret, reject or otherwise.
    let agent_reject = dir.write(
        "agent-reject.json",
        &json!({
            "expected_revision": revision,
            "text": "rejected",
            "actor": "thr-a",
            "origin": "agent",
            "corrects_revision": null,
            "approve": false,
            "confirmed": false,
            "approval": null,
            "user_chat": null,
            "source": null,
            "reject": true
        }),
    );
    let out = cli(&[
        "answer",
        "Q1",
        "--file",
        &agent_reject,
        "--state-dir",
        &dir.state(),
    ]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr
            .starts_with("tsk questions: secret-needs-board: "),
        "{}",
        out.stderr
    );
    let record = stdout_json(&cli(&["questions", "Q1", "--state-dir", &dir.state()]));
    assert_eq!(record["answers"], json!([]), "the question stays open");
}

#[cfg(unix)]
mod external_result {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    fn fake_fnox(dir: &TempDir, output: &str, exit: i32) -> PathBuf {
        let path = dir.0.join("fnox");
        fs::create_dir_all(dir.0.join("config/fnox")).unwrap();
        fs::write(dir.0.join("config/fnox/config.toml"), "").unwrap();
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{}'\nprintf 'diagnostic must stay private\\n' >&2\nexit {exit}\n",
                dir.0.join("argv").display(), output
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn run(dir: &TempDir, bin: &std::path::Path, input: &Value) -> std::process::Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tsk"))
            .args([
                "secret-result",
                "Q1",
                "--file",
                "-",
                "--state-dir",
                &dir.state(),
            ])
            .env("TSK_FNOX_BIN", bin)
            .env("HERDR_ENV", "0")
            .env("HOME", &dir.0)
            .env("XDG_CONFIG_HOME", dir.0.join("config"))
            .env("FNOX_PROFILE", "wrong-profile")
            .env_remove("FNOX_CONFIG_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(input).unwrap())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn open_secret(dir: &TempDir, target: Value) -> Value {
        let task = task_id(dir);
        let asked = ask(dir, &secret_draft(&task, target));
        assert_eq!(asked.code, 0, "{}", asked.stderr);
        stdout_json(&asked)["saved"]["record"]["reference"]["revision"].clone()
    }

    fn read(dir: &TempDir) -> Value {
        stdout_json(&cli(&["questions", "Q1", "--state-dir", &dir.state()]))
    }

    /// Real CLI acceptance: synthetic local configs only; no provider or notification service.
    /// Run with `cargo test --test integration cli_question_secret::external_result::real_fnox_external_result -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires fnox 1.36+"]
    fn real_fnox_external_result() {
        const FIXTURE_VALUE: &str = "synthetic-nonauth-fixture-only";
        const REJECTED_VALUE: &str = "synthetic-invalid-field-only";

        fn command(dir: &TempDir, executable: &str) -> Command {
            let mut cmd = Command::new(executable);
            cmd.env_clear()
                .env(
                    "PATH",
                    std::env::var_os("PATH").expect("PATH for real fnox"),
                )
                .env("HOME", dir.0.join("home"))
                .env("XDG_CONFIG_HOME", dir.0.join("xdg-config"))
                .env("FNOX_CONFIG_DIR", dir.0.join("global-fnox"))
                .env("XDG_STATE_HOME", dir.0.join("xdg-state"))
                .env("TSK_STATE_DIR", dir.state())
                .env("HERDR_ENV", "0")
                // A wrong inherited environment profile must not override the stored target.
                .env("FNOX_PROFILE", "missing-env-profile")
                .current_dir(dir.0.join("workdir"))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            cmd
        }

        fn invoke(dir: &TempDir, args: &[&str], input: Option<&Value>) -> std::process::Output {
            let mut cmd = command(dir, env!("CARGO_BIN_EXE_tsk"));
            cmd.args(args).args(["--state-dir", &dir.state()]);
            let bytes = input.map(|body| serde_json::to_vec(body).unwrap());
            if bytes.is_some() {
                cmd.stdin(Stdio::piped());
            }
            let mut child = cmd.spawn().expect("start real tsk");
            if let Some(bytes) = bytes {
                assert!(!String::from_utf8_lossy(&bytes).contains(FIXTURE_VALUE));
                child.stdin.take().unwrap().write_all(&bytes).unwrap();
            }
            let out = child.wait_with_output().unwrap();
            for bytes in [&out.stdout, &out.stderr] {
                let text = String::from_utf8_lossy(bytes);
                assert!(!text.contains(FIXTURE_VALUE), "fixture value leaked");
                assert!(!text.contains(REJECTED_VALUE), "rejected field leaked");
            }
            out
        }

        fn read_record(dir: &TempDir, q: &str) -> Value {
            let out = invoke(dir, &["questions", q], None);
            assert!(out.status.success(), "fresh questions read failed");
            serde_json::from_slice(&out.stdout).expect("questions JSON")
        }

        fn refused(dir: &TempDir, q: &str, input: &Value, code: i32, error: &str) {
            let before = read_record(dir, q);
            let out = invoke(dir, &["secret-result", q, "--file", "-"], Some(input));
            assert_eq!(out.status.code(), Some(code), "{q}: {error}");
            assert!(out.stdout.is_empty());
            assert!(String::from_utf8_lossy(&out.stderr).contains(error));
            assert_eq!(read_record(dir, q), before, "refusal changed the record");
            println!("PASS {q} refusal={error} exit={code} fresh-read=unchanged");
        }

        fn saved(dir: &TempDir, q: &str, revision: &Value) -> Value {
            let out = invoke(
                dir,
                &["secret-result", q, "--file", "-"],
                Some(&json!({"expected_revision": revision, "result": "set"})),
            );
            assert!(out.status.success(), "{q}: secret-result must succeed");
            assert!(out.stderr.is_empty());
            let result: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(result["saved"]["durability"], "durable");
            let record = read_record(dir, q);
            assert_eq!(result["saved"]["record"], record);
            assert_ne!(record["reference"]["revision"], *revision);
            assert_eq!(record["answers"].as_array().unwrap().len(), 1);
            let answer = &record["answers"][0];
            assert_eq!(answer["text"], "set");
            assert_eq!(answer["actor"], "user");
            assert_eq!(answer["origin"], "user_ui");
            for field in ["approval", "user_chat", "source", "corrects_revision"] {
                assert!(answer[field].is_null());
            }
            println!("PASS {q} durable set actor=user origin=user_ui fresh-read=matched");
            record
        }

        fn check_state(path: &std::path::Path) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    check_state(&path);
                } else {
                    let bytes = fs::read(path).unwrap();
                    let text = String::from_utf8_lossy(&bytes);
                    assert!(!text.contains(FIXTURE_VALUE), "fixture value in tsk state");
                    assert!(
                        !text.contains(REJECTED_VALUE),
                        "rejected field in tsk state"
                    );
                }
            }
        }

        // Keep all config discovery and writes under one owned /tmp root, even on macOS.
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = TempDir(PathBuf::from("/tmp").join(format!(
            "tsk-cli-secret-real-{}-{seq}",
            uuid::Uuid::new_v4()
        )));
        fs::create_dir(&dir.0).unwrap();
        for name in [
            "home",
            "xdg-config",
            "global-fnox",
            "xdg-state",
            "state",
            "workdir",
            "project",
        ] {
            fs::create_dir(dir.0.join(name)).unwrap();
        }
        fs::write(
            dir.0.join("global-fnox/config.toml"),
            format!(
                "[secrets]\nGLOBAL_ONLY = {{ value = {FIXTURE_VALUE:?} }}\nOTHER_ABSENT_KEY = {{ value = {FIXTURE_VALUE:?} }}\nABSENT_KEY_SUFFIX = {{ value = {FIXTURE_VALUE:?} }}\n"
            ),
        )
        .unwrap();
        fs::write(
            dir.0.join("workdir/fnox.toml"),
            format!("[secrets]\nCWD_ONLY = {{ value = {FIXTURE_VALUE:?} }}\n"),
        )
        .unwrap();
        let config = dir.0.join("project/fnox.toml");
        fs::write(
            &config,
            format!(
                "[profiles.base.secrets]\nNAMED_ONLY = {{ value = {FIXTURE_VALUE:?} }}\n[profiles.stage]\ninherits = [\"base\"]\n"
            ),
        )
        .unwrap();
        let version = command(&dir, "fnox")
            .arg("--version")
            .output()
            .expect("install fnox 1.36+");
        assert!(version.status.success(), "fnox --version failed");
        let version = String::from_utf8(version.stdout).unwrap();
        let numbers: Vec<u64> = version
            .split_whitespace()
            .nth(1)
            .expect("fnox version")
            .split('.')
            .take(2)
            .map(|part| part.parse().expect("numeric fnox version"))
            .collect();
        assert!(
            numbers.as_slice() >= [1, 36].as_slice(),
            "requires fnox 1.36+"
        );
        println!(
            "{} data=synthetic/local mock=none notifications=disabled",
            version.trim()
        );

        let added = invoke(&dir, &["add", "-t", "real fnox acceptance", "--desk"], None);
        assert!(added.status.success());
        let listed = invoke(&dir, &["list", "T1", "--json"], None);
        assert!(listed.status.success());
        let task: Value = serde_json::from_slice(&listed.stdout).unwrap();
        let task = task[0]["id"].as_str().unwrap();
        let cases = [
            ("GLOBAL_ONLY", None, None, None),
            ("NAMED_ONLY", Some("stage"), Some(&config), None),
            ("CWD_ONLY", None, None, Some("secret-not-found")),
            ("ABSENT_KEY", None, None, Some("secret-not-found")),
            ("NAMED_ONLY", None, Some(&config), Some("secret-not-found")),
            (
                "NAMED_ONLY",
                Some("does-not-exist"),
                Some(&config),
                Some("fnox-check-failed"),
            ),
            // Global config inheritance is effective even without a direct declaration.
            ("GLOBAL_ONLY", Some("stage"), Some(&config), None),
        ];
        for (index, (key, profile, config, error)) in cases.into_iter().enumerate() {
            let draft = secret_draft(
                task,
                json!({"key": key, "profile": profile, "config": config}),
            );
            let asked = invoke(&dir, &["ask", "--file", "-"], Some(&draft));
            assert!(asked.status.success(), "ask fixture question");
            let ack: Value = serde_json::from_slice(&asked.stdout).unwrap();
            assert_eq!(ack["saved"]["durability"], "durable");
            let q = format!("Q{}", index + 1);
            let before = read_record(&dir, &q);
            let revision = &before["reference"]["revision"];
            let input = json!({"expected_revision": revision, "result": "set"});
            if index == 0 {
                let mut unknown = input.clone();
                unknown["value"] = json!(REJECTED_VALUE);
                refused(&dir, &q, &unknown, 2, "invalid-input");
                let mut stale = input.clone();
                stale["expected_revision"] = json!(uuid::Uuid::new_v4());
                refused(&dir, &q, &stale, 1, "stale-revision");
                for verb in ["answer", "ui-answer"] {
                    let body = if verb == "answer" {
                        json!({"expected_revision": revision, "text": "set", "actor": "thr-a", "origin": "agent"})
                    } else {
                        json!({"expected_revision": revision, "text": "set"})
                    };
                    let out = invoke(&dir, &[verb, &q, "--file", "-"], Some(&body));
                    assert_eq!(out.status.code(), Some(1));
                    assert!(String::from_utf8_lossy(&out.stderr).contains("secret-needs-board"));
                    assert_eq!(read_record(&dir, &q), before);
                }
            }
            if let Some(error) = error {
                refused(&dir, &q, &input, 1, error);
            } else {
                // Both supported selectors cross the actual process boundary.
                let selector = if index == 1 {
                    before["reference"]["id"].as_str().unwrap()
                } else {
                    &q
                };
                let record = saved(&dir, selector, revision);
                if index == 0 {
                    refused(&dir, &q, &input, 1, "stale-revision");
                    let current = json!({"expected_revision": record["reference"]["revision"], "result": "set"});
                    refused(&dir, &q, &current, 1, "already-answered");
                }
            }
        }
        check_state(&dir.0.join("state"));
        println!(
            "PASS values absent from every tsk state file; answer/ui-answer restrictions unchanged"
        );
    }

    #[test]
    fn a_checked_external_result_records_exactly_the_board_answer_and_cannot_repeat() {
        let dir = TempDir::new("external-set");
        let revision = open_secret(&dir, global());
        let bin = fake_fnox(&dir, "OTHER\nSTAGING_API_KEY", 0);
        let input = json!({"expected_revision": revision, "result": "set"});
        let out = run(&dir, &bin, &input);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stderr.is_empty());
        let saved: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(saved["saved"]["durability"], "durable");
        let record = read(&dir);
        let answer = &record["answers"][0];
        assert_eq!(record["answers"].as_array().unwrap().len(), 1);
        assert_eq!(answer["text"], "set");
        assert_eq!(answer["actor"], "user");
        assert_eq!(answer["origin"], "user_ui");
        assert!(answer["approval"].is_null());
        assert!(answer["user_chat"].is_null());
        assert!(answer["source"].is_null());
        assert!(answer["corrects_revision"].is_null());
        assert_ne!(record["reference"]["revision"], revision);
        let argv = fs::read_to_string(dir.0.join("argv")).unwrap();
        assert!(argv.lines().any(|a| a == "list"));
        assert!(
            argv.lines().any(|a| a == "--complete"),
            "name-only mode is mandatory"
        );
        assert!(!argv
            .lines()
            .any(|a| matches!(a, "get" | "set" | "--values" | "-V")));
        assert!(argv.contains("default"), "pin default despite FNOX_PROFILE");
        let repeated = run(&dir, &bin, &input);
        assert_eq!(repeated.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&repeated.stderr).contains("stale-revision"));
        let mut current = input;
        current["expected_revision"] = record["reference"]["revision"].clone();
        let repeated = run(&dir, &bin, &current);
        assert_eq!(repeated.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&repeated.stderr).contains("already-answered"));
        assert_eq!(read(&dir), record);
    }

    #[test]
    fn invalid_result_bodies_refuse_without_echo_or_fnox_or_state_changes() {
        let dir = TempDir::new("external-shape");
        let revision = open_secret(&dir, global());
        let before = read(&dir);
        let bin = fake_fnox(&dir, "STAGING_API_KEY", 0);
        let valid = json!({"expected_revision": revision, "result": "set"});
        let mut cases = vec![
            json!([revision, "set"]),
            json!({"expected_revision": revision, "result": {"set": null}}),
            json!({"expected_revision": revision, "result": "failed"}),
            json!({"expected_revision": revision}),
            json!({"expected_revision": "not-a-uuid", "result": "set"}),
        ];
        for field in [
            "value",
            "text",
            "actor",
            "origin",
            "secret",
            "comment",
            "source",
            "user_chat",
            "approval",
            "corrects_revision",
        ] {
            let mut body = valid.clone();
            body[field] = json!("INPUT_MUST_NOT_BE_ECHOED");
            cases.push(body);
        }
        for body in cases {
            let out = run(&dir, &bin, &body);
            assert_eq!(
                out.status.code(),
                Some(2),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(out.stdout.is_empty());
            let error = String::from_utf8_lossy(&out.stderr);
            assert!(error.contains("secret-result: invalid-input"), "{error}");
            assert!(!error.contains("INPUT_MUST_NOT_BE_ECHOED"));
            assert!(!dir.0.join("argv").exists());
            assert_eq!(read(&dir), before);
        }
    }

    #[test]
    #[ignore = "waits for the 30-second fnox deadline"]
    fn fnox_stdout_from_a_descendant_cannot_extend_the_deadline() {
        let dir = TempDir::new("stdout-deadline");
        let revision = open_secret(&dir, global());
        let before = read(&dir);
        let bin = fake_fnox(&dir, "", 0);
        let pid_file = dir.0.join("descendant.pid");
        fs::write(
            &bin,
            format!(
                "#!/bin/sh\nsleep 40 &\nprintf '%s' \"$!\" > '{}'\nprintf 'STAGING_API_KEY\\n'\nexit 0\n",
                pid_file.display()
            ),
        ).unwrap();
        let started = std::time::Instant::now();
        let out = run(
            &dir,
            &bin,
            &json!({"expected_revision":revision,"result":"set"}),
        );
        let elapsed = started.elapsed();
        // Stop only the background fixture this invocation created, before any assertion.
        if let Ok(pid) = fs::read_to_string(pid_file) {
            let _ = Command::new("kill")
                .arg(pid.trim())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        assert!(
            elapsed < std::time::Duration::from_secs(35),
            "deadline exceeded: {elapsed:?}"
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("fnox-check-failed"));
        assert_eq!(read(&dir), before);
    }

    #[test]
    fn missing_exact_key_and_failed_fnox_keep_the_question_open() {
        for (label, listing, exit, refusal) in [
            (
                "missing",
                "OTHER_STAGING_API_KEY\nSTAGING_API_KEY_SUFFIX",
                0,
                "secret-not-found",
            ),
            ("empty", "", 0, "secret-not-found"),
            ("failure", "STAGING_API_KEY", 3, "fnox-check-failed"),
        ] {
            let dir = TempDir::new(label);
            let revision = open_secret(&dir, global());
            let before = read(&dir);
            let bin = fake_fnox(&dir, listing, exit);
            let out = run(
                &dir,
                &bin,
                &json!({"expected_revision": revision, "result":"set"}),
            );
            assert_eq!(out.status.code(), Some(1));
            assert!(out.stdout.is_empty());
            let error = String::from_utf8_lossy(&out.stderr);
            assert!(error.contains(refusal), "{label}: {error}");
            assert!(!error.contains("diagnostic must stay private"));
            assert_eq!(read(&dir), before);
        }
    }

    #[test]
    fn stale_nonsecret_and_vanished_target_refuse_before_running_fnox() {
        for label in ["stale", "nonsecret", "vanished"] {
            let dir = TempDir::new(label);
            let config = dir.0.join("fnox.toml");
            fs::write(&config, "").unwrap();
            let revision = if label == "nonsecret" {
                let task = task_id(&dir);
                let mut draft = secret_draft(&task, Value::Null);
                draft["kind"] = json!("question");
                let asked = ask(&dir, &draft);
                assert_eq!(asked.code, 0);
                stdout_json(&asked)["saved"]["record"]["reference"]["revision"].clone()
            } else {
                open_secret(
                    &dir,
                    json!({"key":"STAGING_API_KEY","profile":"stage","config": config}),
                )
            };
            let before = read(&dir);
            if label == "vanished" {
                fs::remove_file(&config).unwrap();
            }
            let bin = fake_fnox(&dir, "STAGING_API_KEY", 0);
            let expected = if label == "stale" {
                json!(uuid::Uuid::new_v4())
            } else {
                revision
            };
            let out = run(
                &dir,
                &bin,
                &json!({"expected_revision": expected, "result":"set"}),
            );
            assert_eq!(out.status.code(), Some(1));
            let error = String::from_utf8_lossy(&out.stderr);
            let code = match label {
                "stale" => "stale-revision",
                "nonsecret" => "secret-required",
                _ => "invalid-secret",
            };
            assert!(error.contains(code), "{label}: {error}");
            assert!(!dir.0.join("argv").exists());
            assert_eq!(read(&dir), before);
        }
    }
}
