# Changelog

One `## vX.Y.Z` section per release, newest first, with `## Unreleased` on top. Inside a
section the subsections are, in this order and only when non-empty: `### Breaking`,
`### Added

- A title over 60 characters no longer needs shortening by hand: `tsk add`, `tsk edit --title`, and JSON plan items cut it at a word end, add `…` (60 characters at most), and keep the full title at the top of the notes. Nothing is refused.
, `### Changed`, `### Fixed`. Every user-visible change lands here in the PR
that makes it, written for a user, not a contributor: omit demo alignment, CI wiring,
review history, and other maintainer-only work. On release the version's section becomes
the GitHub release notes verbatim.

## Unreleased

### Breaking

- The board has one **board** tab that always shows every live project: Doing and Todo group their rows by project, desk first, then each project under a dim heading, in task-number order inside each group. The top row holds only `1 board` and `2 questions`. The scope picker (`p`), the **desk** / project / **projects** tabs, `3`, the Projects overview and its preview, the cross-project thread view (`v`), the narrow-pane peek, and the four-stage wide slider are gone, and there is no sort toggle (`S`) or sort control: grouping is permanent. `1` returns to the board tab and its list, and `2` opens the questions tab. Launching inside a Git repository no longer opens that project alone; the board shows everything and the cursor starts on that project's first row.
- The board no longer archives or restores projects: the picker's **archived** tab, its `ctrl+f` / `ctrl+u`, and the read-only archived-project view (`Enter`, `Esc`, `p`) are gone. Use `tsk project archive <name or path>` and `tsk project unarchive <name or path>`. Archiving a task with `ctrl+f` and the done drawer's archived group are unchanged, and a task inside an archived project reached from a related-task line still opens read-only.
- Task progress is now Todo, Doing, or Done, with strict CLI values `todo`, `doing`, and `done`. Store v7 maps open/ready to todo and started/blocked/review to doing, preserving history, undo, steps, archive, and parent links. An older binary refuses v7; rollback requires the original `tsk.json.v<N>` plus `trash.jsonl.pre-v7` after trash rewrites. Question waiting is separate from human progress.

- Tasks no longer carry a thread label. Store v8 drops it from every live and trashed task on load; the first save keeps the v7 file as `tsk.json.v7`, and the first trash rewrite keeps the old trash as `trash.jsonl.pre-v8`. `--thread`, the `tsk list` thread filters and `thread` field, the JSON plan `thread` key (now an `invalid-item` refusal), the board's thread filter and thread field, and quick-add `!t` are gone. A v7 binary refuses a v8 store; rolling back needs those two backups. Name the working Thread in the task notes instead (see Added). Question records with an `audience`, a route, or a finding action are refused by an older binary, which reads them as unavailable rather than guessing.

### Added

