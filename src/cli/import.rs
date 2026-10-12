//! Headless one-way import of Linear issues into tasks (`tsk import linear`).
//!
//! The flow is fetch, then plan, then one locked store transition. Fetching talks to
//! Linear's GraphQL API through an injected transport, so tests feed fixture pages
//! without network. Nothing is written until every page has been fetched and parsed:
//! a fetch failure persists nothing.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::context::snapshot_from_env;
use crate::domain::{DomainState, HumanStatus, ProvenanceOrigin, Task, TaskScope};
use crate::scope::resolve_flag_scope;
use crate::store::{default_state_dir, TaskStore};

/// Linear's GraphQL endpoint.
pub const LINEAR_API_URL: &str = "https://api.linear.app/graphql";
/// The personal API key, sent verbatim as the `Authorization` header.
pub const API_KEY_ENV: &str = "LINEAR_API_KEY";
/// Test hook, not a user setting: a path to fixture JSON that stands in for Linear's
/// HTTP responses. The file holds one GraphQL response object (returned for every page
/// request) or an array of them (returned in page order). Read only by
/// [`default_transport`]; the API key is still required so the missing-key path stays
/// testable end to end.
pub const FIXTURE_ENV: &str = "TSK_LINEAR_FIXTURE";
/// Notes longer than this many characters are cut and marked.
pub const NOTES_LIMIT: usize = 2000;
const TRUNCATION_MARK: &str = "… (truncated; full description in Linear)";
const PAGE_SIZE: usize = 50;
#[cfg(unix)]
const CURL_TIMEOUT_SECS: u32 = 30;

const ISSUES_QUERY: &str = "query TskImport($filter: IssueFilter, $first: Int!, $after: String) { issues(filter: $filter, first: $first, after: $after) { nodes { identifier title url description state { name type } team { key } parent { identifier url } } pageInfo { hasNextPage endCursor } } }";

/// Who the imported issues must be assigned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assignee {
    Me,
    Email(String),
    Any,
}

/// Linear workflow state types accepted by `--state`.
pub const STATE_TYPES: &[&str] = &[
    "triage",
    "backlog",
    "unstarted",
    "started",
    "completed",
    "canceled",
];
const DEFAULT_STATE_TYPES: &[&str] = &["backlog", "unstarted", "started"];

/// Parsed `tsk import linear` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportInput {
    pub team: Option<String>,
    pub linear_project: Option<String>,
    pub assignee: Assignee,
    pub states: Vec<String>,
    pub limit: usize,
    /// Destination scope, as for `add`: `-p` token or `--desk`.
    pub project: Option<String>,
    pub global: bool,
    pub json: bool,
    pub dry_run: bool,
    pub update: bool,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

impl Default for ImportInput {
    fn default() -> Self {
        Self {
            team: None,
            linear_project: None,
            assignee: Assignee::Me,
            states: DEFAULT_STATE_TYPES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            limit: 100,
            project: None,
            global: false,
            json: false,
            dry_run: false,
            update: false,
            state_dir: None,
            help: false,
        }
    }
}

/// Parse `tsk import linear …` arguments, including argv0 and the `import` verb.
pub fn parse(args: &[String]) -> Result<ImportInput, String> {
    if args.get(1).map(String::as_str) != Some("import") {
        return Err("expected import command".into());
    }
    let mut parsed = ImportInput::default();
    let mut source: Option<String> = None;
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        // `--flag=value` and `--flag value` share one arm per flag.
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
            _ => (flag, None),
        };
        let consumed = if inline.is_some() { 1 } else { 2 };
        let value = || -> Result<String, String> {
            if let Some(value) = inline.clone() {
                return Ok(value);
            }
            match args.get(index + 1) {
                Some(value) if !value.starts_with('-') => Ok(value.clone()),
                _ => Err(format!("missing value for {name}")),
            }
        };
        match name {
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            "--json" => {
                parsed.json = true;
                index += 1;
            }
            "--dry-run" => {
                parsed.dry_run = true;
                index += 1;
            }
            "--update" => {
                parsed.update = true;
                index += 1;
            }
            "--desk" => {
                parsed.global = true;
                index += 1;
            }
            "-p" | "--project" => {
                parsed.project = Some(value()?);
                index += consumed;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value()?));
                index += consumed;
            }
            "--team" => {
                let team = value()?.trim().to_string();
                if team.is_empty() {
                    return Err("--team needs a team key".into());
                }
                parsed.team = Some(team);
                index += consumed;
            }
            "--linear-project" => {
                let project = value()?.trim().to_string();
                if project.is_empty() {
                    return Err("--linear-project needs a project name".into());
                }
                parsed.linear_project = Some(project);
                index += consumed;
            }
            "--assignee" => {
                let assignee = value()?.trim().to_string();
                parsed.assignee = match assignee.to_ascii_lowercase().as_str() {
                    "me" => Assignee::Me,
                    "any" => Assignee::Any,
                    _ if assignee.contains('@') => Assignee::Email(assignee),
                    _ => {
                        return Err(format!(
                            "--assignee must be me, any, or an email address, not {assignee}"
                        ))
                    }
                };
                index += consumed;
            }
            "--state" => {
                parsed.states = parse_states(&value()?)?;
                index += consumed;
            }
            "--limit" => {
                let raw = value()?;
                parsed.limit = raw
                    .parse::<usize>()
                    .ok()
                    .filter(|limit| *limit > 0)
                    .ok_or_else(|| format!("--limit must be a positive number, not {raw}"))?;
                index += consumed;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown import argument {flag}")),
            positional => {
                if source.is_some() {
                    return Err(format!("unexpected import argument {positional}"));
                }
                source = Some(positional.to_string());
                index += 1;
            }
        }
    }
    if parsed.help {
        return Ok(parsed);
    }
    match source.as_deref() {
        Some("linear") => {}
        Some(other) => return Err(format!("unknown import source {other}; expected linear")),
        None => return Err("import source is required: tsk import linear".into()),
    }
    if parsed.global && parsed.project.is_some() {
        return Err("--desk cannot be used with --project".into());
    }
    Ok(parsed)
}

