//! Stall checks: Doing tasks whose notes and thread have both gone quiet.
//!
//! Session-only. Nothing here writes the task store. The board keeps the last
//! `herdr agent list` snapshot and when it last asked about each task; a Doing task is
//! stale when its single `Thread:` line names an agent that is not working (or not live),
//! its notes have not changed within the threshold, and the board has not asked about it
//! within that threshold either. Asking is an identifier-only hint sent through
//! `herdr agent prompt`: to the task's own thread, or (check all) to the single live lead.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use uuid::Uuid;

use crate::domain::{HumanStatus, Task};
use crate::threads::{AgentStatus, ThreadRow};
use crate::ui::links::{task_links, TaskLink, ThreadRef};

/// The bold row mark a stale task paints before its question marks; a click on it asks.
pub const REFRESH_MARK: &str = "\u{21bb}";
/// Default quiet time before a Doing task counts as stale.
pub const DEFAULT_STALE_MINUTES: u64 = 30;
/// How often the board refreshes its agent snapshot while idle.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// `TSK_STALE_MINUTES` (a positive whole number), else 30 minutes.
pub fn threshold() -> Duration {
    let minutes = std::env::var("TSK_STALE_MINUTES")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|minutes| *minutes > 0)
        .unwrap_or(DEFAULT_STALE_MINUTES);
    Duration::from_secs(minutes * 60)
}

/// Whether this session runs inside Herdr, the only place a check can be sent.
pub fn inside_herdr() -> bool {
    std::env::var("HERDR_ENV").as_deref() == Ok("1")
}

/// The board's session-only stall state.
#[derive(Debug, Clone, Default)]
pub struct StallState {
    /// Inside Herdr: the check button paints and the idle loop polls.
    pub enabled: bool,
    /// The last successful agent snapshot; `None` until one arrives (no stale marks then).
    pub agents: Option<Vec<ThreadRow>>,
    /// When the last poll was started, for the idle-loop rate limit.
    pub polled_at: Option<Instant>,
    /// When this session last asked about each task.
    pub asked: HashMap<Uuid, SystemTime>,
    /// A background `agent list` still running: its result lands in the slot.
    inflight: Option<AgentSlot>,
}

type AgentSlot = Arc<Mutex<Option<Result<Vec<ThreadRow>, String>>>>;

impl StallState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    /// Whether the idle loop should start another agent poll now.
    pub fn poll_due(&self, now: Instant) -> bool {
        self.enabled
            && self
                .polled_at
                .is_none_or(|at| now.duration_since(at) >= POLL_INTERVAL)
    }

    /// Idle-loop step: take a finished background poll, or start one when it is due. Never
    /// blocks the board: `list` runs on its own thread. A failed poll clears the snapshot,
    /// so no task is marked stale on stale data.
    pub fn tick(&mut self, now: Instant, list: fn() -> Result<Vec<ThreadRow>, String>) {
        if let Some(slot) = self.inflight.take() {
            let finished = slot.lock().ok().and_then(|mut result| result.take());
            match finished {
                Some(Ok(agents)) => self.agents = Some(agents),
                Some(Err(_)) => self.agents = None,
                // Still running: keep waiting, unless the worker died without an answer.
                None if Arc::strong_count(&slot) > 1 => {
                    self.inflight = Some(slot);
                    return;
                }
                None => self.agents = None,
            }
        }
        if !self.poll_due(now) {
            return;
        }
        self.polled_at = Some(now);
        let slot: AgentSlot = Arc::new(Mutex::new(None));
        let writer = Arc::clone(&slot);
        let spawned = std::thread::Builder::new()
            .name("tsk-stall-poll".into())
            .spawn(move || {
                let result = list();
                if let Ok(mut answer) = writer.lock() {
                    *answer = Some(result);
                }
            });
        if spawned.is_ok() {
            self.inflight = Some(slot);
        }
    }

    /// The stale tasks among `tasks`, by the rules in the module doc.
    pub fn stale_ids(
        &self,
        tasks: &[Task],
        now: SystemTime,
        threshold: Duration,
    ) -> BTreeSet<Uuid> {
        let Some(agents) = self.agents.as_deref() else {
            return BTreeSet::new();
        };
        tasks
            .iter()
            .filter(|task| {
                is_stale(
                    task,
                    agents,
                    self.asked.get(&task.id).copied(),
                    now,
                    threshold,
                )
            })
            .map(|task| task.id)
            .collect()
    }
}

