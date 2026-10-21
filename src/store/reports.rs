//! Independent completion documents, serialized under the TaskStore common lock.
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AtomicFilesystem, StdFilesystem, TaskStore, TrashTarget};
use crate::domain::TaskScope;
use crate::fsperm;

const REPORT_FILE: &str = "reports.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRevision {
    pub revision: u64,
    pub markdown: String,
    #[serde(with = "crate::domain::time_serde")]
    pub created_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRecord {
    pub task_id: Uuid,
    pub task_number: u64,
    pub task_title: String,
    pub project: Option<String>,
    pub revision: u64,
    pub markdown: String,
    pub read_revision: Option<u64>,
    pub unread: bool,
    pub revisions: Vec<ReportRevision>,
}

impl ReportRecord {
    pub fn unread(&self) -> bool {
        self.read_revision != Some(self.revision)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportError {
    InvalidInput,
    UnknownTask,
    UnknownReport,
    StaleRevision,
    InvalidDocument,
    Io,
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "report must contain nonblank UTF-8 Markdown",
            Self::UnknownTask => "unknown, deleted, or notice task",
            Self::UnknownReport => "unknown report",
            Self::StaleRevision => "report changed: read its current revision",
            Self::InvalidDocument => "corrupt or unsupported reports document; operation refused",
            Self::Io => "report store I/O or uncertain durability; read back before retrying",
        })
    }
}
impl std::error::Error for ReportError {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportDocument {
    format_version: u32,
    // Prototype-sized whole-document storage; keep the common lock and atomic
    // replacement if a measured load later requires indexing or split files.
    reports: Vec<ReportRecord>,
}

impl Default for ReportDocument {
    fn default() -> Self {
        Self {
            format_version: 1,
            reports: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct ReportStore {
    store: TaskStore,
}

impl ReportStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            store: TaskStore::new(path),
        }
    }

    /// `reports.json`'s change signature (a `stat`, no read or lock), `None` while it does
    /// not exist. The board's idle tick polls it to refresh the report feed.
    pub fn signature(&self) -> Option<crate::store::StoreSignature> {
        TaskStore::file_signature(&self.store.path().join(REPORT_FILE))
    }

    pub fn list(&self) -> Result<Vec<ReportRecord>, ReportError> {
        let _guard = self.store.lock_exclusive().map_err(|_| ReportError::Io)?;
        Ok(self.load_unlocked()?.reports)
    }

    pub fn read(&self, task_id: Uuid) -> Result<ReportRecord, ReportError> {
        self.read_address(TrashTarget::Id(task_id))
    }

    /// Lookup uses the report's preserved identity even after its Task disappears.
    pub fn read_address(&self, selector: TrashTarget) -> Result<ReportRecord, ReportError> {
        self.list()?
            .into_iter()
            .find(|record| matches_record(selector, record))
            .ok_or(ReportError::UnknownReport)
    }

    pub fn register(
        &self,
        selector: TrashTarget,
        markdown: String,
    ) -> Result<ReportRecord, ReportError> {
        self.register_with(selector, markdown, &StdFilesystem)
    }

