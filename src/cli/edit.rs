//! Headless edits: title, notes, assignee, base, after, and who an open block or review
//! waits on.

use std::path::PathBuf;

use crate::agents::AgentProfiles;
use crate::cli::add::has_c0_control;
use crate::cli::parser::TaskAddress;
use crate::context::snapshot_from_env;
use crate::domain::{
    block_text, Assignee, BlockField, BlockKind, BlockOn, DomainError, DomainState, TaskScope,
};
use crate::scope::resolve_flag_scope;
use crate::store::{default_state_dir, TaskStore};
use uuid::Uuid;

/// Parsed field values after the argv boundary. `None` means leave the stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFields {
    pub machine: Option<Option<String>>,
    pub parent: Option<Option<TaskAddress>>,
    pub after: Option<Vec<TaskAddress>>,
    pub title: Option<String>,
    pub notes: Option<String>,
    /// Move to this project (name or path), resolved like `add`.
    pub project: Option<String>,
    /// Move to the desk.
    pub global: bool,
    /// Text added below the stored notes, after one blank line.
    pub append_notes: Option<String>,
    /// `None` leaves it unchanged, `Some(None)` clears it. A name is read by the name rule.
    pub assignee: Option<Option<String>>,
    /// `None` leaves it unchanged, `Some(None)` clears it.
    pub base: Option<Option<String>>,
    /// `None` leaves it unchanged; `Some` replaces the list (empty clears it).
    /// Who the open block or review waits on, as typed; `None` leaves it.
    pub on: Option<String>,
}

/// A successful edit, including an idempotent repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditResult {
    pub number: u64,
    pub title: String,
    /// Size of the stored notes after an edit that supplied `--notes`, else `None`.
    pub notes_bytes: Option<usize>,
    /// What `--assignee` resolved to, for the line that names a person.
    pub assignee: Option<Assignee>,
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
    AfterCycle,
    UnknownProject(String),
    ProjectArchived(String),
    UnknownBase(String),
    AgentConfig(String),
    /// An `--after` task is this task, not on the board, or already done.
    InvalidAfter(String),
    /// An `--after` task already runs after this one.
    AfterLoop(String),
    /// `--on` with no open block or review on the task.
    NotBlocked,
    /// `--on` names this task, or a task not on the board.
    InvalidBlocker,
    /// `--on` text is longer than the block text limit.
    TextTooLong,
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
            Self::AfterCycle => "after-cycle",
            Self::UnknownProject(_) => "unknown-project",
            Self::ProjectArchived(_) => "project-archived",
            Self::UnknownBase(_) => "unknown-base",
            Self::AgentConfig(_) => "agent-config",
            Self::InvalidAfter(_) => "invalid-after",
            Self::AfterLoop(_) => "after-loop",
            Self::NotBlocked => "not-blocked",
            Self::InvalidBlocker => "invalid-blocker",
            Self::TextTooLong => "text-too-long",
            Self::Store(_) => "store-error",
        }
    }
}

/// Replace the supplied title, notes, parent, and/or scope.
///
/// Whitespace-only notes clear the field. Repeating the stored values writes nothing; a
/// repeated append adds its text again.
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

    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    // The name rule: you, an agent profile, or any other name for a person. Broken profiles
    // refuse rather than guess, so a profile's name never becomes a person by accident.
    let assignee = match fields.assignee.as_ref() {
        Some(Some(name)) => {
            let profiles = AgentProfiles::load(&state_dir)
                .map_err(|error| EditError::AgentConfig(error.to_string()))?;
            Some(Some(
                profiles
                    .resolve_assignee(name)
                    .map_err(EditError::AgentConfig)?,
            ))
        }
        Some(None) => Some(None),
        None => None,
    };
    // Like `tsk status … --on`: a broken profile file reads as no profiles.
    let profiles = fields
        .on
        .as_ref()
        .map(|_| AgentProfiles::load(&state_dir).unwrap_or_default());
    let edit = Edit {
        machine: fields.machine,
        title: fields.title,
        notes: fields.notes,
        append_notes: fields.append_notes,
        assignee,
        base: fields.base,
        after: fields.after,
        parent: fields.parent,
        on: fields.on.zip(profiles),
    };
    let store = TaskStore::new(state_dir);
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
                    machine: task.machine.clone(),
                    notes: task.notes.clone(),
                    scope: task.scope.clone(),
                    thread: task.thread.clone(),
                    assignee: task.assigned(),
                    base: task.base.clone(),
                    after: task.after.clone(),
                    soft_deleted: task.soft_deleted,
                    parent_task_id: task.parent_task_id,
                });
            Ok(apply(state, found, &edit, scope))
        })
        .map_err(EditError::Store)?
}

