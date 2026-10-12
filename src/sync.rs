//! `tsk sync`: share the state directory through git.
//!
//! The state directory becomes its own git repository holding `tsk.json` and
//! `trash.jsonl`. `tsk.json` merges with `tsk merge-driver` (a three-way merge by task id,
//! see [`crate::domain::merge_documents`]); `trash.jsonl` merges as a union, which its
//! readers already dedupe by id. Commit and merge run under the store's exclusive lock, so
//! no other tsk writer can change the live document between them; fetch and push run
//! outside it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use uuid::Uuid;

use crate::domain::{merge_documents, DomainState, Task};
use crate::store::{self, TaskStore, STATE_FILE, TRASH_FILE};

const GITIGNORE: &str = ".gitignore";
const GITATTRIBUTES: &str = ".gitattributes";
/// The only paths `tsk sync` commits.
const SYNCED: [&str; 4] = [STATE_FILE, TRASH_FILE, GITIGNORE, GITATTRIBUTES];
const REMOTE: &str = "origin";

/// Machine-local files: lock, backups, temps, the starter-delivery record, and the
/// release-check cache.
const GITIGNORE_LINES: &[&str] = &[
    "tsk.json.lock",
    "tsk.json.1",
    "tsk.json.v*",
    ".tsk.json.tmp.*",
    ".tsk.json.1.tmp.*",
    ".trash.jsonl.tmp.*",
    "delivery.json",
    ".delivery.json.tmp.*",
    "update.json",
    ".update.json.tmp.*",
];
const GITATTRIBUTES_LINES: &[&str] = &["tsk.json merge=tsk", "trash.jsonl merge=union"];

/// A sync failure, split by what the caller can assume about the state directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// Refused or rolled back: the board here is as it was (exit 1).
    Refused(String),
    /// A git or store step failed partway (exit 3); running `tsk sync` again is safe.
    Failed(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Refused(message) | SyncError::Failed(message) => f.write_str(message),
        }
    }
}

/// What `tsk sync --init` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    pub dir: PathBuf,
    /// A new repository was created (otherwise an existing one was repaired).
    pub created_repo: bool,
    /// The `origin` URL, whether just added or already present.
    pub remote: Option<String>,
    /// Short hash of the commit made, if anything needed committing.
    pub commit: Option<String>,
}

/// What `tsk sync` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Short hash of the last local commit made, if the board had uncommitted changes.
    pub commit: Option<String>,
    /// `origin/<branch>`, when a remote is configured.
    pub upstream: Option<String>,
    /// Remote changes were merged into this board.
    pub merged: bool,
    /// Local commits were pushed.
    pub pushed: bool,
    /// Tasks the merge added, changed, and removed here.
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

/// Set up `state_dir` for sync. Idempotent: a re-run repairs the ignore and attribute
/// files and the merge-driver config, and commits only what changed.
pub fn init(state_dir: &Path, remote: Option<&str>) -> Result<InitReport, SyncError> {
    crate::fsperm::ensure_private_dir(state_dir).map_err(|error| {
        SyncError::Refused(format!("cannot use {}: {error}", state_dir.display()))
    })?;
    let git = Git::new(state_dir);
    git.output(&["--version"])?;
    let created_repo = match git.top_level()? {
        Some(root) if same_dir(&root, state_dir) => false,
        Some(root) => {
            return Err(SyncError::Refused(format!(
                "{} is inside the git repository {}; tsk sync needs the state directory to be its own repository",
                state_dir.display(),
                root.display()
            )))
        }
        None => {
            git.run(&["init", "--quiet"])?;
            git.run(&["symbolic-ref", "HEAD", "refs/heads/main"])?;
            true
        }
    };
    let foreign: Vec<String> = git
        .run(&["ls-files", "-z"])?
        .split('\0')
        .filter(|path| !path.is_empty() && !SYNCED.contains(path))
        .map(str::to_string)
        .collect();
    if !foreign.is_empty() {
        return Err(SyncError::Refused(format!(
            "{} already tracks files tsk sync does not manage ({}); use a dedicated repository",
            state_dir.display(),
            foreign.join(", ")
        )));
    }
    let remote = match (remote, git.remote_url()?) {
        (Some(url), None) => {
            git.run(&["remote", "add", REMOTE, url])?;
            Some(url.to_string())
        }
        (Some(url), Some(existing)) if existing != url => {
            return Err(SyncError::Refused(format!(
                "remote {REMOTE} is already {existing}; tsk sync will not change it to {url}"
            )))
        }
        (_, existing) => existing,
    };
    if remote.is_some() && !git.ref_exists("HEAD")? {
        adopt_remote_branch(&git)?;
    }
    ensure_lines(
        &state_dir.join(GITIGNORE),
        "# tsk sync: machine-local files\n",
        GITIGNORE_LINES,
    )?;
    ensure_lines(
        &state_dir.join(GITATTRIBUTES),
        "# tsk sync: merge rules\n",
        GITATTRIBUTES_LINES,
    )?;
    git.ensure_merge_driver()?;
    let store = TaskStore::new(state_dir);
    let commit = locked(&store, || {
        store
            .ensure_document_while_locked()
            .map_err(|error| SyncError::Failed(format!("could not write the board: {error}")))?;
        git.commit_local(&commit_message("init"))
    })?;
    Ok(InitReport {
        dir: state_dir.to_path_buf(),
        created_repo,
        remote,
        commit,
    })
}