    fn register_with<F: AtomicFilesystem>(
        &self,
        selector: TrashTarget,
        markdown: String,
        filesystem: &F,
    ) -> Result<ReportRecord, ReportError> {
        if markdown.trim().is_empty() {
            return Err(ReportError::InvalidInput);
        }
        let _guard = self.store.lock_exclusive().map_err(|_| ReportError::Io)?;
        let mut document = self.load_unlocked()?;
        let state = self.store.load_unlocked().map_err(|_| ReportError::Io)?;
        let task = if let Some(task) = state
            .tasks()
            .iter()
            .find(|task| matches_task(selector, task))
        {
            task.clone()
        } else {
            self.store
                .load_done_history_unlocked()
                .map_err(|_| ReportError::Io)?
                .into_iter()
                .map(|line| line.task)
                .find(|task| matches_task(selector, task))
                .ok_or(ReportError::UnknownTask)?
        };
        if task.soft_deleted || task.is_notice() {
            return Err(ReportError::UnknownTask);
        }
        let index = document
            .reports
            .iter()
            .position(|record| record.task_id == task.id);
        if let Some(index) = index {
            if document.reports[index].markdown == markdown {
                // A visible earlier rename can still be durability-uncertain. Rewrite
                // even a semantic no-op through every sync stage before success.
                self.commit(&document, filesystem)?;
                return Ok(document.reports[index].clone());
            }
        }
        let revision = index.map_or(Ok(1), |index| {
            document.reports[index]
                .revision
                .checked_add(1)
                .ok_or(ReportError::InvalidInput)
        })?;
        let mut revisions =
            index.map_or_else(Vec::new, |index| document.reports[index].revisions.clone());
        revisions.push(ReportRevision {
            revision,
            markdown: markdown.clone(),
            created_at: SystemTime::now(),
        });
        let record = ReportRecord {
            task_id: task.id,
            task_number: task.number.ok_or(ReportError::UnknownTask)?,
            task_title: task.title.clone(),
            project: match &task.scope {
                TaskScope::Global => None,
                TaskScope::Project { path } => Some(path.clone()),
            },
            revision,
            markdown: markdown.clone(),
            read_revision: index.and_then(|index| document.reports[index].read_revision),
            unread: true,
            revisions,
        };
        if let Some(index) = index {
            document.reports[index] = record.clone();
        } else {
            document.reports.push(record.clone());
        }
        self.commit(&document, filesystem)?;
        Ok(record)
    }

    pub fn mark_read(
        &self,
        task_id: Uuid,
        expected_revision: u64,
    ) -> Result<ReportRecord, ReportError> {
        self.mark_read_with(task_id, expected_revision, &StdFilesystem)
    }

    fn mark_read_with<F: AtomicFilesystem>(
        &self,
        task_id: Uuid,
        expected_revision: u64,
        filesystem: &F,
    ) -> Result<ReportRecord, ReportError> {
        let _guard = self.store.lock_exclusive().map_err(|_| ReportError::Io)?;
        let mut document = self.load_unlocked()?;
        let record = document
            .reports
            .iter_mut()
            .find(|record| record.task_id == task_id)
            .ok_or(ReportError::UnknownReport)?;
        if record.revision != expected_revision {
            return Err(ReportError::StaleRevision);
        }
        record.read_revision = Some(expected_revision);
        record.unread = record.unread();
        let result = record.clone();
        self.commit(&document, filesystem)?;
        Ok(result)
    }

    fn load_unlocked(&self) -> Result<ReportDocument, ReportError> {
        let path = self.store.path().join(REPORT_FILE);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() || fsperm::is_reparse_or_symlink(&metadata) => {
                return Err(ReportError::InvalidDocument)
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ReportDocument::default())
            }
            Err(_) => return Err(ReportError::Io),
        }
        fsperm::tighten_file(&path);
        let data = fs::read(path).map_err(|_| ReportError::Io)?;
        let document: ReportDocument =
            serde_json::from_slice(&data).map_err(|_| ReportError::InvalidDocument)?;
        validate_document(&document)?;
        Ok(document)
    }

    fn commit<F: AtomicFilesystem>(
        &self,
        document: &ReportDocument,
        filesystem: &F,
    ) -> Result<(), ReportError> {
        validate_document(document)?;
        fsperm::ensure_private_dir(self.store.path()).map_err(|_| ReportError::Io)?;
        let path = self.store.path().join(REPORT_FILE);
        let temp = self
            .store
            .path()
            .join(format!(".reports.json.tmp.{}", Uuid::new_v4()));
        let data = serde_json::to_vec_pretty(document).map_err(|_| ReportError::InvalidDocument)?;
        let result = (|| -> io::Result<()> {
            let mut file = filesystem.create_file(&temp)?;
            filesystem.write_all(&mut file, &data)?;
            filesystem.sync_file(&file)?;
            drop(file);
            filesystem.rename(&temp, &path)?;
            filesystem.sync_directory(self.store.path())
        })();
        if result.is_err() {
            let _ = filesystem.remove_file(&temp);
        }
        result.map_err(|_| ReportError::Io)
    }
}

