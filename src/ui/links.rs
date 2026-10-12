//! Explicit reference lines in task notes: the issue, PR, CI, and Thread a task names.
//!
//! These are read, never stored separately: no registry, no polling, no forge API. A line
//! counts only when it starts with its exact marker. Identifiers are opaque tokens.

/// One reference a task's notes name, in notes order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskLink {
    /// `Linear: <ID> <URL>` (the importer's marker). `Parent: Linear …` is not this task's
    /// issue and is never read here.
    Issue { id: String, url: String },
    /// `PR: <URL>`.
    Pr { url: String },
    /// `CI: <URL>`.
    Ci { url: String },
    /// `Thread: <agent-name> <workspace-id> <tab-id> <pane-id>`, when it is the task's only
    /// Thread line and carries all four tokens.
    Thread(ThreadRef),
    /// A Thread line that cannot identify one live session: name only, a partial or
    /// overlong tuple, or one of several Thread lines. Shown, never focused.
    UnverifiableThread { text: String, reason: &'static str },
}

/// The full host tuple a Thread line names. Focus needs one live row that matches all four.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRef {
    pub name: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
}

/// Read the reference lines of `notes`, in order.
pub fn task_links(notes: Option<&str>) -> Vec<TaskLink> {
    let Some(notes) = notes else {
        return Vec::new();
    };
    let thread_lines = notes
        .lines()
        .filter(|line| line.trim_start().starts_with("Thread:"))
        .count();
    let mut links = Vec::new();
    for line in notes.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Linear:") {
            let mut tokens = rest.split_whitespace();
            if let (Some(id), Some(url), None) = (tokens.next(), tokens.next(), tokens.next()) {
                links.push(TaskLink::Issue {
                    id: id.to_string(),
                    url: url.to_string(),
                });
            }
        } else if let Some(rest) = line.strip_prefix("PR:") {
            if let Some(url) = single_token(rest) {
                links.push(TaskLink::Pr { url });
            }
        } else if let Some(rest) = line.strip_prefix("CI:") {
            if let Some(url) = single_token(rest) {
                links.push(TaskLink::Ci { url });
            }
        } else if let Some(rest) = line.strip_prefix("Thread:") {
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            let text = tokens.join(" ");
            if text.is_empty() {
                continue;
            }
            let link = match (thread_lines, tokens.as_slice()) {
                (1, [name, workspace, tab, pane]) => TaskLink::Thread(ThreadRef {
                    name: (*name).to_string(),
                    workspace_id: (*workspace).to_string(),
                    tab_id: (*tab).to_string(),
                    pane_id: (*pane).to_string(),
                }),
                (1, [_]) => TaskLink::UnverifiableThread {
                    text,
                    reason: "name only",
                },
                (1, _) => TaskLink::UnverifiableThread {
                    text,
                    reason: "needs name workspace tab pane",
                },
                _ => TaskLink::UnverifiableThread {
                    text,
                    reason: "several Thread lines",
                },
            };
            links.push(link);
        }
    }
    links
}

fn single_token(rest: &str) -> Option<String> {
    let mut tokens = rest.split_whitespace();
    match (tokens.next(), tokens.next()) {
        (Some(url), None) => Some(url.to_string()),
        _ => None,
    }
}

impl TaskLink {
    /// The task-page label column.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Issue { .. } => "issue",
            Self::Pr { .. } => "pr",
            Self::Ci { .. } => "ci",
            Self::Thread(_) | Self::UnverifiableThread { .. } => "thread",
        }
    }

    /// The task-page text after the label.
    pub fn text(&self) -> String {
        match self {
            Self::Issue { id, url } => format!("{id} {url}"),
            Self::Pr { url } | Self::Ci { url } => url.clone(),
            Self::Thread(thread) => format!("{} · enter focuses", thread.name),
            Self::UnverifiableThread { text, reason } => {
                format!("{text} · cannot verify ({reason})")
            }
        }
    }
}