fn parse_states(raw: &str) -> Result<Vec<String>, String> {
    let mut states = Vec::new();
    for state in raw.split(',').map(|s| s.trim().to_ascii_lowercase()) {
        if state.is_empty() {
            continue;
        }
        if !STATE_TYPES.contains(&state.as_str()) {
            return Err(format!(
                "unknown state type {state}; expected {}",
                STATE_TYPES.join(", ")
            ));
        }
        if !states.contains(&state) {
            states.push(state);
        }
    }
    if states.is_empty() {
        return Err("--state needs at least one state type".into());
    }
    Ok(states)
}

/// An import failure. Every variant except `Store` persists nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// `LINEAR_API_KEY` is unset or blank.
    MissingApiKey,
    /// Transport, HTTP status, GraphQL error, or an unreadable response.
    Linear(String),
    /// The HTTP client is not available on this platform.
    Unsupported,
    UnknownProject(String),
    ProjectArchived(String),
    /// Store I/O: the commit is indeterminate.
    Store(String),
}

impl ImportError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingApiKey => "missing-api-key",
            Self::Linear(_) => "linear-error",
            Self::Unsupported => "unsupported-platform",
            Self::UnknownProject(_) => "unknown-project",
            Self::ProjectArchived(_) => "project-archived",
            Self::Store(_) => "store-error",
        }
    }
}

/// One issue as read from Linear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearIssue {
    pub identifier: String,
    pub title: String,
    pub url: String,
    pub description: Option<String>,
    pub state_name: String,
    pub state_type: String,
    pub team_key: Option<String>,
    pub parent: Option<LinearParent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearParent {
    pub identifier: String,
    pub url: String,
}

/// What a Linear state means on the board, or why the issue is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapped {
    Status(HumanStatus),
    Skip,
}

/// Map a Linear workflow state to a human status. Every started state, including
/// review, is `doing`; canceled, duplicate, and unknown types are skipped.
pub fn map_state(state_type: &str, state_name: &str) -> Mapped {
    match state_type {
        "triage" | "backlog" | "unstarted" => Mapped::Status(HumanStatus::Todo),
        "started" if state_name.to_lowercase().contains("review") => {
            Mapped::Status(HumanStatus::Doing)
        }
        "started" => Mapped::Status(HumanStatus::Doing),
        "completed" => Mapped::Status(HumanStatus::Done),
        _ => Mapped::Skip,
    }
}

/// The task title for an issue: `<IDENTIFIER> <title>`, C0 controls flattened to spaces.
pub fn task_title(issue: &LinearIssue) -> String {
    let title: String = issue
        .title
        .chars()
        .map(|c| {
            if c <= '\u{1f}' || c == '\u{7f}' {
                ' '
            } else {
                c
            }
        })
        .collect();
    format!("{} {}", issue.identifier, title.trim())
        .trim()
        .to_string()
}

/// The dedupe marker line every imported task's notes begin with.
pub fn marker_line(issue: &LinearIssue) -> String {
    format!("Linear: {} {}", issue.identifier, issue.url)
}

/// Notes for a new task: marker line, blank line, description (cut at [`NOTES_LIMIT`]).
pub fn task_notes(issue: &LinearIssue) -> String {
    let mut notes = marker_line(issue);
    if let Some(parent) = &issue.parent {
        notes.push_str(&format!(
            "\nParent: Linear {} {}",
            parent.identifier, parent.url
        ));
    }
    let description: String = issue
        .description
        .as_deref()
        .unwrap_or("")
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !(*c <= '\u{1f}' || *c == '\u{7f}'))
        .collect();
    let description = description.trim();
    if !description.is_empty() {
        notes.push_str("\n\n");
        if description.chars().count() > NOTES_LIMIT {
            let cut: String = description.chars().take(NOTES_LIMIT).collect();
            notes.push_str(cut.trim_end());
            notes.push_str("\n\n");
            notes.push_str(TRUNCATION_MARK);
        } else {
            notes.push_str(description);
        }
    }
    notes
}

