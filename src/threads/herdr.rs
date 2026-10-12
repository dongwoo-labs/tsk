//! Herdr calls the threads view makes: list live agents, and focus one.
//!
//! Every call goes through the `herdr` CLI (`HERDR_BIN_PATH` when the host injects it),
//! like the board launcher scripts. Nothing here mutates Herdr beyond focus.

use std::env;
use std::process::Command;

use super::{parse_agent_list, ThreadRow};

fn herdr_bin() -> String {
    env::var("HERDR_BIN_PATH")
        .ok()
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "herdr".into())
}

fn run(args: &[&str]) -> Result<String, String> {
    let output = Command::new(herdr_bin())
        .args(args)
        .output()
        .map_err(|e| format!("could not run herdr {}: {e}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            stdout.trim().to_owned()
        } else {
            stderr.trim().to_owned()
        };
        return Err(format!("herdr {} failed: {detail}", args.join(" ")));
    }
    Ok(stdout)
}

/// All live agents on the current Herdr server.
pub fn list_agents() -> Result<Vec<ThreadRow>, String> {
    parse_agent_list(&run(&["agent", "list"])?)
}

/// Ids and names reach the CLI as arguments: refuse anything that could read as a flag.
fn flag_safe(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

/// The commands that bring `row`'s pane in front of the user, in order.
///
/// `agent focus` moves server focus to the pane (switching workspace and tab). Attached
/// clients follow visible navigation, so the tab is activated through the public route
/// first, the same order the board launcher uses before `plugin pane focus`.
pub fn focus_commands(row: &ThreadRow) -> Result<Vec<Vec<String>>, String> {
    let target = row.focus_target();
    if !flag_safe(target) {
        return Err(format!("refusing unsafe agent target {target:?}"));
    }
    let mut commands = Vec::new();
    if flag_safe(&row.tab_id) {
        commands.push(vec!["tab".into(), "focus".into(), row.tab_id.clone()]);
    }
    commands.push(vec!["agent".into(), "focus".into(), target.to_owned()]);
    Ok(commands)
}

/// Submit an identifier-only hint to the live agent `name`. Exit 0 means Herdr accepted the
/// prompt; it is not evidence that the agent ran a turn or started anything.
pub fn prompt(name: &str, hint: &str) -> Result<(), String> {
    if !flag_safe(name) {
        return Err(format!("refusing unsafe agent target {name:?}"));
    }
    run(&["agent", "prompt", name, hint]).map(|_| ())
}

/// Focus `row`'s pane. Stops at the first failing step and names it.
pub fn focus(row: &ThreadRow) -> Result<(), String> {
    for command in focus_commands(row)? {
        let args: Vec<&str> = command.iter().map(String::as_str).collect();
        run(&args)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::parse_agent_list;

    fn row(name: &str, tab: &str) -> ThreadRow {
        let json = format!(
            r#"{{"result":{{"agents":[{{"name":"{name}","agent_status":"idle",
               "workspace_id":"w1","tab_id":"{tab}","pane_id":"w1:p9"}}]}}}}"#
        );
        parse_agent_list(&json).unwrap().remove(0)
    }

    #[test]
    fn focus_activates_the_tab_then_the_agent() {
        assert_eq!(
            focus_commands(&row("thr-dongwoo-2648", "w1:t2")).unwrap(),
            vec![
                vec!["tab", "focus", "w1:t2"],
                vec!["agent", "focus", "thr-dongwoo-2648"]
            ]
        );
    }

    #[test]
    fn an_unnamed_agent_is_focused_by_its_pane() {
        assert_eq!(
            focus_commands(&row("", "w1:t2")).unwrap()[1],
            vec!["agent", "focus", "w1:p9"]
        );
    }

    #[test]
    fn unsafe_ids_never_reach_the_cli() {
        assert!(focus_commands(&row("-rf", "w1:t2")).is_err());
        // An unsafe tab id is skipped; the agent focus still switches tabs server-side.
        assert_eq!(focus_commands(&row("thr-a-1", "--x")).unwrap().len(), 1);
    }
}