fn validate_document(document: &ReportDocument) -> Result<(), ReportError> {
    if document.format_version != 1 {
        return Err(ReportError::InvalidDocument);
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut numbers = std::collections::BTreeSet::new();
    for record in &document.reports {
        if record.task_id.is_nil()
            || record.task_number == 0
            || !ids.insert(record.task_id)
            || !numbers.insert(record.task_number)
            || record.task_title.trim().is_empty()
            || record.markdown.trim().is_empty()
            || record.revision == 0
            || record
                .read_revision
                .is_some_and(|revision| revision == 0 || revision > record.revision)
            || record.unread != record.unread()
            || record.revisions.last().is_none_or(|last| {
                last.revision != record.revision || last.markdown != record.markdown
            })
            || record
                .revisions
                .iter()
                .enumerate()
                .any(|(index, revision)| {
                    revision.revision != index as u64 + 1 || revision.markdown.trim().is_empty()
                })
        {
            return Err(ReportError::InvalidDocument);
        }
    }
    Ok(())
}

fn matches_record(selector: TrashTarget, record: &ReportRecord) -> bool {
    match selector {
        TrashTarget::Id(id) => record.task_id == id,
        TrashTarget::Number(number) => record.task_number == number,
    }
}
fn matches_task(selector: TrashTarget, task: &crate::domain::Task) -> bool {
    match selector {
        TrashTarget::Id(id) => task.id == id,
        TrashTarget::Number(number) => task.number == Some(number),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::run_with;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn seed() -> (ReportStore, Uuid) {
        let dir = std::env::temp_dir().join(format!(
            "tsk-reports-sync-{}-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let output = run_with(
            [
                "tsk",
                "add",
                "-t",
                "report task",
                "--desk",
                "--state-dir",
                dir.to_str().unwrap(),
            ],
            io::empty(),
            true,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        let id = TaskStore::new(&dir).load().unwrap().tasks()[0].id;
        (ReportStore::new(dir), id)
    }

    struct FailDirectorySync;
    impl AtomicFilesystem for FailDirectorySync {
        type File = fs::File;
        fn create_file(&self, path: &Path) -> io::Result<Self::File> {
            StdFilesystem.create_file(path)
        }
        fn write_all(&self, file: &mut Self::File, data: &[u8]) -> io::Result<()> {
            StdFilesystem.write_all(file, data)
        }
        fn sync_file(&self, file: &Self::File) -> io::Result<()> {
            StdFilesystem.sync_file(file)
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            StdFilesystem.rename(from, to)
        }
        fn sync_directory(&self, _: &Path) -> io::Result<()> {
            Err(io::Error::other("injected directory sync failure"))
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            StdFilesystem.remove_file(path)
        }
    }

    #[test]
    fn identical_registration_reconfirms_uncertain_durability_without_revision_change() {
        let (store, id) = seed();
        assert_eq!(
            store.register_with(TrashTarget::Id(id), "# result\n".into(), &FailDirectorySync),
            Err(ReportError::Io)
        );
        assert_eq!(
            store.read(id).unwrap().revision,
            1,
            "rename was visible but not durable"
        );
        assert_eq!(
            store.register_with(TrashTarget::Id(id), "# result\n".into(), &FailDirectorySync),
            Err(ReportError::Io)
        );
        assert_eq!(
            store.read(id).unwrap().revision,
            1,
            "retry must preserve the visible revision"
        );
        assert_eq!(
            store
                .register(TrashTarget::Id(id), "# result\n".into())
                .unwrap()
                .revision,
            1
        );
    }

    #[test]
    fn repeated_mark_read_reconfirms_uncertain_durability() {
        let (store, id) = seed();
        store
            .register(TrashTarget::Id(id), "result".into())
            .unwrap();
        assert_eq!(
            store.mark_read_with(id, 1, &FailDirectorySync),
            Err(ReportError::Io)
        );
        assert!(!store.read(id).unwrap().unread());
        assert_eq!(
            store.mark_read_with(id, 1, &FailDirectorySync),
            Err(ReportError::Io)
        );
        assert!(!store.mark_read(id, 1).unwrap().unread());
    }
}
