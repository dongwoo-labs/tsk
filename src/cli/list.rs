//! Read-only headless task listing.

use std::path::PathBuf;

use serde::Serialize;
use uuid::Uuid;

use crate::cli::parser::{parse_task_address, TaskAddress};
use crate::context::snapshot_from_env;
use crate::domain::{HumanStatus, TaskScope};
use crate::scope::resolve_permissive_project_path;
use crate::store::{default_state_dir, TaskStore};

/// Parsed `list` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListInput {
    pub json: bool,
    pub project: Option<String>,
    pub global: bool,
    pub all: bool,
    pub done: bool,
    pub deleted: bool,
    pub archived: bool,
    pub todo: bool,
    pub doing: bool,
    /// With `--done`: also list done tasks moved to the dated done files.
    pub history: bool,
    /// One task addressed by UUID or human number: single-task listing with full detail.
    pub task: Option<TaskAddress>,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

/// The mutually exclusive task set requested by `list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListView {
    Open,
    Done,
    Deleted,
    Archived,
}

/// A list failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListError {
    Store(String),
    /// A well-formed task address that addresses no task in the store.
    UnknownTask,
}

/// One task visible to the list command.
#[derive(Debug, Serialize)]
pub(crate) struct ListRow {
    pub(crate) id: Uuid,
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) status: HumanStatus,
    pub(crate) project: Option<String>,
    pub(crate) parent_task_id: Option<Uuid>,
    pub(crate) children_done: usize,
    pub(crate) children_total: usize,
    /// `archived` / `project archived` mark, set only in the archived view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) archived: Option<&'static str>,
}

/// Complete detail attached only to a direct single-task listing.
#[derive(Debug)]
pub(crate) struct DirectTaskDetails {
    pub(crate) notes: Option<String>,
    pub(crate) steps: Vec<crate::cli::steps::StepLine>,
    pub(crate) parent: Option<ParentRow>,
    pub(crate) children: Vec<ListRow>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ParentRow {
    pub(crate) id: Uuid,
    pub(crate) resolved: bool,
    pub(crate) number: Option<u64>,
    pub(crate) title: Option<String>,
}

/// Read-only result for the list command.
#[derive(Debug)]
pub struct ListResult {
    pub(crate) rows: Vec<ListRow>,
    pub(crate) view: ListView,
    pub(crate) include_scope: bool,
    /// Notes and step lines for one directly addressed task; absent for filtered listings.
    pub(crate) direct: Option<DirectTaskDetails>,
}

/// Parse `tsk list` arguments, including argv0 and the `list` subcommand.
pub fn parse(args: &[String]) -> Result<ListInput, String> {
    if args.get(1).map(String::as_str) != Some("list") {
        return Err("expected list command".into());
    }

    let mut input = ListInput {
        json: false,
        project: None,
        global: false,
        all: false,
        done: false,
        deleted: false,
        archived: false,
        todo: false,
        doing: false,
        history: false,
        task: None,
        state_dir: None,
        help: false,
    };
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            flag if flag.starts_with("--project=") => {
                input.project = Some(flag["--project=".len()..].to_owned());
                index += 1;
            }
            "--json" => {
                input.json = true;
                index += 1;
            }
            "-p" | "--project" => {
                input.project = Some(value(flag)?);
                index += 2;
            }
            "--desk" => {
                input.global = true;
                index += 1;
            }
            "--all" => {
                input.all = true;
                index += 1;
            }
            "--done" => {
                input.done = true;
                index += 1;
            }
            "--deleted" => {
                input.deleted = true;
                index += 1;
            }
            "--archived" => {
                input.archived = true;
                index += 1;
            }
            "--todo" => {
                input.todo = true;
                index += 1;
            }
            "--doing" => {
                input.doing = true;
                index += 1;
            }
            "--history" => {
                input.history = true;
                index += 1;
            }
            "--help" => {
                input.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                input.state_dir = Some(PathBuf::from(flag["--state-dir=".len()..].to_owned()));
                index += 1;
            }
            "--state-dir" => {
                input.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            flag if !flag.starts_with('-') => {
                if input.task.is_some() {
                    return Err(format!("unknown list argument {flag}"));
                }
                input.task = Some(parse_task_address(flag)?);
                index += 1;
            }
            _ => return Err(format!("unknown list argument {flag}")),
        }
    }

