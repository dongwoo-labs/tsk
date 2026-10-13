---
name: tsk-cli
description: Work the user's tsk task board from the command line. Use when asked to add, update, edit, start, block, finish, archive, or restore a task on the board (or "tsk", "the tsk board", "the desk"), to add or tick steps, to answer "what's on the board", "what's next", "what's on deck", "what needs me", or to refine a task ("refine T12", "let's discuss T12", "what's missing from T12", "improve / rewrite this task"). Always use the CLI (`tsk add|list|status|edit|steps|archive|trash|import|ask|questions|answer|route`), never the TUI.
version: 1.5.0
---

# tsk: the user's task board

tsk is the user's task board. Tasks have a human status (`todo` · `doing` · `done`),
live on the **desk** (no project) or in a **project** (a Git repo root, named
by its basename). The user sees the board in a TUI; you never
open it. Every read and write goes through the CLI, and the user's board updates live.

`tsk --help` is the syntax reference: commands, statuses, and every flag. Run `tsk help <command>`
before the first use of a command in a session; this file only holds what `--help` cannot tell
you: how to behave on someone else's board.

## If tsk is not installed

Run `tsk --version`. If the command is unavailable, give the user https://gettsk.sh/docs/install.md
and stop. Never run an installer, package manager, or source build unless they asked.

## Board language

| The user says | Statuses | Read |
| --- | --- | --- |
| what needs me | Questions, not Task status | `tsk questions --json` in the explicit Agent state directory |
| in motion | `doing` | `tsk list --doing --json` |
| on deck, what's next | `todo` | `tsk list --todo --json` |
| other projects, everything live | `todo`, `doing` | `tsk list --all --json`, `--desk`, `-p <project>` |
| done, archived, deleted | | `tsk list --done --json`, `--archived`, `--deleted` (each may combine with a scope flag) |
| one task, in full | | `tsk list T12 --json` (notes, steps with `short_id`, parent, children) |

`T12`, `t12`, `12`, and the UUID all address the same task. Prefer `T12`, it is what the user sees.
New tasks start `todo`. `doing` means work is in progress; `done` means it is finished.
Questions and approval records are separate from Task status. Legacy status values and the `start`
alias are usage errors, not alternate spellings.

## Rules

1. **Human status is the user's.** When your work is finished, report the result and leave
   the Task in `doing`. Set `done` only when the user says the task is done or told you to
   close it. Never mark tasks done from your own progress.
2. **Use a known project name or a full path.** A bare `-p name` must resolve to one existing
   project; unknown or ambiguous names refuse. An existing absolute directory is the only way to
   create a new project destination. `tsk edit T12 -p <project>` (or `--desk`) moves a task filed
   under the wrong scope; it resolves the destination the same way.
3. **Read JSON, not presentation.** Always add `--json` to `tsk list`. Human output is for showing
   the user or troubleshooting interactively. Mutation acknowledgements may be human-only: trust
   the exit and refusal codes, then verify state with `tsk list … --json`.
4. **Never blind-retry.** Read the exit code, then act (table below). `add`, `status`, `edit`,
   `archive` are idempotent; `steps toggle` and `steps remove` are not, so run
   `tsk list T12 --json` before retrying any `steps` command.
5. **Task commands use the selected board.** Never pass `--state-dir` unless the user asked.
   Question commands are the exception: use the task's explicitly assigned non-personal Agent state
   directory. If no Agent directory is assigned, stop rather than selecting one or using the personal board.
6. **Ignore notice rows.** Rows the board paints as `N1`… are human-only (starter tasks and
   release notes). `tsk list --json` never shows them and no command addresses them.
7. Values that begin with `-` need the `=` form: `--title="-fix parser"`, `--notes="-5 degrees"`.
8. **Name links in notes, one per line.** `Linear: <ID> <URL>`, `PR: <URL>`, `CI: <URL>`, and
   `Thread: <agent-name> <workspace-id> <tab-id> <pane-id>` are the references the task page
   reads. Keep URLs and short results only; never copy PR or CI detail. A Thread line focuses
   only when it is the task's one Thread line and all four tokens match one live pane.

## Task-command exit contract

| Exit | Meaning | Do |
| --- | --- | --- |
| 0 | done, or already true | after a mutation, verify with `tsk list … --json`; otherwise nothing |
| 1 | refusal of one or more items; valid siblings persisted | fix and retry only the refused subset |
| 2 | usage or parse error, nothing persisted | correct the invocation, run again |
| 3 | store I/O, commit indeterminate | `tsk list … --json` (also `--done`, `--archived`), retry only what is missing |

`add`, `status`, `edit`, `steps`, `archive` and `unarchive` print a stable refusal code first
(`tsk status: unknown-task: T12 is not on the board`), listed per command under
`tsk help <command>`. Branch on the code; the message is not a contract. `trash restore` and
`project` refuse with a message only.

## Workflows

**What's on the board.** `tsk list --json` (this repo's project, or the desk outside Git). Group by
`status` in board language: Doing, then Todo. Read unanswered questions separately.

**Start work on a task.** `tsk list T12 --json` for notes and steps. `tsk status T12 doing`.
Tick steps as you go: `tsk steps T12 toggle <short_id>`.

**Hand back.** Leave the Task in Doing and state what you did and what the user must inspect.
When an answer is needed, ask a Task-linked question rather than changing the Task to a waiting status.

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
Private originals stay in Git-excluded `questions-private/`; put only shareable summaries in references.
Set `audience` (`user` or `lead`) on every new question: the user's questions tab and waiting line
count only `user` questions, and a question without one shows as target unknown. Change it later
with `tsk route <Q> --file` and the current `expected_revision`; routing keeps the number, text, and
answers. A finding that may need follow-up work carries `lead_recipient`, the exact live Lead name
the board sends its run request to. Routing and that address grant no approval or execution authority.

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
   - Exists: `tsk edit T12 --title "…" --notes "…"`.
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
