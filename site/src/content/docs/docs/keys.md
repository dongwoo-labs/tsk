---
title: Keys
description: Every keyboard shortcut, by surface.
---

Every key tsk binds, by surface. Press `?` on the board or the task page for the same list, searchable. Clicks are on [Mouse](/docs/board/#mouse).

Press `?` on the board, task page, or another non-text surface for a searchable list of all shortcuts. Status and delete shortcuts use **Ctrl**; navigation uses bare keys. The exceptions are listed below: bare `e` / `a` on the task page and the answer keys of the questions tab. The website demo uses bare status keys because browsers reserve control chords.

## Board

| Action | Key |
| --- | --- |
| Select previous / next task | `↑` / `↓` or `k` / `j` |
| Enter / leave multi-select | `Shift+M` |
| Mark current task, then move in multi-select | `Shift+↑` / `Shift+↓` |
| Mark / unmark cursored task in multi-select | `Space` |
| Open task (focus the detail at 110+ columns) | `Enter` |
| Focus detail / list at 110+ columns | `→` / `←` or `l` / `h` |
| Add task | `+` |
| Edit title | `ctrl+e` |
| Set Doing from Todo | `ctrl+s` |
| Set Todo, including reopening Done | `ctrl+o` |
| Mark done | `ctrl+d` |
| Delete, with a second press to confirm | `ctrl+x` or `ctrl+Delete` |
| Undo completion/deletion; restore an archived selection | `ctrl+u` |
| Archive / restore selected task (archived tasks live in the done drawer's archived group) | `ctrl+f` |
| Check stalled tasks: ask the Herdr lead about stalled and Todo tasks (inside Herdr) | `ctrl+l` |
| Open / close done drawer | `d` |
| Fold or unfold archived in the done drawer | `g` |
| Show / hide notes status lines (hidden on launch, session-only) | `v` |
| Board tab, list focus | `1` |
| Questions tab | `2` |
| Reports tab | `3` |
| Open this Task's completion report | `r` |
| Search current board rows | `/` |
| Command palette | `:` |
| All keys | `?` |

`ctrl+q` quits from task views, Help, and pickers too. It does not quit while editing text, using board search, typing a quick-add line, or filtering the palette. Save or cancel any unsaved draft first. During save recovery, resolve the pending save instead.

`Esc` leaves multi-select and clears its marks first, then closes the current layer. At the board root, with multi-select inactive, it quits without confirmation. In the wide layout, `Esc` from the detail returns focus to the list after any editor or overlay is dismissed. A further `Esc` at the list root quits, unless a parked draft blocks quitting.

`Shift+M` enters or leaves multi-select while the board owns input. While it is active, `Space`, shifted arrows, and a plain task click change marks instead of opening a task. Removing the last mark leaves the mode active. `Shift+M` again from the board, `Esc`, a task action, or a view change leaves it and clears the set. Text entry keeps `Shift+M` as a capital `M`; `Esc` leaves multi-select before cancelling that surface.

When tasks are marked, the status, delete, and archive shortcuts act on that set; without marks they act on the cursor. `Enter` and `ctrl+e` remain cursor-only. `ctrl+s` changes Todo tasks to Doing and leaves Doing tasks unchanged. Marked Done tasks are skipped, and the status row says so: `doing N · skipped M done (ctrl+o reopens them)`. `ctrl+o` sets Todo, including for Done tasks. `ctrl+b` and `ctrl+r` open the [block and review cards](#detailed-task-status-and-assignment). `ctrl+n` does nothing. One `ctrl+u` reverses an entire marked completion or deletion.

## Task page

In view mode, before you start an edit:

| Action | Key |
| --- | --- |
| Select the parent line, steps, **+ step**, children, questions, and links | `Tab` / `Shift+Tab` |
| Activate first step, then move among the parent, steps, and children | `↓`, then `↑` / `↓` |
| Toggle selected step | `Enter` |
| Open the selected parent or child; on `parent  none`, choose a parent | `Enter` |
| Add step (below the selected step, else at the end) | `ctrl+a` or `a` |
| Move selected step up or down | `alt+↑` / `alt+↓` |
| Edit title or selected step | `ctrl+e` or `e` |
| Open the selected question, show a link's URL, or focus a verified Thread | `Enter` |
| Questions tab | `2` |
| Reports tab | `3` |
| Open this Task's completion report | `r` |
| Change task status | Board status shortcuts above |
| Next / previous task in list order | `J` / `K` |
| Choose this task's parent | `P` |
| Clear this task's parent | `U` |
| Mark/remove selected step; otherwise confirm/delete task | `ctrl+x` |
| Command palette | `:` |
| All shortcuts | `?` |
| Back to the task a relation opened from; else close task (back to the list at 110+ columns) | `Esc` |
| Quit the board | `ctrl+q` |

Click a step to select it. Click **+ step** to add at the end. Click any row of a parent or child line to open that task. Field clicks become editable only after task editing starts.

`alt+↑` / `alt+↓`, the parent keys `P` / `U`, and `e` / `a` (which start an edit, as `ctrl+e` / `ctrl+a` do) are the task-page shortcuts without `ctrl`. macOS reserves `ctrl+↑` / `ctrl+↓` for Mission Control. `alt+↑` / `alt+↓` also works during task editing while a step is selected and no field is open. Your terminal must send `alt+↑` as one modified key (xterm's default); a terminal set to prefix Option or Alt with `Esc` delivers `Esc` and then the arrow, which closes the task page.

`P` and `U` act on the open task only, never on a marked set, and only while no text field is open; in a field they type capital letters. A draft with unsaved changes refuses both until you save or cancel it. `P` opens the parent picker; `U` clears the parent at once. Neither changes either task's status or scope. [Related tasks](/docs/task-page/#related-tasks).

## Editing

| Action | Key |
| --- | --- |
| Next / previous field | `Tab` / `Shift+Tab` |
| Save task edit | `Shift+Enter` |
| Cancel field | `Esc` or `ctrl+c` |
| Move from Title to Notes | `Enter` in Title |
| New line | `Enter` in Notes |
| Add another step | `ctrl+a` on the task page |
| Save new step and open next empty row | `Enter` in new step |
| Retain existing-step rename in session | `Enter` in existing step |
| Stage selected-step removal | `ctrl+x` in task edit |
| Open Scope picker / confirm selection | `Enter` |
| Cycle Scope | `Space` or `←` / `→` |
| Line start / end | `Home` / `End` |
| Word left / right | `ctrl+←` / `ctrl+→` |
| Delete backward / forward | `Backspace` / `Delete` |
| Move through wrapped notes | `↑` / `↓` |

- Keys inside the reply and feedback box: [Blocked](/docs/task-page/#blocked) and [Review](/docs/task-page/#review).
- Digits pick options on the task page. `1` `2` `3` switch views only on the board.
- `alt+↑` / `alt+↓` also work while editing, with a step selected and no field open.
- Your terminal must send `alt+↑` as one modified key, as xterm does by default. A terminal that sends Option or Alt as a leading `Esc` closes the page instead.

## Text editing

| Action | Key |
| --- | --- |
| Save the task edit | `shift+Enter` |
| Cancel the field | `Esc` or `ctrl+c` |
| Next / previous field | `Tab` / `shift+Tab` |
| Title: move to Notes | `Enter` |
| Notes: new line | `Enter` |
| Notes: move through wrapped lines | `↑` / `↓` |
| New step: save it and open the next row | `Enter` |
| Existing step: keep the rename in this edit | `Enter` |
| Add a step from a selected step, **+ step**, the Scope row, or a selected Thread | `ctrl+a` |
| Stage removal of the selected step | `ctrl+x` |
| Open the Scope, Assignee, Base, or After picker; confirm the choice. In the After picker, `Enter` pins and `shift+Enter` saves | `Enter` |
| Cycle Scope or Assignee without opening it | `Space` or `←` / `→` |
| Open / close the Thread editor | `Enter` |
| Line start / end | `Home` / `End` or `ctrl+a` / `ctrl+e` |
| Word left / right | `alt+←` / `alt+→`, `ctrl+←` / `ctrl+→`, or `alt+b` / `alt+f` |
| Delete a character backward / forward | `Backspace` / `Delete` |
| Delete a word backward | `alt+Backspace` or `ctrl+w` |
| Delete a word forward | `alt+Delete` or `alt+d` |
| Delete to line start / end | `ctrl+u` / `ctrl+k` |

In the step editor, `ctrl+d` and `ctrl+o` still act on the task, and `ctrl+x` stages the step's removal until the task edit is saved. In a new step's row, `ctrl+x` discards that row, as `Esc` does.

- Field order: [Edit](/docs/task-page/#edit).
- The word and line keys work in every text field: Title, Notes, steps, Thread, and quick-add.
- A word is a run of non-space characters, as with Option+arrow in macOS. In Notes, word moves cross line breaks and a word delete stops at one.
- In Notes, `ctrl+u` at a line start joins the line above, and `ctrl+k` at a line end joins the next line.
- Search, the palette, Help, and the pickers you type into have no cursor. There, `alt+Backspace` and `ctrl+w` delete the query's last word, and `ctrl+u` and `ctrl+k` do nothing. The assignee picker's `ctrl+u` is the exception: it puts back a removed name.
- Outside text fields, `ctrl+u` is undo.
- Paste keeps line breaks in Notes and turns them into spaces in one-line fields.
- On macOS, `Cmd+←` and `Cmd+→` arrive as `ctrl+a` and `ctrl+e`, and `Cmd+Backspace` as `ctrl+u`.
- On macOS, Option reaches tsk only when the terminal sends it as Meta. Turn on Terminal.app's **Use Option as Meta key**, or set iTerm2's Option key to **Esc+**. If Option+B types `∫`, use `ctrl+←`, `ctrl+→`, and `ctrl+w`.
- On Windows, AltGr letters type as characters.

### If shift+Enter does not save

Some terminals send a plain line feed for `shift+Enter`, such as Ghostty with `keybind = shift+enter=text:\n`. tsk reads it as `ctrl+j` and changes nothing. In a form or the After picker it shows `Shift+Enter sent Ctrl+J: check terminal keys`. Remove the binding, or map `shift+Enter` to a sequence that keeps the Shift modifier.

## Questions tab

| Action | Key |
| --- | --- |
| Move in the list | `↑` / `↓` or `k` / `j` |
| Open a record | `Enter` (also `→` / `l` at 110+ columns) |
| Back to the list | `Esc` (also `←` / `h` at 110+ columns) |
| Quit from the list | `Esc` |
| Move the highlighted choice | `↑` / `↓` or `k` / `j` on the record |
| Pick the highlighted choice | `Enter` |
| Pick a choice by number | `1`–`9` |
| Open the `Type something…` field, or a secret's field | type on that row, or `Enter` |
| Save a typed answer, or hand a secret to fnox | `Enter` in the field |
| Leave a field (the draft stays; a secret is wiped) | `Esc` in the field |
| Approve / reject an approval | `1` / `2` |
| Close an open secret with no value | `2` |
| Write a comment on an approval (`Enter` answers with it alone, `Esc` keeps it for approve or reject) | `3` or `m` |
| Correct an answer | `c` |
| Open the task | `t` |
| Focus the record's origin thread in Herdr (one live agent with that exact name) | `o` |
| Resend a lead request | `r` |
| Show or hide answered records (CLOSED) | `z` |
| Next / previous record | `J` / `K` |
| Scroll a record page | `PgUp` / `PgDn` |
| Board tab | `1` (from the list, or a page without choices) |

Clicks follow the keys: a click on a choice picks it at once, and a line or footer entry naming a key acts as that key (`↑↓ move` and `1-n pick` only name the keys).

These bare keys save answers on purpose: the tab owns the keyboard, and a pick submits at once, the way Claude Code's answer form does. Approve saves only the exact terms the page showed; if the record changed, it refuses and shows the new terms. On the `Type something…` row letters type (`j` / `k` / `J` / `K` and digits keep their meaning). While a field is open, every key is text, `Enter` saves, and `Esc` leaves the field; `ctrl+q` and `ctrl+c` do not quit. A secret's value types the same way but shows only `•`, and `Enter` runs `fnox set` instead of saving text.

## Reports tab

| Action | Key |
| --- | --- |
| Select a document in the list | `↑` / `↓` or `k` / `j` |
| Open the selected document and acknowledge its displayed revision | `Enter` |
| Scroll an open document | `↑` / `↓`, `k` / `j`, `PgUp` / `PgDn`, or wheel |
| Show an earlier or later revision of the open document | `[` / `]` |
| Back to the list | `Esc` |
| Board / questions / reports tab | `1` / `2` / `3` |
| Quit outside text entry | `ctrl+q` |

A new report revision is unread even if an earlier revision was opened. Selecting a row, showing an earlier revision, and CLI lookup do not acknowledge it.

## Quick-add

| Action | Key |
| --- | --- |
| Save and close | `Enter` |
| Save and keep adding | `shift+Enter` |
| Open notes and details | `Tab` |
| Cancel the line, or return from details | `Esc` |

In the Herdr popup, `shift+Enter` saves and closes it. `Esc` or `ctrl+c` discards the popup, after first closing an open step editor or picker. [Capture](/docs/capture/).

## Pickers, search, palette, help

| Surface | Keys |
| --- | --- |
| Parent picker (`P`) | Type or paste to filter by number or title; arrows or `Tab` select, and on a long wrapped title `↑` / `↓` first show its other rows; the wheel scrolls rows; `Enter` saves the parent (or **no parent**); a click on any row of an option saves it; `Esc` closes without saving |
| Board search | Type or paste; `Backspace` edits; `Enter` pins; `Esc` clears and closes |
| Palette | Type to filter; arrows or `Tab` select; `Enter` runs; `Esc` closes |
| Help | Type or paste to filter by key or action; arrows, page keys, or wheel scroll; `Esc` clears the search, then closes |
| Archived-project launch prompt | `y` restores; `n` or `Esc` keeps archived; `?` opens Help |
| Failed save | `r` or `Enter` retries; `c` or `Esc` cancels |

`j` and `k` are text in board search, the parent picker, and Help search, not navigation. `?` and `/` are text in every input field, including Help search.

## Wide layout

At 110 usable columns or wider, the list and the selected task's detail always sit side by side. These keys move focus when you are not editing:

| Focus | `→` / `l` | `←` / `h` | `Enter` | `Esc` |
| --- | --- | --- | --- | --- |
| List | Detail | No change | Detail | Quit at root |
| Detail | No change | List | List | List |

On the detail or a task page, `J` / `K` opens the next or previous task in list order. `Tab` navigates task fields or steps; it does not switch focus.

## Esc and quit

[Board views and mouse behavior](/docs/board/#wide-layout).

## Detailed task status and assignment

| Action | Key |
| --- | --- |
| Block or unblock | `ctrl+b` |
| Send to review or back to ready | `ctrl+r` |
| Assign a task | `@` |

These actions use upstream's [block and review cards](/docs/board/#block-review-reply). They do not answer or approve an Agent question.
