---
title: Task page
description: Read and edit a task's title, notes, and project, and see its questions and links.
---

Read notes and steps, then edit the task when you need to change it.

## Open

Double-click a task or select it and press `Enter`.

At 110 usable columns or wider, the selected task's details always show beside the list. Press `→` / `l` or `Enter`, or click inside the details, to focus them; `←` / `h` or `Esc` returns focus to the list and keeps any parked draft ([wide layout](/docs/board/#wide-layout)). Below 110 columns the page takes the whole pane; `Esc` or click **esc close** returns. In view mode, `ctrl+q` quits the whole board rather than closing the page; save or cancel unsaved edits first.

Press `J` / `K` on the page to open the next or previous task in the list's order. A draft with unsaved changes refuses to switch, and a task the list does not show (opened from a [related task](#related-tasks)) refuses with `this task is not in the list · esc back`.

Click the task's `T` number to copy it. The page shows its status, notes, parent, steps, direct children, questions, findings, links, and paper trail. The footer shows the machine or `unspecified`, assignee, base, waiting tasks, and scope when applicable. Dates and actions live in the paper trail. Long text wraps.

## Edit

The page opens in view mode.

| Action | Key |
| --- | --- |
| Edit title, or the selected step | `ctrl+e` or `e` |
| Add a step | `ctrl+a` or `a` |
| Edit notes | `ctrl+e`, then `Tab` |
| Move through editable fields | `Tab` / `Shift+Tab`, after starting an edit |
| Save the task edit | `Shift+Enter` |
| Cancel the current field | `Esc` or `ctrl+c` |

Clicking Title, Notes, or Scope does not start an edit from view mode. Start editing first; then click the field you want.

In view mode, `Tab` visits the parent, steps, **+ step**, children, questions, findings, links, and paper trail. It then returns to the block or review section, or the parent when neither exists. `Shift+Tab` reverses the ring.

The first `Tab` enters a block or review section when present, otherwise the first step. Arrows move between the parent, steps, and children. View-mode `Tab` does not cycle editable fields.

During task editing, the forward order is Title → Notes → steps → **+ step** → Assignee → Base → After → Scope → Title. `Shift+Tab` reverses the ring. A field click moves the cursor and retains staged changes.

## Scope

**Scope** is the task's project or desk. Select it while editing, press `Enter`, choose a destination, then `Enter` again. Scope also supports cycling with `Space`, `←`, or `→`. Archived projects are not offered.

The task page does not edit grouping labels set with the CLI's `--thread` option. To name the working Thread, use a `Thread:` notes line ([links](#questions-findings-and-links)).

## Save

`Shift+Enter` saves the task's title, notes, scope, and staged changes to existing steps.

| While editing | `Enter` does this |
| --- | --- |
| Title | Move to Notes |
| Notes | Insert a line |
| Existing step | Keep the rename in the current edit session |
| New step | Save that step and open another empty row |

New steps and step moves save independently. Cancelling the task edit does not remove new steps already saved or undo moves. [Step editing details](/docs/steps/).

Press `Esc` to discard changes to the current field. Other staged changes remain until you save or cancel the task edit. Cancelling the task edit also restores removed steps. Save or cancel before selecting a different task.

