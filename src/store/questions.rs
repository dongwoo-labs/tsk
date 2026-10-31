//! Locked private-snapshot commits for Agent boards.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AtomicFilesystem, StdFilesystem, TaskStore};
use crate::domain::*;

const PRIVATE_DIR: &str = "questions-private";

/// The only answer text a secret question records: fnox took the value.
pub const SECRET_SET: &str = "set";

/// The only other answer text a secret question records: closed without a value.
pub const SECRET_REJECTED: &str = "rejected";

/// An approval answered with a comment alone: neither approved nor rejected, the
/// comment carries the person's words and the terms stay unapproved.
pub const COMMENTED: &str = "commented";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionDurability {
    Durable,
    /// Shared reference is visible, but a failed directory sync prevents a durability claim.
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionSaveResult {
    pub record: QuestionRecord,
    pub durability: QuestionDurability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionError {
    AgentStoreRequired,
    InvalidInput,
    UnknownTask,
    UnknownQuestion,
    StaleRevision,
    InvalidCorrection,
    ApprovalRefused,
    PrivateRecordUnavailable,
    NotSaved,
    /// Save failed and readback could not establish whether the reference committed.
    CommitUnknown,
    StoreUnavailable,
    /// A finding's history links several distinct created tasks.
    LinkAmbiguous,
    /// A secret is typed on the board, which hands it to fnox; no other path answers it.
    SecretNeedsBoard,
    /// A secret question's shape or fnox target is invalid.
    InvalidSecret,
}

impl std::fmt::Display for QuestionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AgentStoreRequired => {
                "question commands require an explicit non-personal Agent state directory"
            }
            Self::InvalidInput => "invalid question or answer input",
            Self::UnknownTask => "unknown, deleted, or notice task",
            Self::UnknownQuestion => "unknown question",
            Self::StaleRevision => "question changed: read back its current revision",
            Self::InvalidCorrection => "correction must link the current answer revision",
            Self::ApprovalRefused => {
                "approval requires confirmed user evidence and exact current approval terms"
            }
            Self::PrivateRecordUnavailable => {
                "referenced private question record is missing or corrupt; mutation refused"
            }
            Self::NotSaved => "question was not saved; retry with the same expected revision",
            Self::CommitUnknown => "question commit is unknown; read back before retrying",
            Self::StoreUnavailable => "question store is unavailable",
            Self::LinkAmbiguous => {
                "finding history links several created tasks; nothing was created"
            }
            Self::InvalidSecret => {
                "a secret question needs audience user, no options, approval, or finding, and secret {key, profile, config}: key [A-Za-z_][A-Za-z0-9_]*, profile [A-Za-z0-9_-]+ or null, config an absolute path to an existing regular (non-symlink) fnox.toml or null for global"
            }
            Self::SecretNeedsBoard => {
                "a secret is entered only in the tsk board questions tab, which hands it to fnox"
            }
        })
    }
}
impl std::error::Error for QuestionError {}

/// CLI inventory keeps unreadable questions visible without inventing their answer state.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum QuestionListEntry {
    Record(Box<QuestionRecord>),
    Unavailable {
        reference: QuestionRef,
        unavailable: bool,
    },
}

/// Capability for an explicitly selected Agent board, never an implicit personal board.
/// This is an isolation boundary, not authentication against the same OS account.
#[derive(Debug)]
pub struct QuestionStore {
    store: TaskStore,
}

fn resolved_path(path: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    // Resolve symlinks before `..`: lexical normalization would give a different
    // identity for an existing alias followed by a parent component.
    if absolute.exists() {
        return fs::canonicalize(absolute).ok();
    }
    let parent = resolved_path(absolute.parent()?)?;
    Some(parent.join(absolute.file_name()?))
}

