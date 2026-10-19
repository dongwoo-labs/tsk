//! Headless task creation for flag and JSON-plan input.

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::cli::parser::{parse_task_address, FlagAdd, TaskAddress};
use crate::context::snapshot_from_env;
use crate::domain::{DomainState, ProvenanceOrigin, TaskScope};
use crate::scope::{resolve_flag_scope, resolve_project_path};
use crate::store::{default_state_dir, TaskStore};

/// A flag-add failure after parsing and before presenting an output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    EmptyTitle,
    InvalidTitle,
    /// An explicit project token did not identify one valid destination.
    UnknownProject(String),
    /// The resolved scope is an archived project (cwd default or explicit `-p`).
    ProjectArchived(String),
    InvalidParent,
    Store(String),
}

impl AddError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::EmptyTitle => "empty-title",
            Self::InvalidTitle => "invalid-title",
            Self::UnknownProject(_) => "unknown-project",
            Self::ProjectArchived(_) => "project-archived",
            Self::InvalidParent => "invalid-parent",
            Self::Store(_) => "store-error",
        }
    }
}

/// A machine-readable result from a JSON plan.
#[derive(Debug, Serialize)]
pub struct PlanResult {
    created: Vec<Created>,
    existing: Vec<Existing>,
    failed: Vec<Failed>,
}

impl PlanResult {
    pub fn has_failures(&self) -> bool {
        !self.failed.is_empty()
    }
}

#[derive(Debug, Serialize)]
struct Created {
    i: usize,
    id: Uuid,
    number: u64,
    title: String,
}

/// An accepted item that already has a matching non-soft-deleted task.
#[derive(Debug, Serialize)]
struct Existing {
    i: usize,
    id: Uuid,
    number: u64,
    title: String,
}

#[derive(Debug, Serialize)]
struct Failed {
    i: usize,
    title: Option<String>,
    code: &'static str,
    error: String,
}

struct PlanItem {
    parent: Option<TaskAddress>,
    i: usize,
    title: String,
    notes: Option<String>,
    /// `None` is omitted, `Some(None)` is JSON null/global.
    project: Option<Option<String>>,
}

struct ResolvedPlanItem {
    parent: Option<TaskAddress>,
    i: usize,
    title: String,
    notes: Option<String>,
    scope: TaskScope,
}

/// Result of one accepted flag add.
#[derive(Debug)]
pub enum FlagAddResult {
    Created {
        id: Uuid,
        number: u64,
        title: String,
        project: Option<String>,
    },
    Existing {
        id: Uuid,
        number: u64,
        title: String,
        project: Option<String>,
    },
}

/// Create one headless task, or report a non-soft-deleted match without changing it.
pub fn run(input: FlagAdd) -> Result<FlagAddResult, AddError> {
    let title = input.title.ok_or(AddError::EmptyTitle)?;
    // AC-21 rejects any C0 control, including one that end trimming would remove.
    if has_c0_control(&title) {
        return Err(AddError::InvalidTitle);
    }
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(AddError::EmptyTitle);
    }

    let store = TaskStore::new(input.state_dir.unwrap_or_else(default_state_dir));
    let snapshot = snapshot_from_env();
    let project = input.project;
    let global = input.global;
    let notes = input.notes.filter(|notes| !notes.trim().is_empty());
    let (title, notes) = fit_title(&title, notes);
    store
        .locked_transition_if_changed(|domain| {
            let scope = match resolve_flag_scope(project.as_deref(), global, domain, &snapshot) {
                Ok(scope) => scope,
                Err(error) => {
                    return Ok((
                        Err(AddError::UnknownProject(error.message(
                            project.as_deref().expect("only project flags can fail"),
                        ))),
                        false,
                    ));
                }
            };
            if let TaskScope::Project { path } = &scope {
                if domain.is_project_archived(path) {
                    let short = crate::ui::render::short_project(path).to_string();
                    return Ok((Err(AddError::ProjectArchived(short)), false));
                }
            }
            let outcome = if let Some(task) = existing_task(domain, &title, &scope) {
                Ok(FlagAddResult::Existing {
                    id: task.id,
                    number: task
                        .number
                        .expect("loaded tasks receive a number before CLI presentation"),
                    title: task.title.clone(),
                    project: scope_project(&task.scope),
                })
            } else {
                let project = scope_project(&scope);
                let parent = match resolve_parent(domain, input.parent) {
                    Ok(parent) => parent,
                    Err(error) => return Ok((Err(error), false)),
                };
                let id = match domain.create_with_parent(
                    &title,
                    notes,
                    scope,
                    ProvenanceOrigin::Capture,
                    parent,
                ) {
                    Ok(id) => id,
                    Err(error) => {
                        return Ok((Err(AddError::Store(error.to_string())), false));
                    }
                };
                domain.assign_numbers_for_persistence();
                let number = domain
                    .get(id)
                    .and_then(|task| task.number)
                    .expect("new tasks receive a number while the store lock is held");
                Ok(FlagAddResult::Created {
                    id,
                    number,
                    title,
                    project,
                })
            };
            let changed = matches!(outcome, Ok(FlagAddResult::Created { .. }));
            Ok((outcome, changed))
        })
        .map_err(AddError::Store)
        .and_then(|outcome| outcome)
}

