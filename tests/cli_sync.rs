//! `tsk sync` and `tsk merge-driver` through the real binary and real git.
//! Every git call runs with a throwaway HOME and no global or system git config, against
//! a local bare repository standing in for the remote.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::domain::HumanStatus;
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Option<Self> {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git is not available; skipping {label}");
            return None;
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("tsk-sync-{label}-{nanos}-{seq}"));
        fs::create_dir_all(root.join("home")).expect("create sandbox");
        Some(Self { root })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn isolate(&self, command: &mut Command) {
        command
            .env("HOME", self.path("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "tsk test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "tsk test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("XDG_CONFIG_HOME");
    }

    fn git(&self, args: &[&str]) -> Output {
        let mut command = Command::new("git");
        command.args(args).current_dir(&self.root);
        self.isolate(&mut command);
        let output = command.output().expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn tsk(&self, state: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tsk"));
        command
            .args(args)
            .current_dir(&self.root)
            .env("TSK_STATE_DIR", state)
            .env("TSK_NO_UPDATE_CHECK", "1");
        self.isolate(&mut command);
        command.output().expect("run tsk")
    }

    fn tsk_ok(&self, state: &Path, args: &[&str]) -> String {
        let output = self.tsk(state, args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "tsk {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("UTF-8 output")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// `(number, title, status)` by task id, as a board shows it.
fn board(state: &Path) -> BTreeMap<uuid::Uuid, (Option<u64>, String, HumanStatus)> {
    TaskStore::new(state)
        .load()
        .expect("load board")
        .tasks()
        .iter()
        .map(|task| (task.id, (task.number, task.title.clone(), task.status)))
        .collect()
}

#[test]
fn two_clones_converge_with_unique_numbers_and_the_later_edit() {
    let Some(sandbox) = Sandbox::new("converge") else {
        return;
    };
    // A bare remote created without config names a branch nobody pushes, so the clone
    // below checks nothing out: init must still join the board already there.
    sandbox.git(&["init", "--quiet", "--bare", "remote.git"]);
    let remote = sandbox.path("remote.git");
    let a = sandbox.path("a");
    let b = sandbox.path("b");

    sandbox.tsk_ok(&a, &["add", "-t", "shared task", "--desk"]);
    sandbox.tsk_ok(&a, &["sync", "--init", remote.to_str().unwrap()]);
    let first = sandbox.tsk_ok(&a, &["sync"]);
    assert!(first.contains("pushed to origin/main"), "{first}");

    sandbox.git(&["clone", "--quiet", remote.to_str().unwrap(), "b"]);
    sandbox.tsk_ok(&b, &["sync", "--init"]);
    sandbox.tsk_ok(&b, &["sync"]);
    assert_eq!(board(&a), board(&b), "the clone starts from A's board");

    sandbox.tsk_ok(&a, &["add", "-t", "only on a", "--desk"]);
    sandbox.tsk_ok(&b, &["add", "-t", "only on b", "--desk"]);
    let original_a = board(&a);
    let original_b = board(&b);
    sandbox.tsk_ok(&a, &["status", "T1", "started"]);
    // Edited after A's edit, so B's status wins the both-sides change.
    sandbox.tsk_ok(&b, &["status", "T1", "blocked"]);

    sandbox.tsk_ok(&a, &["sync"]);
    // B keeps its own newer edit and its own T2; A's new task arrives renumbered.
    let merged = sandbox.tsk_ok(&b, &["sync"]);
    assert!(
        merged.contains("merged origin/main: 1 added, 0 updated, 0 removed"),
        "{merged}"
    );
    // A takes B's edit to T1 and the renumber of its own new task.
    let pulled = sandbox.tsk_ok(&a, &["sync"]);
    assert!(
        pulled.contains("merged origin/main: 1 added, 2 updated, 0 removed"),
        "{pulled}"
    );
    let settled = sandbox.tsk_ok(&b, &["sync", "--json"]);
    assert!(settled.contains("\"pushed\":false"), "{settled}");

    let on_a = board(&a);
    assert_eq!(on_a, board(&b), "both boards converge");
    assert_eq!(on_a.len(), 3);
    let a_id = *original_a
        .iter()
        .find(|(_, row)| row.1 == "only on a")
        .unwrap()
        .0;
    let b_id = *original_b
        .iter()
        .find(|(_, row)| row.1 == "only on b")
        .unwrap()
        .0;
    assert_eq!(original_a[&a_id].0, Some(2));
    assert_eq!(original_b[&b_id].0, Some(2));
    assert_eq!(
        on_a[&a_id].0,
        Some(3),
        "incoming A task keeps its id but changes number"
    );
    assert_eq!(on_a[&b_id].0, Some(2), "B's local task retains its number");
    let numbers: BTreeSet<u64> = on_a.values().filter_map(|(number, ..)| *number).collect();
    assert_eq!(numbers, BTreeSet::from([1, 2, 3]), "numbers are unique");
    let shared = on_a
        .values()
        .find(|(_, title, _)| title == "shared task")
        .expect("shared task");
    assert_eq!(shared.0, Some(1));
    assert_eq!(shared.2, HumanStatus::Blocked);
    let titles: BTreeSet<&str> = on_a.values().map(|(_, title, _)| title.as_str()).collect();
    assert!(titles.contains("only on a") && titles.contains("only on b"));
}

fn pair(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    sandbox.git(&[
        "init",
        "--quiet",
        "--bare",
        "--initial-branch=main",
        "remote.git",
    ]);
    let a = sandbox.path("a");
    let b = sandbox.path("b");
    sandbox.tsk_ok(&a, &["add", "-t", "shared", "--desk"]);
    sandbox.tsk_ok(
        &a,
        &[
            "sync",
            "--init",
            sandbox.path("remote.git").to_str().unwrap(),
        ],
    );
    sandbox.tsk_ok(&a, &["sync"]);
    sandbox.git(&[
        "clone",
        "--quiet",
        sandbox.path("remote.git").to_str().unwrap(),
        "b",
    ]);
    let read = sandbox.tsk_ok(&b, &["list", "--all", "--json"]);
    assert!(
        read.contains("shared"),
        "a fresh independent clone can read without sync"
    );
    sandbox.tsk_ok(&b, &["sync", "--init"]);
    (a, b)
}

fn timestamp(state: &Path, seconds: u64) {
    let path = state.join("tsk.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    document["tasks"][0]["updated_at"] = serde_json::json!([seconds, 0]);
    fs::write(path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
}

fn settle(sandbox: &Sandbox, a: &Path, b: &Path) {
    sandbox.tsk_ok(a, &["sync"]);
    sandbox.tsk_ok(b, &["sync"]);
    sandbox.tsk_ok(a, &["sync"]);
    sandbox.tsk_ok(b, &["sync"]);
    assert_eq!(board(a), board(b));
}

#[test]
fn concurrent_edits_obey_skewed_clock_and_tie_keeps_the_merging_side() {
    let Some(sandbox) = Sandbox::new("clock") else {
        return;
    };
    let (a, b) = pair(&sandbox);
    sandbox.tsk_ok(&a, &["edit", "T1", "--title", "future clock a"]);
    timestamp(&a, 2_000_000_000);
    sandbox.tsk_ok(&b, &["edit", "T1", "--title", "later real edit b"]);
    timestamp(&b, 1_000_000_000);
    settle(&sandbox, &a, &b);
    assert_eq!(board(&a).values().next().unwrap().1, "future clock a");
    sandbox.tsk_ok(&a, &["edit", "T1", "--title", "tie a"]);
    timestamp(&a, 2_100_000_000);
    sandbox.tsk_ok(&b, &["edit", "T1", "--title", "tie b"]);
    timestamp(&b, 2_100_000_000);
    settle(&sandbox, &a, &b);
    assert_eq!(board(&a).values().next().unwrap().1, "tie b");
}

#[test]
fn merge_conflict_aborts_without_changing_store_bytes_and_can_retry() {
    let Some(sandbox) = Sandbox::new("rollback") else {
        return;
    };
    let (a, b) = pair(&sandbox);
    let attrs = fs::read_to_string(a.join(".gitattributes")).unwrap();
    fs::write(
        a.join(".gitattributes"),
        attrs.replace("tsk.json merge=tsk", "tsk.json merge=tsk -text"),
    )
    .unwrap();
    fs::write(
        b.join(".gitattributes"),
        attrs.replace("tsk.json merge=tsk", "tsk.json merge=tsk text"),
    )
    .unwrap();
    sandbox.tsk_ok(&a, &["edit", "T1", "--title", "local rollback"]);
    sandbox.tsk_ok(&b, &["edit", "T1", "--title", "remote rollback"]);
    sandbox.tsk_ok(&b, &["sync"]);
    let before = fs::read(a.join("tsk.json")).unwrap();
    let refused = sandbox.tsk(&a, &["sync"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("board here is unchanged"));
    assert_eq!(fs::read(a.join("tsk.json")).unwrap(), before);
    assert!(!a.join(".git/MERGE_HEAD").exists());
    assert!(sandbox
        .git(&["-C", a.to_str().unwrap(), "ls-files", "-u"])
        .stdout
        .is_empty());
    fs::write(
        b.join(".gitattributes"),
        fs::read(a.join(".gitattributes")).unwrap(),
    )
    .unwrap();
    sandbox.tsk_ok(&b, &["sync"]);
    settle(&sandbox, &a, &b);
    // Fetch fails after local commit, but the user's exact file remains safe.
    fs::rename(sandbox.path("remote.git"), sandbox.path("offline.git")).unwrap();
    let before = fs::read(a.join("tsk.json")).unwrap();
    let failed = sandbox.tsk(&a, &["sync"]);
    assert_eq!(failed.status.code(), Some(3));
    assert_eq!(fs::read(a.join("tsk.json")).unwrap(), before);
    fs::rename(sandbox.path("offline.git"), sandbox.path("remote.git")).unwrap();
    settle(&sandbox, &a, &b);
}

#[test]
fn trash_union_and_delete_against_edit_keep_the_edited_task() {
    let Some(sandbox) = Sandbox::new("trash") else {
        return;
    };
    let (a, b) = pair(&sandbox);
    sandbox.tsk_ok(&a, &["add", "-t", "remove a", "--desk"]);
    sandbox.tsk_ok(&a, &["add", "-t", "remove b", "--desk"]);
    sandbox.tsk_ok(&a, &["add", "-t", "anchor", "--desk"]);
    settle(&sandbox, &a, &b);
    let ids: Vec<_> = TaskStore::new(&a)
        .load()
        .unwrap()
        .tasks()
        .iter()
        .map(|task| task.id)
        .collect();
    // The CLI has no delete verb. Seed real trash through the existing store boundary.
    for (path, deleted) in [(&a, ids[0]), (&b, ids[2])] {
        let store = TaskStore::new(path);
        let mut state = store.load().unwrap();
        state.soft_delete(deleted).unwrap();
        state.soft_delete(ids[1]).unwrap();
        state.complete(ids[3]).unwrap();
        store.save(&state).unwrap();
    }
    sandbox.tsk_ok(&b, &["edit", "T1", "--title", "survives deletion"]);
    settle(&sandbox, &a, &b);
    let final_board = board(&a);
    assert_eq!(final_board[&ids[0]].1, "survives deletion");
    assert!(!final_board.contains_key(&ids[1]));
    for path in [&a, &b] {
        let deleted = sandbox.tsk_ok(path, &["list", "--deleted", "--all", "--json"]);
        assert!(
            deleted.contains("remove a") && deleted.contains("remove b"),
            "{deleted}"
        );
        let text = fs::read_to_string(path.join("trash.jsonl")).unwrap();
        for id in &ids[..3] {
            assert!(text.contains(&id.to_string()), "missing trash id {id}");
        }
    }
}

#[cfg(unix)]
#[test]
fn sync_holds_the_store_lock_until_push_finishes() {
    use std::os::unix::fs::PermissionsExt;
    use std::thread;
    use std::time::{Duration, Instant};
    let Some(sandbox) = Sandbox::new("push-lock") else {
        return;
    };
    let (a, _) = pair(&sandbox);
    sandbox.tsk_ok(&a, &["edit", "T1", "--title", "pending push"]);
    let ready = sandbox.path("push-ready");
    let release = sandbox.path("push-release");
    let hook = a.join(".git/hooks/pre-push");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.02; done\n",
            ready.display(),
            release.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_tsk"));
    command
        .args(["sync"])
        .current_dir(&sandbox.root)
        .env("TSK_STATE_DIR", &a)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    sandbox.isolate(&mut command);
    let mut sync = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !ready.exists() {
        sync.kill().unwrap();
        sync.wait().unwrap();
        panic!("push hook did not start");
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_tsk"));
    command
        .args(["status", "T1", "started"])
        .current_dir(&sandbox.root)
        .env("TSK_STATE_DIR", &a)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    sandbox.isolate(&mut command);
    let mut writer = command.spawn().unwrap();
    thread::sleep(Duration::from_millis(200));
    let wrote_before_push_finished = writer.try_wait().unwrap().is_some();
    fs::write(&release, "release").unwrap();
    assert!(sync.wait().unwrap().success());
    assert!(writer.wait().unwrap().success());
    assert!(
        !wrote_before_push_finished,
        "a second CLI writer changed the store while sync was pushing"
    );
    assert_eq!(board(&a).values().next().unwrap().2, HumanStatus::Started);
}

#[cfg(unix)]
#[test]
fn push_race_retries_once_and_a_second_remote_move_stops_safely() {
    use std::os::unix::fs::PermissionsExt;
    let Some(sandbox) = Sandbox::new("push-race") else {
        return;
    };
    let (a, b) = pair(&sandbox);
    let c = sandbox.path("c");
    sandbox.git(&[
        "clone",
        "--quiet",
        sandbox.path("remote.git").to_str().unwrap(),
        "c",
    ]);
    sandbox.tsk_ok(&c, &["sync", "--init"]);
    let hook = a.join(".git/hooks/pre-push");
    let count = sandbox.path("push-count");
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let install_hook = |limit: u32| {
        fs::write(&hook, format!(
            "#!/bin/sh\nset -eu\ncount=0\n[ ! -f {counter} ] || count=$(cat {counter})\ncount=$((count+1))\nprintf '%s\\n' \"$count\" > {counter}\nif [ \"$count\" -le {limit} ]; then\n unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE\n export TSK_STATE_DIR={state}\n {binary} add -t \"race $count limit {limit}\" --desk\n {binary} sync\nfi\n",
            counter=quote(&count), state=quote(&c), binary=quote(Path::new(env!("CARGO_BIN_EXE_tsk")))
        )).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    };
    install_hook(1);
    sandbox.tsk_ok(&a, &["add", "-t", "race local one", "--desk"]);
    sandbox.tsk_ok(&a, &["sync"]);
    assert_eq!(fs::read_to_string(&count).unwrap().trim(), "2");
    settle(&sandbox, &a, &b);
    fs::write(&count, "0").unwrap();
    install_hook(2);
    sandbox.tsk_ok(&a, &["add", "-t", "race local two", "--desk"]);
    let failed = sandbox.tsk(&a, &["sync"]);
    assert_eq!(failed.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("push was rejected twice"));
    assert_eq!(fs::read_to_string(&count).unwrap().trim(), "2");
    assert!(sandbox
        .tsk_ok(&a, &["list", "--all", "--json"])
        .contains("race local two"));
    assert!(!a.join(".git/MERGE_HEAD").exists());
    fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
    settle(&sandbox, &a, &b);
    assert!(board(&a).values().any(|row| row.1 == "race local two"));
}

#[test]
fn init_is_idempotent_and_installs_the_merge_driver() {
    let Some(sandbox) = Sandbox::new("init") else {
        return;
    };
    let state = sandbox.path("state");
    let first = sandbox.tsk_ok(&state, &["sync", "--init"]);
    assert!(first.contains("committed"), "{first}");
    let second = sandbox.tsk_ok(&state, &["sync", "--init"]);
    assert!(second.contains("nothing to commit"), "{second}");

    let gitignore = fs::read_to_string(state.join(".gitignore")).unwrap();
    assert_eq!(gitignore.matches("tsk.json.lock").count(), 1);
    assert!(gitignore.contains("delivery.json"));
    let attributes = fs::read_to_string(state.join(".gitattributes")).unwrap();
    assert!(attributes.contains("tsk.json merge=tsk"));
    assert!(attributes.contains("trash.jsonl merge=union"));
    let log = sandbox.git(&["-C", state.to_str().unwrap(), "rev-list", "--count", "HEAD"]);
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "1");
    let tracked = sandbox.git(&["-C", state.to_str().unwrap(), "ls-files"]);
    assert_eq!(
        String::from_utf8_lossy(&tracked.stdout).trim(),
        ".gitattributes\n.gitignore\ntsk.json"
    );
    let driver = sandbox.git(&["-C", state.to_str().unwrap(), "config", "merge.tsk.driver"]);
    assert!(String::from_utf8_lossy(&driver.stdout).contains(" merge-driver %O %A %B"));

    let unset = sandbox.tsk(&sandbox.path("elsewhere"), &["sync"]);
    assert_eq!(unset.status.code(), Some(1), "an uninitialized dir refuses");
}

#[test]
fn merge_driver_refuses_mismatched_formats_and_leaves_ours_untouched() {
    let Some(sandbox) = Sandbox::new("driver") else {
        return;
    };
    let state = sandbox.path("state");
    sandbox.tsk_ok(&state, &["add", "-t", "one", "--desk"]);
    let ours_text = fs::read_to_string(state.join("tsk.json")).unwrap();
    let base = sandbox.path("base.json");
    let ours = sandbox.path("ours.json");
    let theirs = sandbox.path("theirs.json");
    fs::write(&base, "").unwrap();
    fs::write(&ours, &ours_text).unwrap();
    fs::write(
        &theirs,
        ours_text.replace("\"format_version\": 5", "\"format_version\": 6"),
    )
    .unwrap();
    let args = [
        "merge-driver",
        base.to_str().unwrap(),
        ours.to_str().unwrap(),
        theirs.to_str().unwrap(),
    ];

    let refused = sandbox.tsk(&state, &args);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(fs::read_to_string(&ours).unwrap(), ours_text);

    fs::write(&theirs, &ours_text).unwrap();
    let merged = sandbox.tsk(&state, &args);
    assert_eq!(merged.status.code(), Some(0));
    assert!(fs::read_to_string(&ours).unwrap().contains("\"one\""));

    let help = sandbox.tsk_ok(&state, &["--help"]);
    assert!(
        !help.contains("merge-driver"),
        "the driver verb stays hidden"
    );
}