/// The existing copy of an issue: a live (not soft-deleted) task in any scope whose notes
/// contain a line starting with `Linear: <IDENTIFIER> `.
pub fn find_existing<'a>(domain: &'a DomainState, identifier: &str) -> Option<&'a Task> {
    let prefix = format!("Linear: {identifier} ");
    domain.tasks().iter().find(|task| {
        !task.soft_deleted
            && !task.is_notice()
            && task.notes.as_deref().is_some_and(|notes| {
                notes
                    .lines()
                    .any(|line| line.starts_with(&prefix) || line == prefix.trim_end())
            })
    })
}

/// The GraphQL `IssueFilter` for the parsed flags.
pub fn issue_filter(input: &ImportInput) -> Value {
    let mut filter = serde_json::Map::new();
    filter.insert("state".into(), json!({ "type": { "in": input.states } }));
    if let Some(team) = &input.team {
        filter.insert("team".into(), json!({ "key": { "eq": team } }));
    }
    if let Some(project) = &input.linear_project {
        filter.insert(
            "project".into(),
            json!({ "name": { "eqIgnoreCase": project } }),
        );
    }
    match &input.assignee {
        Assignee::Me => {
            filter.insert("assignee".into(), json!({ "isMe": { "eq": true } }));
        }
        Assignee::Email(email) => {
            filter.insert("assignee".into(), json!({ "email": { "eq": email } }));
        }
        Assignee::Any => {}
    }
    Value::Object(filter)
}

/// One page request body.
pub fn page_request(filter: &Value, first: usize, after: Option<&str>) -> Value {
    json!({
        "query": ISSUES_QUERY,
        "variables": { "filter": filter, "first": first, "after": after },
    })
}

/// One parsed page: its issues and the cursor of the next page, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub issues: Vec<LinearIssue>,
    pub next: Option<String>,
}

/// The joined messages of a non-empty top-level GraphQL `errors` array.
fn graphql_errors(response: &Value) -> Option<String> {
    let errors = response.get("errors")?.as_array()?;
    if errors.is_empty() {
        return None;
    }
    let messages: Vec<&str> = errors
        .iter()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .collect();
    Some(if messages.is_empty() {
        "GraphQL error".to_string()
    } else {
        messages.join("; ")
    })
}

/// Parse one GraphQL `issues` response. A top-level `errors` array is a Linear error.
pub fn parse_page(response: &Value) -> Result<Page, ImportError> {
    if let Some(detail) = graphql_errors(response) {
        return Err(ImportError::Linear(detail));
    }
    let issues = response
        .pointer("/data/issues")
        .ok_or_else(|| ImportError::Linear("response has no data.issues".into()))?;
    let nodes = issues
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| ImportError::Linear("response has no issues.nodes".into()))?;
    let mut parsed = Vec::with_capacity(nodes.len());
    for node in nodes {
        let text = |key: &str| node.get(key).and_then(Value::as_str).map(str::to_string);
        let identifier = text("identifier")
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| ImportError::Linear("issue without identifier".into()))?;
        parsed.push(LinearIssue {
            title: text("title").unwrap_or_default(),
            url: text("url").unwrap_or_default(),
            description: text("description"),
            state_name: node
                .pointer("/state/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            state_type: node
                .pointer("/state/type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            team_key: node
                .pointer("/team/key")
                .and_then(Value::as_str)
                .map(str::to_string),
            parent: match node.get("parent") {
                None | Some(Value::Null) => None,
                Some(parent) => Some(LinearParent {
                    identifier: parent
                        .get("identifier")
                        .and_then(Value::as_str)
                        .filter(|id| !id.trim().is_empty())
                        .ok_or_else(|| ImportError::Linear("parent without identifier".into()))?
                        .trim()
                        .to_string(),
                    url: parent
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }),
            },
            identifier: identifier.trim().to_string(),
        });
    }
    let has_next = issues
        .pointer("/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next = issues
        .pointer("/pageInfo/endCursor")
        .and_then(Value::as_str)
        .filter(|_| has_next)
        .map(str::to_string);
    Ok(Page {
        issues: parsed,
        next,
    })
}

/// Fetch up to `input.limit` issues, following cursors. `transport` posts one request
/// body and returns the decoded response.
pub fn fetch_issues(
    input: &ImportInput,
    transport: &mut dyn FnMut(&Value) -> Result<Value, ImportError>,
) -> Result<Vec<LinearIssue>, ImportError> {
    let filter = issue_filter(input);
    let mut issues = Vec::new();
    let mut after: Option<String> = None;
    while issues.len() < input.limit {
        let first = (input.limit - issues.len()).min(PAGE_SIZE);
        let response = transport(&page_request(&filter, first, after.as_deref()))?;
        let page = parse_page(&response)?;
        let received = page.issues.len();
        issues.extend(page.issues);
        match page.next {
            Some(cursor) if received > 0 => after = Some(cursor),
            _ => break,
        }
    }
    issues.truncate(input.limit);
    Ok(issues)
}

/// A row in the report. `number` is `None` only for a dry-run create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportRow {
    pub identifier: String,
    pub number: Option<u64>,
    pub title: String,
    pub status: &'static str,
}

