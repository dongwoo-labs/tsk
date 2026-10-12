---
title: Task page
description: Read and edit a task's title, notes, and project, and see its questions and links.
---

Read notes and steps, then edit the task when you need to change it.

## Open

Double-click a task or select it and press `Enter`.

At 110 usable columns or wider, the selected task's details always show beside the list. Press `→` / `l` or `Enter`, or click inside the details, to focus them; `←` / `h` or `Esc` returns focus to the list and keeps any parked draft ([wide layout](/docs/board/#wide-layout)). Below 110 columns the page takes the whole pane; `Esc` or click **esc close** returns. In view mode, `ctrl+q` quits the whole board rather than closing the page; save or cancel unsaved edits first.

Press `J` / `K` on the page to open the next or previous task in the list's order. A draft with unsaved changes refuses to switch, and a task the list does not show (opened from a [related task](#related-tasks)) refuses with `this task is not in the list · esc back`.

Click the task's `T` number to copy it. The page shows its status, notes, parent, steps, direct children, questions, findings, links, project, and dates. Its footer shows the project, then the created and updated dates. Long text wraps.

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

In view mode, `Tab` moves the selection through the parent line, the steps, **+ step**, the direct children, the questions and findings, and the links, then back to the parent line; `Shift+Tab` reverses it. The first `Tab` selects the first step. `↑` from the first step reaches the parent line, and `↓` from the last step reaches the first child. View-mode `Tab` does not cycle task fields. Status shortcuts remain available on the task page; use `ctrl+e`, then `Tab` to reach Notes.

During task editing, the forward order is Title → Notes → steps → **+ step** → Scope → Title. `Shift+Tab` reverses the ring. A field click moves the cursor and retains staged changes.

## Scope

**Scope** is the task's project or desk. Select it while editing, press `Enter`, choose a destination, then `Enter` again. Scope also supports cycling with `Space`, `←`, or `→`. Archived projects are not offered.

Tasks no longer carry a thread label. To name the Thread working on a task, add a `Thread:` line to its notes ([links](#questions-findings-and-links)).

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

In task view, status shortcuts act on the open task even when a step is selected. Multi-select and marks retained from the board are ignored and clear when an action runs. `ctrl+o` sets Todo, including reopening Done; `ctrl+s` sets Doing from Todo and `ctrl+d` sets Done. Removed `ctrl+n`, `ctrl+b`, and `ctrl+r` status keys do nothing. `Enter` toggles the selected step only.

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

Opening a related task keeps the list as it is: its project, search, sort, and cursor. The cursor follows only when the list shows that task. Status and other verbs act on the task the page shows. `Esc` returns to the task you came from, skipping any that was deleted meanwhile, and closes the page when none is left. `J` / `K` to a list neighbour, a new list selection, a project change, or closing the page ends that trail. A draft with unsaved changes refuses to open another task. A missing parent refuses with `<UUID> not found · link kept` and keeps the link. A task in an archived project opens read-only: you can move through it and open its relations, and every change, `ctrl+u` included, refuses with `project <name> is archived · read only`. On an archived task, `ctrl+u` unarchives the task the page shows.

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

## Notes markdown

Notes display basic Markdown. Editing shows the original text.

| Write | Display |
| --- | --- |
| `**bold**` | Bold |
| `*emphasis*` or `_emphasis_` | Underlined |
| `` `code` `` | Dim, with backticks |
| `# Heading` through `###### Heading` | Bold and underlined |
| `- item` or `* item` | List |
| Triple-backtick fenced block | Dim block; inline styling disabled |

Unmatched asterisks and underscores inside words stay literal. Markdown checkboxes such as `- [ ]` are text; use [steps](/docs/steps/) for an interactive checklist.
