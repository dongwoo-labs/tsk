//! Threads view: the live coding-agent sessions Herdr runs, grouped by execution status.
//!
//! Read-only. The rows come from `herdr agent list`; nothing here reads or writes the task
//! store. A row is one live agent: its Herdr name (an agent-home handle such as
//! `thr-dongwoo-2648`), its Herdr lifecycle status, where it lives, and the one-line
//! `inbox` token the inbox mod reports for its pane.

pub mod herdr;
pub mod ui;

use std::collections::HashMap;

use serde_json::Value;

/// Name prefixes of agent-home sessions shown by default: the lead, issue threads, and
/// the retired controller and orchestrator roles while any of them still runs.
pub const ROLE_PREFIXES: [&str; 4] = ["lead-", "thr-", "ctl-", "orch-"];

/// Herdr's agent lifecycle status, as `agent list` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentStatus {
    fn parse(raw: &str) -> Self {
        match raw {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Unknown => "unknown",
        }
    }
}

/// Herdr execution-state groups, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// Herdr reports blocked execution; this does not imply an open user question.
    Blocked,
    /// Working on a turn.
    InMotion,
    /// Ready for input: idle, done, or unclassified.
    OnDeck,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Blocked, Group::InMotion, Group::OnDeck];

    pub fn title(self) -> &'static str {
        match self {
            Self::Blocked => "BLOCKED",
            Self::InMotion => "IN MOTION",
            Self::OnDeck => "ON DECK",
        }
    }
}

/// One live agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRow {
    /// Herdr agent name; empty when the agent is unnamed.
    pub name: String,
    /// Agent kind label such as `claude` or `codex`.
    pub agent: Option<String>,
    pub status: AgentStatus,
    /// The inbox mod's one-line status for this pane, when it reported one.
    pub inbox: Option<String>,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub focused: bool,
    /// Herdr's per-agent state change counter; lets the view time how long a status held.
    pub state_change_seq: u64,
    pub cwd: Option<String>,
    /// Herdr's `terminal_title_stripped`: the session name a hand-started agent (one with
    /// no Herdr name, such as a lead launched in its own pane) still shows.
    pub title: Option<String>,
}

impl ThreadRow {
    /// The tracker issue key a `thr-` handle names: `thr-dongwoo-2648` → `DONGWOO-2648`.
    /// None for other roles or a name with no `<team>-<number>` tail.
    pub fn issue_key(&self) -> Option<String> {
        let rest = self.name.strip_prefix("thr-")?;
        let (team, number) = rest.rsplit_once('-')?;
        if team.is_empty()
            || !team.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            || number.is_empty()
            || !number.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        Some(format!("{}-{number}", team.to_ascii_uppercase()))
    }

    pub fn is_role_session(&self) -> bool {
        ROLE_PREFIXES.iter().any(|p| self.name.starts_with(p))
    }

    /// Classify only Herdr's execution status; inbox text is an optional annotation.
    pub fn group(&self) -> Group {
        match self.status {
            AgentStatus::Blocked => Group::Blocked,
            AgentStatus::Working => Group::InMotion,
            _ => Group::OnDeck,
        }
    }

    /// What to call the row: the Herdr name, else the pane id.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.pane_id
        } else {
            &self.name
        }
    }

    /// Target for `herdr agent focus`: the unique live name, else the hosting pane id.
    pub fn focus_target(&self) -> &str {
        self.display_name()
    }

    /// The session's own name: the Herdr name, else the terminal title, else the pane id.
    pub fn session_name(&self) -> &str {
        match self.title.as_deref() {
            Some(title) if self.name.is_empty() && !title.is_empty() => title,
            _ => self.display_name(),
        }
    }
}

/// Parse `herdr agent list` JSON. An error response or an unreadable document is an
/// error, never an empty list: a failed lookup must not read as "no threads".
pub fn parse_agent_list(json: &str) -> Result<Vec<ThreadRow>, String> {
    let data: Value =
        serde_json::from_str(json).map_err(|e| format!("unreadable agent list: {e}"))?;
    if let Some(error) = data.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string());
        return Err(format!("herdr agent list failed: {message}"));
    }
    let agents = data
        .get("result")
        .and_then(|r| r.get("agents"))
        .and_then(Value::as_array)
        .ok_or_else(|| "agent list has no result.agents".to_string())?;
    Ok(agents.iter().filter_map(parse_agent).collect())
}

