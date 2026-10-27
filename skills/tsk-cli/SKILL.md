---
name: tsk-cli
description: Work the user's tsk task board from the command line. Use when asked to add, update, edit, start, block, finish, archive, or restore a task on the board (or "tsk", "the tsk board", "the desk"), to add or tick steps, to answer "what's on the board", "what's next", "what's on deck", "what needs me", or to refine a task ("refine T12", "let's discuss T12", "what's missing from T12", "improve / rewrite this task"). Always use the CLI (`tsk add|list|status|edit|notes|steps|archive|trash|import|ask|questions|answer|route|reply|dispatch|clean|backup|restore|secret-result`), never the TUI.
version: 1.7.0
---

# tsk: the user's task board

tsk is the user's task board. Tasks have a human status (`open` · `ready` · `started` · `blocked` ·
`review` · `done`) and live on the **desk** (no project) or in a **project** (a Git repo root, named
by its basename). A task may carry a **thread** label, an **assignee** (an agent profile from
`config.toml`, the user, or a person), and a dispatch **base** branch. The user sees the board in a TUI; you never open it.
Every read and write goes through the CLI, and the board updates live.

Legacy Agent consumers may still use `todo`/`doing` and `--todo`/`--doing`. Unprojected JSON remains an array and its `status` uses `todo`, `doing`, or `done`; richer states remain available to human-status commands.

`T12`, `t12`, `12`, and the task's UUID all address the same task. Prefer `T12`: it is what the user
sees. Run `tsk help <command>` before you first use a command in a session; the full reference is
https://gettsk.sh/docs/cli.md.

If `tsk --version` fails, give the user https://gettsk.sh/docs/install.md and stop. Never run an
installer, package manager, or source build unless they asked.

## Workflow

1. **Read.** `tsk list T12 --json` gives the task in full: notes, steps with their `short_id`, the
   open `block` or `review`, the `dispatch` record, and `activity` (who did what, newest first). If
   you were relaunched, also read the record that closed last (`closed_at`): the last `past_blocks`
   entry's `replies`, or the last `past_reviews` entry's `feedback` and failed `checks`.
2. **Start.** `tsk status T12 started --no-dispatch` when you do the work yourself. Without
   `--no-dispatch`, starting an assigned task launches its agent instead of you. A task waiting on
   another refuses with `after-not-done`; add `--force` only when the user said to start it anyway.
3. **Work.** Tick steps as you finish them: `tsk steps T12 toggle <short_id>`.
4. **Hand back, or block.**
   - Done with your part: `tsk status T12 review --done "…" --check "…" [--check "…"] [--next "…"]`.
     Keep `--done` to one short sentence; the board paints it under the row. Add one `--check` per
     thing the user should verify. `--on <agent>` hands the review to another agent profile.
   - Stuck on the user: `tsk status T12 blocked --why "<your question>"`, with `--needs "…"` for what
     would unblock you and one `--option "…"` per answer they can pick. Keep `--needs`, or `--why`
     without it, to one short sentence. `--on T202` waits on another task instead.
   - Running either again while it is open edits the same review round or block. To change only who
     it waits on, keeping the round and its replies: `tsk edit T12 --on <who>`.
5. **Stop.** Say in one line what to look at, then wait. The user answers in your session:
   `[tsk T12 unblocked] …` means the block is closed with their answer. `[tsk T12 reply] …` is a
   reply to the open block or review; the status stays. `[tsk T12 sent back] …`, with any
   `Failed checks: …`, means the task is `started` again with their feedback. Address it, then hand
   back again: the next review opens a new round.

`tsk reply T12 "…"` adds a reply to the open block or review round. It is not safe to repeat, and a
task with nothing open refuses with `not-blocked`. When the user asks you to send their answer to the
task's running agent, add `--send`: it sends only that reply and prints `sent to @<agent>` or
`not sent: <reason>`.

## Rules

1. **Human status is the user's.** Set `review` when your work is finished. Set `done` only when the
   user says the task is done, and never unblock or approve your own task unless told.
2. **Read JSON, not presentation.** Always add `--json` to `tsk list`. Write commands may answer in
   text only: trust the exit and refusal codes, then verify with `tsk list … --json`.
3. **Never blind-retry.** `add`, `status`, `edit`, and `archive` are safe to repeat. `reply`,
   `dispatch`, `edit --append-notes`, `steps add`, `steps toggle`, and `steps remove` are not:
   read the task first.
