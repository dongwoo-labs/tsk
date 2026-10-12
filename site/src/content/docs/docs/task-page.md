---
title: Task page
description: Read and edit a task's title, notes, project, and thread.
---

Read notes and steps, then edit the task when you need to change it.

## Open

Double-click a task or select it and press `Enter`.

At 110 usable columns or wider, the selected task's details always show beside the list. Press `→` / `l` or `Enter`, or click inside the details, to focus them; `←` / `h` or `Esc` returns focus to the list and keeps any parked draft ([wide layout](/docs/board/#wide-layout)). Below 110 columns the page takes the whole pane; `Esc` or click **esc close** returns. In view mode, `ctrl+q` quits the whole board rather than closing the page; save or cancel unsaved edits first.

Press `J` / `K` on the page to open the next or previous task in the list's order. A draft with unsaved changes refuses to switch.

Click the task's `T` number to copy it. The page shows its status, notes, parent and direct children, steps, project, thread, and dates. Its footer puts `#thread` before the project, then the created and updated dates. Long text wraps.

## Edit

The page opens in view mode.

| Action | Key |
| --- | --- |
| Edit title, or the selected step | `ctrl+e` |
| Edit notes | `ctrl+e`, then `Tab` |
| Move through editable fields | `Tab` / `Shift+Tab`, after starting an edit |
| Save the task edit | `Shift+Enter` |
| Cancel the current field | `Esc` or `ctrl+c` |

Clicking Title, Notes, Scope, or Thread does not start an edit from view mode. Start editing first; then click the field you want.

In view mode, `Tab` selects steps and **+ step**. It does not cycle task fields. Status shortcuts remain available on the task page; use `ctrl+e`, then `Tab` to reach Notes.

During task editing, the forward order is Title → Notes → steps → **+ step** → Thread → Scope → Title. `Shift+Tab` reverses the ring. A field click moves the cursor and retains staged changes.

## Scope and thread

**Scope** is the task's project or desk. **Thread** groups related work within a project.

| Field | Change it |
| --- | --- |
| Scope | Select it while editing, press `Enter`, choose a destination, then `Enter` again |
| Thread | Select it, then press `Enter` or click again to edit its name |

Scope also supports cycling with `Space`, `←`, or `→`. Archived projects are not offered.

Thread is optional and follows the [thread name rules](/docs/capture/#thread-names).

## Save

`Shift+Enter` saves the task's title, notes, scope, thread, and staged changes to existing steps.

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

In task view, status shortcuts act on the open task even when a step is selected. Multi-select and marks retained from the board are ignored and clear when an action runs. `ctrl+n` sets ready and `ctrl+o` sets open; `ctrl+s` starts an open or ready task. `Enter` toggles the selected step only.

While editing a field, use its edit keys. Save or cancel to return to the task's status actions.

Scroll with the wheel or scrollbar while editing. Typing or moving the Notes cursor brings it back into view.

## Related tasks

A child Task is separate tracked work; a Step remains part of this task. Each task has at most one direct parent, and a parent can belong to a different scope.

Between the notes and the steps, the page shows one parent line, then the direct children:

| Parent line | Meaning |
| --- | --- |
| `parent  none · P to set` | No parent |
| `parent  ↑ T10 <title> · <status>` | The parent task; `archived` replaces the status of an archived parent |
| `parent  ↑ <UUID> not found · link kept` | The parent was deleted or is missing; its id stays on this task |

When the task has children, `children 1/3 done` follows, then one row per direct child with its status glyph, number, and title. Children in every scope and status count, archived ones included; deleted tasks do not. When every child is done and this task is not, the page says `all children done · ctrl+d completes this task`. Nothing completes it for you.

| Action | Key |
| --- | --- |
| Choose a parent | `P`, type to filter, arrows to select, `Enter` to save |
| Clear the parent | `U`, or `P` and choose **no parent** |

`P` and `U` work in view mode on the open task, including the detail beside the list at 110+ columns. In a text field they type capital letters, and a draft with unsaved changes refuses both until you save or cancel it. The picker lists every other task by number; **current** marks the present parent and **archived** an archived task. A task under this one is marked **under this task** and refused, because choosing it would form a cycle. Each change saves at once; a failed save offers [retry or cancel](/docs/board/#save-failures).

Changing or clearing a parent never changes the status, scope, or children of either task. Archive and Done do not break a relationship. Deleting a parent never deletes, detaches, or completes its children; they keep its id and show it as not found. Saving other fields of a task keeps its parent as it is, found or not.

From the CLI, `tsk add --parent T10` and `tsk edit T12 --parent T10` set a parent, `tsk edit T12 --no-parent` clears it, and `tsk list T10` shows direct children with their Done/total count. A Linear import keeps the external parent as a `Parent: Linear <identifier> <URL>` line in Notes.

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
