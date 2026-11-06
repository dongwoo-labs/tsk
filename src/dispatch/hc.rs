//! Explicit local hc identities. Never consult hc's implicit default session.
use crate::threads::{AgentStatus, ThreadRow};
use serde_json::Value;
use std::{env, path::Path, process::Command, time::Duration};

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
fn selection(backend: Option<&str>, session: Option<&str>) -> Result<Option<String>, String> {
    match backend {
        None | Some("herdr") => Ok(None),
        Some("hc") => session
            .filter(|s| token(s))
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| "hc requires an explicit valid HC_SESSION".into()),
        Some(_) => Err("TSK_THREAD_BACKEND must be herdr or hc (no fallback)".into()),
    }
}
pub(crate) fn selected() -> Result<Option<String>, String> {
    let backend = match env::var("TSK_THREAD_BACKEND") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(_) => return Err("TSK_THREAD_BACKEND must be herdr or hc (no fallback)".into()),
    };
    selection(backend.as_deref(), env::var("HC_SESSION").ok().as_deref())
}
pub(crate) fn enabled() -> bool {
    env::var("TSK_THREAD_BACKEND").as_deref() == Ok("hc")
}
pub(crate) fn available(in_herdr: bool) -> Result<bool, String> {
    selected().map(|session| session.is_some() || in_herdr)
}
pub(crate) fn record_session(workspace: &str) -> Result<String, String> {
    selected()?;
    let session = workspace
        .strip_prefix("hc:")
        .filter(|s| token(s))
        .ok_or("invalid hc workspace identity")?;
    // Record-directed operations remain hc even when the selection changed back to Herdr.
    if env::var("HC_SESSION").as_deref() != Ok(session) {
        return Err("HC_SESSION does not match the recorded hc session".into());
    }
    Ok(session.into())
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("hc terminal has no {key}"))
}
fn label<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    field(value.get("labels").ok_or("hc terminal has no labels")?, key)
}
fn local(address: &str) -> bool {
    // hc terminal addresses use a decimal leading digit and consonant/digit tail.
    address.strip_prefix('t').is_some_and(|suffix| {
        let mut bytes = suffix.bytes();
        bytes
            .next()
            .is_some_and(|first| (b'1'..=b'9').contains(&first))
            && bytes.all(|byte| b"0123456789bcdfghjkmnpqrstvwxyz".contains(&byte))
    })
}
fn task_label(task: &str) -> bool {
    task.strip_prefix('t').is_some_and(|number| {
        !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && !number.starts_with('0')
    })
}
fn same_directory(left: &str, right: &str) -> bool {
    match (
        super::canonical_cleanup_path(Path::new(left)),
        super::canonical_cleanup_path(Path::new(right)),
    ) {
        (Ok(left), Ok(right)) => super::same_path(&left, &right),
        _ => false,
    }
}
fn identity(value: &Value) -> Result<(), String> {
    if !local(field(value, "address")?) {
        return Err("hc requires a local terminal address".into());
    }
    let task = label(value, "task")?;
    if !task_label(task) {
        return Err("hc task label must be a lowercase t<number>".into());
    }
    for key in ["owner_lead", "thread", "machine"] {
        if !token(label(value, key)?) {
            return Err(format!("invalid hc {key} label"));
        }
    }
    if !label(value, "owner_lead")?.starts_with("lead-")
        || field(value, "name")? != label(value, "thread")?
    {
        return Err("hc lead/thread labels do not identify the named session".into());
    }
    let sha = label(value, "sha")?;
    if sha.len() < 7 || sha.len() > 40 || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("invalid hc sha label".into());
    }
    let worktree = label(value, "worktree")?;
    if !Path::new(worktree).is_absolute() || !same_directory(worktree, field(value, "cwd")?) {
        return Err("hc worktree label and cwd do not match".into());
    }
    Ok(())
}
fn live(value: &Value) -> Result<bool, String> {
    match field(value, "state")? {
        "finished" => Ok(false),
        "live" if value.get("reason").and_then(Value::as_str) == Some("holder_lost") => Ok(false),
        "live" => Ok(true),
        _ => Err("unknown hc terminal lifecycle state".into()),
    }
}
fn unique(values: &[Value], task: u64, worktree: &str) -> Result<Option<String>, String> {
    let mut found = None;
    for value in values {
        if !live(value)? {
            continue;
        }
        identity(value)?;
        if label(value, "task")? != format!("t{task}")
            || !same_directory(label(value, "worktree")?, worktree)
        {
            return Err("hc terminal task/worktree mismatch".into());
        }
        if found.is_some() {
            return Err("multiple live hc terminals match this task/worktree".into());
        }
        found = Some(field(value, "address")?.to_owned());
    }
    Ok(found)
}