/// Before the first commit, name the local branch after the remote's, so this machine's
/// first sync merges into the board already there instead of pushing a second branch. A
/// clone of a remote whose HEAD names a missing branch ends up here with nothing checked
/// out. Prefers the current name, then `main`, then `master`, then the first branch.
fn adopt_remote_branch(git: &Git) -> Result<(), SyncError> {
    git.fetch()?;
    let prefix = format!("{REMOTE}/");
    let branches: Vec<String> = git
        .run(&[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("refs/remotes/{REMOTE}/"),
        ])?
        .lines()
        .filter_map(|name| name.strip_prefix(&prefix))
        .filter(|name| *name != "HEAD" && !name.is_empty())
        .map(str::to_string)
        .collect();
    let current = git
        .output(&["symbolic-ref", "--short", "-q", "HEAD"])?
        .success()
        .unwrap_or_default();
    let chosen = [current.as_str(), "main", "master"]
        .into_iter()
        .find(|name| branches.iter().any(|branch| branch == name))
        .map(str::to_string)
        .or_else(|| branches.first().cloned());
    if let Some(branch) = chosen {
        git.run(&["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])?;
    }
    Ok(())
}

/// Commit local changes, then, when `origin` exists, fetch, merge, and push. A push the
/// remote rejected because it moved is retried once after another fetch and merge.
pub fn sync(state_dir: &Path) -> Result<SyncReport, SyncError> {
    let git = Git::new(state_dir);
    match git.top_level()? {
        Some(root) if same_dir(&root, state_dir) => {}
        _ => {
            return Err(SyncError::Refused(format!(
                "{} is not set up for sync; run tsk sync --init first",
                state_dir.display()
            )))
        }
    }
    // A fresh clone has no repo-local config, so the driver is (re)installed every run.
    git.ensure_merge_driver()?;
    let store = TaskStore::new(state_dir);
    let message = commit_message("sync");
    let mut report = SyncReport {
        commit: locked(&store, || git.commit_local(&message))?,
        ..SyncReport::default()
    };
    if git.remote_url()?.is_none() {
        return Ok(report);
    }
    let branch = git
        .output(&["symbolic-ref", "--short", "-q", "HEAD"])?
        .success()
        .ok_or_else(|| {
            SyncError::Refused(
                "the sync repository is not on a branch (detached HEAD); check out a branch first"
                    .into(),
            )
        })?;
    let upstream = format!("{REMOTE}/{branch}");
    report.upstream = Some(upstream.clone());
    git.fetch()?;
    for attempt in 0..2 {
        let remote_exists = git.ref_exists(&format!("refs/remotes/{upstream}"))?;
        if remote_exists && !git.is_ancestor(&upstream, "HEAD")? {
            let counts = locked(&store, || {
                if let Some(commit) = git.commit_local(&message)? {
                    report.commit = Some(commit);
                }
                merge_upstream(&git, &store, &upstream, &message)
            })?;
            report.merged = true;
            report.added += counts.0;
            report.updated += counts.1;
            report.removed += counts.2;
        }
        if remote_exists && git.rev(&upstream)? == git.rev("HEAD")? {
            return Ok(report);
        }
        let push = git.output(&["push", "--quiet", "-u", REMOTE, &branch])?;
        if push.ok {
            report.pushed = true;
            return Ok(report);
        }
        if attempt == 1 {
            return Err(SyncError::Failed(format!(
                "{upstream} moved again while syncing, so the push was rejected twice: {}",
                push.detail()
            )));
        }
        git.fetch()?;
        let moved = git.ref_exists(&format!("refs/remotes/{upstream}"))?
            && !git.is_ancestor(&upstream, "HEAD")?;
        if !moved {
            return Err(SyncError::Failed(format!(
                "git push to {upstream} failed: {}",
                push.detail()
            )));
        }
    }
    unreachable!("the push loop returns on its second attempt")
}

/// Merge `upstream` into the checked-out board. Caller holds the store lock. Returns the
/// number of tasks added, updated, and removed here. Any failure leaves the working tree
/// as it was before the merge.
fn merge_upstream(
    git: &Git,
    store: &TaskStore,
    upstream: &str,
    message: &str,
) -> Result<(usize, usize, usize), SyncError> {
    let before = store
        .load_while_locked()
        .map_err(|error| SyncError::Refused(format!("the board here cannot be read: {error}")))?;
    let incoming = git.output(&["show", &format!("{upstream}:{STATE_FILE}")])?;
    if incoming.ok {
        store::parse_document(&incoming.stdout).map_err(|error| {
            SyncError::Refused(format!(
                "the board on {upstream} cannot be read by this tsk ({error}); upgrade tsk, then run tsk sync again"
            ))
        })?;
    }
    store
        .retain_backup_while_locked()
        .map_err(|error| SyncError::Failed(format!("could not back up the board: {error}")))?;
    let merge = git.output(&[
        "merge",
        "--no-commit",
        "--allow-unrelated-histories",
        "--quiet",
        upstream,
    ])?;
    if !merge.ok {
        if git.ref_exists("MERGE_HEAD")? {
            git.run(&["merge", "--abort"])?;
        }
        return Err(SyncError::Refused(format!(
            "could not merge {upstream}, the board here is unchanged: {}",
            merge.detail()
        )));
    }
    let after = match store.load_while_locked() {
        Ok(after) => after,
        Err(error) => {
            if git.ref_exists("MERGE_HEAD")? {
                git.run(&["merge", "--abort"])?;
            } else {
                // A fast-forward has no merge to abort; step back to the pre-merge commit.
                git.run(&["reset", "--merge", "ORIG_HEAD"])?;
            }
            return Err(SyncError::Refused(format!(
                "merging {upstream} produced a board this tsk cannot load ({error}); the board here is unchanged"
            )));
        }
    };
    if git.ref_exists("MERGE_HEAD")? {
        git.commit(&format!("{message} (merge {upstream})"))?;
    }
    Ok(count_changes(&before, &after))
}

fn count_changes(before: &DomainState, after: &DomainState) -> (usize, usize, usize) {
    let before: BTreeMap<Uuid, &Task> = before.tasks().iter().map(|t| (t.id, t)).collect();
    let after: BTreeMap<Uuid, &Task> = after.tasks().iter().map(|t| (t.id, t)).collect();
    let added = after.keys().filter(|id| !before.contains_key(id)).count();
    let removed = before.keys().filter(|id| !after.contains_key(id)).count();
    let updated = after
        .iter()
        .filter(|(id, task)| before.get(id).is_some_and(|old| old != *task))
        .count();
    (added, updated, removed)
}

/// `tsk merge-driver <base> <ours> <theirs>`: three-way merge of `tsk.json` documents,
/// writing the result over `ours`. Refuses, leaving `ours` untouched, when a document does
/// not parse or the two sides declare different store formats.
pub fn merge_driver(base: &Path, ours: &Path, theirs: &Path) -> Result<(), String> {
    let read = |path: &Path| {
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let (base_text, ours_text, theirs_text) = (read(base)?, read(ours)?, read(theirs)?);
    let version = |text: &str, side: &str| {
        store::document_format_version(text).map_err(|error| format!("{side}: {error}"))
    };
    let (ours_version, theirs_version) = (
        version(&ours_text, "ours")?,
        version(&theirs_text, "theirs")?,
    );
    if ours_version != theirs_version {
        return Err(format!(
            "store formats differ (ours {ours_version}, theirs {theirs_version}); run the same tsk version on every machine"
        ));
    }
    let parse = |text: &str, side: &str| {
        store::parse_document(text).map_err(|error| format!("{side}: {error}"))
    };
    let ours_state = parse(&ours_text, "ours")?;
    let theirs_state = parse(&theirs_text, "theirs")?;
    // Git passes an empty base when the sides share no ancestor.
    let base_state = if base_text.trim().is_empty() {
        None
    } else {
        Some(parse(&base_text, "base")?)
    };
    let merged = merge_documents(base_state.as_ref(), &ours_state, &theirs_state)
        .map_err(|error| error.to_string())?;
    let data = serde_json::to_string_pretty(&merged).map_err(|error| error.to_string())?;
    fs::write(ours, data).map_err(|error| format!("{}: {error}", ours.display()))
}

fn locked<T>(store: &TaskStore, f: impl FnOnce() -> Result<T, SyncError>) -> Result<T, SyncError> {
    store
        .with_exclusive_lock(f)
        .map_err(|error| SyncError::Failed(format!("could not lock the board: {error}")))?
}

/// Append each missing line to a managed file, writing `header` first in a new file.
fn ensure_lines(path: &Path, header: &str, lines: &[&str]) -> Result<(), SyncError> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(SyncError::Failed(format!("{}: {error}", path.display())));
        }
    };
    let present: BTreeSet<&str> = existing.lines().map(str::trim).collect();
    let missing: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| !present.contains(line))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut text = existing.clone();
    if text.is_empty() {
        text.push_str(header);
    } else if !text.ends_with('\n') {
        text.push('\n');
    }
    for line in missing {
        text.push_str(line);
        text.push('\n');
    }
    fs::write(path, text).map_err(|error| SyncError::Failed(format!("{}: {error}", path.display())))
}

