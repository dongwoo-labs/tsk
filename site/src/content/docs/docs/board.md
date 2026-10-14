---
title: Board
description: Find tasks, switch projects, answer questions, and keep work moving.
---

Click to navigate, or use the keyboard. The footer shows actions for the cursor or your marked task set; those actions are clickable.

## Navigate

The top row holds the **1 board** and **2 questions** tabs, a scope control, and a sort control. Press `1` to return to the board tab and its list, or `2` for the [questions tab](#questions). Press `p` or click the scope to choose what the board shows:

| Scope | Contents |
| --- | --- |
| **all** | Every live task: your desk and every live project |
| **desk** | Doing tasks across all live projects; Todo tasks from your desk |
| *project name* | Tasks in that project |

Launching inside a Git repository opens that project. Outside Git, tsk opens **all**.

Rows show the `T`/`N` number and the title; the section already names the status. A bold `!` follows when an approval on the task waits on you, and `?n` counts its other questions and findings waiting on you. A task with direct children adds a dim `⊂done/total` count at the right edge, such as `⊂1/3`; it counts children in every scope and status, archived ones included ([related tasks](/docs/task-page/#related-tasks)). Rows follow task numbers by default. Press `S` or click the sort control to group rows by project instead (desk first, then project path); press `S` again to return to number order. In project order on **all** or **desk**, a dim project name heads each group. The sort is session-only and never changes a task. The [CLI](/docs/cli/#list) keeps its own order.

In Herdr, `prefix+t` opens or focuses a board in the current workspace. If one is already open in another tab there, Herdr switches to that tab and focuses its pane, preserving its view and edits. Otherwise, it opens a board beside your work. Boards in other workspaces stay untouched; all boards share the same tasks.

## Quit or go back

Press `ctrl+q` to quit from the board or a task view, including Help and pickers. While a text editor, board search, quick-add line, or palette query owns input, `ctrl+q` keeps its editing behavior instead. Unsaved drafts must be saved or cancelled before quitting, including a draft parked in the detail column while the list has focus. Save recovery must be resolved first; quit attempts leave its failure message intact.

`Esc` leaves multi-select and clears its marked tasks before it closes the current layer. At the board root, with multi-select inactive and no page, popup, search, or header selection left to dismiss, it quits without confirmation. This applies on every scope. In the wide layout, `Esc` from the detail column returns focus to the list once any editor or overlay is dismissed; a task draft stays parked beside it. Quick capture's popup-close behavior is unchanged.

## Projects

Press `p` to choose a scope: **all**, **desk**, or a project.

- Click a choice to switch to it; `Enter` switches to the highlighted one.
- Check the footer for the selected project's full path.

## Search

Press `/` to search the rows the board currently shows. It matches tasks by title, notes, step text, or task number such as `T12`, case-insensitively. Every whitespace-separated word must match somewhere in the same task.

Typing or pasting filters immediately. Empty sections disappear and section counts show only matches, the closed **DONE** heading's count included. Done rows appear only while the drawer is open.

Press `Enter` to pin the query and return to board keys. Navigation, task actions, the done drawer, and a second `Enter` then act on the filtered rows; the query remains in the footer. Press `Esc` while typing to clear and close search, or press it once on a pinned board to clear the query before normal `Esc` behavior resumes. Changing scope also clears it.

## Status

| Section | Status |
| --- | --- |
| **DOING** | `doing` |
| **TODO** | `todo` |
| Done drawer | `done` |

Questions do not create a blocked Task status. Human progress is independent of Herdr execution status. On the desk, **DOING** includes live project tasks and **TODO** contains only desk tasks. A project board shows its own tasks. Notice tasks lead within each group. There is no separate inbox group; with the done drawer open, `g` folds or unfolds its archived group.

Every section lists its rows in board order: `N` tasks first, then ascending `T` number, with tasks not yet numbered last. Editing a task, ticking a step, or setting a status never reorders a section; a status change only moves the task to its new section.

Move the cursor with `↑`/`↓` or `j`/`k`. Press `Shift+M` to enter multi-select. While it is active, press `Space` to toggle the cursored task, hold `Shift` with `↑`/`↓` to mark the current task before moving, or click a task to toggle it. Marked rows show `▪`; the cursor remains `▸`. Removing the last mark leaves the mode active. `Shift+M` again while the board owns input, a task action, `Esc`, or a view change such as folding a group or switching scope or the done drawer exits the mode and clears the session-only set. Text entry keeps `Shift+M` as a capital `M`; `Esc` leaves multi-select before cancelling that surface.

On a task-board list, `ctrl+s`, `ctrl+o`, `ctrl+d`, `ctrl+x`, and `ctrl+f` act on the marked set when it is non-empty. With no marks they act on the cursor. `Enter`, `ctrl+e`, and task-page actions remain cursor-only.

| Key | Action |
| --- | --- |
| `ctrl+s` | Move Todo tasks to Doing |
| `ctrl+o` | Set Todo, including reopening Done |
| `ctrl+d` | Mark Done |

Doing and Done tasks are skipped by `ctrl+s`; the status row reports skipped Done tasks. Repeating an absolute status does nothing. The old next, block and review status shortcuts are removed.

Agents can set any status with [the CLI](/docs/cli/#status). Task status does not change automatically when steps are checked or an agent stops.

## Questions

The **2 questions** tab lists the questions, approvals, and findings agents filed on an explicit Agent board (`TSK_STATE_DIR` set). The personal board records none and says so. When something waits on you, the board shows `waiting on you  approval n · question n · finding n  → 2` above its sections, with a separate `target unknown n` when records name no target; click it or press `2`. The tab's number counts approvals, questions, and findings waiting on you, and scope, sort, `p`, `S`, and search do not apply on it.

| Group | Contents |
| --- | --- |
| **NEEDS APPROVAL** | Open approvals addressed to you |
| **FOR YOU** | Open questions and secret requests addressed to you |
| **FINDINGS** | Open findings addressed to you |
| **WITH LEAD** | Open records addressed to the lead; shown, never counted |
| **CLOSED** | Answered, approved, rejected, or closed records; folded until `z` or a click on its heading shows them |

A record from an older agent that names no audience shows apart as *target unknown · route with tsk route*. tsk never guesses its target, offers no answer or correction on it (answered or not), and leaves it out of the tab number and row marks; an agent sets the audience with [`tsk route`](/docs/cli/#route).

Move with `↑`/`↓` or `j`/`k` and press `Enter` to open a record: a full page below 110 columns, the list and the record side by side at 110 and up (`→`/`l` opens, `←`/`h` goes back).

A record waiting on you is answered with one form, the way Claude Code asks: the question text, then a numbered choice list. The highlight starts on the first choice, or on the recommendation when there is one.

| Record | Choices |
| --- | --- |
| Question with options | its options, then `n. Type something…` |
| Question without options | one answer field, already focused |
| Approval | `1. Approve these exact terms` · `2. Reject` · `3. Add a comment…`, below the exact terms |
| Finding | `1. Create the task and run it` · `2. Create the task only` · `3. Close` |
| Secret | one masked field, already focused |

Answered, closed, lead-addressed, and target-unknown records show no choices.

| Key | Action |
| --- | --- |
| `↑` / `↓` or `j` / `k` | Move the highlight |
| `Enter` | Pick the highlighted choice at once; in a field, save |
| `1`–`9` | Pick that choice at once |
| `Esc` | In a field, leave it (the draft stays); otherwise back to the list; at the list, quit |
| `m` | The approval's comment row, as picking it does |
| `c` | Correct a saved answer; it becomes a new revision |
| `t` | Open the record's task; `Esc` returns to the record |
| `r` | Resend the lead request of a create-and-run finding |
| `J` / `K` | Next or previous record in list order |
| `PgUp` / `PgDn` or the wheel | Scroll the page |

Picking saves: there is no separate choose-then-save step. On the `Type something…` row, typing a character (or `Enter`) opens an inline text field; `Enter` saves the text as the answer and `Esc` leaves the field with the draft kept on the row, where `Enter` reopens it. A second `Esc` discards a kept draft, and the next leaves the page. While a field is open every key is text, and `ctrl+q` and `ctrl+c` do not quit. On a question without options the field opens focused; after `Esc` its page keys work again and `Enter` reopens it. On a page with choices the digits pick, so `1` and `2` switch tabs only from the list or a page without choices.

The mouse does the same. A click on a choice picks it at once, as its digit does (a click on the text row opens the field). A click on a line that names a key (the task line, `c corrects`, `r resends`, a pending save) acts as that key, and every footer action is a button too (`↑↓ move` and `1-n pick` only name the keys). At 110 columns and up, a click on the previewed record opens it before it acts.

Once an answer, approval, rejection, or decision is saved, the page closes and the list comes back with the cursor in the same place, now on the next record still waiting; the status line names the save and its hint. A record opened from a task page returns to that task page instead. A save the store cannot prove durable stays on the page until `Enter` confirms it. Answered records fold into **CLOSED**, so `j`/`k` and `J`/`K` walk only what still waits; `z` shows or hides them for the session.

A page opens at its top. Scrolling never moves the highlight; moving the highlight or typing brings that line back into view above the `↓ n more rows` line.

An approval shows its exact terms (action, target, scope, revision, undo) above its choices. Picking **Approve these exact terms** approves the terms the page showed when it opened: tsk compares them with the record on screen and with a fresh read of the store, and saves only when the revision and every term still match. If anything changed, nothing is saved, the status row says so, and the page now binds to the terms it shows, so you read them before picking `1` again. **Add a comment…** (or `m`) opens an inline field for free text beside the decision: `Enter` or `Esc` keeps it on the row, and it rides whichever of approve or reject you pick next, saved with it in one answer. It is never itself an approval, is never checked against the approval terms, and is dropped if you leave the record without deciding. A saved comment shows on the page and in history beside the `approved` or `rejected` text.

A secret request names where its value goes: the fnox `key`, `profile` (`default` when none), and `config` (`global` when none). The page opens on a masked field: each character shows as `•`, a paste drops its line breaks, and `Esc` discards and wipes it (`Enter` opens a new one). `Enter` re-reads the request, checks that it is unchanged and its config is still a regular file, then runs `fnox set` with the value on standard input and fnox's output discarded. On success the answer is recorded as `set`, nothing else; the value is never saved, shown, or sent to the agent. When fnox fails, the status row says `fnox set failed` (with its exit code), nothing is saved, and the request stays open. A secret is never corrected; the agent asks again to replace it. Set `TSK_FNOX_BIN` to run a different fnox binary.

A finding offers **Create the task and run it**, **Create the task only** (Todo), or **Close**. Creating makes one new task in the finding's task scope (refused, with nothing saved, when that project is archived), titled with the finding's shareable summary (Doing to run it, Todo otherwise); the private detail is never copied. A correction or close never creates a second task. Create-and-run then sends an identifier-only request to the finding's `lead_recipient`, only when exactly one live Herdr agent has that name. The page reports whether the request and the origin-thread hint were sent, and why not: no recipient set, no live agent, several agents with that name, outside Herdr, or a failed send. `r` reads the record back before resending.

`c` on an answered question or finding shows its form again; the next pick or typed answer saves as a correction, and the earlier answer stays in history. A typed answer to a record that changed meanwhile is not saved and not moved onto the newer revision: the text stays on the page until `Esc` discards it. When the store cannot prove a save durable, the page keeps it and `Enter` confirms that exact revision. When a save's outcome is unknown, tsk reads the record back first and never retries on its own; it treats the visible answer as that save only when it matches what you chose, and otherwise sends no hint. While an answer is typed or a save is unresolved, quitting is refused on every tab until you save, confirm, or cancel it.

## Notices

Your first board open seeds four desk tasks with `N` ids (not `T`). They teach the board by being ordinary tasks: open, change status, archive, or delete. `ctrl+d`, `ctrl+f`, and `ctrl+x` all dismiss a notice the same way; that starter task never re-seeds. After an upgrade, one `What's new in tsk` task can appear the same way. Details and the delivery record live under [storage](/docs/storage/#starter-guides-and-release-notes). Starter notices keep their seeded statuses so the tour order stays useful; a new release notice lands in NEEDS YOU as `review`, so it sits where you look first.

## Mouse

| Action | Result |
| --- | --- |
| Click a tab or selector | Change view or open its choices |
| Click a task outside multi-select | Select it; in wide panes the detail beside the list follows it and the list keeps focus |
| Click a task or its `T`/`N` number in multi-select | Move the cursor there and toggle its mark |
| Double-click a task | Open it: full screen in narrow panes, focused in the detail column in wide panes |
| Click inside the detail column | Focus the detail |
| Click its `T` or `N` number | Copy that id |
| Click a footer action | Run that action |
| Wheel or drag a scrollbar | Scroll |
| Drag across text | Select and copy on release |

## Wide layout

At **110 usable columns** or wider, the board is always two columns: the list on the left and the selected task's detail on the right. Moving the cursor updates the detail. Only focus changes:

| Key | Action |
| --- | --- |
| `→` / `l` or `Enter` | Focus the detail of the selected task |
| `←` / `h` or `Esc` | Return focus to the list |
| `J` / `K` | On the detail or task page, open the next or previous task in list order |
| `P` / `U` | On the detail or task page, choose or clear the task's parent |

`J` and `K` skip group headings and follow the current scope, sort, search, and drawer. At either end they stay put and say `last task in this list` or `first task in this list`; if the open task has left the list, or a parent or child line opened a task the list does not show, they say `this task is not in the list · esc back`. `Enter` on a parent or child line of the detail opens that task without changing the list; `Esc` returns to the task before it. A task draft with unsaved changes refuses to switch until you save or cancel it.

The list keeps the done drawer and archived fold state, so a done task opened from the drawer stays selected beside its detail. While editing, arrows move the text cursor instead.

Narrowing the pane shows one surface, the one that had focus; widening it restores both columns. Each new session starts with list focus.

| Pane size | Layout |
| --- | --- |
| 110 columns or wider | List and detail side by side |
| At least 78×24, below 110 columns | Single board or task page |
| Smaller | Compact layout; usable down to 40×10 |

## Completed tasks

The **DONE** heading starts closed: it shows the count of done tasks in the current scope with `d to open`, and no done rows, so the cursor, marks, and `J` / `K` skip them. Press `d` or click the heading to open or close the done drawer. Select a done task and press `ctrl+o` to return it to Todo.

The drawer's **archived** group starts closed. Click its heading or press `Enter` on it to expand. `g` folds or unfolds the group while the drawer is open.

## Archive

Archiving hides work without changing its status.

| Archive | Restore |
| --- | --- |
| Task: select it and press `ctrl+f` | Open the done drawer's archived group, select it, then press `ctrl+u` or `ctrl+f` |
| Project: press `p`, select it, then `ctrl+f` | Switch to the picker's **archived** tab, select it, then `ctrl+u` or `ctrl+f` |

Use `Tab`, `←`, or `→` to switch project-picker tabs. Archiving has no undo entry.

### View an archived project

Press `Enter` on a project in the picker's **archived** tab. Its tasks open read-only. Press `ctrl+u` to restore the project, or `Esc` or `p` to leave.

Archived projects are excluded from task-destination pickers. Restoring a project leaves individually archived tasks archived.

### Launch inside an archived project

tsk asks whether to restore it. Choose `y` to restore and open that project's board, or `n`/`Esc` to keep it archived. Keeping it archived sends new captures to your desk for that session.

## Delete and undo

Press `ctrl+x` twice to delete the marked tasks, or the cursored task when nothing is marked. The confirmation and recovery messages show the task count for a marked set. `ctrl+Delete` is an alternative.

One `ctrl+u` undoes the whole marked deletion or completion. If another writer has changed any task in that batch since the action, undo refuses without changing any of them and remains available to retry. On an archived cursor with no marks, `ctrl+u` restores that task instead.

Deleted tasks remain available through [trash commands](/docs/cli/#trash) for a limited time. They do not appear on the board.

## Palette

Press `:` on the board or a task page and type to find an action. Use arrows or `Tab` to select, `Enter` to run, and `Esc` to close.

| Available actions | When |
| --- | --- |
| New task, undo, done drawer, help, quit | Always |
| Set todo/doing/done, edit notes, change scope, delete | A task is selected |
| Move step up, move step down | A task-page step is selected |
| Retry save, cancel save | A save has failed |

Search matches letters in order: `ssd` finds `set status: doing`.

## Help

Press `?` on the board, task page, or another non-text surface to open the searchable shortcut reference. A dim divider separates its focused search field from shortcuts, grouped one binding per row by function. Long descriptions wrap beneath the description column. The card uses at most half the terminal height, except below 15 rows where it may grow to six rows so one result stays visible. Type or paste to filter by a key, action, group, or related term; use arrows, page keys, or the wheel to scroll. `Esc` clears a nonempty search first, then closes Help.

When a text field already owns input, `?` remains text instead of opening Help.

## Save failures

If saving fails, the draft stays open.

- `r` or `Enter`: retry.
- `c` or `Esc`: cancel the pending change.

Resolve the failure before making another change. See [storage](/docs/storage/) for directory and backup information.