/// A skipped issue and its Linear state type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedRow {
    pub identifier: String,
    pub state: String,
}

/// The stable result of one import run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub dry_run: bool,
    pub fetched: usize,
    pub created: Vec<ReportRow>,
    pub updated: Vec<ReportRow>,
    pub unchanged: Vec<ReportRow>,
    pub skipped: Vec<SkippedRow>,
}

pub fn status_name(status: HumanStatus) -> &'static str {
    match status {
        HumanStatus::Todo => "todo",
        HumanStatus::Doing => "doing",
        HumanStatus::Done => "done",
    }
}

/// Run the import with the process transport (`LINEAR_API_KEY`, curl, fixture hook).
pub fn run(input: ImportInput) -> Result<ImportReport, ImportError> {
    let key = std::env::var(API_KEY_ENV)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .ok_or(ImportError::MissingApiKey)?;
    let mut transport = default_transport(key);
    run_with(input, &mut transport)
}

/// Run the import with an injected transport.
pub fn run_with(
    input: ImportInput,
    transport: &mut dyn FnMut(&Value) -> Result<Value, ImportError>,
) -> Result<ImportReport, ImportError> {
    let issues = fetch_issues(&input, transport)?;
    let store = TaskStore::new(input.state_dir.clone().unwrap_or_else(default_state_dir));
    let snapshot = snapshot_from_env();
    store
        .locked_transition_if_changed(|domain| {
            let scope =
                match resolve_flag_scope(input.project.as_deref(), input.global, domain, &snapshot)
                {
                    Ok(scope) => scope,
                    Err(error) => {
                        let token = input.project.as_deref().unwrap_or_default();
                        return Ok((
                            Err(ImportError::UnknownProject(error.message(token))),
                            false,
                        ));
                    }
                };
            if let TaskScope::Project { path } = &scope {
                if domain.is_project_archived(path) {
                    let short = crate::ui::render::short_project(path).to_string();
                    return Ok((Err(ImportError::ProjectArchived(short)), false));
                }
            }
            let report = match apply(domain, &issues, scope, &input) {
                Ok(report) => report,
                Err(error) => return Ok((Err(error), false)),
            };
            let changed =
                !input.dry_run && (!report.created.is_empty() || !report.updated.is_empty());
            Ok((Ok(report), changed))
        })
        .map_err(ImportError::Store)
        .and_then(|outcome| outcome)
}

/// Plan every issue against the domain and, unless dry-run, apply creates and updates.
fn apply(
    domain: &mut DomainState,
    issues: &[LinearIssue],
    scope: TaskScope,
    input: &ImportInput,
) -> Result<ImportReport, ImportError> {
    let mut report = ImportReport {
        dry_run: input.dry_run,
        fetched: issues.len(),
        created: Vec::new(),
        updated: Vec::new(),
        unchanged: Vec::new(),
        skipped: Vec::new(),
    };
    let mut seen = HashSet::new();
    let mut new_children = Vec::new();
    for issue in issues {
        if !seen.insert(issue.identifier.clone()) {
            continue;
        }
        let Mapped::Status(status) = map_state(&issue.state_type, &issue.state_name) else {
            report.skipped.push(SkippedRow {
                identifier: issue.identifier.clone(),
                state: issue.state_type.clone(),
            });
            continue;
        };
        let title = task_title(issue);
        if let Some(task) = find_existing(domain, &issue.identifier) {
            let (id, number, current_title, current_status) =
                (task.id, task.number, task.title.clone(), task.status);
            let (notes, task_scope, thread) =
                (task.notes.clone(), task.scope.clone(), task.thread.clone());
            let title_differs = current_title != title;
            let status_differs = current_status != status;
            if input.update && (title_differs || status_differs) {
                if !input.dry_run {
                    if title_differs {
                        domain
                            .edit(id, &title, notes, task_scope, thread)
                            .expect("an existing task accepts a non-empty title");
                    }
                    if status_differs {
                        domain
                            .set_status(id, status)
                            .expect("an existing task accepts a status");
                    }
                }
                report.updated.push(ReportRow {
                    identifier: issue.identifier.clone(),
                    number,
                    title,
                    status: status_name(status),
                });
            } else {
                report.unchanged.push(ReportRow {
                    identifier: issue.identifier.clone(),
                    number,
                    title: current_title,
                    status: status_name(current_status),
                });
            }
            continue;
        }
        let number = if input.dry_run {
            None
        } else {
            Some(create(
                domain,
                &title,
                task_notes(issue),
                scope.clone(),
                status,
            ))
        };
        if !input.dry_run {
            if let Some(parent) = &issue.parent {
                let id = find_existing(domain, &issue.identifier)
                    .expect("newly imported task exists")
                    .id;
                new_children.push((id, parent.identifier.clone()));
            }
        }
        report.created.push(ReportRow {
            identifier: issue.identifier.clone(),
            number,
            title,
            status: status_name(status),
        });
    }
    // Only new children participate. Existing copies never reconcile, even with --update.
    for (child, identifier) in new_children {
        if let Some(parent) = find_existing(domain, &identifier).map(|task| task.id) {
            domain
                .set_parent(child, Some(parent))
                .map_err(|error| ImportError::Linear(error.to_string()))?;
        }
    }
    Ok(report)
}