fn parse_agent(agent: &Value) -> Option<ThreadRow> {
    let text = |key: &str| agent.get(key).and_then(Value::as_str).map(str::to_owned);
    let pane_id = text("pane_id")?;
    Some(ThreadRow {
        name: text("name").unwrap_or_default(),
        agent: text("display_agent").or_else(|| text("agent")),
        status: AgentStatus::parse(agent.get("agent_status").and_then(Value::as_str)?),
        inbox: tokens(agent.get("tokens"))
            .remove("inbox")
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty()),
        workspace_id: text("workspace_id").unwrap_or_default(),
        tab_id: text("tab_id").unwrap_or_default(),
        pane_id,
        focused: agent
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        state_change_seq: agent
            .get("state_change_seq")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cwd: text("foreground_cwd").or_else(|| text("cwd")),
        title: text("terminal_title_stripped")
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty()),
    })
}

/// Metadata tokens arrive as a `{key: value}` map; accept `[[key, value]]` pairs as well,
/// the shape Herdr uses for workspace tokens.
fn tokens(value: Option<&Value>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    match value {
        Some(Value::Object(map)) => {
            for (key, value) in map {
                if let Some(value) = value.as_str() {
                    out.insert(key.clone(), value.to_owned());
                }
            }
        }
        Some(Value::Array(pairs)) => {
            for pair in pairs {
                if let (Some(key), Some(value)) = (
                    pair.get(0).and_then(Value::as_str),
                    pair.get(1).and_then(Value::as_str),
                ) {
                    out.insert(key.to_owned(), value.to_owned());
                }
            }
        }
        _ => {}
    }
    out
}

/// Rows the view shows, in board order: group, then lead before threads, then name.
pub fn visible_rows(rows: &[ThreadRow], show_all: bool) -> Vec<ThreadRow> {
    let mut out: Vec<ThreadRow> = rows
        .iter()
        .filter(|row| show_all || row.is_role_session())
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        (a.group(), !a.name.starts_with("lead-"), a.display_name()).cmp(&(
            b.group(),
            !b.name.starts_with("lead-"),
            b.display_name(),
        ))
    });
    out
}