pub fn question_capability(explicit_state_dir: Option<&Path>) -> Result<(), QuestionError> {
    let selected = explicit_state_dir
        .filter(|path| !path.as_os_str().is_empty())
        .and_then(resolved_path)
        .ok_or(QuestionError::AgentStoreRequired)?;
    let mut personal = Vec::new();
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        personal.push(PathBuf::from(home).join(".tsk"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE").filter(|home| !home.is_empty()) {
        personal.push(PathBuf::from(home).join(".tsk"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|home| !home.is_empty()) {
        personal.push(PathBuf::from(local).join("tsk"));
    }
    if personal
        .iter()
        .filter_map(|path| resolved_path(path))
        .any(|path| {
            if selected.starts_with(&path) {
                return true;
            }
            // Reserve personal-root case variants even before creation. Canonical
            // parents keep existing symlink and parent-component semantics intact.
            path.parent()
                .zip(path.file_name())
                .and_then(|(parent, name)| {
                    selected
                        .strip_prefix(parent)
                        .ok()?
                        .components()
                        .next()
                        .map(|first| {
                            first
                                .as_os_str()
                                .as_encoded_bytes()
                                .eq_ignore_ascii_case(name.as_encoded_bytes())
                        })
                })
                .unwrap_or(false)
        })
    {
        return Err(QuestionError::AgentStoreRequired);
    }
    Ok(())
}

impl QuestionStore {
    pub fn new(explicit_state_dir: PathBuf) -> Result<Self, QuestionError> {
        question_capability(Some(&explicit_state_dir))?;
        Ok(Self {
            store: TaskStore::new(explicit_state_dir),
        })
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    pub fn list(&self, task_id: Option<Uuid>) -> Result<Vec<QuestionRecord>, QuestionError> {
        // The shared document is atomic; each referenced private revision is immutable
        // and published before it. Keep this one reference snapshot for the whole read.
        let state = self
            .store
            .load_snapshot()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        state
            .questions()
            .iter()
            .filter(|reference| task_id.is_none_or(|id| reference.task_id == id))
            .map(|reference| self.read_private(reference))
            .collect()
    }

    /// A missing snapshot on another machine is unknown, never an absent question.
    /// Keep `list` strict for callers that require a complete private feed.
    pub fn list_entries(
        &self,
        task_id: Option<Uuid>,
    ) -> Result<Vec<QuestionListEntry>, QuestionError> {
        // The shared document is atomic; each referenced private revision is immutable
        // and published before it. Keep this one reference snapshot for the whole read.
        let state = self
            .store
            .load_snapshot()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        state
            .questions()
            .iter()
            .filter(|reference| task_id.is_none_or(|id| reference.task_id == id))
            .map(|reference| match self.read_private(reference) {
                Ok(record) => Ok(QuestionListEntry::Record(Box::new(record))),
                Err(QuestionError::PrivateRecordUnavailable) => {
                    Ok(QuestionListEntry::Unavailable {
                        reference: reference.clone(),
                        unavailable: true,
                    })
                }
                Err(error) => Err(error),
            })
            .collect()
    }

    pub fn read(&self, id: Uuid) -> Result<QuestionRecord, QuestionError> {
        // The shared document is atomic; each referenced private revision is immutable
        // and published before it. Keep this one reference snapshot for the whole read.
        let state = self
            .store
            .load_snapshot()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        let reference = state
            .questions()
            .iter()
            .find(|reference| reference.id == id)
            .ok_or(QuestionError::UnknownQuestion)?;
        self.read_private(reference)
    }

    pub fn ask(&self, draft: QuestionDraft) -> Result<QuestionSaveResult, QuestionError> {
        validate_question(&draft)?;
        if draft.kind == QuestionKind::Secret && !valid_secret_question(&draft) {
            return Err(QuestionError::InvalidSecret);
        }
        if draft.audience.is_none() {
            return Err(QuestionError::InvalidInput);
        }
        if draft.kind == QuestionKind::Approval && !draft.options.is_empty() {
            return Err(QuestionError::InvalidInput);
        }
        let _guard = self
            .store
            .lock_exclusive()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        let mut state = self
            .store
            .load_unlocked()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        if state
            .get(draft.task_id)
            .is_none_or(|task| task.soft_deleted || task.is_notice())
        {
            return Err(QuestionError::UnknownTask);
        }
        let number = state
            .questions()
            .iter()
            .map(|reference| reference.number)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(QuestionError::InvalidInput)?;
        let reference = QuestionRef {
            id: Uuid::new_v4(),
            number,
            task_id: draft.task_id,
            kind: draft.kind,
            summary: draft.summary.clone(),
            revision: Uuid::new_v4(),
        };
        let record = QuestionRecord {
            reference: reference.clone(),
            draft,
            created_at: now(),
            answers: Vec::new(),
            routes: Vec::new(),
        };
        state.questions.push(reference);
        self.commit(&mut state, record)
    }

    /// Save Recovery confirms the same visible commit without appending another answer.
    /// Only after all syncs succeed can a caller retry its identifier-only notification.
    pub fn confirm_durability(
        &self,
        id: Uuid,
        expected_revision: Uuid,
    ) -> Result<QuestionSaveResult, QuestionError> {
        let _guard = self
            .store
            .lock_exclusive()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        let state = self
            .store
            .load_unlocked()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        let reference = state
            .questions()
            .iter()
            .find(|reference| reference.id == id)
            .ok_or(QuestionError::UnknownQuestion)?;
        if reference.revision != expected_revision {
            return Err(QuestionError::StaleRevision);
        }
        let record = self.read_private(reference)?;
        let directory = self.private_directory()?;
        OpenOptions::new()
            .write(true)
            .open(directory.join(format!("{}.json", reference.revision)))
            .and_then(|file| file.sync_all())
            .map_err(|_| QuestionError::CommitUnknown)?;
        StdFilesystem
            .sync_directory(&directory)
            .map_err(|_| QuestionError::CommitUnknown)?;
        OpenOptions::new()
            .write(true)
            .open(self.store.state_file())
            .and_then(|file| file.sync_all())
            .map_err(|_| QuestionError::CommitUnknown)?;
        StdFilesystem
            .sync_directory(self.path())
            .map_err(|_| QuestionError::CommitUnknown)?;
        Ok(QuestionSaveResult {
            record,
            durability: QuestionDurability::Durable,
        })
    }

    /// Change only the audience (and optionally the run-request address) under the same
    /// Q-ID. The text, terms, answers, and history move unchanged into a new immutable
    /// revision; an answered question stays answered.
    pub fn route(&self, id: Uuid, draft: RouteDraft) -> Result<QuestionSaveResult, QuestionError> {
        if draft
            .lead_recipient
            .as_deref()
            .is_some_and(|name| !agent_name(name))
        {
            return Err(QuestionError::InvalidInput);
        }
        let _guard = self
            .store
            .lock_exclusive()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        let mut state = self
            .store
            .load_unlocked()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        let index = state
            .questions()
            .iter()
            .position(|reference| reference.id == id)
            .ok_or(QuestionError::UnknownQuestion)?;
        let mut record = self.read_private(&state.questions()[index])?;
        if draft.expected_revision != record.reference.revision {
            return Err(QuestionError::StaleRevision);
        }
        // Only the user's board can take a secret; it never moves to the lead.
        if record.draft.kind == QuestionKind::Secret && draft.audience != Audience::User {
            return Err(QuestionError::InvalidSecret);
        }
        record.draft.audience = Some(draft.audience);
        if let Some(name) = draft.lead_recipient.clone() {
            record.draft.lead_recipient = Some(name);
        }
        // Answers keep their own revisions; the route is its own history event and
        // becomes the head revision the shared reference points at.
        let revision = Uuid::new_v4();
        record.routes.push(QuestionRoute {
            revision,
            after_answers: record.answers.len(),
            audience: draft.audience,
            lead_recipient: draft.lead_recipient,
            at: now(),
        });
        record.reference.revision = revision;
        state.questions[index] = record.reference.clone();
        self.commit(&mut state, record)
    }

    /// Terms cannot be silently edited. Ask a new Q for changed approval terms.
    pub fn answer(
        &self,
        id: Uuid,
        draft: AnswerDraft,
    ) -> Result<QuestionSaveResult, QuestionError> {
        self.answer_with_task(id, draft, |_| Ok(None))
    }

    /// D2701 owns the finding action. This boundary loans it the same store lock:
    /// create a task and append the answer in one shared replacement, or neither.
    pub fn answer_with_task(
        &self,
        id: Uuid,
        draft: AnswerDraft,
        create: impl FnOnce(&mut DomainState) -> Result<Option<Uuid>, String>,
    ) -> Result<QuestionSaveResult, QuestionError> {
        let _guard = self
            .store
            .lock_exclusive()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        let mut state = self
            .store
            .load_unlocked()
            .map_err(|_| QuestionError::StoreUnavailable)?;
        let index = state
            .questions()
            .iter()
            .position(|reference| reference.id == id)
            .ok_or(QuestionError::UnknownQuestion)?;
        let mut record = self.read_private(&state.questions()[index])?;
        if draft.expected_revision != record.reference.revision {
            return Err(QuestionError::StaleRevision);
        }
        if draft.text.trim().is_empty() || draft.actor.trim().is_empty() {
            return Err(QuestionError::InvalidInput);
        }
        match record.current_answer() {
            Some(previous) if draft.corrects_revision != Some(previous.revision) => {
                return Err(QuestionError::InvalidCorrection)
            }
            None if draft.corrects_revision.is_some() => {
                return Err(QuestionError::InvalidCorrection)
            }
            _ => {}
        }
        validate_answer(&record, &draft)?;
        // The board records `set` after fnox took the value, or `rejected` closing it with
        // no value and no fnox call; nothing else answers it.
        if record.draft.kind == QuestionKind::Secret {
            let text_ok = if draft.reject {
                draft.text == SECRET_REJECTED
            } else {
                draft.text == SECRET_SET
            };
            if draft.origin != AnswerOrigin::UserUi
                || !text_ok
                || draft.corrects_revision.is_some()
                || draft.approve
            {
                return Err(QuestionError::SecretNeedsBoard);
            }
        } else if draft.reject {
            return Err(QuestionError::InvalidInput);
        }
        if draft.action.is_some() && record.draft.kind != QuestionKind::Finding {
            return Err(QuestionError::InvalidInput);
        }
        // The whole preserved history decides the link, so a correction or close keeps
        // it and the creation closure never runs twice for one finding.
        let created_task_id = match record.finding_link() {
            FindingLink::One(existing) => Some(existing),
            FindingLink::Ambiguous => return Err(QuestionError::LinkAmbiguous),
            FindingLink::None => {
                let existing_ids: std::collections::BTreeSet<Uuid> =
                    state.tasks().iter().map(|task| task.id).collect();
                let created = create(&mut state).map_err(|_| QuestionError::InvalidInput)?;
                if let Some(task_id) = created {
                    if record.draft.kind != QuestionKind::Finding
                        || existing_ids.contains(&task_id)
                        || state
                            .get(task_id)
                            .is_none_or(|task| task.is_notice() || task.soft_deleted)
                    {
                        return Err(QuestionError::InvalidInput);
                    }
                }
                created
            }
        };
        state.assign_numbers_for_persistence();
        let revision = Uuid::new_v4();
        record.answers.push(QuestionAnswer {
            revision,
            created_task_id,
            corrects_revision: draft.corrects_revision,
            text: draft.text,
            actor: draft.actor,
            origin: draft.origin,
            at: now(),
            approval: if draft.approve { draft.approval } else { None },
            user_chat: draft.user_chat,
            source: draft.source,
            action: draft.action,
            comment: draft.comment,
        });
        record.reference.revision = revision;
        state.questions[index] = record.reference.clone();
        self.commit(&mut state, record)
    }

    fn private_directory(&self) -> Result<PathBuf, QuestionError> {
        private_directory(self.path())
    }

    fn read_private(&self, reference: &QuestionRef) -> Result<QuestionRecord, QuestionError> {
        read_private(self.path(), reference)
    }

    fn commit(
        &self,
        state: &mut DomainState,
        record: QuestionRecord,
    ) -> Result<QuestionSaveResult, QuestionError> {
        let directory = self.private_directory()?;
        if !directory.exists() {
            fs::create_dir(&directory).map_err(|_| QuestionError::NotSaved)?;
            StdFilesystem
                .sync_directory(self.path())
                .map_err(|_| QuestionError::NotSaved)?;
        }
        // Keep runtime records excluded even when the Agent state directory is in
        // another Git checkout that does not carry tsk's repository ignore rule.
        let ignore = directory.join(".gitignore");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ignore)
        {
            Ok(mut file) => {
                file.write_all(b"*\n")
                    .and_then(|()| file.sync_all())
                    .map_err(|_| QuestionError::NotSaved)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata =
                    fs::symlink_metadata(&ignore).map_err(|_| QuestionError::NotSaved)?;
                if !metadata.is_file()
                    || metadata.file_type().is_symlink()
                    || fs::read(&ignore).map_err(|_| QuestionError::NotSaved)? != b"*\n"
                {
                    return Err(QuestionError::NotSaved);
                }
                OpenOptions::new()
                    .write(true)
                    .open(&ignore)
                    .and_then(|file| file.sync_all())
                    .map_err(|_| QuestionError::NotSaved)?;
            }
            Err(_) => return Err(QuestionError::NotSaved),
        }
        let file = directory.join(format!("{}.json", record.reference.revision));
        let content =
            serde_json::to_vec_pretty(&record).map_err(|_| QuestionError::InvalidInput)?;
        // No chmod: standard create permissions follow the caller's existing umask.
        let mut snapshot = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
            .map_err(|_| QuestionError::NotSaved)?;
        snapshot
            .write_all(&content)
            .and_then(|()| snapshot.sync_all())
            .map_err(|_| QuestionError::NotSaved)?;
        drop(snapshot);
        StdFilesystem
            .sync_directory(&directory)
            .map_err(|_| QuestionError::NotSaved)?;
        // Sync parent on retries too: creation might have been visible but not durable.
        StdFilesystem
            .sync_directory(self.path())
            .map_err(|_| QuestionError::NotSaved)?;
        state.clear_merge_bases();
        match self.store.save_unlocked(state) {
            Ok(()) => Ok(QuestionSaveResult {
                record,
                durability: QuestionDurability::Durable,
            }),
            Err(_) => {
                let visible = self
                    .store
                    .load_unlocked()
                    .map_err(|_| QuestionError::CommitUnknown)?;
                if visible
                    .questions()
                    .iter()
                    .any(|reference| reference == &record.reference)
                {
                    self.read_private(&record.reference)?;
                    Ok(QuestionSaveResult {
                        record,
                        durability: QuestionDurability::Uncertain,
                    })
                } else {
                    Err(QuestionError::NotSaved)
                }
            }
        }
    }
}

fn private_directory(state_dir: &Path) -> Result<PathBuf, QuestionError> {
    let directory = state_dir.join(PRIVATE_DIR);
    if let Ok(metadata) = fs::symlink_metadata(&directory) {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(QuestionError::PrivateRecordUnavailable);
        }
    }
    Ok(directory)
}

pub(super) fn read_private(
    state_dir: &Path,
    reference: &QuestionRef,
) -> Result<QuestionRecord, QuestionError> {
    let file = private_directory(state_dir)?.join(format!("{}.json", reference.revision));
    let metadata =
        fs::symlink_metadata(&file).map_err(|_| QuestionError::PrivateRecordUnavailable)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(QuestionError::PrivateRecordUnavailable);
    }
    let content = fs::read(file).map_err(|_| QuestionError::PrivateRecordUnavailable)?;
    let record: QuestionRecord =
        serde_json::from_slice(&content).map_err(|_| QuestionError::PrivateRecordUnavailable)?;
    if record.reference != *reference
        || record.draft.task_id != reference.task_id
        || record.draft.kind != reference.kind
        || record.draft.summary != reference.summary
        || record
            .head_revision()
            .is_some_and(|revision| revision != reference.revision)
    {
        return Err(QuestionError::PrivateRecordUnavailable);
    }
    validate_question(&record.draft).map_err(|_| QuestionError::PrivateRecordUnavailable)?;
    Ok(record)
}

/// Whether the question `reference` names is still waiting for an answer. A record that
/// cannot be read counts as open: a done task it names stays in `tsk.json`.
pub(super) fn reference_is_open(state_dir: &Path, reference: &QuestionRef) -> bool {
    read_private(state_dir, reference)
        .ok()
        .is_none_or(|record| record.is_open())
}

fn now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

/// Every field-level problem with approval terms, named by field.
fn terms_problems(terms: &ApprovalTerms) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, value) in [
        ("target", &terms.target),
        ("action", &terms.action),
        ("scope", &terms.scope),
        ("rollback", &terms.rollback),
    ] {
        if value.trim().is_empty() {
            problems.push(format!("approval.{name}: must not be blank"));
        }
    }
    if !matches!(terms.h.len(), 40 | 64) || !terms.h.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        problems.push(
            "approval.h: must be a hex Git SHA (40 digits) or plan digest (64 digits)".into(),
        );
    }
    problems
}