fn create(
    domain: &mut DomainState,
    title: &str,
    notes: String,
    scope: TaskScope,
    status: HumanStatus,
) -> u64 {
    let id: Uuid = domain
        .create(title, Some(notes), scope, ProvenanceOrigin::Capture, None)
        .expect("imported titles always carry the identifier");
    if status != HumanStatus::Todo {
        domain
            .set_status(id, status)
            .expect("a just-created task accepts a status");
    }
    domain.assign_numbers_for_persistence();
    domain
        .get(id)
        .and_then(|task| task.number)
        .expect("new tasks receive a number while the store lock is held")
}

/// The process transport: the fixture hook when `TSK_LINEAR_FIXTURE` is set, otherwise
/// one HTTPS POST per page to [`LINEAR_API_URL`].
pub fn default_transport(key: String) -> impl FnMut(&Value) -> Result<Value, ImportError> {
    let fixture = std::env::var_os(FIXTURE_ENV).map(PathBuf::from);
    let mut page = 0usize;
    move |body: &Value| {
        if let Some(path) = &fixture {
            let current = page;
            page += 1;
            return fixture_page(path, current);
        }
        post_graphql(&key, body)
    }
}

fn fixture_page(path: &std::path::Path, page: usize) -> Result<Value, ImportError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        ImportError::Linear(format!(
            "could not read fixture {}: {error}",
            path.display()
        ))
    })?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| ImportError::Linear(format!("invalid fixture JSON: {error}")))?;
    match value {
        Value::Array(pages) => pages
            .get(page)
            .cloned()
            .ok_or_else(|| ImportError::Linear(format!("fixture has no page {}", page + 1))),
        other => Ok(other),
    }
}

/// POST one GraphQL body with the system curl. The key goes to curl on stdin as a header
/// file (`-H @-`), so it never appears on the process command line.
#[cfg(unix)]
fn post_graphql(key: &str, body: &Value) -> Result<Value, ImportError> {
    if key.contains(['\r', '\n']) {
        return Err(ImportError::Linear(format!(
            "{API_KEY_ENV} must be a single line"
        )));
    }
    let curl = crate::cli::update::curl_path().map_err(ImportError::Linear)?;
    post_graphql_with(&curl, key, body)
}

