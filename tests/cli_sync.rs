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