/// The edit after the boundary resolved its names.
struct Edit {
    machine: Option<Option<String>>,
    title: Option<String>,
    notes: Option<String>,
    append_notes: Option<String>,
    assignee: Option<Option<Assignee>>,
    base: Option<Option<String>>,
    after: Option<Vec<TaskAddress>>,
    parent: Option<Option<TaskAddress>>,
    /// The typed `--on` value and the profiles it is read against.
    on: Option<(String, AgentProfiles)>,
}

struct Found {
    machine: Option<String>,
    id: Uuid,
    number: Option<u64>,
    title: String,
    notes: Option<String>,
    scope: TaskScope,
    thread: Option<String>,
    assignee: Option<Assignee>,
    base: Option<String>,
    after: Vec<u64>,
    soft_deleted: bool,
    parent_task_id: Option<Uuid>,
}

fn apply(
    state: &mut DomainState,
    found: Option<Found>,
    fields: &Edit,
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
    let (next_title, next_notes) = if fields.title.is_some() {
        super::add::fit_title(&next_title, next_notes)
    } else {
        (next_title, next_notes)
    };
    let notes_bytes = fields
        .notes
        .is_some()
        .then(|| next_notes.as_ref().map_or(0, String::len));
    let next_scope = scope.unwrap_or_else(|| found.scope.clone());
    if next_scope != found.scope {
        if let TaskScope::Project { path } = &next_scope {
            if state.is_project_archived(path) {
                let short = crate::ui::render::short_project(path).to_string();
                return (Err(EditError::ProjectArchived(short)), false);
            }
        }
    }
    // Appended under the lock, to the notes as stored now, so a concurrent edit is kept.
    let next_notes = match (fields.append_notes.as_deref(), next_notes) {
        (Some(text), Some(notes)) if !notes.trim().is_empty() => {
            Some(format!("{}\n\n{text}", notes.trim_end()))
        }
        (Some(text), _) => Some(text.to_string()),
        (None, notes) => notes,
    };
    let next_assignee = fields
        .assignee
        .clone()
        .unwrap_or_else(|| found.assignee.clone());
    let next_base = fields.base.clone().unwrap_or_else(|| found.base.clone());
    if let Some(base) = fields.base.as_ref().and_then(|value| value.as_deref()) {
        if let Err(error) = crate::git_base::validate_task_base(&found.scope, base, true) {
            return (Err(EditError::UnknownBase(error)), false);
        }
    }
    let next_after = match &fields.after {
        None => found.after.clone(),
        Some(addresses) => {
            let mut numbers = Vec::new();
            for address in addresses {
                let Some(number) = state
                    .tasks()
                    .iter()
                    .find(|task| address.matches(task) && !task.soft_deleted)
                    .and_then(|task| task.number)
                else {
                    return (
                        Err(EditError::InvalidAfter(format!(
                            "{} is not on the board",
                            address.display()
                        ))),
                        false,
                    );
                };
                if !numbers.contains(&number) {
                    numbers.push(number);
                }
            }
            numbers
        }
    };
    let changed_after = next_after != found.after;
    if changed_after {
        if let Err(error) = state.check_after(found.id, &next_after) {
            return (Err(after_error(error)), false);
        }
    }
    let on = match fields.on.as_ref() {
        Some((value, profiles)) => match waiting_on(state, found.id, number, value, profiles) {
            Ok(on) => Some(on),
            Err(error) => return (Err(error), false),
        },
        None => None,
    };
    let changed_on = on.as_ref().is_some_and(|on| {
        state
            .get(found.id)
            .and_then(|task| task.block.as_ref())
            .is_some_and(|block| block.on != *on)
    });
    let assignee = fields.assignee.clone().flatten();
    let next_machine = fields
        .machine
        .clone()
        .unwrap_or_else(|| found.machine.clone());
    if next_machine == found.machine
        && parent_task_id == found.parent_task_id
        && next_scope == found.scope
        && next_title == found.title
        && next_notes == found.notes
        && next_assignee == found.assignee
        && next_base == found.base
        && !changed_after
        && !changed_on
    {
        return (
            Ok(EditResult {
                number,
                title: found.title,
                notes_bytes,
                assignee,
            }),
            false,
        );
    }
    if let Err(error) = state.validate_parent(found.id, parent_task_id) {
        return (
            Err(match error {
                DomainError::ParentCycle(_) => EditError::ParentCycle,
                _ => EditError::InvalidParent,
            }),
            false,
        );
    }
    let fields_changed = next_scope != found.scope
        || next_title != found.title
        || next_notes != found.notes
        || next_assignee != found.assignee
        || next_base != found.base;
    let edited = state.as_one_change(found.id, |state| {
        let edited = if fields_changed {
            state.edit_with_assignee_and_base(
                found.id,
                &next_title,
                next_notes,
                next_scope,
                found.thread,
                next_assignee,
                next_base,
            )
        } else {
            Ok(())
        };
        let edited = edited.and_then(|()| {
            if parent_task_id != found.parent_task_id {
                state.set_parent(found.id, parent_task_id).map(|_| ())
            } else {
                Ok(())
            }
        });
        let edited = edited.and_then(|()| {
            if changed_after {
                state.edit_after(found.id, &next_after).map(|_| ())
            } else {
                Ok(())
            }
        });
        let edited = edited.and_then(|()| match on {
            Some(on) if changed_on => state.set_on(found.id, on).map(|_| ()),
            _ => Ok(()),
        });
        edited.and_then(|()| {
            if next_machine != found.machine {
                state.set_machine(found.id, next_machine).map(|_| ())
            } else {
                Ok(())
            }
        })
    });
    match edited {
        Ok(()) => (
            Ok(EditResult {
                number,
                title: next_title,
                notes_bytes,
                assignee,
            }),
            true,
        ),
        Err(DomainError::EmptyTitle) => (Err(EditError::EmptyTitle), false),
        Err(DomainError::UnknownId(_)) => (Err(EditError::UnknownTask), false),
        Err(DomainError::InvalidParent(_)) => (Err(EditError::InvalidParent), false),
        Err(DomainError::ParentCycle(_)) => (Err(EditError::ParentCycle), false),
        Err(
            error @ (DomainError::AfterSelf(_)
            | DomainError::AfterUnknown(_)
            | DomainError::AfterDone(_)
            | DomainError::AfterLoop { .. }),
        ) => (Err(after_error(error)), false),
        Err(other) => (Err(EditError::Store(other.to_string())), false),
    }
}