/// The task's thread, when its notes carry exactly one complete `Thread:` line.
pub fn thread_of(task: &Task) -> Option<ThreadRef> {
    task_links(task.notes.as_deref())
        .into_iter()
        .find_map(|link| match link {
            TaskLink::Thread(reference) => Some(reference),
            _ => None,
        })
}

/// The one live agent matching the whole four-token reference, never a name-only guess.
pub fn live_agent<'a>(reference: &ThreadRef, agents: &'a [ThreadRow]) -> Option<&'a ThreadRow> {
    let mut matches = agents.iter().filter(|row| {
        row.name == reference.name
            && row.workspace_id == reference.workspace_id
            && row.tab_id == reference.tab_id
            && row.pane_id == reference.pane_id
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

/// Whether exactly one live agent carries `name`; `herdr agent prompt` targets by name.
pub fn unique_name(name: &str, agents: &[ThreadRow]) -> bool {
    agents.iter().filter(|row| row.name == name).count() == 1
}

/// Whether `time` lies within `threshold` of `now` (a future time counts as recent).
fn recent(time: SystemTime, now: SystemTime, threshold: Duration) -> bool {
    now.duration_since(time)
        .map(|age| age < threshold)
        .unwrap_or(true)
}

/// A live Doing task is stale when its single Thread line names an agent that is not
/// working, its notes are older than `threshold`, and it was not asked about within it.
pub fn is_stale(
    task: &Task,
    agents: &[ThreadRow],
    asked: Option<SystemTime>,
    now: SystemTime,
    threshold: Duration,
) -> bool {
    if task.status != HumanStatus::Doing || task.soft_deleted || task.archived {
        return false;
    }
    let Some(reference) = thread_of(task) else {
        return false;
    };
    if recent(task.updated_at, now, threshold) {
        return false;
    }
    if asked.is_some_and(|at| recent(at, now, threshold)) {
        return false;
    }
    !live_agent(&reference, agents).is_some_and(|row| row.status == AgentStatus::Working)
}

/// Whole minutes since the task last changed.
pub fn quiet_minutes(task: &Task, now: SystemTime) -> u64 {
    now.duration_since(task.updated_at)
        .map(|age| age.as_secs() / 60)
        .unwrap_or(0)
}

/// The identifier-only hint a per-task check sends to the task's thread.
pub fn task_hint(identifier: &str, id: Uuid, minutes: u64) -> String {
    format!(
        "tsk status check {identifier} task={id}: no progress for {minutes} min. Reply by updating the Task notes status card (Now/Next/Blocked) with `tsk edit`; if blocked, say what you need."
    )
}

/// The one hint check-all sends to the single live lead.
pub fn lead_hint(stale: &[String], todo: &[String]) -> String {
    let list = |ids: &[String]| {
        if ids.is_empty() {
            "none".to_string()
        } else {
            ids.join(" ")
        }
    };
    format!(
        "tsk stall check: stale doing {}; todo {}. Ask each stale task's thread for its status card, and start a thread for every todo task that can proceed now without asking the user (still ask only for deploys or irreversible actions).",
        list(stale),
        list(todo)
    )
}

/// The single live lead (`lead-` prefix) in `agents`, or why there is none.
/// "no live lead" with what Herdr did list, so a misnamed lead shows on the status row.
fn no_lead_detail(agents: &[ThreadRow]) -> String {
    let named: Vec<&str> = agents
        .iter()
        .map(|row| row.name.as_str())
        .filter(|name| !name.is_empty())
        .collect();
    let unnamed = agents.len() - named.len();
    let mut seen: Vec<String> = named.iter().take(6).map(|name| name.to_string()).collect();
    if named.len() > 6 {
        seen.push(format!("+{} more", named.len() - 6));
    }
    if unnamed > 0 {
        seen.push(format!("{unnamed} unnamed"));
    }
    if seen.is_empty() {
        "no live lead: herdr agent list is empty".to_string()
    } else {
        format!(
            "no live lead (no name starts with lead-; agents: {})",
            seen.join(", ")
        )
    }
}

pub fn single_lead(agents: &[ThreadRow]) -> Result<&str, String> {
    let leads: Vec<&str> = agents
        .iter()
        .map(|row| row.name.as_str())
        .filter(|name| name.starts_with("lead-"))
        .collect();
    match leads.as_slice() {
        [lead] => Ok(lead),
        [] => Err(no_lead_detail(agents)),
        many => Err(format!("{} live leads", many.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DomainState, ProvenanceOrigin, TaskScope};

    const MIN: Duration = Duration::from_secs(60);

    fn agent(name: &str, status: AgentStatus) -> ThreadRow {
        ThreadRow {
            name: name.into(),
            agent: Some("claude".into()),
            status,
            inbox: None,
            workspace_id: "w1".into(),
            tab_id: "w1:t2".into(),
            pane_id: "w1:p3".into(),
            focused: false,
            state_change_seq: 0,
            cwd: None,
        }
    }

    fn task(notes: Option<&str>, status: HumanStatus, age: Duration) -> (Task, SystemTime) {
        let mut state = DomainState::new();
        let id = state
            .create(
                "work",
                notes.map(str::to_string),
                TaskScope::Global,
                ProvenanceOrigin::Manual,
            )
            .expect("create");
        let mut task = state.get(id).expect("task").clone();
        task.status = status;
        let now = task.updated_at + age;
        (task, now)
    }

    const THREAD: &str = "Now: building\nThread: thr-a-1 w1 w1:t2 w1:p3";

    #[test]
    fn a_quiet_doing_task_with_an_idle_thread_is_stale_only_past_the_threshold() {
        let agents = [agent("thr-a-1", AgentStatus::Idle)];
        let (fresh, now) = task(Some(THREAD), HumanStatus::Doing, 29 * MIN);
        assert!(!is_stale(&fresh, &agents, None, now, 30 * MIN));
        let (quiet, now) = task(Some(THREAD), HumanStatus::Doing, 31 * MIN);
        assert!(is_stale(&quiet, &agents, None, now, 30 * MIN));
    }

    #[test]
    fn a_working_thread_is_never_stale_but_a_missing_one_is() {
        let (quiet, now) = task(Some(THREAD), HumanStatus::Doing, 90 * MIN);
        assert!(!is_stale(
            &quiet,
            &[agent("thr-a-1", AgentStatus::Working)],
            None,
            now,
            30 * MIN
        ));
        assert!(is_stale(&quiet, &[], None, now, 30 * MIN));
        // A name-only match on another pane is not this thread.
        let mut elsewhere = agent("thr-a-1", AgentStatus::Working);
        elsewhere.pane_id = "w1:p9".into();
        assert!(is_stale(&quiet, &[elsewhere], None, now, 30 * MIN));
    }

    #[test]
    fn asking_suppresses_the_mark_for_one_threshold() {
        let agents = [agent("thr-a-1", AgentStatus::Blocked)];
        let (quiet, now) = task(Some(THREAD), HumanStatus::Doing, 90 * MIN);
        assert!(!is_stale(
            &quiet,
            &agents,
            Some(now - 10 * MIN),
            now,
            30 * MIN
        ));
        assert!(is_stale(
            &quiet,
            &agents,
            Some(now - 40 * MIN),
            now,
            30 * MIN
        ));
    }

    #[test]
    fn todo_tasks_and_tasks_without_one_complete_thread_line_are_not_stale() {
        let agents = [agent("thr-a-1", AgentStatus::Idle)];
        let (todo, now) = task(Some(THREAD), HumanStatus::Todo, 90 * MIN);
        assert!(!is_stale(&todo, &agents, None, now, 30 * MIN));
        let (bare, now) = task(Some("Now: building"), HumanStatus::Doing, 90 * MIN);
        assert!(!is_stale(&bare, &agents, None, now, 30 * MIN));
        let (name_only, now) = task(Some("Thread: thr-a-1"), HumanStatus::Doing, 90 * MIN);
        assert!(!is_stale(&name_only, &agents, None, now, 30 * MIN));
    }

    #[test]
    fn no_snapshot_means_no_stale_marks() {
        let (quiet, now) = task(Some(THREAD), HumanStatus::Doing, 90 * MIN);
        let mut state = StallState::new(true);
        assert!(state
            .stale_ids(std::slice::from_ref(&quiet), now, 30 * MIN)
            .is_empty());
        state.agents = Some(Vec::new());
        assert!(state
            .stale_ids(std::slice::from_ref(&quiet), now, 30 * MIN)
            .contains(&quiet.id));
    }

    #[test]
    fn polling_is_off_outside_herdr_and_rate_limited_inside() {
        let now = Instant::now();
        assert!(!StallState::new(false).poll_due(now));
        let mut state = StallState::new(true);
        assert!(state.poll_due(now));
        state.polled_at = Some(now);
        assert!(!state.poll_due(now + Duration::from_secs(5)));
        assert!(state.poll_due(now + POLL_INTERVAL));
    }

    fn two_agents() -> Result<Vec<ThreadRow>, String> {
        Ok(vec![
            agent("lead-a", AgentStatus::Idle),
            agent("thr-a-1", AgentStatus::Working),
        ])
    }

    fn failing() -> Result<Vec<ThreadRow>, String> {
        Err("herdr agent list failed".into())
    }

    fn settle(state: &mut StallState, list: fn() -> Result<Vec<ThreadRow>, String>) {
        let start = Instant::now();
        for _ in 0..500 {
            state.tick(start, list);
            if state.inflight.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the background poll never finished");
    }

    #[test]
    fn a_background_poll_stores_the_snapshot_and_a_failed_one_clears_it() {
        let mut state = StallState::new(true);
        settle(&mut state, two_agents);
        assert_eq!(state.agents.as_ref().map(Vec::len), Some(2));
        // Not due again inside the interval: nothing new starts.
        state.tick(Instant::now(), failing);
        assert!(state.inflight.is_none());
        state.polled_at = None;
        settle(&mut state, failing);
        assert!(state.agents.is_none());
        // Outside Herdr nothing ever runs.
        let mut off = StallState::new(false);
        off.tick(Instant::now(), two_agents);
        assert!(off.inflight.is_none() && off.agents.is_none());
    }

    #[test]
    fn the_lead_hint_names_stale_and_todo_tasks_and_a_single_lead_is_required() {
        let hint = lead_hint(&["T12".into(), "T15".into()], &[]);
        assert!(hint.starts_with("tsk stall check: stale doing T12 T15; todo none."));
        assert_eq!(
            single_lead(&[
                agent("lead-a", AgentStatus::Idle),
                agent("thr-x-1", AgentStatus::Idle)
            ]),
            Ok("lead-a")
        );
        assert!(single_lead(&[]).is_err());
        assert!(single_lead(&[
            agent("lead-a", AgentStatus::Idle),
            agent("lead-b", AgentStatus::Idle)
        ])
        .is_err());
    }

    #[test]
    fn a_name_two_live_panes_share_is_not_a_prompt_target() {
        let mut other = agent("thr-a", AgentStatus::Idle);
        other.pane_id = "w1:p9".into();
        let agents = vec![agent("thr-a", AgentStatus::Idle), other];
        assert!(!unique_name("thr-a", &agents));
        assert!(unique_name("thr-a", &agents[..1]));
    }

    #[test]
    fn no_lead_names_what_herdr_listed() {
        let mut unnamed = agent("", AgentStatus::Working);
        unnamed.pane_id = "w1:p8".into();
        let why = single_lead(&[agent("thr-a", AgentStatus::Idle), unnamed]).unwrap_err();
        assert_eq!(
            why,
            "no live lead (no name starts with lead-; agents: thr-a, 1 unnamed)"
        );
        assert_eq!(
            single_lead(&[]).unwrap_err(),
            "no live lead: herdr agent list is empty"
        );
    }
}