fn same_dir(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn commit_message(verb: &str) -> String {
    let stamp = crate::setup::utc_timestamp(SystemTime::now());
    // `YYYYMMDD-HHMMSS` reads better as `YYYY-MM-DD HH:MM:SS`.
    let readable = if stamp.len() == 15 {
        format!(
            "{}-{}-{} {}:{}:{}",
            &stamp[0..4],
            &stamp[4..6],
            &stamp[6..8],
            &stamp[9..11],
            &stamp[11..13],
            &stamp[13..15]
        )
    } else {
        stamp
    };
    format!("tsk {verb}: {} {readable} UTC", hostname())
}

fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the pointer and length describe a writable buffer that outlives the call.
        let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if result == 0 {
            let end = buffer
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(buffer.len());
            let name = String::from_utf8_lossy(&buffer[..end]).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    for variable in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(name) = std::env::var(variable) {
            if !name.trim().is_empty() {
                return name.trim().to_string();
            }
        }
    }
    "unknown-host".into()
}

/// Quote a path for the POSIX shell git runs merge drivers through.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

struct GitOutput {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl GitOutput {
    fn success(self) -> Option<String> {
        self.ok.then_some(self.stdout)
    }

    fn detail(&self) -> String {
        let text = [self.stderr.trim(), self.stdout.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        if text.is_empty() {
            "no output".into()
        } else {
            text.replace('\n', " ")
        }
    }
}

struct Git<'a> {
    dir: &'a Path,
}

impl<'a> Git<'a> {
    fn new(dir: &'a Path) -> Self {
        Self { dir }
    }