4. **Use a known project name or a full path.** For `tsk add -p <project>`,
   unknown or ambiguous names refuse; `tsk list -p` lists nothing for them. Only an
   existing absolute directory creates a project ([scope](https://gettsk.sh/docs/cli.md#scope)).
5. **Never pass `--state-dir`** unless the user asked. It points at a different board.
6. **Ignore notice rows.** Rows the board paints as `N1`… are for the user; no command reaches them.
7. **Values that start with `-` need `=`:** `--title="-fix parser"`. For `tsk reply` and `tsk steps`
   text, put `--` just before it.
8. **Names are exact.** `--assignee` takes a `config.toml` profile, `me` for the user, or a person's
   name. A name that is not a profile prints `(not an agent profile)`: check it for typos
   ([names](https://gettsk.sh/docs/dispatch.md#assignee-names)). `--base` takes an existing branch,
   and `--thread` a [thread name](https://gettsk.sh/docs/capture.md#thread-names)
   ([add](https://gettsk.sh/docs/cli.md#add)).
9. **After means wait.** `tsk edit T13 --after T12` holds T13 until T12 is done
   ([after](https://gettsk.sh/docs/board.md#after)).
10. **Steps are the user's plan.** Add them only when the user wants order of work tracked, never
    for your own bookkeeping ([steps](https://gettsk.sh/docs/cli.md#steps)).
11. **Many tasks at once:** `cat plan.json | tsk add`; on exit 1, retry only the `failed` items
    ([JSON plans](https://gettsk.sh/docs/cli.md#json-plans)).
12. **A restored task may change number.** `tsk trash restore T12` prints the number it got; use that
    one from then on ([trash](https://gettsk.sh/docs/cli.md#trash)).

### Agent-board notes

8. **Name links in notes, one per line.** `Linear: <ID> <URL>`, `PR: <URL>`, `CI: <URL>`, and
   `Thread: <agent-name> <workspace-id> <tab-id> <pane-id>` are the references the task page
   reads. Keep URLs and short results only; never copy PR or CI detail. A Thread line focuses
   only when it is the task's one Thread line and all four tokens match one live pane.
9. **Keep a status card at the top of the notes.** First lines `Now: <current state>`,
   `Next: <next action>` (optional), `Blocked: <what, waiting on whom>` (only while blocked), and
   `Done when: <completion condition>`; reference lines follow. Replace a card line in place
   with `tsk notes T12 set Now: "…"`, never append a second one. Use
   `tsk notes T12 remove Blocked:` once unblocked. These commands read and update under the store
   lock and preserve other lines. `tsk notes T12 append "PR: <URL>"` appends a reference;
   it is not idempotent, so read back before retrying an uncertain append. Use `set PR:` to keep
   only one PR line. `tsk edit --notes` still replaces the full body; reserve it for a full rewrite. Past 4 KB both
   still save (exit 0) but print a stderr `warning:`: trim the log and keep links, not history. The board uses one line per task by default, with a bold `×` for a non-empty
   `Blocked:` line on a task not done. `v` from the board list shows or hides the second line
   for this session: the first `Blocked:` line (bold) or else the `Now:` line of a Doing task.


## Reading the board

`tsk list --json` reads this repo's project, or the desk outside Git. Present it in board language:
what needs the user first, then in motion, then on deck. Rows come grouped by status, and within a
status they are already in the board's order; keep it.

| The user says | Read |
| --- | --- |
| what needs me | `blocked` and `review` rows that wait on the user (below) |
| in motion | `started` rows, plus `blocked` and `review` rows that wait elsewhere |
| on deck, what's next | `tsk list --ready --json` |
| inbox, untriaged | `tsk list --open --json` |
| other projects, everything live | `tsk list --all --json`, or `--desk`, `-p <project>` |
| a thread across repos | `tsk list --all --thread X --json` (desk included); `--no-thread` for the rest |
| grouped by project, thread, or assignee | `tsk list --all --group project --json`: rows gain `group` |
| assigned work | `tsk list --assignee <name> --json` |
| done, archived, deleted | `tsk list --done --json`, `--archived`, `--deleted`, with any scope flag |

**What needs the user.** For each row with a non-null `block` or `review`, check its `on`:

- `you`, or no `on`: needs the user.
- `task:<n>`: needs the user once T<n> is done or gone; until then it is in motion.
- `agent:<name>` or `other:<text>`: in motion, not the user's.

When both records are null, JSON `status: doing` does not identify a block or review. Do not infer a request from it.

## Dispatch and cleanup

Only when the user asks.

- **Dispatch:** `tsk dispatch T12`, or `tsk status T12 started` on an assigned task never
  dispatched. It launches the assignee's agent in a new worktree and Herdr workspace and sets the
  task `started`. Watch it with `herdr agent get t12-<assignee>`.
- `already-dispatched`, or `agent-gone` from a start, means a launch exists. Relaunch with `--again`
  only when the user wants it.
- A `no launch: <reason>` line after a start means the task started but nothing launched (outside
  Herdr, or a desk task). Tell the user; do not retry.
- **Clean up** after review and merge: `tsk clean T12`, or `tsk status T12 done --clean` when the
  user also said the task is done. Never clean the worktree you run in. Uncommitted work refuses
  with `dirty-worktree`. A kept branch comes with its reason (`branch.reason` in `--json`); deleting
  it is the user's call.
- **Merge** only when asked: `tsk status T12 done --merge` merges the task's pull request with `gh`,
  then sets done and cleans. A refusal (`no-pr`, `pr-not-open`, `merge-failed`, …) changed nothing:
  report gh's reason and never merge another way.

## Exit codes

| Exit | Meaning | Do |
| --- | --- | --- |
| 0 | done, or already true | after a write, verify with `tsk list … --json` |
| 1 | refused; in a plan, the valid items saved | fix the cause, retry only what refused |
| 2 | usage error, nothing saved | fix the command and run it again |
| 3 | store error, the write is indeterminate | `tsk list --all --json` (also `--done`, `--archived`); redo only what is missing |

A refusal prints a stable code first: `tsk status: unknown-task: T12 is not on the board`. Branch on
the code, never the message. `tsk help <command>` lists each command's codes. `project`, `trash`,
`backup`, and `restore` refuse with a message only. After exit 3 from `dispatch`, or from a start that
dispatched, a message saying `the agent may still run` means it may run without its record: check
`herdr agent get t12-<assignee>` and read the task before retrying.

## Agent questions and related tasks

**Questions.** Run `tsk help ask`, `tsk help questions`, `tsk help answer`, and `tsk help route`
first. These commands
require an explicitly selected non-personal Agent state directory. They never fall back to the
personal board. Read the current Q revision before answering; pass it as `expected_revision`.
Corrections append and name the current answer revision. Question writes report saved durability and
notification separately; an uncertain commit exits 1. After an uncertain write, read the record before
retrying. Visible readback alone does not confirm durability. Notification failure does not undo a saved answer, and notification acceptance does
not prove the recipient read it. Ordinary Agent or imported answers do not authorize execution.
Approval must preserve confirmed UI evidence or verified original user-chat evidence and match the
exact target, action, Git SHA or plan digest, scope, and rollback terms. Never fabricate that evidence.
An approval answer may carry an optional `comment`: free text beside the `approve`/`reject`
decision, never itself an approval and never checked against the stored terms, refused on anything
but an approval or when blank.
Private originals stay in Git-excluded `questions-private/`; put only shareable summaries in references.
`audience` (`user` or `lead`) is required on every new question: `tsk ask` refuses one without it
(exit 1, nothing saved). The user's questions tab and waiting line count only `user` questions. Change it later
with `tsk route <Q> --file` and the current `expected_revision`; routing keeps the number, text, and
answers. A finding that may need follow-up work carries `lead_recipient`, the exact live Lead name
the board sends its run request to. Routing and that address grant no approval or execution authority.

**Ask for a secret.** Never ask the user to paste a key into chat, and never put one in a
question. File a secret question; the user types the value on the board, which hands it to
`fnox set` itself:

```json
{"task_id": "<task UUID>", "kind": "secret", "summary": "staging API key", "origin_thread": "thr-a", "recipient": "lead-a", "text": "Why the key is needed", "options": [], "recommendation": null, "impact": null, "blocking_scope": null, "approval": null, "finding": null, "source": null, "audience": "user", "secret": {"key": "STAGING_API_KEY", "profile": null, "config": null}}
```

`key` matches `[A-Za-z_][A-Za-z0-9_]*`; `profile` is a fnox profile or `null` (default);
`config` is the absolute path of an existing `fnox.toml` or `null` (global, `fnox set -g`).
`audience` must be `user`. Wait until `tsk questions Q<n>` shows the answer `set` or
`rejected`; these are the only things tsk records, and no tsk command can show the value.
`rejected` means the user closed the request with no value: ask again only if the key is
still needed. On `set`, read the value only inside your own process with
`fnox exec -- <command>` or `fnox get <KEY>` piped straight into its use, never echoed to
output. An external secret-input interface can also store the value through fnox. After
that interface confirms storage to the question's fixed target, an agent or the interface can
run `tsk secret-result <Q UUID> --file - --state-dir <Agent-dir>` with only
`{"expected_revision":"<revision shown to the user>","result":"set"}`. This path checks
the effective fnox config/profile's key name and records the same `set` answer. It does not
prove a fresh write, a direct declaration, or a valid value. It does not lock fnox storage:
a stale CAS refuses the Tsk answer but cannot undo a value already written. Keep the Q UUID,
revision, and target fixed; never replace a stale revision and automatically resend.
For this path, pass the stored profile explicitly to fnox (`default` when null); do not let
`FNOX_PROFILE` choose another target. Never put a value, text, actor, or origin in the result
JSON. Read back after an uncertain save; notification failure alone is not a reason to retry.
`tsk answer` always refuses a secret question with `secret-needs-board`; only the board or
`tsk ui-answer`'s `reject: true` can reject it with no value. Neither takes an external `set`.

**Plan as steps.** When the user wants order of work tracked on the task: `tsk steps T12 add
"…"` per step, in order. Do not add steps for your own bookkeeping. To fit a step into an existing
list, `add "…" --after <short_id>` (or `--before`); to reorder, `tsk steps T12 move <short_id>
--before|--after <short_id>` or `--first`/`--last`. A moved step keeps its short id and checked
state, and a repeated move is safe.

**Capture many.** `cat plan.json | tsk add` with
`[{"title": "…", "notes": "…", "project": "…"}]`; only `title` is required, an
omitted `project` takes the default scope. The result lists `created`, `existing` and `failed`
items; on exit 1 retry only the `failed` items, never the whole plan. Full shape:
https://gettsk.sh/docs/cli.md#json-plans.

**Import from Linear.** Only when the user asks to pull Linear issues onto the board. Run
`tsk import linear --dry-run` with their filters first, show the report, then run it without
`--dry-run`. It needs `LINEAR_API_KEY`; on `missing-api-key` ask the user to set it, never ask
for the key itself. Do not pass `--update` unless asked: it overwrites board status with Linear's.

## Refine a task

Use when the user asks to refine, discuss, improve, rewrite, or find what's missing from a task,
or brings a rough idea that should become one. This is a shaping pass, not a build: no acceptance
criteria, no design document, no code. It ends with a better task on the board.

> **Hard gate: propose, then write.** Show the full rewrite (title and notes) and get a yes before
> any `tsk edit` or `tsk add`. `edit --notes` replaces the whole body; a silent write loses the
> user's own words.
>
> To add to the notes, such as a brief, use `tsk edit T12 --append-notes "…"`. Never rewrite the
> user's notes with `--notes` without asking.

1. **Read.** `tsk list T12 --json` for the task as written. Then `tsk list --json` (and `--all
   --json` when the task may belong elsewhere) for neighbours in the same project or under the
   same parent: duplicates, a task this depends on or unblocks, a parent it should join.
2. **Ground in code.** Before saying anything, open the files, docs, and tests the task touches.
   Check the user's claims against what exists; when the code answers a question, do not ask it.
3. **Diverge briefly.** Two or three approaches with tradeoffs, recommendation first and why.
   Scale to the task: a bug fix gets two lines, a feature gets a short list. Draw a mock in a code
   block when the shape lands better than prose (a screen, a table, a CLI transcript).
4. **Grill, one question at a time.** Multiple-choice with a recommended answer. Prefer reading
   code over asking. Sharpen fuzzy or overloaded words into one canonical term and use only that
   term afterwards. When an answer exposes that the framing was wrong, go back to step 3 for that
   part instead of building on it.
5. **Settle and propose.** Present the rewrite in the notes shape below, then the exact commands
   you will run. Stop and wait.
6. **Write back on a yes.**
   - Exists: `tsk edit T12 --title "…" --notes "…"` (add `--assignee <profile>` or `--unassign` only when agreed).
   - Does not exist yet: `tsk add -t "…" -n "…"` in the agreed scope (`--desk`, `-p`), with
     `--parent T12` if agreed.
   - Steps only when the user wants order of work tracked: `tsk steps T12 add "…"` per step.
   - Never change status. `edit` cannot change scope: for a scope change re-add in the
     right scope and archive the stray; say so before doing it.
7. **Stop.** The hand-off is the updated or created task. Report its number and title in one line.
   Do not offer builders, worktrees, or plans, and do not start building unless asked.

**Title**: the outcome, verb-first, under about 60 characters (`Make tsk --help the CLI reference`,
not `help improvements`).

**Notes shape** (the board renders `**bold**`, `*em*`, `` `code` ``, `#` headings, `-` lists,
fenced code):

```
Ask: <the user's words, verbatim, one or two lines>

Settled
- <one actionable sentence per decision>

Rules / edge cases
- <behaviour at the boundaries, refusals, what must not change>

Shape
- <files, modules, surfaces to touch; what not to touch>

Open
- <deferred question, and why it can wait>
```

Omit a heading that would be empty. Keep the user's terms once sharpened; the notes are the brief
a future agent (or the user) builds from without this conversation.