/// Every problem with a draft, named by field, in check order. The flag marks a secret
/// problem; the first one decides which error `ask` refuses with.
fn draft_problems(draft: &QuestionDraft) -> Vec<(bool, String)> {
    let mut problems: Vec<(bool, String)> = Vec::new();
    let mut note = |secret: bool, message: String| problems.push((secret, message));
    for (name, value) in [
        ("summary", &draft.summary),
        ("origin_thread", &draft.origin_thread),
        ("recipient", &draft.recipient),
        ("text", &draft.text),
    ] {
        if value.trim().is_empty() {
            note(false, format!("{name}: must not be blank"));
        }
    }
    if draft.options.iter().any(|option| option.trim().is_empty()) {
        note(false, "options: no option may be blank".into());
    }
    if draft.secret.is_some() && draft.kind != QuestionKind::Secret {
        note(true, "secret: only kind secret carries it".into());
    }
    for (name, value) in [
        ("origin_thread", Some(draft.origin_thread.as_str())),
        ("recipient", Some(draft.recipient.as_str())),
        ("lead_recipient", draft.lead_recipient.as_deref()),
    ] {
        if value.is_some_and(|value| !agent_name(value)) {
            note(
                false,
                format!("{name}: must be a Herdr agent name ([a-z][a-z0-9_-]{{0,31}})"),
            );
        }
    }
    match draft.kind {
        QuestionKind::Approval => {
            match draft.approval.as_ref() {
                None => note(
                    false,
                    "approval: required for kind approval (target, action, h, scope, rollback)"
                        .into(),
                ),
                Some(terms) => {
                    for message in terms_problems(terms) {
                        note(false, message);
                    }
                }
            }
            if draft.finding.is_some() {
                note(false, "finding: not allowed for kind approval".into());
            }
            // The options-takes-none rule for kind approval is checked in `ask` (and its
            // message added in `question_problems`) rather than here: `draft_problems` also
            // runs on every read (`read_private`), and a rule added here would retroactively
            // invalidate approval records saved before the rule existed, refusing an entire
            // `list`/`read` call for one old record (same reasoning as `audience` below).
        }
        QuestionKind::Finding => {
            if draft
                .finding
                .as_ref()
                .is_none_or(|finding| finding.detail.trim().is_empty())
            {
                note(false, "finding.detail: required for kind finding".into());
            }
            if draft.approval.is_some() {
                note(false, "approval: only kind approval carries it".into());
            }
        }
        QuestionKind::Question => {
            if draft.approval.is_some() {
                note(false, "approval: only kind approval carries it".into());
            }
            if draft.finding.is_some() {
                note(false, "finding: only kind finding carries it".into());
            }
        }
        // A secret goes to the user's board only, as one fnox target and nothing to pick.
        QuestionKind::Secret => {
            if draft.audience != Some(Audience::User) {
                note(true, "audience: kind secret needs \"user\"".into());
            }
            if !draft.options.is_empty() {
                note(true, "options: kind secret takes none".into());
            }
            if draft.approval.is_some() || draft.finding.is_some() {
                note(true, "approval, finding: kind secret takes neither".into());
            }
            if draft
                .secret
                .as_ref()
                .is_none_or(|target| !target.is_well_formed())
            {
                note(true, "secret: missing or malformed fnox target".into());
            }
        }
    }
    problems
}