If another writer deletes the task, saving refuses. If a save fails, use [retry or cancel](/docs/board/#save-failures).

## Status and steps

In task view, status shortcuts act on the open task even when a step is selected. Multi-select and marks retained from the board are ignored and clear when an action runs. `ctrl+o` sets Todo, including reopening Done; `ctrl+s` sets Doing from Todo and `ctrl+d` sets Done. `ctrl+b` and `ctrl+r` open the [block and review cards](/docs/board/#block-review-reply). `ctrl+n` does nothing. `Enter` toggles the selected step only.

While editing a field, use its edit keys. Save or cancel to return to the task's status actions.

Scroll with the wheel or scrollbar while editing. Typing or moving the Notes cursor brings it back into view.

## Related tasks

A child Task is separate tracked work; a Step remains part of this task. Each task has at most one direct parent, and a parent can belong to a different scope.

Between the notes and the steps, the page shows one parent line. The direct children follow the steps' **+ step** row.

| Parent line | Meaning |
| --- | --- |
| `parent  none · P to set` | No parent |
| `parent  ↑ T10 <title> · <status>` | The parent task; `archived` replaces the status of an archived parent |
| `parent  ↑ <UUID> not found · link kept` | The parent was deleted or is missing; its id stays on this task |

When the task has children, `children 1/3 done` comes after **+ step**, then one row per direct child with its status glyph, number, and title. Children in every scope and status count, archived ones included; deleted tasks do not. When every child is done and this task is not, the page says `all children done · ctrl+d completes this task`. Nothing completes it for you.

| Action | Key |
| --- | --- |
| Choose a parent | `P`, or `Enter` on `parent  none`; type to filter, arrows to select, `Enter` to save |
| Clear the parent | `U`, or `P` and choose **no parent** |
| Open the parent or a child | Select its line, then `Enter`; or click any of its rows |
| Return to the previous task | `Esc` |

`P` and `U` work in view mode on the open task, including the detail beside the list at 110+ columns. In a text field they type capital letters, and a draft with unsaved changes refuses both until you save or cancel it. The picker lists every other task by number; **current** marks the present parent and **archived** an archived task. A long title wraps onto more rows; `↑` / `↓` show the rest of a tall option before moving to the next one, and the wheel scrolls its rows. A task under this one is marked **under this task** and refused, because choosing it would form a cycle. Each change saves at once; a failed save offers [retry or cancel](/docs/board/#save-failures).

Opening a related task keeps the list as it is: its search and cursor. The cursor follows only when the list shows that task. Status and other verbs act on the task the page shows. `Esc` returns to the task you came from, skipping any that was deleted meanwhile, and closes the page when none is left. `J` / `K` to a list neighbour, a new list selection, a tab change, or closing the page ends that trail. A draft with unsaved changes refuses to open another task. A missing parent refuses with `<UUID> not found · link kept` and keeps the link. A task in an archived project opens read-only: you can move through it and open its relations, and every change, `ctrl+u` included, refuses with `project <name> is archived · read only`. On an archived task, `ctrl+u` unarchives the task the page shows.

Changing or clearing a parent never changes the status, scope, or children of either task. Archive and Done do not break a relationship. Deleting a parent never deletes, detaches, or completes its children; they keep its id and show it as not found. Saving other fields of a task keeps its parent as it is, found or not.

From the CLI, `tsk add --parent T10` and `tsk edit T12 --parent T10` set a parent, `tsk edit T12 --no-parent` clears it, and `tsk list T10` shows direct children with their Done/total count. A Linear import keeps the external parent as a `Parent: Linear <identifier> <URL>` line in Notes.

## Questions, findings, and links

After the children, the page lists the task's agent records on an explicit Agent board: **questions n open** (questions and approvals), then **findings**, each with its Q number, state, and summary. Select one and press `Enter` to open it on the [questions tab](/docs/board/#questions); `Esc` comes back to this task.

**links** lists the reference lines in the notes, each on its own line:

| Notes line | Shows |
| --- | --- |
| `Linear: <ID> <URL>` | The issue |
| `PR: <URL>` | The pull request |
| `CI: <URL>` | The CI run |
| `Thread: <agent-name> <workspace-id> <tab-id> <pane-id>` | The Thread working on the task |

`Enter` on an issue, PR, or CI line shows its URL on the status row; tsk calls no forge. `Enter` on a Thread line focuses that Herdr pane only when it is the task's only Thread line, carries all four tokens, and exactly one live agent matches all four. A name-only, partial, or duplicated Thread line shows `cannot verify` and focuses nothing. With no live match, several matches, or outside Herdr, the status row says so and nothing is focused. tsk never falls back to a question's origin Thread.

## Status card

Keep the first lines of the notes as a short status card, each one replaced in place rather than appended:

```text
Now: running the full test suite on the branch
Next: open the PR
Blocked: API key, waiting on the infra team
Done when: the PR is merged and the release notes mention it
PR: https://github.com/acme/atlas/pull/12
```

| Line | Use |
| --- | --- |
| `Now:` | The current state |
| `Next:` | The next action (optional) |
| `Blocked:` | What blocks the task and who it waits on; only while blocked |
| `Done when:` | The completion condition |

Reference lines (`PR:`, `Linear:`, `Issue:`, `CI:`, `Thread:`) follow the card. The [board rows](/docs/board/#rows) use one line per task by default and show a bold `×` for a non-empty `Blocked:` line on any task that is not done. Press `v` from the board list to show or hide the second status line for this session: `Blocked:` paints bold as `blocked: …`; otherwise a `Now:` line on a Doing task paints dim under its title. Each board status line stays on one line and is shortened with `…` when it does not fit; the task page still shows the full notes with wrapping. Keys are case-insensitive, a leading `- ` is allowed, and only the first matching line counts. The card is a convention: tsk never sets a status from it.

## Notes markdown

Notes display Markdown in mono styles, the way agents usually write them. Editing shows the original text. Long lines wrap on the displayed text, so a hidden marker never moves a wrap point, and a wrapped list item, quote, or table row keeps its indent or bar on every row. Selecting and copying a row copies what it displays.

| Write | Display |
| --- | --- |
| `**bold**` | Bold |
| `*emphasis*` or `_emphasis_` | Underlined |
| `~~struck~~` | Dim |
| `` `code` `` | Dim, with backticks |
| `[text](https://…)` | `text` underlined, then the URL dim in parentheses; just the URL when the text is the URL |
| `<https://…>` | The URL, underlined |
| `\*` and other backslash escapes | The character itself |
| `# Heading` through `###### Heading` | Bold and underlined, without the `#` |
| `- item`, `* item`, or `+ item` | List with a dim marker |
| `1. item` or `1) item` | Numbered list with a dim number |
| `- [ ] item` / `- [x] item` | `☐ item` / `☑ item`, a checked item dim |
| `> quote` | A dim `│` bar per level |
| `---`, `***`, or `___` | A dim rule across the width |
| `\| a \| b \|` table rows | Cells separated by a dim `│`; the row above a `\|---\|` divider bold, the divider hidden |
| Triple-backtick fenced block | Dim block; inline styling disabled |

Leading spaces stay, so nested lists keep their depth. Unmatched asterisks and underscores inside words stay literal. Checkbox items are display only; use [steps](/docs/steps/) for an interactive checklist. The questions tab renders [record bodies](/docs/board/#questions) the same way.

## Blocked

A blocked task opens with a BLOCKED section between the title and the notes:

```
@claude blocked on you · 2h

The login test fails only on CI. Which way?

 1  Skip it on CI
 2  Fix the CI image

claude · 3m   Recommend 2; it is a one-line change.
you · 1m      Go with 2.

1-2 choose · r reply · ctrl+s reply and unblock · w waiting on
────────────────────────────────────────────────
```

- The top line says who raised the block, who it waits on, and for how long: `@claude blocked on you · 2h`, `@claude blocked on T202 · 5m`, or `T202 is done · unblock it · 1h`.
- Below it come the why, the needs, the numbered options, and the replies. Each reply shows `you` or the agent profile, and its age. A deleted reply leaves a dim `deleted` stub.
- The last line lists only the keys that work at the cursor. A block that waits on another task or on something else shows `ctrl+b unblock` there.
- When the block closes, it moves to the paper trail.

### Reply box

From the page:

| Action | Key |
| --- | --- |
| Open the reply box | `r` |
| Open it starting from option N | `1`…`9`, or `Enter` on the option |
| Change who it [waits on](#waiting-on) | `w`, or `Enter` on the top line |
| Edit why, on, and needs (top line selected), or your selected reply | `ctrl+e` |
| Delete your selected reply | `ctrl+x` |

In the box:

| Action | Key |
| --- | --- |
| New line | `Enter` |
| Save the reply | `shift+Enter` |
| Save the reply and unblock | `ctrl+s` |
| Cancel | `Esc` |

- A saved reply waits. Unblocking sends every reply the agent has not received yet.
- Unblocking moves an unassigned task to ready. An assigned task starts: it dispatches, asks before relaunching a gone agent, or sends the replies to its running agent. [Dispatch](/docs/dispatch/#dispatch).
- `ctrl+s` outside the box, with nothing typed, unblocks the same way.
- `r` on a blocked board row opens the same box under the row.
- The box belongs to the block it opened on. If that block closes or is replaced elsewhere, saving refuses and keeps your draft.
- You can edit an agent's block, but not its replies.


## Review

A task in review opens with a REVIEW section in the same place:

```
@claude needs your review · 40m

Built the widget; empty input handled.

Check
 ✓ A works
 ○ B works on empty input
 ○ C works

you · 5m      B still fails on an empty string.

tab next field · enter cycle check · r feedback · ctrl+s send back · ctrl+d approve
w waiting on
───────────────────────────────────────────────────────────────────────────────────
```

- The top line says who asked for the review, who it is on, and for how long: `@claude needs your review · 40m`, or `@claude needs a review from @pi · 1m`.
- Below it come what was done, what is next, the checks, and your feedback. Checks are `✓` passed, `✗` failed, or `○` open.
- Passed checks fold into a dim `N passed ▸` line. A check you pass stays in place until the page opens again, or until you fold the line.
- The page opens on the first open check.
- Checks belong to the review and never become steps.
- When the round closes, it moves to the paper trail. The page shows no round number.

### Feedback box

From the page:

| Action | Key |
| --- | --- |
| Cycle the selected check: open → passed → failed → open | `Enter`, or click it |
| Show / fold the passed checks | `Enter` on the `N passed` line, or click it |
| Open the feedback box | `r` |
| Send back, with nothing typed | `ctrl+s` |
| Approve | `ctrl+d` |
| Change who it [waits on](#waiting-on) | `w`, or `Enter` on the top line |
| Edit done, checks, next, and on (top line selected), or your selected feedback | `ctrl+e` |
| Delete your selected feedback | `ctrl+x` |

In the box:

| Action | Key |
| --- | --- |
| New line | `Enter` |
| Save the feedback; the task stays in review | `shift+Enter` |
| Send back | `ctrl+s` |
| Approve | `ctrl+d` |
| Cancel | `Esc` |

- Send back sets the task to started and sends the round's feedback and failed checks to its running agent. With nothing typed, it needs a failed check.
- Approve marks the task done, through the [cleanup card](/docs/dispatch/#complete-and-clean-up) when its dispatch is live. Nothing is sent.
- Saved feedback waits for the send-back.
- The empty box reads `feedback to @claude…`. Like the reply box, it belongs to the round it opened on.
- Editing the checks keeps the state of every check whose text is unchanged.


## Waiting on

Hand an open block or review to someone else without touching the rest of it. On a blocked or review page, press `w` wherever the cursor is, or `Enter` on the top line. On the board, use **set waiting on…** in the palette for the selected task or a marked set.

1. The picker lists you, every agent profile, and the current choice, then **type a name…**. The current choice is selected and marked, such as `@sam · current`.
2. `←` / `→` or `↑` / `↓` move through the choices. Type to filter, or to give a name.
3. `Enter` saves. `Esc` changes nothing.

A typed name reads like an [assignee name](/docs/dispatch/#assignee-names): `you` and `me` are you, a profile's name is that agent, and anything else is kept as typed, such as `coordinator`. On a block, a task number such as `T202` makes it wait on that task.

- The block or review round stays the same: its text, checks, and replies are kept.
- The top line keeps its sentence and its age, and names the new one: `@claude blocked on you · 42s` becomes `@claude blocked on @pi · 48s`.
- The paper trail records one entry, such as `on: you → @pi`. One `ctrl+u` undoes it, for one task or a marked set.
- With tasks marked, a task with no open block or review refuses the whole set.
- From the CLI: [`tsk edit T12 --on`](/docs/cli/#edit).


## Paper trail

Below the steps, the paper trail lists who did what to the task, newest first. It opens collapsed as `PAPER TRAIL · 14 ▸`. Press `g`, or `Enter` on the heading, to expand it.

- An agent's entries name its profile, such as `@claude`. Yours read `you`, or your [configured name](/docs/storage/#board-defaults).
- The same entry by the same author within a minute groups into one: `3 steps checked`.
- Closed blocks and review rounds end in `▸`. `Enter` or a click expands one in place.
- The trail stays expanded while you stay on the task, and collapses when another task opens.

| Entry | Example |
| --- | --- |
| Created | `created · you 3d` |
| Status change | `open → started · @claude 2m`, `started · after T202` (its last prerequisite was done) |
| Assignee, base | `assigned @claude`, `unassigned`, `base ⎇ origin/dev`, `base cleared` |
| Waiting on | `on: you → @pi` |
| Dispatch | `dispatched tsk/t12-fix from origin/main @ 3333b79`, or `relaunched …` |
| Cleanup | `cleaned · worktree removed, branch kept` |
| Edits | `title edited`, `notes edited`, `after edited`: which fields, never the text |
| Steps | `step added`, `step checked` |
| Archive and delete | `archived`, `unarchived`, `deleted`, `restored` |
| A closed block | `blocked on you · need creds · 1 reply ▸` |
| A closed review round | `review round 2 · sent back · 1 failed ▸` |

An expanded block shows who blocked it and when, the why, the needs, the options, every reply, and who closed it. An expanded review round shows who set it, who it was on, what was done, each check, what is next, and the feedback.
