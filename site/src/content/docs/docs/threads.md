---
title: Threads
description: See every live lead and issue-thread session in Herdr, and jump to one.
---

The threads pane lists the coding-agent sessions running in your Herdr session, grouped by whose move is next. It reads Herdr and never writes to your board.

Open it with the **Open tsk threads** plugin action, or run `tsk threads` in any Herdr pane. Bind the action to a key in your Herdr config if you want one; `tsk setup herdr` does not add a default chord.

## What it shows

By default the pane shows sessions whose Herdr name starts with `lead-` or `thr-` (and the retired `ctl-` and `orch-` roles while one still runs). Press `a` to show every agent.

| Group | A session lands here when |
| --- | --- |
| NEEDS YOU | Herdr reports it `blocked` on an approval or question dialog, or its inbox line starts with `!` (signed out, usage limit, API error) |
| IN MOTION | Herdr reports it `working` |
| ON DECK | it is `idle`, `done`, or Herdr cannot classify it |

Each row shows the session's name, its Herdr status, how long that status has held since the pane started watching, and the one-line status the [inbox](https://github.com/dongwoo-labs/inbox) mod reports for the session's pane, when it reports one. `●` marks the session Herdr has focused. The selected row unfolds to show the full inbox line, the issue key a `thr-` name carries (`thr-dongwoo-2648` is `DONGWOO-2648`), the agent kind, its tab and pane, and its working directory.

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
| `tsk threads --json [--all]` | Print the visible sessions as JSON: name, issue, group, status, inbox, agent, workspace, tab, pane, focused |
| `tsk threads focus <name>` | Focus the live agent with that Herdr name (or pane id) |

`--json` and `focus` exit `3` when Herdr cannot be read, `1` when the named agent is not live or focus fails, and `2` on a usage error.