/// Field-level problems with a draft, all at once, for the refusal text. Empty means the
/// shape is valid; whether the task exists is checked under the store lock.
pub fn question_problems(draft: &QuestionDraft) -> Vec<String> {
    let mut problems: Vec<String> = draft_problems(draft)
        .into_iter()
        .map(|(_, message)| message)
        .collect();
    // A secret without audience already says so above.
    if draft.audience.is_none() && draft.kind != QuestionKind::Secret {
        problems.push(AUDIENCE_REQUIRED.into());
    }
    // The board paints Approve/Reject/Comment for an approval, never options, so a
    // numbered pick would reach the lead as a Reject.
    if draft.kind == QuestionKind::Approval && !draft.options.is_empty() {
        problems.push(APPROVAL_TAKES_NO_OPTIONS.into());
    }
    problems
}

/// Only `ask` requires it: records saved before this rule stay readable and answerable.
const AUDIENCE_REQUIRED: &str =
    "audience: required, \"user\" or \"lead\"; add it to the question JSON";

/// Only `ask` refuses it: an approval record saved before this rule stays readable and
/// answerable.
const APPROVAL_TAKES_NO_OPTIONS: &str = "options: kind approval takes none";

fn validate_question(draft: &QuestionDraft) -> Result<(), QuestionError> {
    match draft_problems(draft).first() {
        None => Ok(()),
        Some((true, _)) => Err(QuestionError::InvalidSecret),
        Some((false, _)) => Err(QuestionError::InvalidInput),
    }
}