/// Create every valid item from a parsed JSON plan in one durable write.
pub fn run_plan(
    values: Vec<Value>,
    state_dir: Option<std::path::PathBuf>,
) -> Result<PlanResult, AddError> {
    let mut valid = Vec::new();
    let mut failed = Vec::new();
    for (i, value) in values.into_iter().enumerate() {
        match parse_plan_item(i, value) {
            Ok(item) => valid.push(item),
            Err(item_failure) => failed.push(item_failure),
        }
    }

    if valid.is_empty() {
        return Ok(PlanResult {
            created: Vec::new(),
            existing: Vec::new(),
            failed,
        });
    }

    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    let snapshot = snapshot_from_env();
    store
        .locked_transition_if_changed(|domain| {
            let (resolved, resolution_failures) = resolve_plan_items(valid, domain, &snapshot);
            failed.extend(resolution_failures);
            let mut created = Vec::with_capacity(resolved.len());
            let mut existing = Vec::new();
            for item in resolved {
                if let TaskScope::Project { path } = &item.scope {
                    if domain.is_project_archived(path) {
                        failed.push(fail_item(
                            item.i,
                            Some(item.title),
                            "project-archived",
                            "project is archived: use --desk, -p, or tsk project unarchive",
                        ));
                        continue;
                    }
                }
                if let Some(task) = existing_task(domain, &item.title, &item.scope) {
                    existing.push(Existing {
                        i: item.i,
                        id: task.id,
                        number: task
                            .number
                            .expect("loaded tasks receive a number before CLI presentation"),
                        title: item.title,
                    });
                    continue;
                }
                let parent = match resolve_parent(domain, item.parent) {
                    Ok(parent) => parent,
                    Err(_) => {
                        failed.push(fail_item(
                            item.i,
                            Some(item.title),
                            "invalid-parent",
                            "parent is missing, deleted, or a notice",
                        ));
                        continue;
                    }
                };
                let id = domain
                    .create_with_parent(
                        &item.title,
                        item.notes,
                        item.scope,
                        ProvenanceOrigin::Capture,
                        parent,
                    )
                    .map_err(|error| error.to_string())?;
                domain.assign_numbers_for_persistence();
                let number = domain
                    .get(id)
                    .and_then(|task| task.number)
                    .expect("new tasks receive a number while the store lock is held");
                created.push(Created {
                    i: item.i,
                    id,
                    number,
                    title: item.title,
                });
            }
            let changed = !created.is_empty();
            // Resolution and archived-project refusals join parse failures inside the
            // transaction, so restore item order before reporting.
            failed.sort_by_key(|failure| failure.i);
            Ok((
                PlanResult {
                    created,
                    existing,
                    failed,
                },
                changed,
            ))
        })
        .map_err(AddError::Store)
}

fn resolve_plan_items(
    items: Vec<PlanItem>,
    domain: &DomainState,
    snapshot: &crate::context::InvocationSnapshot,
) -> (Vec<ResolvedPlanItem>, Vec<Failed>) {
    let mut resolved = Vec::with_capacity(items.len());
    let mut failed = Vec::new();
    for item in items {
        let scope = match item.project.as_ref() {
            Some(None) => TaskScope::Global,
            Some(Some(project)) => match resolve_project_path(project, domain, Some(snapshot)) {
                Ok(path) => TaskScope::Project { path },
                Err(error) => {
                    failed.push(fail_item(
                        item.i,
                        Some(item.title),
                        "unknown-project",
                        error.message(project),
                    ));
                    continue;
                }
            },
            None => snapshot.default_scope.clone(),
        };
        resolved.push(ResolvedPlanItem {
            parent: item.parent,
            i: item.i,
            title: item.title,
            notes: item.notes,
            scope,
        });
    }
    (resolved, failed)
}

pub(crate) fn resolve_parent(
    domain: &DomainState,
    address: Option<TaskAddress>,
) -> Result<Option<Uuid>, AddError> {
    address
        .map(|address| {
            domain
                .tasks()
                .iter()
                .find(|task| address.matches(task) && !task.soft_deleted)
                .map(|task| task.id)
                .ok_or(AddError::InvalidParent)
        })
        .transpose()
}

fn scope_project(scope: &TaskScope) -> Option<String> {
    match scope {
        TaskScope::Global => None,
        TaskScope::Project { path } => Some(path.clone()),
    }
}

fn existing_task<'a>(
    domain: &'a DomainState,
    title: &str,
    scope: &TaskScope,
) -> Option<&'a crate::domain::Task> {
    // Notice rows are board-only (no CLI address reaches them, not even their UUID) and
    // deliberately carry no T number, so a title collision with a seeded guide or the
    // announcement must never resolve as existing: the add creates an ordinary task.
    domain.tasks().iter().find(|task| {
        !task.soft_deleted && !task.is_notice() && task.title == title && &task.scope == scope
    })
}