/// [`post_graphql`] with curl injected so a test can stand one in.
#[cfg(unix)]
pub fn post_graphql_with(
    curl: &std::path::Path,
    key: &str,
    body: &Value,
) -> Result<Value, ImportError> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // The shared hardened policy, minus `--fail`: Linear reports GraphQL errors with a
    // 4xx status and a JSON body worth showing.
    let mut child = Command::new(curl)
        // Must be the first argument: caller curlrc trace/verbose settings can log
        // the Authorization header even though it is supplied only on stdin.
        .arg("-q")
        .args([
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "-sSL",
            "--max-time",
        ])
        .arg(CURL_TIMEOUT_SECS.to_string())
        .args([
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-H",
            "@-",
        ])
        .args(["--write-out", "\n%{http_code}", "--data-binary"])
        .arg(body.to_string())
        .arg(LINEAR_API_URL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ImportError::Linear(format!(
                "could not start {}: {error} (set TSK_UPDATE_CURL to an absolute curl path)",
                curl.display()
            ))
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("Authorization: {key}\n").as_bytes())
            .map_err(|error| ImportError::Linear(format!("could not pass the API key: {error}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| ImportError::Linear(format!("curl failed: {error}")))?;
    if !output.status.success() {
        // Raw curl diagnostics can contain credentials; report only process status.
        return Err(ImportError::Linear(format!(
            "could not reach Linear: curl failed ({})",
            output.status
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, status) = stdout.rsplit_once('\n').unwrap_or(("", stdout.as_ref()));
    let status: u16 = status.trim().parse().unwrap_or(0);
    interpret_response(status, body)
}

#[cfg(not(unix))]
fn post_graphql(_key: &str, _body: &Value) -> Result<Value, ImportError> {
    Err(ImportError::Unsupported)
}

/// Turn an HTTP status and body into a GraphQL response or a Linear error.
pub fn interpret_response(status: u16, body: &str) -> Result<Value, ImportError> {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    if (200..300).contains(&status) {
        return parsed.ok_or_else(|| {
            ImportError::Linear("Linear returned a response that is not JSON".into())
        });
    }
    // A GraphQL error body explains a 4xx better than the status code.
    if let Some(detail) = parsed.as_ref().and_then(graphql_errors) {
        return Err(ImportError::Linear(format!("HTTP {status}: {detail}")));
    }
    let hint = match status {
        401 | 403 => " (check LINEAR_API_KEY)",
        _ => "",
    };
    Err(ImportError::Linear(format!("HTTP {status}{hint}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn issue(identifier: &str, state_type: &str, state_name: &str) -> LinearIssue {
        LinearIssue {
            parent: None,
            identifier: identifier.into(),
            title: format!("title of {identifier}"),
            url: format!(
                "https://linear.app/acme/issue/{}",
                identifier.to_lowercase()
            ),
            description: Some("Body text".into()),
            state_name: state_name.into(),
            state_type: state_type.into(),
            team_key: Some("ENG".into()),
        }
    }

    const FIXTURE: &str = r#"{
      "data": {
        "issues": {
          "nodes": [
            {
              "identifier": "ENG-2654",
              "title": "tsk fork: import Linear issues",
              "url": "https://linear.app/acme/issue/ENG-2654/tsk-fork-import",
              "description": "Pull my issues onto the board.\r\n\nSecond paragraph.",
              "state": { "name": "In Progress", "type": "started" },
              "team": { "key": "ENG" }
            },
            {
              "identifier": "ENG-2655",
              "title": "Review the importer",
              "url": "https://linear.app/acme/issue/ENG-2655/review",
              "description": null,
              "state": { "name": "In Review", "type": "started" },
              "team": { "key": "ENG" }
            },
            {
              "identifier": "ENG-2656",
              "title": "Dropped idea",
              "url": "https://linear.app/acme/issue/ENG-2656/dropped",
              "description": "",
              "state": { "name": "Canceled", "type": "canceled" },
              "team": { "key": "ENG" }
            }
          ],
          "pageInfo": { "hasNextPage": true, "endCursor": "cursor-1" }
        }
      }
    }"#;

    #[test]
    fn parses_a_realistic_page() {
        let page = parse_page(&serde_json::from_str(FIXTURE).unwrap()).unwrap();
        assert_eq!(page.next.as_deref(), Some("cursor-1"));
        assert_eq!(page.issues.len(), 3);
        let first = &page.issues[0];
        assert_eq!(first.identifier, "ENG-2654");
        assert_eq!(first.state_type, "started");
        assert_eq!(first.state_name, "In Progress");
        assert_eq!(first.team_key.as_deref(), Some("ENG"));
        assert_eq!(page.issues[1].description, None);
    }

    #[test]
    fn last_page_has_no_next_cursor() {
        let page = parse_page(&json!({"data": {"issues": {"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": "x"}}}})).unwrap();
        assert_eq!(page.next, None);
    }

    #[test]
    fn graphql_errors_are_linear_errors() {
        let error =
            parse_page(&json!({"errors": [{"message": "Authentication required"}]})).unwrap_err();
        assert_eq!(error, ImportError::Linear("Authentication required".into()));
        assert_eq!(error.code(), "linear-error");
        let http = interpret_response(400, r#"{"errors":[{"message":"bad filter"}]}"#).unwrap_err();
        assert_eq!(http, ImportError::Linear("HTTP 400: bad filter".into()));
        let auth = interpret_response(401, "nope").unwrap_err();
        assert_eq!(
            auth,
            ImportError::Linear("HTTP 401 (check LINEAR_API_KEY)".into())
        );
    }

    #[test]
    fn status_mapping_covers_review_and_canceled() {
        assert_eq!(
            map_state("backlog", "Backlog"),
            Mapped::Status(HumanStatus::Todo)
        );
        assert_eq!(
            map_state("unstarted", "Todo"),
            Mapped::Status(HumanStatus::Todo)
        );
        assert_eq!(
            map_state("started", "In Progress"),
            Mapped::Status(HumanStatus::Doing)
        );
        assert_eq!(
            map_state("started", "In Review"),
            Mapped::Status(HumanStatus::Doing)
        );
        assert_eq!(
            map_state("started", "CODE REVIEW"),
            Mapped::Status(HumanStatus::Doing)
        );
        assert_eq!(
            map_state("completed", "Done"),
            Mapped::Status(HumanStatus::Done)
        );
        assert_eq!(map_state("canceled", "Canceled"), Mapped::Skip);
        assert_eq!(map_state("canceled", "Duplicate"), Mapped::Skip);
        assert_eq!(map_state("duplicate", "Duplicate"), Mapped::Skip);
    }

    #[test]
    fn notes_lead_with_the_marker_and_truncate_long_descriptions() {
        let mut long = issue("ENG-1", "started", "In Progress");
        long.description = Some("x".repeat(NOTES_LIMIT + 10));
        let notes = task_notes(&long);
        let mut lines = notes.lines();
        assert_eq!(
            lines.next(),
            Some("Linear: ENG-1 https://linear.app/acme/issue/eng-1")
        );
        assert_eq!(lines.next(), Some(""));
        assert!(notes.ends_with(TRUNCATION_MARK));
        assert_eq!(notes.matches('x').count(), NOTES_LIMIT);

        let mut empty = issue("ENG-2", "started", "In Progress");
        empty.description = None;
        assert_eq!(task_notes(&empty), marker_line(&empty));
    }

    #[test]
    fn filter_follows_flags() {
        let input = parse(&args(&[
            "tsk",
            "import",
            "linear",
            "--team",
            "ENG",
            "--linear-project",
            "tsk",
            "--assignee",
            "a@b.c",
            "--state",
            "started,completed",
            "--limit",
            "7",
        ]))
        .unwrap();
        assert_eq!(input.limit, 7);
        assert_eq!(
            issue_filter(&input),
            json!({
                "state": {"type": {"in": ["started", "completed"]}},
                "team": {"key": {"eq": "ENG"}},
                "project": {"name": {"eqIgnoreCase": "tsk"}},
                "assignee": {"email": {"eq": "a@b.c"}},
            })
        );
        let defaults = parse(&args(&["tsk", "import", "linear"])).unwrap();
        assert_eq!(
            issue_filter(&defaults),
            json!({
                "state": {"type": {"in": ["backlog", "unstarted", "started"]}},
                "assignee": {"isMe": {"eq": true}},
            })
        );
        let any = parse(&args(&["tsk", "import", "linear", "--assignee=any"])).unwrap();
        assert!(issue_filter(&any).get("assignee").is_none());
    }

    #[test]
    fn parse_refuses_bad_input() {
        for bad in [
            &["tsk", "import"][..],
            &["tsk", "import", "jira"],
            &["tsk", "import", "linear", "--state", "done"],
            &["tsk", "import", "linear", "--limit", "0"],
            &["tsk", "import", "linear", "--assignee", "bob"],
            &["tsk", "import", "linear", "--desk", "-p", "x"],
            &["tsk", "import", "linear", "--bogus"],
            &["tsk", "import", "linear", "extra"],
        ] {
            assert!(
                parse(&args(bad)).is_err(),
                "{bad:?} should be a usage error"
            );
        }
        assert!(parse(&args(&["tsk", "import", "--help"])).unwrap().help);
    }

    #[test]
    fn pagination_follows_cursors_up_to_the_limit() {
        let input = ImportInput {
            limit: 3,
            ..ImportInput::default()
        };
        let mut requests = Vec::new();
        let mut transport = |body: &Value| -> Result<Value, ImportError> {
            requests.push(body.clone());
            let n = requests.len();
            Ok(json!({"data": {"issues": {
                "nodes": [
                    {"identifier": format!("A-{n}1"), "title": "t", "url": "u", "state": {"name": "Todo", "type": "unstarted"}},
                    {"identifier": format!("A-{n}2"), "title": "t", "url": "u", "state": {"name": "Todo", "type": "unstarted"}}
                ],
                "pageInfo": {"hasNextPage": true, "endCursor": format!("c{n}")}
            }}}))
        };
        let issues = fetch_issues(&input, &mut transport).unwrap();
        assert_eq!(issues.len(), 3);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["variables"]["first"], 3);
        assert_eq!(requests[0]["variables"]["after"], Value::Null);
        assert_eq!(requests[1]["variables"]["first"], 1);
        assert_eq!(requests[1]["variables"]["after"], "c1");
    }

    #[cfg(unix)]
    #[test]
    fn curl_gets_the_key_on_stdin_never_in_argv() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("curl");
        let curl = dir.join("curl");
        std::fs::write(
            &curl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{argv}'\ncat > '{stdin}'\nprintf '{{\"data\":{{\"issues\":{{\"nodes\":[],\"pageInfo\":{{\"hasNextPage\":false}}}}}}}}\\n200'\n",
                argv = dir.join("argv").display(),
                stdin = dir.join("stdin").display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let body = page_request(&json!({}), 5, None);
        let value = post_graphql_with(&curl, "lin_api_secret", &body).unwrap();
        assert!(parse_page(&value).unwrap().issues.is_empty());
        let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
        assert!(!argv.contains("lin_api_secret"), "{argv}");
        assert!(argv.lines().any(|line| line == "@-"));
        assert!(argv.lines().any(|line| line == "=https"));
        assert_eq!(argv.lines().last(), Some(LINEAR_API_URL));
        assert_eq!(
            std::fs::read_to_string(dir.join("stdin")).unwrap(),
            "Authorization: lin_api_secret\n"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tsk-import-unit-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_transport(
        issues: Vec<LinearIssue>,
    ) -> impl FnMut(&Value) -> Result<Value, ImportError> {
        move |_body: &Value| {
            let nodes: Vec<Value> = issues
                .iter()
                .map(|issue| {
                    json!({
                        "identifier": issue.identifier,
                        "title": issue.title,
                        "url": issue.url,
                        "description": issue.description,
                        "state": {"name": issue.state_name, "type": issue.state_type},
                        "team": {"key": issue.team_key},
                    })
                })
                .collect();
            Ok(
                json!({"data": {"issues": {"nodes": nodes, "pageInfo": {"hasNextPage": false, "endCursor": null}}}}),
            )
        }
    }

    fn desk_input(dir: &std::path::Path) -> ImportInput {
        ImportInput {
            global: true,
            state_dir: Some(dir.to_path_buf()),
            ..ImportInput::default()
        }
    }

    fn sample() -> Vec<LinearIssue> {
        vec![
            issue("ENG-1", "unstarted", "Todo"),
            issue("ENG-2", "started", "In Review"),
            issue("ENG-3", "canceled", "Canceled"),
        ]
    }

    #[test]
    fn creates_maps_and_dedupes_on_rerun() {
        let dir = temp_dir("dedupe");
        let report = run_with(desk_input(&dir), &mut fixture_transport(sample())).unwrap();
        assert_eq!(report.created.len(), 2);
        assert_eq!(
            report.skipped,
            vec![SkippedRow {
                identifier: "ENG-3".into(),
                state: "canceled".into()
            }]
        );
        let state = TaskStore::new(&dir).load().unwrap();
        let review = find_existing(&state, "ENG-2").unwrap();
        assert_eq!(review.status, HumanStatus::Doing);
        assert_eq!(review.title, "ENG-2 title of ENG-2");
        assert!(find_existing(&state, "ENG-3").is_none());
        // A prefix identifier never matches a longer one.
        assert!(find_existing(&state, "ENG-").is_none());

        let again = run_with(desk_input(&dir), &mut fixture_transport(sample())).unwrap();
        assert!(again.created.is_empty());
        assert_eq!(again.unchanged.len(), 2);
        assert_eq!(TaskStore::new(&dir).load().unwrap().tasks().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn existing_copy_is_untouched_by_default_and_refreshed_with_update() {
        let dir = temp_dir("update");
        run_with(desk_input(&dir), &mut fixture_transport(sample())).unwrap();
        let mut changed = sample();
        changed[0].title = "renamed upstream".into();
        changed[0].state_type = "completed".into();
        changed[0].state_name = "Done".into();

        let default = run_with(desk_input(&dir), &mut fixture_transport(changed.clone())).unwrap();
        assert!(default.updated.is_empty());
        let state = TaskStore::new(&dir).load().unwrap();
        assert_eq!(
            find_existing(&state, "ENG-1").unwrap().status,
            HumanStatus::Todo
        );

        let update = ImportInput {
            update: true,
            ..desk_input(&dir)
        };
        let report = run_with(update, &mut fixture_transport(changed)).unwrap();
        assert_eq!(report.updated.len(), 1);
        assert_eq!(report.updated[0].status, "done");
        assert_eq!(report.unchanged.len(), 1);
        let state = TaskStore::new(&dir).load().unwrap();
        let task = find_existing(&state, "ENG-1").unwrap();
        assert_eq!(task.status, HumanStatus::Done);
        assert_eq!(task.title, "ENG-1 renamed upstream");
        assert!(task.notes.as_deref().unwrap().starts_with("Linear: ENG-1 "));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dedupe_ignores_soft_deleted_copies() {
        let dir = temp_dir("deleted");
        run_with(desk_input(&dir), &mut fixture_transport(sample())).unwrap();
        let store = TaskStore::new(&dir);
        let mut state = store.load().unwrap();
        let id = find_existing(&state, "ENG-1").unwrap().id;
        state.soft_delete(id).unwrap();
        store.save(&state).unwrap();
        let report = run_with(desk_input(&dir), &mut fixture_transport(sample())).unwrap();
        assert_eq!(report.created.len(), 1);
        assert_eq!(report.created[0].identifier, "ENG-1");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = temp_dir("dry");
        let input = ImportInput {
            dry_run: true,
            ..desk_input(&dir)
        };
        let report = run_with(input, &mut fixture_transport(sample())).unwrap();
        assert!(report.dry_run);
        assert_eq!(report.created.len(), 2);
        assert!(report.created.iter().all(|row| row.number.is_none()));
        assert!(!dir.join("tsk.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fetch_failure_persists_nothing() {
        let dir = temp_dir("fail");
        let mut transport =
            |_: &Value| -> Result<Value, ImportError> { Err(ImportError::Linear("down".into())) };
        let error = run_with(desk_input(&dir), &mut transport).unwrap_err();
        assert_eq!(error.code(), "linear-error");
        assert!(!dir.join("tsk.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