/// Read `--on` for the task's open record by the name rule: on a block a task address names
/// another live task; on a review it is text. No open record refuses with `not-blocked`.
fn waiting_on(
    state: &DomainState,
    id: Uuid,
    number: u64,
    value: &str,
    profiles: &AgentProfiles,
) -> Result<BlockOn, EditError> {
    let kind = state
        .get(id)
        .and_then(|task| task.block.as_ref())
        .map(|block| block.kind)
        .ok_or(EditError::NotBlocked)?;
    match profiles.resolve_on(value, kind) {
        BlockOn::Task(task) => {
            let exists = state.tasks().iter().any(|other| {
                other.number == Some(task) && !other.soft_deleted && !other.is_notice()
            });
            if task == number || !exists || kind != BlockKind::Blocked {
                return Err(EditError::InvalidBlocker);
            }
            Ok(BlockOn::Task(task))
        }
        BlockOn::Other(text) => match block_text(Some(&text), BlockField::On) {
            Ok(Some(text)) => Ok(BlockOn::Other(text)),
            Ok(None) => Ok(BlockOn::You),
            Err(_) => Err(EditError::TextTooLong),
        },
        on => Ok(on),
    }
}

fn after_error(error: DomainError) -> EditError {
    match error {
        DomainError::AfterLoop { .. } => EditError::AfterLoop(error.to_string()),
        DomainError::UnknownId(_) => EditError::UnknownTask,
        other => EditError::InvalidAfter(other.to_string()),
    }
}
