---
title: Capture
description: Add a task from the board, a Herdr popup, or the terminal.
---

Press `+` or click **+ add**. Type a title, then press `Enter`. On the website demo, the capture footer actions are clickable. New captures land in **TODO** with status `todo`.

## Quick-add

1. Press `+` or click **+ add**. The board stays visible.
2. Type a title, with [title tokens](#title-tokens) if you like.
3. Press `Enter` to save, or `shift+Enter` to save and add another.

The board stays visible. A saved task flashes and becomes selected in Todo. Invalid input stays open with an explanation.

Clicking a task while the quick-add line is open discards the draft and selects that task.

## Details

Press `Tab` from quick-add to add notes, steps, or a scope. The draft uses the full pane and opens in Notes. Its forward field ring is Title → Notes → steps → **+ step** → Assignee → Base → After → Scope → Title; `Shift+Tab` reverses it.

- `shift+Enter` saves.
- `Esc` returns to the line. `Tab` brings your details back while the line is unchanged.
- `Enter` on the line saves it without the details.

## Destination

| Where you add | Default destination |
| --- | --- |
| Board launched inside a live project's repository | That project |
| Board launched anywhere else | Desk |
| Herdr quick-capture popup | The focused pane's repository inside Git; desk outside Git |

Title tokens or the draft's Scope field change the destination. Archived projects cannot receive new tasks. Launching inside an archived project: [Done drawer and archive](/docs/board/#done-drawer-and-archive).

## Title tokens

Set a project, assignee, base branch, or the tasks to run after while you type the title:

```text
Fix login timeout !p atlas !a reviewer !b main
Ship the release notes !w T202 !w T205
Buy coffee !p !a
```

| Token | Sets |
| --- | --- |
| `!p` | Desk |
| `!p name` | The existing project with that basename, ignoring case |
| `!p /path` | An existing absolute directory, creating a project there if needed |
| `!a` | Unassigned |
| `!a name` | That agent profile, you (`!a me`), or a person, by the [assignee name](/docs/dispatch/#assignee-names) rules |
| `!b branch` | The dispatch base: an existing local or remote branch in the task's repository |
| `!w T202` | Run after T202 ("wait for"). Repeat for several. `!w 202` works too |

- Each token takes one word. Put a bare `!p` or `!a` at the end, or before another token or a `#word`.
- Tokens leave the saved title, and the remaining words join with single spaces. A title is required.
- `!p name` must match one project the board knows: one with tasks, or the launch repository. Otherwise the draft stays open and says so, such as `project atlss is not on the board`. An archived project refuses with `project atlas is archived`.
- `!p` creates a project only from an existing absolute directory, `/…` or `~/…`. A relative path or a file keeps the draft open.
- `!b` is checked against the branches on disk, without fetching. An unknown branch refuses on save and keeps the draft open. [Base branch](/docs/dispatch/#base-branch).
- `!w` takes any task number on the board, in any project. A missing or done task refuses and keeps the draft open. [After](/docs/board/#after).

## Thread names

Thread names are lowercased and must:

- Start with an ASCII letter or digit.
- Contain only ASCII letters, digits, `-`, or `.`.
- Be at most 32 characters.

These names apply to the CLI's `--thread` option. Quick-add treats `!t` and `#name` as ordinary title text.

## Herdr quick capture

In Herdr, press `prefix+a` after [setup](/docs/install/#add-to-herdr). A popup opens with the title focused, prefilled with any text selected in the pane you came from.

- `shift+Enter` saves and closes the popup.
- `Esc` discards it, after first closing an open step editor or picker.
- A failed save keeps the draft open to retry or cancel.

To open the popup from a script, invoke the plugin action for your platform:

| Platform | Command |
| --- | --- |
| Type or edit the title | Name the task |
| Add notes or steps | Include details before saving |
| `Shift+Enter` | Save and close the popup |
| `Esc` | Discard and close |

## In the terminal

Selected text from the invoking pane prefills the title. An archived launch project falls back to desk. A failed save keeps the draft open for retry or cancel.

You can also invoke the popup explicitly:

```sh
herdr plugin action invoke quick-capture --plugin herdr-tsk
```

For the same capture flow in your terminal, run `tsk capture`. `TSK_MODE=capture tsk` is equivalent. Captured tasks start `todo`.

## From an agent or script

```sh
tsk add -t "Fix login timeout"
# the new task starts todo; use tsk status T<n> doing when work begins
```

The task starts `open`. [`tsk add`](/docs/cli/#add) lists every option.

The browser demo uses sample data and does not dispatch agents, launch worktrees, or perform backup writes. Its capture supports project tokens; the terminal also supports assignee, base, and after tokens.