/// Headless report: one JSON object per visible row, for agents and scripts.
pub fn rows_json(rows: &[ThreadRow]) -> String {
    let list: Vec<Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "name": row.name,
                "issue": row.issue_key(),
                "group": match row.group() {
                    Group::Blocked => "blocked",
                    Group::InMotion => "in_motion",
                    Group::OnDeck => "on_deck",
                },
                "status": row.status.label(),
                "inbox": row.inbox,
                "agent": row.agent,
                "workspace_id": row.workspace_id,
                "tab_id": row.tab_id,
                "pane_id": row.pane_id,
                "focused": row.focused,
            })
        })
        .collect();
    serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"id":"cli:agent:list","result":{"type":"agent_list","agents":[
      {"terminal_id":"t1","name":"thr-dongwoo-2648","agent":"claude","agent_status":"working",
       "tokens":{"inbox":"2 · tsk fork 위치는?"},"workspace_id":"w1","tab_id":"w1:t2",
       "pane_id":"w1:p3","focused":false,"state_change_seq":4,"revision":9},
      {"terminal_id":"t2","name":"lead-main","agent":"claude","agent_status":"idle",
       "workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1","focused":true,"revision":2},
      {"terminal_id":"t3","name":"thr-dongwoo-2650","agent":"claude","agent_status":"blocked",
       "tokens":{"inbox":"Allow push main to origin? 허용?"},"workspace_id":"w1",
       "tab_id":"w1:t3","pane_id":"w1:p4","focused":false,"revision":1},
      {"terminal_id":"t4","name":"thr-dongwoo-2651","agent":"claude","agent_status":"idle",
       "tokens":{"inbox":"! Signed out: /login"},"workspace_id":"w2","tab_id":"w2:t1",
       "pane_id":"w2:p1","focused":false,"revision":1},
      {"terminal_id":"t5","agent":"codex","agent_status":"working","workspace_id":"w2",
       "tab_id":"w2:t1","pane_id":"w2:p2","focused":false,"revision":1}
    ]}}"#;

    #[test]
    fn parses_names_status_location_and_inbox_token() {
        let rows = parse_agent_list(LIST).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].name, "thr-dongwoo-2648");
        assert_eq!(rows[0].status, AgentStatus::Working);
        assert_eq!(rows[0].inbox.as_deref(), Some("2 · tsk fork 위치는?"));
        assert_eq!(rows[0].tab_id, "w1:t2");
        assert_eq!(rows[0].state_change_seq, 4);
        assert_eq!(rows[1].inbox, None);
        assert_eq!(rows[4].name, "");
        assert_eq!(rows[4].focus_target(), "w2:p2");
    }

    #[test]
    fn an_error_response_is_an_error_not_an_empty_list() {
        let err = parse_agent_list(r#"{"id":"x","error":{"code":"no_server","message":"down"}}"#)
            .unwrap_err();
        assert!(err.contains("down"));
        assert!(parse_agent_list("not json").is_err());
        assert!(parse_agent_list(r#"{"result":{}}"#).is_err());
    }

    #[test]
    fn token_pairs_are_accepted_too() {
        let json = r#"{"result":{"agents":[{"name":"thr-a-1","agent_status":"idle",
          "pane_id":"w1:p1","tokens":[["inbox","ok"]]}]}}"#;
        assert_eq!(
            parse_agent_list(json).unwrap()[0].inbox.as_deref(),
            Some("ok")
        );
    }

    #[test]
    fn issue_key_is_recovered_from_thread_handles_only() {
        let row = |name: &str| ThreadRow {
            name: name.into(),
            ..parse_agent_list(LIST).unwrap()[0].clone()
        };
        assert_eq!(
            row("thr-dongwoo-2648").issue_key().as_deref(),
            Some("DONGWOO-2648")
        );
        assert_eq!(row("thr-eng-12").issue_key().as_deref(), Some("ENG-12"));
        assert_eq!(row("lead-main").issue_key(), None);
        assert_eq!(row("thr-notes").issue_key(), None);
        assert_eq!(row("thr--12").issue_key(), None);
    }

    #[test]
    fn groups_follow_herdr_execution_status() {
        let rows = parse_agent_list(LIST).unwrap();
        assert_eq!(rows[0].group(), Group::InMotion);
        assert_eq!(rows[1].group(), Group::OnDeck);
        assert_eq!(rows[2].group(), Group::Blocked);
        // Inbox annotations never change the execution-state group.
        assert_eq!(rows[3].group(), Group::OnDeck);
        for (status, expected) in [
            (AgentStatus::Working, Group::InMotion),
            (AgentStatus::Idle, Group::OnDeck),
            (AgentStatus::Done, Group::OnDeck),
            (AgentStatus::Unknown, Group::OnDeck),
            (AgentStatus::Blocked, Group::Blocked),
        ] {
            for inbox in [None, Some("! Signed out: /login"), Some("2 · question")] {
                let row = ThreadRow {
                    status,
                    inbox: inbox.map(str::to_owned),
                    ..rows[0].clone()
                };
                assert_eq!(row.group(), expected);
            }
        }
    }

    #[test]
    fn visible_rows_keep_role_sessions_in_board_order() {
        let rows = parse_agent_list(LIST).unwrap();
        let names: Vec<String> = visible_rows(&rows, false)
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(
            names,
            [
                "thr-dongwoo-2650",
                "thr-dongwoo-2648",
                "lead-main",
                "thr-dongwoo-2651"
            ]
        );
        assert_eq!(visible_rows(&rows, true).len(), 5);
    }

    #[test]
    fn json_report_names_issue_group_and_inbox() {
        let rows = visible_rows(&parse_agent_list(LIST).unwrap(), false);
        let report: Value = serde_json::from_str(&rows_json(&rows)).unwrap();
        assert_eq!(report[0]["issue"], "DONGWOO-2650");
        assert_eq!(report[0]["group"], "blocked");
        assert_eq!(report[1]["inbox"], "2 · tsk fork 위치는?");
        assert_eq!(report[3]["group"], "on_deck");
        assert_eq!(report[3]["inbox"], "! Signed out: /login");
    }
}