    if input.task.is_some() && (input.all || input.global || input.project.is_some()) {
        return Err("task operand cannot be used with --project, --desk, or --all".into());
    }
    if input.task.is_some() && (input.done || input.deleted) {
        return Err("task operand cannot be used with --done or --deleted".into());
    }
    if input.all && (input.global || input.project.is_some()) {
        return Err("--all cannot be used with --project or --desk".into());
    }
    if input.global && input.project.is_some() {
        return Err("--desk cannot be used with --project".into());
    }
    if input.task.is_some() && (input.done || input.deleted || input.archived) {
        return Err("task operand cannot be used with --done, --deleted, or --archived".into());
    }
    if input.task.is_some() && (input.todo || input.doing) {
        return Err("task operand cannot be used with --todo or --doing".into());
    }
    if input.done && input.deleted {
        return Err("--done cannot be used with --deleted".into());
    }
    if input.archived && (input.done || input.deleted) {
        return Err("--archived cannot be used with --done or --deleted".into());
    }
    if input.history && !input.done {
        return Err("--history needs --done".into());
    }
    if input.todo && input.doing {
        return Err("--todo cannot be used with --doing".into());
    }
    if (input.todo || input.doing) && (input.done || input.deleted || input.archived) {
        return Err(
            "--todo and --doing cannot be used with --done, --deleted, or --archived".into(),
        );
    }
    Ok(input)
}

/// Load tasks in the requested scope and view, ordered by their displayed status groups.
pub fn run(input: ListInput) -> Result<ListResult, ListError> {
    let store = TaskStore::new(input.state_dir.unwrap_or_else(default_state_dir));
    let live = store
        .load()
        .map_err(|error| ListError::Store(error.to_string()))?;
    // A direct lookup and `--history` also read the dated done files, so moved tasks,
    // their relationships, and their project names resolve like live ones.
    let domain = if input.task.is_some() || input.history {
        let moved = store
            .load_done_history()
            .map_err(|error| ListError::Store(error.to_string()))?;
        live.with_moved_tasks(moved.into_iter().map(|line| line.task))
    } else {
        live
    };
    if let Some(task_address) = input.task {
        // Single-task listing ignores cwd and filters, and includes complete readable detail.
        let task = domain
            .tasks()
            .iter()
            .find(|task| task_address.matches(task))
            .ok_or(ListError::UnknownTask)?;
        let view = if task.soft_deleted {
            ListView::Deleted
        } else if task.status == HumanStatus::Done {
            ListView::Done
        } else {
            ListView::Open
        };
        return Ok(ListResult {
            rows: vec![row_for(task, &domain)],
            view,
            include_scope: false,
            direct: Some(DirectTaskDetails {
                notes: task.notes.clone(),
                steps: crate::cli::steps::step_lines(&task.steps),
                parent: domain.parent(task.id).map(|parent| match parent {
                    crate::domain::ParentRelation::Resolved(task) => ParentRow {
                        id: task.id,
                        resolved: true,
                        number: task.number,
                        title: Some(task.title.clone()),
                    },
                    crate::domain::ParentRelation::Unresolved(id) => ParentRow {
                        id,
                        resolved: false,
                        number: None,
                        title: None,
                    },
                }),
                children: domain
                    .direct_children(task.id)
                    .into_iter()
                    .map(|child| row_for(child, &domain))
                    .collect(),
            }),
        });
    }
    let scope = (!input.all).then(|| match input.project.as_deref() {
        Some(project) => TaskScope::Project {
            path: resolve_permissive_project_path(project, &domain, Some(&snapshot_from_env())),
        },
        None if input.global => TaskScope::Global,
        None => snapshot_from_env().default_scope,
    });
    let view = if input.deleted {
        ListView::Deleted
    } else if input.done {
        ListView::Done
    } else if input.archived {
        ListView::Archived
    } else {
        ListView::Open
    };
    if view == ListView::Deleted {
        return deleted_rows(&store, &domain, &scope, input.all);
    }
    let mut tasks = domain
        .tasks()
        .iter()
        .filter(|task| !task.is_notice())
        .filter(|task| scope.as_ref().is_none_or(|scope| task.scope == *scope))
        .filter(|task| match view {
            ListView::Open => {
                !task.soft_deleted
                    && !domain.is_hidden(task)
                    && if input.todo {
                        task.status == HumanStatus::Todo
                    } else if input.doing {
                        task.status == HumanStatus::Doing
                    } else {
                        is_open(task.status)
                    }
            }
            ListView::Done => {
                !task.soft_deleted && !domain.is_hidden(task) && task.status == HumanStatus::Done
            }
            ListView::Deleted => task.soft_deleted,
            ListView::Archived => {
                !task.soft_deleted
                    && (task.archived
                        || domain.is_project_archived(match &task.scope {
                            TaskScope::Project { path } => path,
                            TaskScope::Global => "",
                        }))
            }
        })
        .collect::<Vec<_>>();
    // Same order as the board: status group, then the board's section order within it.
    tasks.sort_by(|a, b| {
        status_group_rank(a.status)
            .cmp(&status_group_rank(b.status))
            .then_with(|| crate::ui::queue::cmp_within_status(a, b))
    });
    let rows = tasks
        .into_iter()
        .map(|task| {
            let mut row = row_for(task, &domain);
            if view == ListView::Archived {
                // "archived" wins over "project archived".
                row.archived = Some(if task.archived {
                    "archived"
                } else {
                    "project archived"
                });
            }
            row
        })
        .collect::<Vec<_>>();
    Ok(ListResult {
        rows,
        view,
        include_scope: input.all,
        direct: None,
    })
}

