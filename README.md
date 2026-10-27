# tsk

[![CI](https://github.com/smarzban/tsk/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/smarzban/tsk/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platforms: Linux • macOS • Windows](https://img.shields.io/badge/platforms-Linux%20%E2%80%A2%20macOS%20%E2%80%A2%20Windows-blue.svg)](https://gettsk.sh/docs/install/)

A terminal task board for you and your agents: one shared queue, a TUI for you, a CLI for them.

Keep the next task, the work in progress, and the things waiting on you in view.
Press `+` to capture a thought, add notes and steps when it needs a plan, and give
your agent the task number when you are ready. New tasks start in the open inbox. Legacy CLI JSON keeps `todo`, `doing`, and `done`; questions wait separately from Task status.

### Beside your agent

![The tsk board beside an agent working on the selected Redis-to-Postgres migration task](docs/images/board-in-herdr.png)

In Herdr, `prefix+t` opens or focuses your workspace's board, even across tabs.
Other workspaces keep their own view of the same tasks. `prefix+a` opens quick capture.
Click a task's `T` number to copy it, then paste it into your agent conversation.

### Room to think

![A wide tsk board with the selected task's notes and steps open alongside it](docs/images/wide-task-page.png)

In a wide pane, click a task or press `→` or `l` to open it beside the board; `←` or `h`
brings you back. `Enter` opens the task full screen and `Esc` returns. Without Herdr,
run `tsk` in any terminal.

### Related tasks

Use `tsk add -t "Implement parser" --parent T10` to create a child task, and
`tsk edit T12 --parent T11` or `--no-parent` to change or clear its parent.
`tsk list T10` shows direct children and their calculated Done/total count.
On the board, press `P` on a task page to choose its parent and `U` to clear it;
the page lists the direct children, and their parent's row shows `⊂done/total`.
Press `Enter` on the parent or a child line to open it, and `Esc` to go back.
Relationships never complete or delete other tasks. See the [CLI guide](https://gettsk.sh/docs/cli/#edit).

### From task to implementation

Define your agents once in `config.toml`, press `@` to assign a task, and press
`ctrl+s` to start it. Starting an assigned task dispatches it: tsk creates a Git
worktree and a Herdr workspace for the task and launches the agent there. When the
pull request is merged, `ctrl+d` completes the task and removes the worktree and
branch, never touching uncommitted or unmerged work. Dispatch runs in Herdr; on
Windows it is a preview.

## Quickstart

### Install

macOS and Linux (ARM64 or x86-64):

```sh
curl -fsSL https://gettsk.sh/install.sh | sh
```

Or with Homebrew:

```sh
brew install smarzban/tap/tsk
```

Windows 10/11 (ARM64 or x86-64), in a PowerShell window:

```powershell
irm https://www.gettsk.sh/install.ps1 | iex
```

The installers add tsk to your PATH and offer to set up Herdr and your coding agents
when they find them. If prompted, reopen your terminal. Options, upgrades, and
uninstall are in the [install guide](https://gettsk.sh/docs/install/).

### Add to Herdr

[Install Herdr](https://herdr.dev/docs/install/) first, then:

```sh
tsk setup herdr
herdr server reload-config
```

### Give your agent the skill

Two skills: `tsk-cli` teaches your agent to read the board, add tasks, and update
their status and steps. `tsk-coordinator` lets it brief builder agents, review their
pull requests, and report back on the board:

```sh
tsk setup
```

In a terminal, `tsk setup` finds your agents and asks once. To install for one agent,
name it: `tsk setup pi`, or `omp`, `claude`, `cursor`, `grok`, `codex`, or `opencode`.
[Skill directories and options](https://gettsk.sh/docs/cli/#setup).

### Agent questions

On a separate Agent board selected explicitly with `TSK_STATE_DIR` or `--state-dir`,
use `tsk ask --file question.json`, `tsk questions`, and `tsk answer Q1 --file answer.json`.
Questions link to a Task UUID without changing its status. Shared task data contains only
shareable summaries and references; full text and append-only answers stay in the
Git-excluded `questions-private/` directory. Answers use expected-revision CAS and are
saved before identifier-only Herdr notification. Agent replies do not grant deployment
or irreversible-change approval. Same-OS-account UI impersonation is not prevented.
`tsk route Q1 --file route.json` sets who a question is for (`audience` `user` or
`lead`). On that board, press `2` for the questions tab: approve, answer, correct, or turn
a finding into a task, and see what waits on you above the board's sections. A secret
question (`kind: secret`) lets an agent ask for a key: you type it on the board, tsk hands
it to `fnox set`, and the agent only sees the answer `set`.
[Question JSON and retry contract](https://gettsk.sh/docs/cli/#task-linked-questions).

### Completion reports

Register a Markdown document with `tsk report T12 --file completion.md --json` and read it back with `tsk report T12 --json`. Changed text creates a new revision; identical text keeps the existing one. `tsk reports --unread --json` lists the documents not yet opened, including those attached to older Done tasks.

Press `3` on the board for the dedicated reports tab, then `Enter` to open a rendered document. Only opening acknowledges that revision; CLI lookup and selecting a row do not. A new revision becomes unread again. `r` on a Task opens its report directly. Reports are shareable, are separate from private questions, and never change Task status or grant approval.

[Report commands and readback fields](https://gettsk.sh/docs/cli/#completion-reports).

### Open the board

Run `tsk`, or press **prefix+t** in Herdr.

**No project setup needed.** Inside Git, tsk opens the repository project. Outside
Git, it opens every task: your desk and all your projects.

### Add a task

```sh
tsk add -t "your task title"
```

Or press `+` on the board, `prefix+a` in Herdr, or ask your agent: “Add a task to fix
the login timeout, with steps to reproduce the bug.”

## Usage

The board shows every task at once, grouped by project: your desk first, then each
project under its own heading. Run it inside a repo and the cursor starts on that
project. Each task uses one line: its number, title, and progress such as `✓2/5`.
A bold `×` marks a task with a `Blocked:` notes line; open it to read the reason.
Press `v` to show or hide the `Now:` or `Blocked:` line under the title for this session.

Mouse or keyboard, your choice. Double-click a task
to open it, and scroll through your board. The actions along the bottom are
clickable too, including adding a task and changing its status. `Shift+M` marks
several tasks so one action covers them all.

Prefer the keyboard? A few keys for everyday use:

| Key | Action |
| --- | --- |
| `↑` / `↓` or `j` / `k` | Move between tasks |
| `Enter` | Open the selected task |
| `→` / `←` or `l` / `h` | Wide pane: move between board and task |
| `+` | Add a task |
| `ctrl+s` | Start an open, ready, or blocked task |
| `ctrl+o` | Set Todo, including reopening Done |
| `ctrl+b` | Block a task with a reason, or unblock it back to ready |
| `ctrl+r` | Send a task to review with what was done and what to check, or back to ready |
| `ctrl+d` | Mark the selected task done |
| `ctrl+u` | Undo the last change |
| `@` | Assign a task to an agent, yourself, or a person |
| `shift+M` | Enter or leave multi-select |
| `shift+↑` / `shift+↓` | In multi-select, mark the current task, then move |
| `Space` or click | In multi-select, mark or unmark a task |
| `v` | Show or hide notes status lines (session-only) |
| `d` | Show or hide done tasks |
| `/` | Search the board |
| `:` | Open the command palette |
| `?` | Show every shortcut |

[Full keymap](https://gettsk.sh/docs/keys/) ·
[Board guide](https://gettsk.sh/docs/board/) ·
[CLI reference](https://gettsk.sh/docs/cli/)

## Docs

The [user guide](https://gettsk.sh/docs/) covers installation, the board, agents,
and the full CLI reference.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md) for development and pull-request guidance.
Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
