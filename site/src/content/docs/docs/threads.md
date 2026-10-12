---
title: Threads
description: See every live lead and issue-thread session in Herdr, and jump to one.
---

The threads pane lists the coding-agent sessions running in your Herdr session, grouped only by Herdr execution status. It reads Herdr and never reads or writes the task or question store. Execution status does not indicate whether a user answer is needed.

Open it with the **Open tsk threads** plugin action, or run `tsk threads` in any Herdr pane. Bind the action to a key in your Herdr config if you want one; `tsk setup herdr` does not add a default chord.

## What it shows

By default the pane shows sessions whose Herdr name starts with `lead-` or `thr-` (and the retired `ctl-` and `orch-` roles while one still runs). Press `a` to show every agent.

| Group | A session lands here when |
| --- | --- |
| BLOCKED | Herdr reports execution as `blocked`; this does not imply an open question or a wait for the user |
| IN MOTION | Herdr reports it `working` |
| ON DECK | it is `idle`, `done`, or Herdr cannot classify it |

Each row shows the session's name, its Herdr status, how long that status has held since the pane started watching, and the one-line status the [inbox](https://github.com/dongwoo-labs/inbox) mod reports for the session's pane, when it reports one. `●` marks the session Herdr has focused. The selected row unfolds to show the full inbox line, the issue key a `thr-` name carries (`thr-dongwoo-2648` is `DONGWOO-2648`), the agent kind, its tab and pane, and its working directory.

The inbox line is an optional annotation from Herdr metadata. Its text, including a leading `!`, never changes a session's group or derives a user-wait indication. The pane does not query a question store or API.

The pane re-reads Herdr every 1.5 seconds. A failed read says why on the second line and keeps the last rows on screen.

## Keys

| Key | Action |
| --- | --- |
| `Enter` | Focus the selected session: its tab, then its pane |
| `↑` / `↓`, `k` / `j` | Move the selection |
| `Home` / `End` | First / last session |
| `a` | Show every agent, or only lead and thread sessions |
| `r` | Refresh now |
| `q`, `Esc` | Close the pane |

## From the command line

| Command | What it does |
| --- | --- |
| `tsk threads` | Open the pane |
| `tsk threads --json [--all]` | Print the visible sessions as JSON: name, issue, group (`blocked`, `in_motion`, `on_deck`), status, inbox, agent, workspace, tab, pane, focused |
| `tsk threads focus <name>` | Focus the live agent with that Herdr name |

To link a task to its Thread, add `Thread: <agent-name> <workspace-id> <tab-id> <pane-id>` to the task's notes; the [task page](/docs/task-page/#questions-findings-and-links) focuses it only when all four tokens match one live session. `tsk threads focus` matches the name alone.

`--json` and `focus` exit `3` when Herdr cannot be read, `1` when the named agent is not live or focus fails, and `2` on a usage error.