/// `--deleted` listing: live soft-deleted tasks plus trash entries, deduped by id
/// with the live copy winning, ordered by `deleted_at` descending. Trash entries
/// render exactly like live rows and keep their `T<n>` number. Notices are skipped
/// in both places.
fn deleted_rows(
    store: &TaskStore,
    domain: &crate::domain::DomainState,
    scope: &Option<TaskScope>,
    include_scope: bool,
) -> Result<ListResult, ListError> {
    let trash = store
        .load_trash()
        .map_err(|error| ListError::Store(error.to_string()))?;
    let in_scope = |task: &crate::domain::Task| {
        !task.is_notice() && scope.as_ref().is_none_or(|scope| task.scope == *scope)
    };
    let mut dated: Vec<(std::time::SystemTime, ListRow)> = domain
        .tasks()
        .iter()
        .filter(|task| task.soft_deleted && in_scope(task))
        .map(|task| {
            (
                task.soft_deleted_at().unwrap_or(task.updated_at),
                row_for(task, domain),
            )
        })
        .collect();
    let live_ids: std::collections::BTreeSet<Uuid> =
        domain.tasks().iter().map(|task| task.id).collect();
    for line in trash {
        // A task in both places is listed once, from the live copy.
        if live_ids.contains(&line.task.id) || !in_scope(&line.task) {
            continue;
        }
        dated.push((line.deleted_at, row_for(&line.task, domain)));
    }
    dated.sort_by(|(left, _), (right, _)| right.cmp(left));
    Ok(ListResult {
        rows: dated.into_iter().map(|(_, row)| row).collect(),
        view: ListView::Deleted,
        include_scope,
        direct: None,
    })
}

fn row_for(task: &crate::domain::Task, domain: &crate::domain::DomainState) -> ListRow {
    let (children_done, children_total) = domain.child_counts(task.id);
    ListRow {
        parent_task_id: task.parent_task_id,
        children_done,
        children_total,
        id: task.id,
        number: task
            .number
            .expect("loaded tasks receive a number before CLI presentation"),
        title: task.title.clone(),
        status: task.status,
        project: match &task.scope {
            TaskScope::Global => None,
            TaskScope::Project { path } => Some(path.clone()),
        },
        archived: None,
    }
}

fn is_open(status: HumanStatus) -> bool {
    matches!(status, HumanStatus::Todo | HumanStatus::Doing)
}

fn status_group_rank(status: HumanStatus) -> u8 {
    match status {
        HumanStatus::Doing => 0,
        HumanStatus::Todo => 1,
        HumanStatus::Done => 2,
    }
}