    fn output(&self, args: &[&str]) -> Result<GitOutput, SyncError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.dir)
            .args(args)
            // Never prompt for credentials on a terminal, and never point at another
            // repository through an inherited environment.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    SyncError::Refused("git is not installed or not on PATH".into())
                } else {
                    SyncError::Failed(format!("could not run git: {error}"))
                }
            })?;
        Ok(GitOutput {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }

    fn run(&self, args: &[&str]) -> Result<String, SyncError> {
        let output = self.output(args)?;
        if output.ok {
            Ok(output.stdout)
        } else {
            Err(SyncError::Failed(format!(
                "git {} failed: {}",
                args.join(" "),
                output.detail()
            )))
        }
    }

    fn top_level(&self) -> Result<Option<PathBuf>, SyncError> {
        Ok(self
            .output(&["rev-parse", "--show-toplevel"])?
            .success()
            .map(PathBuf::from))
    }

    fn remote_url(&self) -> Result<Option<String>, SyncError> {
        Ok(self.output(&["remote", "get-url", REMOTE])?.success())
    }

    fn ref_exists(&self, name: &str) -> Result<bool, SyncError> {
        Ok(self.output(&["rev-parse", "--verify", "-q", name])?.ok)
    }

    fn rev(&self, name: &str) -> Result<String, SyncError> {
        self.run(&["rev-parse", name])
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, SyncError> {
        Ok(self
            .output(&["merge-base", "--is-ancestor", ancestor, descendant])?
            .ok)
    }

    fn fetch(&self) -> Result<(), SyncError> {
        let output = self.output(&["fetch", "--quiet", REMOTE])?;
        if output.ok {
            Ok(())
        } else {
            Err(SyncError::Failed(format!(
                "git fetch {REMOTE} failed: {}",
                output.detail()
            )))
        }
    }

    /// Point `merge.tsk.driver` at this binary. Repo-local config is not cloned, so every
    /// machine installs its own path.
    fn ensure_merge_driver(&self) -> Result<(), SyncError> {
        let binary = crate::setup::installed_binary()
            .or_else(|_| std::env::current_exe())
            .map_err(|error| SyncError::Failed(format!("cannot locate the tsk binary: {error}")))?;
        let driver = format!(
            "{} merge-driver %O %A %B",
            shell_quote(&binary.to_string_lossy())
        );
        self.run(&["config", "merge.tsk.name", "tsk board three-way merge"])?;
        self.run(&["config", "merge.tsk.driver", &driver])?;
        Ok(())
    }

    /// Identity overrides for a machine with no git identity configured.
    fn identity_args(&self) -> Result<Vec<&'static str>, SyncError> {
        let known = self.output(&["var", "GIT_AUTHOR_IDENT"])?.ok
            && self.output(&["var", "GIT_COMMITTER_IDENT"])?.ok;
        Ok(if known {
            Vec::new()
        } else {
            vec!["-c", "user.name=tsk", "-c", "user.email=tsk@localhost"]
        })
    }

    fn commit(&self, message: &str) -> Result<(), SyncError> {
        let mut args = self.identity_args()?;
        args.extend([
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "--no-verify",
            "-m",
            message,
        ]);
        self.run(&args).map(|_| ())
    }

    /// Stage the synced files and commit them if anything changed. Caller holds the
    /// store lock. Returns the new commit's short hash.
    fn commit_local(&self, message: &str) -> Result<Option<String>, SyncError> {
        let present: Vec<&str> = SYNCED
            .iter()
            .copied()
            .filter(|path| self.dir.join(path).exists())
            .collect();
        if present.is_empty() {
            return Ok(None);
        }
        let mut add = vec!["add", "--"];
        add.extend(present);
        self.run(&add)?;
        if self.output(&["diff", "--cached", "--quiet"])?.ok {
            return Ok(None);
        }
        self.commit(message)?;
        self.run(&["rev-parse", "--short", "HEAD"]).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_survives_spaces_and_single_quotes() {
        assert_eq!(shell_quote("/opt/my tsk/bin/tsk"), "'/opt/my tsk/bin/tsk'");
        assert_eq!(shell_quote("/it's/tsk"), "'/it'\\''s/tsk'");
    }

    #[test]
    fn ensure_lines_appends_only_what_is_missing() {
        let dir = std::env::temp_dir().join(format!("tsk-sync-lines-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".gitignore");
        fs::write(&path, "custom\ntsk.json.lock").unwrap();
        ensure_lines(&path, "# header\n", &["tsk.json.lock", "tsk.json.1"]).unwrap();
        ensure_lines(&path, "# header\n", &["tsk.json.lock", "tsk.json.1"]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "custom\ntsk.json.lock\ntsk.json.1\n"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