pub(crate) fn run(session: &str, args: &[String]) -> Result<Value, String> {
    if !token(session) || env::var("HC_SESSION").as_deref() != Ok(session) {
        return Err("hc requires the explicit matching HC_SESSION".into());
    }
    let mut command = Command::new("hc");
    command.env("HC_SESSION", session).args(args);
    let output = crate::git_base::bounded_process_output(command, "hc", Duration::from_secs(10))?;
    // Do not echo argv, stdout, stderr, or error messages: a command can contain a reply.
    if !output.status.success() {
        return Err(format!(
            "hc {} failed (exit {})",
            args.first().map(String::as_str).unwrap_or("command"),
            output.status.code().unwrap_or(-1)
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "unreadable hc response".into())
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}
fn terminals(session: &str, filters: &[String]) -> Result<Vec<Value>, String> {
    let mut args = strings(&["ls", "--json"]);
    for filter in filters {
        args.extend(["--label".into(), filter.clone()]);
    }
    let list = run(session, &args)?;
    let entries = list
        .get("terminals")
        .and_then(Value::as_array)
        .ok_or("hc list has no terminals")?;
    let mut values = Vec::new();
    for entry in entries {
        let address = field(entry, "address")?;
        if !local(address) {
            return Err("remote hc terminals are unsupported".into());
        }
        // ls summaries omit labels; always fetch the authoritative terminal before use.
        let value = run(session, &strings(&["get", address, "--json"]))?;
        if field(&value, "address")? != address {
            return Err("hc get returned a different terminal".into());
        }
        values.push(value);
    }
    Ok(values)
}
pub(crate) fn resolve(
    number: u64,
    record: &crate::domain::Dispatch,
) -> Result<Option<String>, String> {
    let session = record_session(&record.herdr_workspace_id)?;
    unique(
        &terminals(
            &session,
            &[
                format!("task=t{number}"),
                format!("worktree={}", record.worktree),
            ],
        )?,
        number,
        &record.worktree,
    )
    .map(|id| id.map(|id| format!("hc:{session}:{id}")))
}
pub(crate) fn agent(
    number: u64,
    record: &crate::domain::Dispatch,
    expected: Option<&str>,
) -> Result<super::PaneAgent, String> {
    let Some(pane) = resolve(number, record)? else {
        return Ok(super::PaneAgent::Absent);
    };
    if expected.is_some_and(|expected| expected != pane) {
        return Err("hc terminal reference changed".into());
    }
    let session = record_session(&record.herdr_workspace_id)?;
    let id = pane
        .rsplit(':')
        .next()
        .ok_or("invalid hc terminal reference")?;
    let value = run(&session, &strings(&["get", id, "--json"]))?;
    identity(&value)?;
    if !live(&value)?
        || label(&value, "task")? != format!("t{number}")
        || !same_directory(label(&value, "worktree")?, &record.worktree)
    {
        return Err("hc terminal identity changed".into());
    }
    Ok(super::PaneAgent::Present {
        name: Some(field(&value, "name")?.into()),
    })
}
pub(crate) fn send(
    number: u64,
    record: &crate::domain::Dispatch,
    pane: &str,
    text: &str,
) -> Result<(), super::PromptError> {
    let session = record_session(&record.herdr_workspace_id).map_err(super::PromptError::Failed)?;
    if resolve(number, record)
        .map_err(super::PromptError::Failed)?
        .as_deref()
        != Some(pane)
    {
        return Err(super::PromptError::Failed(
            "hc terminal reference changed".into(),
        ));
    }
    let id = pane
        .rsplit(':')
        .next()
        .ok_or_else(|| super::PromptError::Failed("invalid hc terminal reference".into()))?;
    run(&session, &strings(&["send", id, text, "--json"]))
        .map(|_| ())
        .map_err(|error| {
            if error.ends_with("(exit 9)") {
                super::PromptError::AgentBlocked
            } else {
                super::PromptError::Failed(error)
            }
        })
}
pub(crate) fn stop(
    number: u64,
    record: &crate::domain::Dispatch,
    expected: Option<&str>,
) -> Result<bool, String> {
    let session = record_session(&record.herdr_workspace_id)?;
    let Some(pane) = resolve(number, record)? else {
        return Ok(false);
    };
    if expected.is_some_and(|expected| expected != pane) {
        return Err("hc terminal identity changed".into());
    }
    let id = pane
        .rsplit(':')
        .next()
        .ok_or("invalid hc terminal reference")?;
    // Resolve again at the mutation boundary, including duplicate detection.
    if resolve(number, record)?.as_deref() != Some(pane.as_str()) {
        return Err("hc terminal identity changed".into());
    }
    run(&session, &strings(&["kill", id, "--json"]))?;
    if resolve(number, record)?.is_some() {
        return Err("hc terminal is still live after kill".into());
    }
    Ok(true)
}
pub(crate) fn list() -> Result<Vec<ThreadRow>, String> {
    let session = selected()?.ok_or("hc is not selected")?;
    terminals(&session, &[])?
        .into_iter()
        .filter_map(|value| match live(&value) {
            Ok(false) => None,
            Err(error) => Some(Err(error)),
            Ok(true) => Some(row(&session, &value)),
        })
        .collect()
}
fn row(session: &str, value: &Value) -> Result<ThreadRow, String> {
    identity(value)?;
    let status = match value.pointer("/status/state").and_then(Value::as_str) {
        Some("idle") => AgentStatus::Idle,
        Some("working") => AgentStatus::Working,
        Some("done") => AgentStatus::Done,
        Some("blocked") => AgentStatus::Blocked,
        _ => AgentStatus::Unknown,
    };
    Ok(ThreadRow {
        name: field(value, "name")?.into(),
        agent: None,
        status,
        inbox: None,
        workspace_id: format!("hc:{session}"),
        tab_id: "-".into(),
        pane_id: format!("hc:{session}:{}", field(value, "address")?),
        focused: false,
        state_change_seq: 0,
        cwd: Some(label(value, "worktree")?.into()),
        title: None,
    })
}
pub(crate) fn attach_hint(row: &ThreadRow) -> Result<String, String> {
    attach_reference(&row.workspace_id, &row.pane_id)
}
pub(crate) fn attach_reference(workspace: &str, pane: &str) -> Result<String, String> {
    let session = workspace
        .strip_prefix("hc:")
        .filter(|s| token(s))
        .ok_or("invalid hc workspace identity")?;
    let prefix = format!("hc:{session}:");
    let id = pane
        .strip_prefix(&prefix)
        .filter(|id| local(id))
        .ok_or("invalid hc terminal reference")?;
    // Focus is deliberately not implemented. A stale reference cannot trigger a command.
    Ok(format!(
        "hc focus is unsupported; run HC_SESSION={session} hc attach {id}"
    ))
}
pub(crate) fn prompt_name(name: &str, text: &str) -> Result<(), String> {
    let session = selected()?.ok_or("hc is not selected")?;
    let values = terminals(&session, &[])?;
    let mut matches = Vec::new();
    for value in &values {
        if !live(value)? {
            continue;
        }
        identity(value)?;
        let pane = format!("hc:{session}:{}", field(value, "address")?);
        if field(value, "name")? == name || pane == name {
            matches.push((value, pane));
        }
    }
    let [(value, _)] = matches.as_slice() else {
        return Err("hc prompt needs one live named terminal".into());
    };
    let number = label(value, "task")?
        .trim_start_matches('t')
        .parse()
        .map_err(|_| "invalid hc task label")?;
    prompt_task(number, &row(&session, value)?, text)
}

pub(crate) fn prompt_task(number: u64, target: &ThreadRow, text: &str) -> Result<(), String> {
    let record = crate::domain::Dispatch {
        argv: vec![],
        worktree: target
            .cwd
            .as_deref()
            .ok_or("hc task hint has no worktree")?
            .into(),
        branch: String::new(),
        base: None,
        base_ref: None,
        base_commit: None,
        base_remote: None,
        herdr_workspace_id: target.workspace_id.clone(),
        at: std::time::SystemTime::now(),
        cleaned: false,
    };
    if agent(number, &record, Some(&target.pane_id))?
        != (super::PaneAgent::Present {
            name: Some(target.name.clone()),
        })
    {
        return Err("hc task hint target changed".into());
    }
    send(number, &record, &target.pane_id, text).map_err(|_| "hc prompt failed".into())
}

pub(crate) fn metadata(notes: Option<&str>, session: &str) -> Result<(String, String), String> {
    let notes = notes.ok_or("hc dispatch requires Owner lead: and Thread: notes")?;
    let one = |prefix: &str| -> Result<Vec<&str>, String> {
        let lines: Vec<_> = notes
            .lines()
            .filter_map(|line| line.trim().strip_prefix(prefix))
            .collect();
        let [line] = lines.as_slice() else {
            return Err(format!("hc dispatch requires exactly one {prefix} line"));
        };
        Ok(line.split_whitespace().collect())
    };
    let owner = one("Owner lead:")?;
    let leads: Vec<_> = owner
        .iter()
        .filter(|name| token(name) && name.starts_with("lead-"))
        .collect();
    let [lead] = leads.as_slice() else {
        return Err("hc Owner lead: requires exactly one lead handle".into());
    };
    let thread = one("Thread:")?;
    let name = thread
        .first()
        .filter(|name| token(name))
        .ok_or("hc Thread: requires a thread name")?;
    match thread.as_slice() {
        [_] => {}
        [_, workspace, tab, pane]
            if *workspace == format!("hc:{session}")
                && *tab == "-"
                && pane
                    .strip_prefix(&format!("hc:{session}:"))
                    .is_some_and(local) => {}
        _ => {
            return Err(
                "hc Thread: needs a name or an exact four-token local hc link in this HC_SESSION"
                    .into(),
            )
        }
    }
    Ok((lead.to_string(), name.to_string()))
}

pub(crate) fn worktree_path(
    host: &impl super::DispatchHost,
    session: &str,
    number: u64,
    branch: &str,
) -> Result<std::path::PathBuf, String> {
    let launcher = host.launcher_path(&format!("hc:{session}:t{number}"))?;
    let state = launcher
        .parent()
        .and_then(Path::parent)
        .ok_or("hc launcher has no state directory")?;
    Ok(state
        .join("worktrees")
        .join(session)
        .join(super::herdr_worktree_directory(branch)))
}

pub(crate) fn launch(
    host: &mut super::SystemDispatchHost,
    eligible: &super::EligibleDispatch,
    base_override: Option<&str>,
) -> Result<super::Launched, super::DispatchError> {
    use super::{Created, DispatchError as Error, DispatchHost, LaunchUndo, Launched};
    let session = selected()
        .map_err(Error::Herdr)?
        .ok_or_else(|| Error::Herdr("hc is not selected".into()))?;
    if cfg!(windows) {
        return Err(Error::UnsupportedPlatform(
            "hc dispatch requires a POSIX launcher".into(),
        ));
    }
    let (lead, thread) =
        metadata(eligible.task.notes.as_deref(), &session).map_err(Error::AgentConfig)?;
    if let Some(record) = eligible.task.dispatch.as_ref() {
        if record.herdr_workspace_id != format!("hc:{session}") {
            return Err(Error::Herdr(
                "relaunch backend/session differs from the recorded dispatch".into(),
            ));
        }
    }
    // Check all live terminals for this task, not just one predicted checkout.
    let existing =
        terminals(&session, &[format!("task=t{}", eligible.number)]).map_err(Error::Herdr)?;
    for value in &existing {
        if live(value).map_err(Error::Herdr)? {
            return Err(Error::Herdr(
                "a live hc terminal already carries this task label".into(),
            ));
        }
    }
    if let Some(refusal) = crate::agents::unix_env_refusal(&eligible.profile.env) {
        return Err(Error::AgentConfig(refusal));
    }
    let key = format!("hc:{session}:t{}", eligible.number);
    let launcher = host.launcher_path(&key).map_err(Error::Herdr)?;
    super::sh_launch_line(&launcher)?;
    let project = &eligible.project;
    let explicit = base_override.or(eligible.task.base.as_deref());
    let old = eligible.task.dispatch.as_ref();
    let (mut record, made, warning) = if let Some(old) = old {
        record_session(&old.herdr_workspace_id).map_err(Error::Herdr)?;
        (old.clone(), Created::Workspace, None)
    } else {
        let choice = host
            .resolve_base_choice(project, explicit)
            .map_err(|reason| {
                if explicit.is_some() {
                    Error::UnknownBase(reason)
                } else {
                    Error::NoDefaultBase(reason)
                }
            })?;
        let names = super::DispatchNames::new(eligible.number, &eligible.task.title);
        let branch = super::free_branch(host, project, &names, eligible.number)?;
        let worktree =
            worktree_path(host, &session, eligible.number, &branch).map_err(Error::Herdr)?;
        let record = crate::domain::Dispatch {
            argv: vec![],
            worktree: worktree.to_string_lossy().into_owned(),
            branch,
            base: Some(choice.reference),
            base_ref: Some(choice.full_ref),
            base_commit: choice.commit.clone(),
            base_remote: choice.remote,
            herdr_workspace_id: format!("hc:{session}"),
            at: std::time::SystemTime::now(),
            cleaned: false,
        };
        (
            record,
            Created::Worktree {
                branch_from: choice.commit,
            },
            choice.warning,
        )
    };
    let worktree = Path::new(&record.worktree);
    let needs_create = !worktree.exists();
    if !needs_create {
        let registered = super::git_worktree_paths(project).map_err(Error::Herdr)?;
        let canonical = super::canonical_cleanup_path(worktree).map_err(Error::Herdr)?;
        if !registered.iter().any(|p| super::same_path(p, &canonical)) {
            return Err(Error::Herdr(
                "hc worktree is not registered in the task repository".into(),
            ));
        }
    }
    let base = record
        .base
        .as_deref()
        .map(|base| crate::git_base::short_name_for_remote(base, record.base_remote.as_deref()))
        .unwrap_or_default();
    let steps = super::rendered_steps(&eligible.task);
    let mut rendered = eligible.profile.render(
        &crate::agents::RenderContext {
            number: eligible.number,
            title: &eligible.task.title,
            notes: eligible.task.notes.as_deref().unwrap_or_default(),
            steps: &steps,
            worktree: &record.worktree,
            branch: &record.branch,
            base: &base,
        },
        eligible.settings.as_ref(),
    );
    let state_dir =
        std::path::absolute(&host.state_dir).map_err(|e| Error::Herdr(e.to_string()))?;
    rendered.env.insert(
        "TSK_STATE_DIR".into(),
        state_dir.to_string_lossy().into_owned(),
    );
    rendered
        .env
        .insert("TSK_THREAD_BACKEND".into(), "hc".into());
    rendered.env.insert("HC_SESSION".into(), session.clone());
    let machine = crate::machine::current().map_err(Error::AgentConfig)?;
    if needs_create {
        std::fs::create_dir_all(
            worktree
                .parent()
                .ok_or_else(|| Error::Herdr("hc worktree has no parent".into()))?,
        )
        .map_err(|e| Error::Herdr(e.to_string()))?;
        let branch_exists = host
            .branch_tip(project, &record.branch)
            .map_err(Error::Herdr)?
            .is_some();
        let mut args = vec!["worktree", "add"];
        if !branch_exists {
            args.extend(["-b", record.branch.as_str()]);
        }
        args.push(&record.worktree);
        args.push(if branch_exists {
            &record.branch
        } else {
            record
                .base_commit
                .as_deref()
                .or(record.base.as_deref())
                .ok_or_else(|| Error::Herdr("hc dispatch has no base".into()))?
        });
        let output =
            crate::git_base::git_process_output_timeout(project, &args, Duration::from_secs(30))
                .map_err(Error::Herdr)?;
        if !output.status.success() {
            return Err(Error::Herdr("git worktree add failed".into()));
        }
    }
    let made = if needs_create && old.is_some() {
        Created::Worktree { branch_from: None }
    } else {
        made
    };
    let mut undo = LaunchUndo {
        project: project.clone(),
        created: made,
        hc_terminal: Some((eligible.number, String::new())),
    };
    record.argv = rendered.argv.clone();
    record.at = std::time::SystemTime::now();
    record.cleaned = false;
    let outcome = (|| {
        let sha = crate::git_base::git_output(worktree, &["rev-parse", "--short=7", "HEAD"])
            .map_err(Error::Herdr)?;
        let script=rendered.shell_script(&launcher.to_string_lossy()).replace("rm -f -- \"$0\"\n",
            "rm -f -- \"$0\"\nunset CLAUDECODE CLAUDE_CODE_ENTRYPOINT CODEX_THREAD_ID HERDR_ENV HERDR_WORKSPACE_ID HERDR_TAB_ID HERDR_PANE_ID\n");
        host.write_launcher(&key, &script).map_err(Error::Herdr)?;
        // Recheck immediately before run, after the potentially slow git work.
        if terminals(&session, &[format!("task=t{}", eligible.number)])
            .map_err(Error::Herdr)?
            .iter()
            .map(live)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Error::Herdr)?
            .into_iter()
            .any(|v| v)
        {
            return Err(Error::Herdr(
                "a live hc terminal already carries this task label".into(),
            ));
        }
        let mut args = strings(&[
            "run",
            "--bg",
            "--name",
            &thread,
            "--cwd",
            &record.worktree,
            "--outlive-parent",
            "--json",
        ]);
        for (key, value) in [
            ("task", format!("t{}", eligible.number)),
            ("owner_lead", lead),
            ("thread", thread.clone()),
            ("worktree", record.worktree.clone()),
            ("machine", machine),
            ("sha", sha.trim().into()),
        ] {
            args.extend(["--label".into(), format!("{key}={value}")]);
        }
        args.extend(strings(&["--", "/bin/sh", &launcher.to_string_lossy()]));
        // A failed or timed-out run may still have spawned. Preserve its checkout unless
        // the returned terminal identity can be verified during rollback.
        undo.hc_terminal = None;
        let launched = run(&session, &args).map_err(Error::Herdr)?;
        let address = field(&launched, "address").map_err(Error::Herdr)?;
        if !local(address) {
            return Err(Error::Herdr("hc run returned a non-local address".into()));
        }
        let pane = format!("hc:{session}:{address}");
        // Keep the exact returned address for safe rollback; no persistent pane field.
        undo.hc_terminal = Some((eligible.number, pane.clone()));
        let snapshot =
            run(&session, &strings(&["get", address, "--json"])).map_err(Error::Herdr)?;
        identity(&snapshot).map_err(Error::Herdr)?;
        if field(&snapshot, "address").map_err(Error::Herdr)? != address
            || label(&snapshot, "task").map_err(Error::Herdr)? != format!("t{}", eligible.number)
            || !same_directory(
                label(&snapshot, "worktree").map_err(Error::Herdr)?,
                &record.worktree,
            )
        {
            return Err(Error::Herdr(
                "hc run returned an inconsistent identity".into(),
            ));
        }
        Ok(pane)
    })();
    match outcome {
        Ok(pane) => Ok(Launched {
            thread_link: Some(format!("Thread: {thread} hc:{session} - {pane}")),
            record,
            naming: None,
            warning,
            undo,
        }),
        Err(error) => {
            host.remove_launcher(&key);
            // If hc could have accepted a timed-out run, keep the checkout until a verified lookup.
            let left = super::undo_launch(host, &record, &undo);
            Err(match left {
                Some(left) => error.noting(&format!("left behind: {}", left.what)),
                None => error,
            })
        }
    }
}

pub(crate) fn undo(
    host: &mut super::SystemDispatchHost,
    record: &crate::domain::Dispatch,
    undo: &super::LaunchUndo,
) -> Option<super::Leftover> {
    use super::{Created, DispatchHost, Leftover};
    let fail = |reason: String| {
        Some(Leftover {
            what: format!("hc worktree {} ({reason})", record.worktree),
            workspace_open: true,
        })
    };
    // Rollback without the terminal context cannot prove a safe target. Preserve everything.
    let Some((number, pane)) = &undo.hc_terminal else {
        return fail(
            "launch identity uncertain; inspect the recorded session before retrying".into(),
        );
    };
    if let Err(error) = stop(*number, record, Some(pane)) {
        return fail(error);
    }
    if let Created::Worktree { branch_from } = &undo.created {
        match host.worktree_dirty(Path::new(&record.worktree)) {
            Ok(false) => {}
            Ok(true) => return fail("uncommitted changes; terminal stopped".into()),
            Err(error) => return fail(error),
        }
        if let Err(error) = host.remove_git_worktree(&undo.project, Path::new(&record.worktree)) {
            return fail(error);
        }
        if let Some(tip) = branch_from {
            if let Err(error) = host.delete_branch_at_tip(&undo.project, &record.branch, tip) {
                return fail(error);
            }
        }
    }
    None
}

pub(crate) fn verify_reference(
    notes: Option<&str>,
    workspace: &str,
    pane: &str,
) -> Result<(), String> {
    let session = workspace
        .strip_prefix("hc:")
        .filter(|s| token(s))
        .ok_or("invalid hc workspace identity")?;
    metadata(notes, session)?;
    let reference = crate::ui::links::task_links(notes)
        .into_iter()
        .find_map(|link| {
            if let crate::ui::links::TaskLink::Thread(reference) = link {
                Some(reference)
            } else {
                None
            }
        })
        .ok_or("hc dispatch requires an exact four-token Thread: reference")?;
    if reference.workspace_id != workspace || reference.tab_id != "-" || reference.pane_id != pane {
        return Err("hc Thread: reference does not match the live terminal".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_dispatch_reference_must_match_the_requeried_terminal() {
        let prefix = "Owner lead: lead-a\nThread: ";
        assert!(super::verify_reference(
            Some(&format!("{prefix}thr-a hc:example - hc:example:t7")),
            "hc:example",
            "hc:example:t7"
        )
        .is_ok());
        let rejected: Vec<_> = [
            "thr-a",
            "thr-a hc:other - hc:other:t7",
            "thr-a hc:example - hc:example:t8",
            "thr-a hc:example tab hc:example:t7",
            "thr-a hc:example - hc:example:t7 extra",
        ]
        .into_iter()
        .map(|reference| {
            super::verify_reference(
                Some(&format!("{prefix}{reference}")),
                "hc:example",
                "hc:example:t7",
            )
            .is_err()
        })
        .collect();
        assert!(rejected.iter().all(|value| *value), "{rejected:?}");
    }
    use serde_json::json;
    #[test]
    fn launch_metadata_is_explicit_and_unambiguous() {
        assert_eq!(
            super::metadata(Some("Owner lead: lead-a default\nThread: thr-a"), "example").unwrap(),
            ("lead-a".into(), "thr-a".into())
        );
        assert_eq!(
            super::metadata(
                Some("Owner lead: dev-01 default lead-a\nThread: thr-a"),
                "example"
            )
            .unwrap(),
            ("lead-a".into(), "thr-a".into())
        );
        for notes in [
            None,
            Some("Thread: thr-a"),
            Some("Owner lead: lead-a"),
            Some("Owner lead: lead-a\nOwner lead: lead-b\nThread: thr-a"),
            Some("Owner lead: lead-a\nThread: thr-a\nThread: thr-b"),
            Some("Owner lead: not-lead\nThread: thr-a"),
            Some("Owner lead: lead-a\nThread: thr-a hc:other - hc:other:t7"),
            Some("Owner lead: lead-a\nThread: thr-a w1 w1:t1 w1:p1"),
            Some("Owner lead: lead-a\nThread: thr-a hc:example - hc:example:remote/t7"),
        ] {
            assert!(super::metadata(notes, "example").is_err(), "{notes:?}");
        }
    }
    use super::*;
    fn terminal() -> Value {
        let directory = std::env::temp_dir().to_string_lossy().into_owned();
        json!({"address":"t7", "name":"thr-a", "state":"live", "reason":null, "cwd":directory,
            "labels":{"task":"t12","owner_lead":"lead-a","thread":"thr-a",
                "worktree":directory,"machine":"dev-01","sha":"abcdef1"},
            "status":null,"reported":false})
    }
    #[test]
    fn backend_requires_an_explicit_safe_session_without_fallback() {
        assert_eq!(selection(None, None).unwrap(), None);
        assert_eq!(selection(Some("herdr"), None).unwrap(), None);
        assert_eq!(
            selection(Some("hc"), Some("example")).unwrap(),
            Some("example".into())
        );
        for (backend, session) in [
            ("unknown", Some("example")),
            ("hc", None),
            ("hc", Some("")),
            ("hc", Some("../default")),
            ("hc", Some("-bad")),
            ("", None),
        ] {
            assert!(
                selection(Some(backend), session).is_err(),
                "{backend} {session:?}"
            );
        }
    }
    #[test]
    fn local_identity_requires_all_labels_and_consistent_cwd() {
        assert!(identity(&terminal()).is_ok());
        let accepted: Vec<_> = ["t1b", "t65jk"]
            .into_iter()
            .map(|address| {
                let mut value = terminal();
                value["address"] = json!(address);
                (address, identity(&value).is_ok())
            })
            .collect();
        assert!(accepted.iter().all(|(_, valid)| *valid), "{accepted:?}");
        for key in ["task", "owner_lead", "thread", "worktree", "machine", "sha"] {
            let mut value = terminal();
            value["labels"].as_object_mut().unwrap().remove(key);
            assert!(identity(&value).is_err(), "missing {key}");
        }
        for (key, bad) in [
            ("task", "T12"),
            ("task", "t0"),
            ("task", "t1b"),
            ("worktree", "relative"),
            ("owner_lead", "lead-a extra"),
            ("owner_lead", "not-lead"),
            ("thread", "thr-other"),
            ("sha", "not-a-sha"),
        ] {
            let mut value = terminal();
            value["labels"][key] = json!(bad);
            assert!(identity(&value).is_err(), "{key} {bad}");
        }
        for address in [
            "other/t7", "-bad", "hc:x:t7", "", "t0", "t01", "t1a", "t1i", "t1l", "t1o", "t1u",
            "t1B",
        ] {
            let mut value = terminal();
            value["address"] = json!(address);
            assert!(identity(&value).is_err(), "{address}");
        }
        let mut mismatch = terminal();
        mismatch["cwd"] = json!("/");
        assert!(identity(&mismatch).is_err());
    }
    #[test]
    fn terminal_resolution_never_guesses_or_treats_finished_as_live() {
        let value = terminal();
        let directory = field(&value, "cwd").unwrap().to_string();
        assert_eq!(
            unique(std::slice::from_ref(&value), 12, &directory).unwrap(),
            Some("t7".into())
        );
        assert!(unique(&[value.clone(), value.clone()], 12, &directory).is_err());
        assert!(unique(std::slice::from_ref(&value), 13, &directory).is_err());
        assert!(unique(std::slice::from_ref(&value), 12, "/").is_err());
        for (state, reason) in [("finished", Value::Null), ("live", json!("holder_lost"))] {
            let mut ended = value.clone();
            ended["state"] = json!(state);
            ended["reason"] = reason;
            assert_eq!(unique(&[ended], 12, &directory).unwrap(), None);
        }
        let mut malformed = value;
        malformed["state"] = json!("mystery");
        assert!(unique(&[malformed], 12, &directory).is_err());
    }
}