fn parse_plan_item(i: usize, value: Value) -> Result<PlanItem, Failed> {
    let Some(object) = value.as_object() else {
        return Err(fail_item(i, None, "invalid-item", "item must be an object"));
    };
    let title = match object.get("title") {
        None => return Err(fail_item(i, None, "empty-title", "title is required")),
        Some(Value::String(title)) => title,
        Some(_) => return Err(fail_item(i, None, "invalid-item", "title must be a string")),
    };
    let trimmed_title = title.trim().to_string();
    if has_c0_control(title) {
        return Err(fail_item(
            i,
            Some(trimmed_title),
            "invalid-title",
            "title contains a control character",
        ));
    }
    if trimmed_title.is_empty() {
        return Err(fail_item(
            i,
            Some(trimmed_title),
            "empty-title",
            "title is empty",
        ));
    }

    let notes = match object.get("notes") {
        None | Some(Value::Null) => None,
        Some(Value::String(notes)) if notes.trim().is_empty() => None,
        Some(Value::String(notes)) => Some(notes.clone()),
        Some(_) => {
            return Err(fail_item(
                i,
                Some(trimmed_title),
                "invalid-item",
                "notes must be a string or null",
            ));
        }
    };
    let project = match object.get("project") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(project)) => Some(Some(project.clone())),
        Some(_) => {
            return Err(fail_item(
                i,
                Some(trimmed_title),
                "invalid-item",
                "project must be a string or null",
            ));
        }
    };
    // Thread labels left the store in v8. Refuse a value rather than drop it silently.
    if object.get("thread").is_some_and(|thread| !thread.is_null()) {
        return Err(fail_item(
            i,
            Some(trimmed_title),
            "invalid-item",
            "thread labels were removed; drop the thread field",
        ));
    }

    let parent =
        match object.get("parent") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(parse_task_address(value).map_err(|error| {
                fail_item(i, Some(trimmed_title.clone()), "invalid-parent", error)
            })?),
            Some(_) => {
                return Err(fail_item(
                    i,
                    Some(trimmed_title),
                    "invalid-parent",
                    "parent must be a task address string or null",
                ))
            }
        };
    let (trimmed_title, notes) = fit_title(&trimmed_title, notes);
    Ok(PlanItem {
        parent,
        i,
        title: trimmed_title,
        notes,
        project,
    })
}

fn fail_item(
    i: usize,
    title: Option<String>,
    code: &'static str,
    error: impl Into<String>,
) -> Failed {
    Failed {
        i,
        title,
        code,
        error: error.into(),
    }
}

/// Longest stored title, in characters, counting the closing `…`.
pub(crate) const TITLE_MAX_CHARS: usize = 60;

/// Fit a title into `TITLE_MAX_CHARS`: cut at a word end, append `…`, and keep the full
/// title in the notes. A title that already fits passes through unchanged.
pub(crate) fn fit_title(title: &str, notes: Option<String>) -> (String, Option<String>) {
    if title.chars().count() <= TITLE_MAX_CHARS {
        return (title.to_string(), notes);
    }
    let head: String = title.chars().take(TITLE_MAX_CHARS - 1).collect();
    let next_is_space = title
        .chars()
        .nth(TITLE_MAX_CHARS - 1)
        .is_some_and(char::is_whitespace);
    let cut = if next_is_space {
        head.as_str()
    } else {
        match head.rfind(char::is_whitespace) {
            Some(at) if !head[..at].trim().is_empty() => &head[..at],
            _ => head.as_str(),
        }
    };
    let short = format!("{}…", cut.trim_end());
    let notes = match notes {
        Some(existing) if existing.contains(title) => Some(existing),
        Some(existing) => Some(format!("{title}\n\n{existing}")),
        None => Some(title.to_string()),
    };
    (short, notes)
}

pub(crate) fn has_c0_control(value: &str) -> bool {
    value.chars().any(|character| character <= '\u{001f}')
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::cli::parser::FlagAdd;

    #[test]
    fn title_controls_are_rejected_before_end_trimming() {
        let input = FlagAdd {
            parent: None,
            title: Some("hello\n".into()),
            notes: None,
            project: None,
            global: false,
            json: false,
            state_dir: None,
            file: None,
            has_item_flags: true,
            help: false,
        };
        assert_eq!(run(input).unwrap_err().code(), "invalid-title");
    }
}

#[cfg(test)]
mod fit_title_tests {
    use super::*;

    #[test]
    fn short_title_passes_through() {
        assert_eq!(fit_title("short", None), ("short".into(), None));
    }

    #[test]
    fn long_title_cuts_at_word_end_and_keeps_full_in_notes() {
        let long = "word ".repeat(30);
        let long = long.trim();
        let (title, notes) = fit_title(long, Some("extra".into()));
        assert!(title.chars().count() <= TITLE_MAX_CHARS);
        assert!(title.ends_with("word…"));
        assert_eq!(notes.unwrap(), format!("{long}\n\nextra"));
    }

    #[test]
    fn unbroken_long_title_hard_cuts_by_chars() {
        let long = "한".repeat(80);
        let (title, notes) = fit_title(&long, None);
        assert_eq!(title.chars().count(), TITLE_MAX_CHARS);
        assert_eq!(notes.unwrap(), long);
    }
}