/// Whether the draft is a well-formed secret question whose config is present now:
/// checked at ask time and again right before the board runs fnox.
pub fn valid_secret_question(draft: &QuestionDraft) -> bool {
    draft.kind == QuestionKind::Secret
        && validate_question(draft).is_ok()
        && draft
            .secret
            .as_ref()
            .is_some_and(SecretTarget::config_present)
}

pub fn agent_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn validate_answer(record: &QuestionRecord, draft: &AnswerDraft) -> Result<(), QuestionError> {
    // A comment is text beside an approval's y/n decision, never an approval of its own:
    // only an approval question accepts one, and it never changes what `approve` checks.
    if let Some(comment) = draft.comment.as_deref() {
        if record.draft.kind != QuestionKind::Approval || comment.trim().is_empty() {
            return Err(QuestionError::InvalidInput);
        }
    }
    if !draft.approve {
        if draft.approval.is_some() {
            return Err(QuestionError::ApprovalRefused);
        }
        return Ok(());
    }
    if record.draft.kind != QuestionKind::Approval
        || !draft.confirmed
        || draft.approval != record.draft.approval
        || draft.approval.is_none()
    {
        return Err(QuestionError::ApprovalRefused);
    }
    match draft.origin {
        AnswerOrigin::UserUi => Ok(()),
        AnswerOrigin::LeadUserChat
            if draft.user_chat.as_ref().is_some_and(|evidence| {
                evidence.original_verified
                    && !evidence.original_user.trim().is_empty()
                    && !evidence.original_text.trim().is_empty()
                    && !evidence.source.trim().is_empty()
                    && !evidence.original_time.trim().is_empty()
            }) =>
        {
            Ok(())
        }
        _ => Err(QuestionError::ApprovalRefused),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QuestionNotification {
    Notified,
    Failed,
    /// A visible but durability-uncertain commit must not invite the recipient to act.
    NotAttempted,
}

/// Identifier-only resume hint. Never forwards private text or approval evidence.
/// The Herdr command's submission success is not evidence of a completed agent turn.
pub fn notify_question(saved: &QuestionSaveResult, registration: bool) -> QuestionNotification {
    if saved.durability != QuestionDurability::Durable {
        return QuestionNotification::NotAttempted;
    }
    if std::env::var("HERDR_ENV").as_deref() != Ok("1") {
        return QuestionNotification::Failed;
    }
    let recipient = if registration {
        &saved.record.draft.recipient
    } else {
        &saved.record.draft.origin_thread
    };
    let reference = &saved.record.reference;
    let hint = format!("tsk question task={} Q{} id={} revision={}; read back from the Agent state directory before acting.",
        reference.task_id, reference.number, reference.id, reference.revision);
    match std::process::Command::new("herdr")
        .args(["agent", "prompt", recipient, &hint])
        .output()
    {
        Ok(output) if output.status.success() => QuestionNotification::Notified,
        _ => QuestionNotification::Failed,
    }
}
