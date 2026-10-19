//! Headless title, notes, parent, and project edits.

use std::path::PathBuf;

use crate::cli::add::has_c0_control;
use crate::cli::parser::TaskAddress;
use crate::context::snapshot_from_env;
use crate::domain::{DomainError, DomainState, TaskScope};
use crate::scope::resolve_flag_scope;
use crate::store::{default_state_dir, TaskStore};
use uuid::Uuid;

/// Parsed field values after the argv boundary. `None` means leave the stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFields {
    pub parent: Option<Option<TaskAddress>>,
    pub after: Option<Vec<TaskAddress>>,
    pub title: Option<String>,
    pub notes: Option<String>,
    /// Move to this project (name or path), resolved like `add`.
    pub project: Option<String>,
    /// Move to the desk.
    pub global: bool,
}

/// A successful edit, including an idempotent repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditResult {
    pub number: u64,
    pub title: String,
}

/// An edit failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    UnknownTask,
    SoftDeletedTask,
    EmptyTitle,
    InvalidTitle,
    InvalidParent,
    ParentCycle,
    InvalidAfter,
    AfterCycle,
    UnknownProject(String),
    ProjectArchived(String),
    Store(String),
}

impl EditError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::EmptyTitle => "empty-title",
            Self::InvalidTitle => "invalid-title",
            Self::InvalidParent => "invalid-parent",
            Self::ParentCycle => "parent-cycle",
            Self::InvalidAfter => "invalid-after",
            Self::AfterCycle => "after-cycle",
            Self::UnknownProject(_) => "unknown-project",
            Self::ProjectArchived(_) => "project-archived",
            Self::Store(_) => "store-error",
        }
    }
}

/// Replace the supplied title, notes, parent, and/or scope.
///
/// Scope changes only on an explicit `--project` or `--desk`. Whitespace-only notes clear
/// the field. Repeating the stored values writes nothing.
pub fn run(
    target: TaskAddress,
    fields: EditFields,
    state_dir: Option<PathBuf>,
) -> Result<EditResult, EditError> {
    if let Some(title) = fields.title.as_deref() {
        if has_c0_control(title) {
            return Err(EditError::InvalidTitle);
        }
        if title.trim().is_empty() {
            return Err(EditError::EmptyTitle);
        }
    }

    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    let snapshot = snapshot_from_env();
    store
        .locked_transition_if_changed(|state: &mut DomainState| {
            let scope = if fields.project.is_some() || fields.global {
                match resolve_flag_scope(fields.project.as_deref(), fields.global, state, &snapshot)
                {
                    Ok(scope) => Some(scope),
                    Err(error) => {
                        let token = fields.project.as_deref().unwrap_or_default();
                        return Ok((Err(EditError::UnknownProject(error.message(token))), false));
                    }
                }
            } else {
                None
            };
            let found = state
                .tasks()
                .iter()
                .find(|task| target.matches(task))
                .map(|task| Found {
                    id: task.id,
                    number: task.number,
                    title: task.title.clone(),
                    notes: task.notes.clone(),
                    scope: task.scope.clone(),
                    soft_deleted: task.soft_deleted,
                    parent_task_id: task.parent_task_id,
                });
            Ok(apply(state, found, &fields, scope))
        })
        .map_err(EditError::Store)?
}

struct Found {
    id: Uuid,
    number: Option<u64>,
    title: String,
    notes: Option<String>,
    scope: TaskScope,
    soft_deleted: bool,
    parent_task_id: Option<Uuid>,
}

fn apply(
    state: &mut DomainState,
    found: Option<Found>,
    fields: &EditFields,
    scope: Option<TaskScope>,
) -> (Result<EditResult, EditError>, bool) {
    let Some(found) = found else {
        return (Err(EditError::UnknownTask), false);
    };
    let Some(number) = found.number else {
        return (Err(EditError::UnknownTask), false);
    };
    if found.soft_deleted {
        return (Err(EditError::SoftDeletedTask), false);
    }
    let next_title = fields
        .title
        .as_deref()
        .map(str::trim)
        .unwrap_or(found.title.as_str())
        .to_string();
    let next_notes = match fields.notes.as_deref() {
        Some(value) if value.trim().is_empty() => None,
        Some(value) => Some(value.to_string()),
        None => found.notes.clone(),
    };
    let parent_task_id = match fields.parent {
        None => found.parent_task_id,
        Some(None) => None,
        Some(Some(address)) => match state.tasks().iter().find(|task| address.matches(task)) {
            Some(task) if !task.soft_deleted => Some(task.id),
            _ => return (Err(EditError::InvalidParent), false),
        },
    };
    let next_after = match &fields.after {
        None => None,
        Some(addresses) => {
            let mut ids = Vec::with_capacity(addresses.len());
            for address in addresses {
                match state
                    .tasks()
                    .iter()
                    .find(|task| address.matches(task) && !task.soft_deleted)
                {
                    Some(task) => ids.push(task.id),
                    None => return (Err(EditError::InvalidAfter), false),
                }
            }
            Some(ids)
        }
    };
    let (next_title, next_notes) = if fields.title.is_some() {
        super::add::fit_title(&next_title, next_notes)
    } else {
        (next_title, next_notes)
    };
    let next_scope = scope.unwrap_or_else(|| found.scope.clone());
    if next_scope != found.scope {
        if let TaskScope::Project { path } = &next_scope {
            if state.is_project_archived(path) {
                let short = crate::ui::render::short_project(path).to_string();
                return (Err(EditError::ProjectArchived(short)), false);
            }
        }
    }
    let after_changed = match &next_after {
        None => false,
        Some(ids) => {
            let mut deduped: Vec<Uuid> = Vec::new();
            for id in ids {
                if !deduped.contains(id) {
                    deduped.push(*id);
                }
            }
            if let Err(error) = state.validate_after(found.id, &deduped) {
                return (
                    Err(match error {
                        DomainError::AfterCycle(_) => EditError::AfterCycle,
                        _ => EditError::InvalidAfter,
                    }),
                    false,
                );
            }
            state
                .get(found.id)
                .is_some_and(|task| task.after != deduped)
        }
    };
    if next_title == found.title
        && !after_changed
        && next_notes == found.notes
        && parent_task_id == found.parent_task_id
        && next_scope == found.scope
    {
        return (
            Ok(EditResult {
                number,
                title: found.title,
            }),
            false,
        );
    }
    if after_changed {
        let ids = next_after.expect("after_changed implies a new list");
        if let Err(error) = state.set_after(found.id, ids) {
            return (Err(EditError::Store(error.to_string())), false);
        }
    }
    let unchanged_rest = next_title == found.title
        && next_notes == found.notes
        && parent_task_id == found.parent_task_id
        && next_scope == found.scope;
    if unchanged_rest {
        return (
            Ok(EditResult {
                number,
                title: found.title,
            }),
            true,
        );
    }
    match state.edit_with_parent(
        found.id,
        &next_title,
        next_notes,
        next_scope,
        parent_task_id,
    ) {
        Ok(()) => (
            Ok(EditResult {
                number,
                title: next_title,
            }),
            true,
        ),
        Err(DomainError::EmptyTitle) => (Err(EditError::EmptyTitle), false),
        Err(DomainError::UnknownId(_)) => (Err(EditError::UnknownTask), false),
        Err(DomainError::InvalidParent(_)) => (Err(EditError::InvalidParent), false),
        Err(DomainError::ParentCycle(_)) => (Err(EditError::ParentCycle), false),
        Err(other) => (Err(EditError::Store(other.to_string())), false),
    }
}