- Tasks record their creation machine. The board defaults to current-machine and unspecified tasks; machine filters and CLI assignment expose other machines without changing existing tasks.
- Permission questions can carry optional private `permission_context` metadata (working directory, tool input, Task scope snapshot, and explicit notes Holds). `tsk questions --json` preserves it; existing questions without it stay readable. It grants no approval authority, and callers must keep authentication values out of question text and metadata.
- `tsk notes T12 set Now: "Running CI"`, `remove Blocked:`, and `append "PR: <URL>"` update notes one line at a time under the store lock without overwriting other lines. Set replaces the first matching prefix and removes duplicates; remove deletes every match; append adds a line and is not safe to repeat blindly. `tsk list T12 --field id` and `--note Now:` return one value without a Task-row array; `--json` returns a single JSON value, with `null` for an absent value. Existing unprojected list output is unchanged. When the saved notes pass 4 KB, `tsk notes` and `tsk edit --notes` still save and exit 0, and print `tsk <command>: warning: T<n> notes are <size> KB, over 4 KB; …` on stderr: notes are a status card, not a log.
- `tsk secret-result <Q> --file <JSON|-> --state-dir <Agent-dir>` records only `set` after an external secret-input interface stores a value in fnox. Its input is exactly `{"expected_revision":"<UUID>","result":"set"}`; values and other fields are refused. Tsk checks the stored target's effective key name with fnox's name-only listing, then saves the same fixed answer as the board under revision CAS. A missing key, failed check, stale revision, or closed question saves nothing. This checks a name, not value validity or a fresh write, and does not lock or undo the external fnox write. Existing `answer` and `ui-answer` restrictions remain unchanged.
- Completion reports are independent Markdown documents attached to Tasks: register with `tsk report T12 --file completion.md --json`, read with `tsk report T12 --json`, and list with `tsk reports --unread --json`. The **3 reports** tab keeps unread documents first, including reports on older Done tasks; `Enter` opens rendered Markdown and acknowledges only that revision, while CLI reads and selecting a row do not. Changed text appends a revision and becomes unread again; identical text keeps its revision and read state. A report marker on Task rows and `r` on a Task or clean task page open its report directly. Reports use OSS Markdown rendering and scrolling, stay separate from private questions, and never change Task status or grant approval. A document that wraps past the viewer's 65535-row limit shows a refusal instead of a truncated body and is not acknowledged; CLI JSON keeps the full original.
- `[` / `]` on an open report show its earlier and later revisions, read-only. Showing an earlier revision does not mark the report read.
- `o` on a questions-tab record focuses its origin thread in Herdr, including closed and lead-addressed records. Only one live agent with the exact origin name can be focused; the status row reports success or why nothing was focused.
- `tsk add --after <task>` and `tsk edit --after <task>` (repeat the flag for several, `--no-after` clears) make a task wait for other tasks to be done. `tsk status <task> doing` refuses with `waiting-for-after` until every predecessor is done, `tsk list` shows `· after T<n>` for the ones still open, and a missing, deleted, or circular `--after` refuses (`invalid-after`, `after-cycle`). It is a condition only: nothing starts by itself, and parents and children stay unordered. Store v10 adds the optional `after` list and changes no existing data; a v9 binary refuses it. A completed predecessor a live task still waits on stays in `tsk.json` past the usual retention window, the same way a parent with a live child does. `tsk edit --no-after --after <task>` refuses the conflict in either flag order.
- `tsk.json` stays small: a done task moves out of it one day after it is finished, into `done/YYYY-MM-DD.jsonl` by its UTC completion date (`TSK_DONE_KEEP_DAYS` changes the day count). The board's done drawer shows the tasks still in `tsk.json`; `tsk list --done --history` adds the moved ones, and `tsk list T<n>` still finds them by number. A task an undo entry, an open question, or a live child still needs stays put; once its questions are answered or closed it moves, and those questions still name it: the questions tab shows it as moved to done history, `t` points at `tsk list T<n>`, and `tsk questions --task T<n>` still finds it. Moved tasks are read-only for now.
- Stall checks inside Herdr. A Doing task whose single `Thread:` line names an agent that is not working (or no longer live), and that has not changed for 30 minutes (`TSK_STALE_MINUTES`), shows a bold `↻` at the right of its row. Click it, or run the palette's **check task's thread**, to ask that thread to update the task's status card. The top row's **↻ check** button, `ctrl+l`, or the palette's **check stalled tasks** asks the one live `lead-` agent to check every stalled task and to start a thread for each Todo task that can proceed without asking you. The board polls Herdr in the background, writes nothing to your tasks, never changes a status itself, and says on the status row what it sent or why it sent nothing.
- Every open question, approval, and finding in the questions tab ends with **Explain it first**. It answers nothing: inside Herdr it sends the one live `lead-` agent the single line `/wait-what Q<n>`, so its wait-what skill explains the question, and you answer the record afterwards.
- A question option can carry a description: write it as the label, a newline, then what picking it leads to. The form shows the label as the choice and the description dim beneath it, and picking it (on the board, or `tsk ui-answer`'s `choice`) saves the label.
- Notes and question records read as Markdown. The task page, the wide detail, and every questions-tab record (question text, finding detail, impact, approval terms, answers, comments, and history) now hide heading `#` markers and render numbered lists (dim number), `- [ ]` / `- [x]` as `☐` / `☑`, `>` quotes with a dim `│` bar, `---` rules across the width, `[text](url)` links as underlined text with the URL dim after it, `<url>` autolinks, `~~struck~~` text (dim), backslash escapes, nested list indentation, and simple pipe tables with dim `│` separators. Text wraps on what is displayed, so a wrapped list item or quote keeps its indent or bar. Editing still shows the original text.
- On an open approval, **Add a comment…** (or `m`) writes a free-text comment in an inline field. `Enter` answers the approval with the comment alone, saved as `commented` with nothing approved, so you can reply in words without approving or rejecting. `Esc` keeps it on the row instead, and it is saved with whichever of approve or reject you pick next, in that one answer. A comment is never itself an approval, is never checked against the approval's target/action/H/scope/rollback terms, and never weakens what approving requires. It shows on the page and in history beside `approved`, `rejected` or `commented`. `tsk answer` accepts the same optional `comment` beside a decision, and `tsk ui-answer` also takes a `comment` alone on an approval; both refuse it (as `invalid-input`) on anything but an approval or when blank.
- Secret questions. An agent that needs a key asks with `"kind": "secret"`, `"audience": "user"`, and `"secret": {"key", "profile", "config"}` (a fnox key, an optional profile, and an optional absolute `fnox.toml` path; none means global). The questions tab shows the target and opens on a masked field (`•` per character), and `Enter` runs `fnox set` with the value on its standard input only, then records the answer `set`. The value is never saved, shown, or printed: not in the store, the private snapshots, history, arguments, or any command's output. A fnox failure shows `fnox set failed`, saves nothing, and keeps the request open. A secret no longer needed is closed with no value instead: its form's **2. Reject** row (or `tsk ui-answer`'s `reject: true`) records `rejected`, with no `fnox set` call and dropping any value typed but not yet sent. `tsk answer` always refuses a secret question with `secret-needs-board`, even a reject-shaped body; only the board can set the value, and only the board or `tsk ui-answer` can reject it; `tsk route` keeps it with you, and it is never corrected, set or rejected. `TSK_FNOX_BIN` picks another fnox binary. The store moves to v9 with no data change, so a v8 binary refuses it; the first save keeps the v8 file as `tsk.json.v8`.
- `tsk edit T12 -p <project>` moves a task to another project, and `tsk edit T12 --desk` moves it to the desk. The destination resolves like `tsk add -p`; an archived project refuses with `project-archived`. The task keeps its number, status, notes, steps, and parent.
- The questions tab (`2`) on an explicit Agent board. Open questions waiting on you are grouped as **NEEDS APPROVAL**, **FOR YOU**, and **FINDINGS**; **WITH LEAD** and **CLOSED** follow, and a record with no target shows apart as *target unknown* instead of being guessed and takes no answer or correction until it is routed. `Enter` opens one (a full page under 110 columns, list and detail side by side at 110 and up); a record is answered with a numbered choice form (see Changed), `c` corrects an answer as a new revision, `t` opens the question's task, and `J` / `K` step through the list. A page opens at its top, and `PgUp` / `PgDn` or the wheel scroll it without moving the highlight. A finding offers **create the task and run it** (a Doing task), **create the task only** (Todo), or **close**; the new task takes the finding's shareable summary and its task's scope, never the private text; it is refused, with nothing saved, when that project is archived, and a correction or close never creates a second one. Create-and-run sends an identifier-only run request to the finding's `lead_recipient`, only when exactly one live agent has that name; `r` resends it after reading the record back. A typed answer refused because the record changed stays on the page, unsaved and never retargeted, until `Esc`. A save the store cannot prove durable stays on the page until `Enter` confirms that exact revision, and an unknown commit is read back first and counts as your save only when the record shows exactly that answer; nothing retries on its own. While an answer is typed or a save is unresolved, quitting is refused on every tab. The mouse works on a record too: click a choice to pick it, click the task line, `c corrects`, or `r resends` to act as that key does, and click a footer action. At 110 columns and up a click on the previewed record opens it first. Saving an answer returns to the list with the cursor on the next record still waiting (or to the task page it was opened from), and **CLOSED** is folded until `z` or a click on its heading shows it. A record reads in blocks: field labels are dim beside their values, the agent's original text sits indented under its heading, the approval's action, target, and revision are bold, and blank lines separate impact, terms, and the choices. The personal board says it records no Agent questions.
- The board shows `waiting on you  approval n · question n · finding n  → 2` above the sections when questions wait on you (a click opens the tab), plus `target unknown n` for records with no target, and rows end in a bold `!` (an approval waiting on you) or `?n` (questions and findings waiting on you). Lead questions and records without a target are not in the tab number or the row marks.
- The task page lists the task's questions and findings, and the reference lines in its notes: `Linear: <ID> <URL>`, `PR: <URL>`, `CI: <URL>`, and `Thread: <agent-name> <workspace-id> <tab-id> <pane-id>`. `Enter` on a question opens it on the questions tab (`Esc` comes back), on a link shows its URL, and on a Thread line focuses that pane only when the line is the task's only Thread line and all four tokens match one live Herdr pane; a name-only, partial, duplicated, or unmatched line says it cannot be verified and focuses nothing.
- `tsk route <Q> --file <JSON>` changes a question's `audience` (`user` or `lead`) and, optionally, its `lead_recipient` under the same Q number, given the current `expected_revision`. Answers, text, and history stay; an answered question stays answered. `tsk ask` accepts the same two fields.
- On the task page, bare `e` edits the title and notes and bare `a` adds a step, beside `ctrl+e` and `ctrl+a`.
- Progress at a glance on board rows. A task with steps ends in a dim `✓done/total` count, such as `✓2/5`, before the `⊂done/total` child count when both exist (`✓2/5 ⊂1/3`). Under the title, a row shows one status line from the task's notes: a `Blocked: …` line on any task not done, in bold as `blocked: …`; otherwise a `Now: …` line on a Doing task, dim. Keys are case-insensitive, a leading `- ` is allowed, and only the first matching line counts; Todo tasks show no `Now:` line and done tasks show neither. The docs recommend a notes status card: `Now:`, `Next:` (optional), `Blocked:` (only while blocked), and `Done when:` first, each replaced in place, then reference lines such as `PR:` and `Thread:`. The agent skill follows it.
- At 110 columns or wider the board is always a list beside the selected task's detail. `→` / `l` or `Enter` focuses the detail, `←` / `h` or `Esc` returns to the list, and moving the cursor updates the detail.
- `J` / `K` on the detail or task page opens the next or previous task in list order. A draft with unsaved changes refuses to switch.
- Threads pane. `tsk threads` (or the **Open tsk threads** Herdr action) lists the live `lead-` and `thr-` agent sessions grouped only by Herdr execution status as BLOCKED, IN MOTION and ON DECK, with each one's status, how long it has held, and optional inbox metadata as an annotation. Neither blocked execution nor inbox text indicates a wait for the user. `Enter` focuses the selected session's tab and pane; `tsk threads --json` and `tsk threads focus <name>` do the same headlessly. It only reads Herdr and never reads or writes the task or question store.
- Lightweight task relationships from the CLI: `tsk add --parent T12`, `tsk edit T13 --parent T12`, and `tsk edit T13 --no-parent` set, replace, or clear one direct parent. `tsk list T12` shows direct children and their Done/total count; JSON includes stable parent UUIDs. Self-parenting and cycles are refused. Parent Done, archive, or deletion never changes children, and missing/deleted parent links stay unresolved rather than being reassigned. Linear imports keep a `Parent: Linear <identifier> <URL>` Notes reference and link an already imported parent or one created in the same batch, without recursive import or later reconciliation.
- Task relationships on the board. On the task page or the wide detail, `P` opens a parent picker (type to filter, `Enter` saves, `Esc` closes) and `U` clears the parent; both act on the open task only, save at once, and refuse while a draft has unsaved changes. The page shows the parent line before the steps (or `not found · link kept` for a deleted or missing parent, whose id stays), and the direct children with a done count after them. `Tab`, `Shift+Tab`, and the arrows also select the parent and child lines; `Enter` or a click opens that task without changing the list's search or cursor, `Esc` returns to the previous task, and `Enter` on `parent  none` opens the picker. A task in an archived project opens read-only. Long titles wrap in the parent picker. A task under the open one is listed but refused, since it would form a cycle. Rows of tasks with children end in a dim `⊂done/total` count. Nothing changes either task's status, scope, or children.
- `tsk ui-answer <Q> --file <JSON|->` records the answer a user gave in another interface, such as a Claude Code ask prompt, with origin `user_ui`: an option `choice`, typed `text`, `approve` with the exact stored approval terms, or `reject`, given the `expected_revision` the user saw. It goes through the same revision check, approval check, and history as the questions tab; a plain answer never becomes an approval, and a finding stays a board decision. It prints one JSON line and refuses with a stable code such as `stale-revision` or `approval-refused`. Agents never call it: any process running as the same OS user can, so keep it out of agents' tool calls with a hook. `tsk answer` still refuses `user_ui`, and every question command now reads `--file -` from stdin.
- Task-linked questions on an explicit non-personal Agent board: `tsk ask --file`, `tsk questions`, and `tsk answer --file` record questions, approval requests, findings, replies, and append-only corrections. Full text and approval evidence stay in Git-excluded local private snapshots; the shared board holds only identifiers and a shareable summary. Replies require the current Q revision, and approval requires confirmed user evidence matching the exact terms. Identifier-only Herdr notifications are separate from saved replies: failed delivery keeps the answer, and an uncertain save requires readback before retrying.
- `tsk import linear` imports your Linear issues as tasks, one way. Set `LINEAR_API_KEY`, then filter with `--team`, `--linear-project`, `--assignee me|<email>|any` (default `me`), `--state` (Linear state types, default `backlog,unstarted,started`) and `--limit` (default 100), and choose the destination with `-p` or `--desk`. Each task is titled `<IDENTIFIER> <title>` and its notes start with a `Linear: <IDENTIFIER> <url>` line, which keeps a rerun from adding the same issue twice. Started states map to `started` (or `review` when the state name says review), completed to `done`, and canceled issues are skipped. Existing copies stay as you left them unless you pass `--update`; `--dry-run` shows what would change and `--json` prints a report. macOS and Linux only for now.
- Reorder steps. On the task page, `alt+↑` / `alt+↓` moves the selected step (also **move step up** / **move step down** in the `:` palette, which now opens on the task page too), and `ctrl+a` with a step selected adds the new step directly below it. From the CLI, `tsk steps T12 move <short-id> --before|--after <short-id>` or `--first`/`--last`, and `tsk steps T12 add "…"` with the same place flags. A moved step keeps its short ID and checked state. Thanks @LukasGold (#159).

### Changed

- Board lists now use one line per task by default: number, title, and progress counts. A bold `×` marks a non-empty `Blocked:` notes line without showing the reason; open the task with `Enter` to read it. Press `v` with list focus to show or hide the second `Now:` / `Blocked:` status line for the current session; every launch starts with it hidden.
- Board-list titles and status lines each stay on one line, shortened with `…` when they do not fit. Open a task with `Enter` to read its full title and notes with wrapping.
- The stall-check keys answer at once. `ctrl+l`, **↻ check**, and the palette's **check task's thread** paint `Checking stalled tasks…` or `Checking…` on the status row before they ask Herdr, so a slow Herdr no longer leaves the board looking frozen. The result message replaces the phrase when the call ends.
- Project group headings on the board are bold and coloured, each project in one of five terminal colours fixed by its path, so neighbouring groups are easy to tell apart. The desk heading stays uncoloured; set `NO_COLOR` to turn the colours off.
- `tsk ask` names every bad field in one refusal and saves nothing: no Q number, record, or notification. A wrong shape exits 1 with `invalid question: approval.h: must be a hex Git SHA (40 digits) or plan digest (64 digits); approval.target: must not be blank; ...` (a kind approval needs `approval` with non-blank `target`, `action`, `scope`, `rollback` and a hex `h`; other kinds take none). A JSON error (an unknown `kind` such as `decision`, an unknown or missing field) exits 2 and now carries serde's reason, for example ``unknown variant `decision` ``.
- The questions tab answers every record with one form, like Claude Code's answer form. A record page shows the question, then a numbered choice list with the highlight on the first choice (or the recommendation): `↑` / `↓` or `j` / `k` move it, `Enter` picks the highlighted choice at once, `1`–`9` pick by number, and a click on a choice picks it. There is no separate choose-then-save step. A question with options ends with `Type something…`: typing on that row (or `Enter`) opens an inline field, `Enter` saves the text, and `Esc` leaves the field with the draft kept. A question without options and a secret open on one focused field. An approval lists `1. Approve these exact terms`, `2. Reject`, and `3. Add a comment…` (also `m`) below its exact terms; Approve saves only when the terms and revision the page showed still match the record and a fresh read, and otherwise refuses and shows the new terms. A finding lists `1. Create the task and run it`, `2. Create the task only`, and `3. Close`. The `Space` check and the bare `y` / `n` keys are gone, and on a page with choices `1` / `2` pick rather than switch tabs.
- The task page's step heading adds the percent done once the task has steps, such as `steps 1/3 · 33%` (rounded down), and `tsk list T12` opens the steps block with `Steps: 1/3 done (33%)`. The count is read from the steps, never saved, and never completes the task.
- Board rows show the number and title, then the question marks and the dim progress counts (see Added); the section already names the status. Tabs read `1 board` and `2 questions`. A task's project appears in its detail.
- Board sections list rows by task number. A status change moves a task to its new section without putting it on top.
- The **DONE** heading stays on the board while the done drawer is closed, with its count and `d to open`; its rows still appear only in the open drawer.
- Quick-add (`+`) goes to the project you launched the board in when that project is live, and to your desk otherwise; a `!p` token still overrides. Restoring an archived launch project with `y` puts the cursor on that project's first row.

### Fixed

- `tsk list --archived` includes archived tasks already moved to dated `done/` files, including tasks in archived projects. They remain excluded from `tsk list --done --history`; direct lookup and stored tasks are unchanged.
- `tsk ask` now refuses a question with no `audience` (`user` or `lead`): exit 1, nothing saved, and the error says to add it. Before, such a question reached the board with no target. Questions already saved without one are unchanged and still read and answer as before (route them with `tsk route`).
- `tsk ask` now refuses a `kind: "approval"` question that carries `options`: exit 1, nothing saved, error `options: kind approval takes none`. The board paints Approve, Reject, and Comment for an approval, never options, so a numbered pick used to arrive as a Reject. Approvals already saved with options are unchanged.
- `tsk questions` and `tsk questions <Q>` no longer refuse the whole read with `referenced private question record is missing or corrupt; mutation refused` because one approval record saved with options before the rule above existed. That rule now applies only to new `tsk ask` input; a record saved before it stays readable, keeps its options, and no longer drags every other record in the store down with it.
- **↻ check**, `ctrl+l`, and **Explain it first** no longer say `no live lead` when the lead was started by hand: Herdr lists such a lead without a name, so the board now also recognises the one agent whose terminal title starts with `lead-` and sends to its pane. When no lead is found, the status row names the agents Herdr listed.
- Copying a drag selection on the board no longer puts a space after every Hangul, CJK, or other double-width character (`테 스 트`): the copy reads each wide glyph once instead of also reading the blank cell it covers.
- At 110 columns or wider, a long task title wraps onto more rows in the detail's header, while viewing and while editing it, instead of being cut off with `…`. The status and project stay on the first row.
- At 110 columns or wider, the Task view's narrow board rail keeps an open done drawer and the archived group's fold state, like Split does, so a done task opened from the drawer stays selected beside its page instead of vanishing from the rail. Thanks @LukasGold (#174).
- A form editor that receives `Shift+Enter` as `ctrl+j` (a terminal binding that sends a plain line feed, such as Ghostty's `shift+enter=text:\n`) says so on the status row instead of ignoring the key. Thanks @WAWLixiong and @Rag0n.
- `tsk setup herdr` follows a symlinked Herdr `config.toml` (or `herdr` config directory), as dotfiles managers create, instead of failing with `Too many levels of symbolic links`. It edits the target in place, keeps the link, and backs up beside the target; a dangling or looping link is refused with a clear message.
- `ctrl+s` on a marked set that includes done tasks says how many it skipped and how to reopen them, instead of skipping them silently. Thanks @LukasGold.
- Help (`?`) says archived tasks live in the done drawer's archived group, and lists `g` as fold / unfold group. The keys page says which group: the inbox, or the archived group while the done drawer is open. Thanks @LukasGold.
- The task page's project chooser lists options top to bottom, so `↓` moves the highlight down instead of up.
- Windows: `tsk update` and the board's update check work instead of crashing on the first HTTPS request, so update notices appear. A failed check stays silent.
- `tsk add --file` and piped JSON plans accept a leading UTF-8 byte order mark, as Windows PowerShell 5.1 writes by default.
- Windows: characters typed with AltGr, such as `[ ] { } @` on a German layout, insert in every text field (quick-add, task editors, search, the palette, Help, and pickers) instead of being dropped. Thanks @LukasGold (#178).
- `tsk list` orders tasks within each status like the board: ready by oldest pick, open by oldest capture, everything else by most recent status change. `--json` is that flat board order. With `--all`, human output still groups each status by scope, ordering the groups by their first row and keeping the board order inside each group. Thanks @dangerousdan.

- Question commands also refuse case variants of personal-board roots and their child paths before those roots exist. Existing question records and trash backups use non-truncating write-capable sync handles, as Windows requires.

## v0.13.0

### Breaking

- Store format 6: assignees, dispatch, blocks, reviews, after, paper trail, wider undo. Older tsk refuses it; to roll back, restore the `tsk.json.v5` copy.

### Added

- Agent profiles in `config.toml`. The first board open writes a commented starter with examples for Claude Code, Codex, Pi, and Grok.
- Agent prompts: `prompt_append` adds to the default prompt, and `[project."<path>"]` adds per-repository lines and `review_on`. `tsk dispatch T12 --show-prompt` previews it.
- Assign a task to an agent profile, yourself, or a named person with `@`, `!a name`, or `--assignee`. Only an agent dispatches. `v` on Projects shows one assignee's tasks.
- Dispatch: in Herdr, starting an assigned project task (`ctrl+s`, `tsk status T12 started`, `tsk dispatch`) launches its agent in its own Git worktree and workspace. Windows: preview.
- Set the branch a dispatch starts from with `!b branch`, the task page's Base field, or `tsk add`/`tsk edit --base`; otherwise it is the repository's default branch.
- A dispatched task whose agent is gone asks before relaunching in the same worktree (`--again` in the CLI). Starting a marked set opens one card listing what launches.
- Completing a dispatched task offers to remove its worktree and merged branch; with `gh`, squash and rebase merges count. `tsk clean` does the same from a script.
- `m` on the cleanup card merges the task's pull request with `gh`, then completes the task and cleans up. `[merge] method` in `config.toml` picks the method; CLI: `--merge`.
- Block with a reason: `ctrl+b` asks why and what you need. Answer on the task page, under the row (`r`), or with `tsk reply`. Unblocking sends replies to the agent.
- Review with checks: `ctrl+r` asks what was done and what to check. Mark checks, give feedback (`r`, `tsk reply`), then approve (`ctrl+d`) or send back (`ctrl+s`).
- A block or review waiting on someone else moves to IN MOTION as `□` or `△` with `waiting on @name`, so NEEDS YOU holds only yours. Thanks @joops (#171).
- A started task with a live dispatched agent shows `◉`.
- Change who a block or review waits on, keeping its replies: `w` on its task page, palette **set waiting on…**, or `tsk edit --on`.
- A blocked or review row addressed to you shows the asker's words on a dim line under it. `z` hides these lines, leaving `✉`; `[board] messages` sets the default.
- Paper trail: the task page lists who did what, newest first, below the steps, and drops created and updated dates from its footer. `tsk list T12` shows it as `activity`.
- After: a task can wait for other tasks in any project (`!w T202`, the After field, `tsk edit --after`). When the last one is done, a waiting ready task starts. Thanks @ffittschen (#124).
- Agents block and hand back for review with `tsk status`, and use `tsk dispatch`, `tsk clean`, and `tsk reply`. Agent skill 1.6.0 covers them; `tsk update` refreshes installed copies.
- `ctrl+u` undoes status changes, step changes, and archiving; a marked set is one step. Undoing a start that launched an agent leaves the agent running. Thanks @LukasGold (#173, #252).
- Group and sort by assignee with `g` and `s` or `tsk list --group assignee` and `--sort assignee`. The `t` filter gains an `@assignees` tab.
- `[board]` in `config.toml` sets the group and sort a fresh desk or project board opens with, and `me` the name the board calls you.
- Back up tasks and `config.toml` to git on every save or daily. `tsk backup setup` makes a private GitHub repository with `gh`; `tsk backup add <git-url>` adds any other. Thanks @ari-becker (#128).
- `tsk edit --append-notes` adds text below a task's notes after a blank line, without rewriting them.
- `tsk setup` also installs the `tsk-coordinator` skill: your agent can brief builder agents, gate their pull requests with a review, live-test, and report back on the board.
- `tsk restore` lists the newest 20 backed-up versions per machine (`--all` for every one) and puts the one you pick back, keeping the replaced store.
- Sort the project picker and the Projects view by name or age with `s`. Both list projects A–Z by name by default. Thanks @LukasGold (#250).

### Changed

- `ctrl+b` and `ctrl+r` open the block and review cards; `Enter` on an empty card still blocks or sets review at once.
- A footer too narrow for its actions drops whole actions, least likely first, instead of cutting the last one with `…`.
- Board footers show `: commands` next to `? help`, and the task page's action bar ends with it; clicking it opens the command palette.
- Word jumps match macOS Option+arrow: `alt+→` or `ctrl+→` lands at the end of the next word, `alt+←` or `ctrl+←` at the start of the previous one.
- A row whose title wraps above an open peek or live line shows a dim `│` beside each wrapped line, so the row reads as one block.
- Help, footers, and the palette name each action verb first and in full (`ctrl+b` **block / unblock**). Help's **SURFACE CONTROLS** group is renamed **PICKERS & EDITING**.
- The archived-project launch card puts each option on its own line; `↑`/`↓` and `Enter` pick one, as on the cleanup card.
- `ctrl+s` starts a blocked task, closing its block; one `ctrl+u` reopens it. On a task it cannot start, the status row says why. Thanks @LukasGold (#252).

### Fixed

- `tsk help capture` and `tsk capture --help` print the capture reference instead of failing or opening the popup.
- `ctrl+u` names what it undid or why it refused, and undoing a done puts back the task's earlier status instead of open. Thanks @LukasGold (#173).
- Saving a task with a step's text cleared reopens that step with `text required` on its row, as for an empty new step. Thanks @LukasGold (#173).
- A project reached through a symlinked path, such as `/tmp/repo` and `/private/tmp/repo` on macOS, is one project.
- With tasks marked, `Esc` in the thread or view picker closes the picker and keeps the marks.
- With tasks marked, `ctrl+b` and `ctrl+r` skip done tasks, as starting does, and the status row counts them.
- `Esc` on a selected Thread on the task page cancels only the thread, not the whole edit.
- `ctrl+x` in a new step's row discards that row, instead of removing the step above it or deleting a task.
- `tsk steps` takes `--` before step text that starts with `-`, as `tsk reply` does.
- `tsk trash restore` gives a task the next free number when another task holds its old one, so no two tasks share a number.
- `tsk capture --state-dir <dir>` saves into that store instead of the default one; any other argument is refused.
- Notes keep underscores inside words, such as `file_name`, instead of turning them into emphasis.

## v0.12.0

### Added

- Board rows show their thread and project as a dim `#thread · project` label (`desk` for a desk task), leaving out whatever the view already says. Thanks @LukasGold (#175).
- Group, sort, and fold the board: `g` groups by project or thread, `s` sorts by recent, project, thread, or title, `f` folds the group under the cursor, and `Shift+F` folds every group. Group headers are cursor stops: `←` / `→` fold and unfold, `Enter` opens the group's board, and `Shift+M` marks the whole group. Also as clickable `group:` and `sort:` controls and in the `:` palette. Thanks @LukasGold and @GalShnapp (#175, #158).
- Filter the desk by thread and project with `t` (or the `all ▾` control), so a thread that spans repositories reads as one initiative. Thanks @GalShnapp (#158).
- Reorder steps: `alt+↑` / `alt+↓` moves the selected step and `ctrl+a` adds a step below it. From the CLI, `tsk steps T12 move <id> --before|--after <id>` (or `--first` / `--last`), and `tsk steps T12 add` takes the same place flags. Thanks @LukasGold (#159).
- Text editing keys in every field: macOS word moves and deletes (`alt+←` / `alt+→`, `alt+Backspace` or `ctrl+w`, `alt+Delete` or `alt+d`), and `ctrl+u` / `ctrl+k` to delete to the line start or end. The footer shows how to start a new line in Notes. Thanks @maximilliangeorge (#204).
- `tsk list --group project|thread`, `--sort recent|project|thread|title`, and `--no-thread`. `tsk list --all --thread X` lists a thread across every project and the desk.

### Changed

- In a text field `ctrl+a` moves to the line start, so macOS `Cmd+←` no longer adds a step while you type. It still adds a step when no field is open (#204).
- Bare `g` opens **group by** instead of folding the inbox or archived group; fold with `f` or `Shift+F`. `Enter` and a single click on those headings no longer fold them.
- Agent skill 1.5.0 covers threads across projects, grouping, and step reordering. Rerun `tsk setup` to update installed copies.

### Fixed

- Windows: `tsk update` and the board's update check no longer crash on the first HTTPS request, and `tsk add --file` and piped JSON plans accept the byte order mark Windows PowerShell 5.1 writes.
- Windows: characters typed with AltGr, such as `[ ] { } @` on a German layout, insert in every text field. Thanks @LukasGold (#178).
- `tsk setup herdr` follows a symlinked Herdr config, as dotfiles managers create, instead of failing with `Too many levels of symbolic links`. Thanks @yasith (#129).
- `tsk setup` shows terminal control characters in paths as visible escapes instead of printing them raw.
- `tsk list` orders tasks within each status like the board does. Thanks @dangerousdan (#123).
- `tsk list` refuses a repeated `-p`, `--thread`, or `--state-dir` (exit 2) instead of silently keeping the last one.
- A form that receives `Shift+Enter` as `ctrl+j` (such as Ghostty's `shift+enter=text:\n` binding) says so instead of ignoring the key. Thanks @WAWLixiong and @Rag0n (#125).
- Done drawer and archive: Help says where archived tasks live, `ctrl+s` on a marked set says how many done tasks it skipped, and at 110+ columns the Task view keeps the done drawer and archived fold. Thanks @LukasGold (#174).
- The `Deleted T12 "…" · ctrl+u undo` line clears on your next move or after a few seconds; the footer keeps offering undo while it applies.
- The task page's project chooser lists top to bottom, so `↓` moves down.

## v0.11.6

### Added

- Windows 10/11 support on ARM64 and x86-64: native release builds, a PowerShell installer (`irm https://www.gettsk.sh/install.ps1 | iex`), and Herdr integration as a preview.
- `h` and `l` mirror the left and right arrows for closing and opening task details in non-text board and task views. Thanks @pisceskkk.

### Changed

- `tsk update` downloads the installer completely before running it, always follows the latest stable release, refuses to downgrade, and says why a Herdr or skill refresh failed.

## v0.10.1

### Changed

- `tsk update` prints the version it replaces and the one it installs, re-registers an already set-up Herdr plugin without asking, and updates outdated installed agent skills (one `[Y/n]` ask on a terminal, unattended otherwise). It never installs a skill for an agent that had none; with nothing installed it offers the first-install ask.
- `tsk setup herdr` leaves a plugin command you bound to another key alone instead of adding the default chord beside it.

## v0.10.0

### Breaking

- Store format 5: one undo entry can cover a marked set. Rolling back refuses the file; restore `tsk.json.v4`.

### Added

- Multi-select: `Shift+M`, then `space`, `Shift+↑`/`Shift+↓`, or a click marks tasks; status, archive, and delete act on the set. One `ctrl+u` undoes a marked done or delete.
- `/` searches every board tab by title, notes, steps, thread, and task number (project names on the Projects overview). `Enter` pins the filter for normal keys; `Esc` clears it.

### Changed

- `!p name`, `tsk add -p name`, and JSON plan projects refuse unknown or ambiguous names (`unknown-project`) instead of creating a stray project. A new project needs an existing absolute path (`/…` or `~/…`).
- `ctrl+q` quits from every non-editor surface, including task pages and the wide project preview. `Esc` closes layers and wide splits, then quits at the full-board root on any tab. Unsaved drafts still block quitting.
- Agent skill 1.3.0 (`unknown-project` refusal, search and multi-select); rerun `tsk setup` to update installed copies.

## v0.9.0

### Breaking

- Store format 4: existing `ready` tasks move to the new `open` inbox on first save. Rolling back refuses the file; restore `tsk.json.v3`.
- `ctrl+n` now sets ready (it no longer opens Notes) and `ctrl+o` sets open (it no longer reopens to ready).

### Added

- `open` status for captured, untriaged tasks: an expandable `inbox` group under ON DECK, `ctrl+n` picks a task for ready, `ctrl+o` sends it back.
- `tsk status T<n> open`, `tsk list --open`, `tsk list --ready`.
- Projects tab at 110+ columns: the cursored project previews beside the index, `→` makes it a live board, `←` and `Esc` walk back.
- Projects index counts ON DECK and DONE next to NEEDS YOU and IN MOTION.
- `tsk list T<n>` prints the whole task: notes, steps, and thread, in human and JSON form.
- `tsk help <command>` as an alias for `--help`, and `tsk --version` / `-V`.

### Changed

- `tsk --help` is the CLI reference: commands grouped with a Statuses block, and one 80-column skeleton per command (usage, options, examples, refusals, exit codes). Long-form detail lives in the CLI guide.
- Refusals print as `code: message` on stderr for every task command; `archive` and `unarchive` gain stable codes.
- The agent skill (`tsk guide`, `tsk setup <agent>`) is slimmed to rules of engagement, board language, the exit contract, and workflows, reads JSON by default, and gains a `Refine a task` section. Skill 1.2.0; rerun `tsk setup` to update installed copies.
- Task page and expanded quick-add share one Tab ring: Title, Notes, steps, + step, Thread, Scope, back to Title; `Shift+Tab` reverses it.
- Task-page footer shows the thread before the project.
- Inbox heading paints in normal text with no blank row under it.
- `ctrl+s` starts open tasks as well as ready ones.
- Board help (`?`) is a searchable shortcut reference: type to filter by key or action, `Esc` clears then closes.
- Human `tsk list` output wraps to the terminal width, supported from 50 columns.
- The starter tour mentions the projects preview and `tsk help`; the upgrade notice covers this release's features.
- The upgrade notice (`What's new in tsk`) arrives in `review` under NEEDS YOU; the last starter guide seeds as `open` so a fresh board shows both ON DECK groups.
- The Homebrew caveat also points at `tsk setup agents`.

### Fixed

- `tsk setup agents --yes --force` rewrites every detected agent skill even when the version already matches.
- `tsk setup herdr` no longer prints the plugin root on success and reports a previous registration whose root is gone before re-registering.
- The selected task-page footer control is no longer dim inside its highlight.
- Expanded quick-add in a project preview keeps its draft title in the column header and styles the active notes line as an editor.

## v0.8.2

### Breaking

- Task store format 3, migrated automatically on first open. Rolling back to 0.7.x afterwards refuses the file; restore the `tsk.json.v2` backup written beside it if you need to.

### Added

- Starter tour on first board open: four desk tasks (`N1` to `N4`) that teach the tabs, sections, status keys, the task page, and the CLI, each cleared for good with `ctrl+d`, `ctrl+f`, or `ctrl+x`. Agents never see them: `tsk list` hides `N` rows.
- What's new on your desk: after an upgrade, one `N` row can summarise the release with a link to the changelog. This release carries none, so a 0.7.x upgrader sees only the tour.
- `tsk update` upgrades installer-managed copies in place; Homebrew copies print `brew update && brew upgrade tsk`. The board nudges once a day when a newer release is out (`TSK_NO_UPDATE_CHECK` disables).
- `tsk setup` detects the coding agents on your machine and offers to install the tsk skill for each; `tsk setup agents --yes` does it unattended. The curl installer asks the same question once when Herdr is on PATH.
- Added support for OMP with `tsk setup omp`, thanks @bnivanov.
- Outside Git, the current directory is available as a project: `2` opens its board, quick-add files there once it is open. The desk stays the default for capture and the CLI.

### Changed

- The agent skill (`tsk guide`, `tsk setup <agent>`) is rewritten around a quick-reference table and rules of engagement: agents hand work back with `review` and leave `done` to you. Skill version 1.1.0; rerun `tsk setup` to update installed copies.
- Sections hold their order while you work: NEEDS YOU, IN MOTION, DONE and the drawer's ARCHIVED group keep the most recent status change on top, and ON DECK lists its backlog oldest first, `N` rows leading. Editing a task or ticking a step no longer jumps it to the top.

### Fixed

- On a wide board, clicking a task opens its details beside the board and keeps board focus; a double-click during column reflow opens the task you clicked, not a newly exposed control.
- `prefix+t` opens or focuses one board per Herdr workspace, across tabs, and returns to the tab you pressed it from.
- `tsk setup herdr` on a Herdr older than 0.9 says so (`herdr 0.6.8 found; tsk needs 0.9.0 or newer`) and stops before touching the config, instead of failing on `herdr config check` with a usage dump.
- Setup errors that quote Herdr's output keep their line breaks instead of printing a literal `\u{000a}`.

## v0.7.0

### Added

- `tsk guide` prints the agent workflow, and `tsk setup <claude|pi|cursor|grok|codex|opencode|--skill-dir>` installs it as a skill. `tsk --help` ends with a pointer for agents.
- The installer adds `~/.local/bin` to PATH for Bash and Zsh.

### Fixed

- At 110 columns or wider, `Tab` on the quick-add line opened a draft page that was never painted.
- A reopen request replaced by another of the same size in the same instant could be delivered as the first one.

## v0.6.0

### Added

- Native macOS and Linux binaries, a checksum-verifying install script, and a Homebrew formula.
- `tsk setup herdr` registers the installed binary with Herdr and adds `prefix+t` (board) and `prefix+a` (capture).
- CLI: `tsk status`, `tsk edit`, and `tsk steps rename|remove`.
- **NEEDS YOU** section: blocked and review tasks at the top of the board.
- Projects index with search, per-project status counts, and persistent desk · project · projects navigation.
- Quick capture (`tsk capture`) opens the task editor in a Herdr popup; the expanded quick-add takes threads and steps.

## v0.5.0

### Added

- Wide layout: at 110 columns or wider the board and task page form a four-stage slider driven by `←` / `→`.
- Archive for tasks (`ctrl+f`) and projects (from the `p` picker), with a collapsible archived group in the done drawer and read-only focus for archived projects. CLI: `tsk archive`, `tsk unarchive`, `tsk project archive|unarchive`, `tsk list --archived`.
- Trash: deleted tasks move to `trash.jsonl` once undo can no longer reach them and purge after 30 days. `tsk list --deleted` and `tsk trash restore T<n>`.

### Changed

- State files are private on Unix: `~/.tsk` is `0700` and its files `0600`; symlinked state roots are refused.
- Store format 2: a `projects` map; a v1 store migrates on load and keeps a `tsk.json.v1` backup.
- Undo history is capped at 50 entries.
- License is MIT.

## v0.4.0

- Markdown in notes (view and peek): bold, emphasis, code, fenced blocks, headings, lists, in mono modifiers only.
- Mouse text selection: drag to highlight, release to copy via OSC 52.
- Board and task-page scrollbars with click and drag; the current section header pins under the tabs.
- Modal overlays for help, palette, and project picker share one centered card.

## v0.3.0

- Threads: an optional normalized label per task, set with `--thread` on add or in plan JSON, filtered with `list --thread`.

## v0.2.0

- Steps: an ordered checklist on the task page, with `steps <task> add|toggle` on the CLI. Step progress never changes task status.

## v0.1.0

- Queue board for capture and human status (`ready`, `started`, `blocked`, `review`, `done`) inside Herdr, with peek, task page, project scope, done drawer, and undo.
