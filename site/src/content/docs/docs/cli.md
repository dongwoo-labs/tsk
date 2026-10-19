---
title: CLI
description: Commands, arguments, output, and retry rules for the shared task board.
---

The CLI reads and updates the same tasks as the board. A running board picks up saved changes automatically.

```sh
tsk add -t "Fix login timeout"
tsk list
```

## For agents

Install the skill with `tsk setup pi`, or use your [agent's setup target](#setup). `tsk guide` prints the workflow.

1. Read the task with `tsk list T12`.
2. Set its status with `tsk status T12 doing`.
3. Update notes or steps as work progresses.
4. Leave it Doing when handing back work. Record questions separately; set Done only when the user requests it.

The CLI can mark a task done with `tsk status <task> done`. Agent lifecycle does not change task status automatically.

Use `--json` on `add`, `list`, or `import` for machine-readable output. Read the [exit contract](#exit-contract) before retrying a write.

## Task-linked questions

Question commands run only with an explicit non-personal Agent state directory. `TSK_STATE_DIR` also supplies that selection. They do not open or write the personal default board.

`tsk ask` checks the question before it saves anything. A kind `approval` needs `approval` with non-blank `target`, `action`, `scope`, and `rollback` and an `h` of 40 or 64 hex digits; every other kind takes no `approval`. A wrong shape exits 1 with every bad field in one line (`invalid question: approval.h: ...; approval.target: ...`) and creates no Q number, record, or notification. A JSON error (unknown `kind`, an unknown or missing field) exits 2 and names it. Whether the target, repo, or H make sense for your work is the caller's check; `tsk ask` checks only the shape.

```sh
tsk ask --file question.json --state-dir /local/agent-board
tsk questions --task T12 --state-dir /local/agent-board --json
tsk questions Q1 --state-dir /local/agent-board --json
tsk answer Q1 --file answer.json --state-dir /local/agent-board
tsk route Q1 --file route.json --state-dir /local/agent-board
tsk ui-answer Q1 --file - --state-dir /local/agent-board
```

`--file -` reads the JSON input from stdin.

`ask` takes a JSON object with `task_id` (the full Task UUID), `kind` (`question`, `approval`, `finding`, or `secret`), `summary`, `origin_thread`, `recipient`, `text`, `options`, `recommendation`, `impact`, `blocking_scope`, `approval`, `finding`, `source`, `audience` (`user` or `lead`, required: a question without it is refused with exit 1 and nothing saved), and the optional `lead_recipient` and `secret`. Nullable fields can be `null`; options defaults to an empty array, and an `approval` or `secret` question takes none (refused with exit 1, nothing saved). Origin and recipient use Herdr agent names. Q UUID is the connection key; Q number is a display address within this store. Full readback includes every original character. A shareable summary must contain no sensitive text. A record saved before this rule without an `audience` shows as *target unknown* on the board's questions tab: it is never guessed or answered there, and the tab number and row marks leave it out.

`answer` takes `expected_revision`, `text`, `actor`, `origin`, `corrects_revision`, `approve`, `confirmed`, `approval`, `user_chat`, `source`, and the optional `comment`. Ordinary answers use `origin: "agent"`, `approve: false`, and null approval/evidence. A correction names the current answer's revision in `corrects_revision`; it appends instead of replacing history. A stale expected revision refuses without applying another answer. Use `questions` after every uncertain result and before a retry.

Approval terms contain `target`, `action`, `h`, `scope`, and `rollback`. `h` is the exact 40-digit Git SHA or 64-digit plan digest. An ordinary Agent or imported reply cannot issue approval. `tsk answer` refuses `origin: "user_ui"`; only the board's questions tab and [`ui-answer`](#ui-answer) record that origin. A `lead_user_chat` approval also requires `confirmed: true`, exact current terms, and `user_chat` with the verified `original_user`, `original_text`, `source`, `original_time`, and `original_verified: true`. This attestation preserves the original user evidence; it does not authenticate another process using the same OS account.

`comment` is free text beside an approval's `approve`/`reject` decision: non-null only on an approval question, and refused if blank. It is never itself an approval, never checked against the stored approval terms, and never weakens `approve`'s requirements. It is saved with whichever decision the same answer carries and shown in the record's history beside `approved` or `rejected`.

<span id="secret"></span>**Secret questions.** When an agent needs a secret value, it asks with `kind: "secret"`, `audience: "user"`, no options, approval, or finding, and `secret: {"key", "profile", "config"}`: the fnox key (`[A-Za-z_][A-Za-z0-9_]*`), the fnox profile (`[A-Za-z0-9_-]+`, or `null` for the default), and `config` (an absolute path to an existing regular `fnox.toml`, not a symlink, or `null` for the global config). Anything else refuses with exit 1 and nothing saved, and `secret` on another kind refuses the same way. The user types the value in the board's [questions tab](/docs/board/#questions); tsk runs `fnox set [-g | -c <config>] [--profile <profile>] <key>` itself, with the value on fnox's standard input only, then records the answer `set` (origin `user_ui`). The value is never in `tsk.json`, the private snapshots, answer history, arguments, the environment, or any output, so no command can read it back; read it in your own process with `fnox get` or `fnox exec`. A secret no longer needed closes with no value the same way, as the user-only answer `rejected`: its form's **2. Reject** row on the board, or `ui-answer`'s `reject: true`. `tsk answer` always refuses a secret question with `secret-needs-board`, even a reject-shaped body; `route` refuses to move it to the lead; there is no correction. Once set or rejected, it stays closed.

```json
{"task_id": "<task UUID>", "kind": "secret", "summary": "staging API key", "origin_thread": "thr-a", "recipient": "lead-a", "text": "Enter the staging API key for the deploy check", "options": [], "recommendation": null, "impact": null, "blocking_scope": null, "approval": null, "finding": null, "source": null, "audience": "user", "secret": {"key": "STAGING_API_KEY", "profile": null, "config": null}}
```

Finding records contain `kind` (`issue` or `opportunity`), `detail`, and nullable `path`. A finding's answer can carry an `action`: `create_and_run`, `create_only`, or `close`; the board sets it. Optional private source metadata preserves original system, question UUID, comment ID, URL, answer ID, parent question ID, actor, time, revision and applied uncertainty. Historical source approvals never issue current authority.

Writes return JSON `{ "saved": { "record": ..., "durability": "durable" | "uncertain" }, "notification": { "status": "notified" | "failed" | "not_attempted" } }`. A failed notification does not undo the saved answer. An uncertain commit exits 1 and is not notified; visible readback is not proof of durability. Registration hints go to recipient; answers/corrections go to origin Thread, with Task/Q/revision identifiers only. `notified` means Herdr accepted submission, not that the receiver read or applied it. The Lead can query questions after the origin exits. No automatic resend or delivery database is used.

<span id="route"></span>`route` takes `expected_revision`, `audience` (`user` or `lead`), and an optional `lead_recipient` (a Herdr agent name; omitted or `null` keeps the current one). It writes a new revision under the same Q UUID and number and keeps the text, answers, and history; an answered question stays answered. A stale revision refuses and changes nothing. It sends no notification and returns `{ "saved": ... }`.

<span id="ui-answer"></span>`ui-answer` records the answer the user gave in another interface, such as the option they picked or the text they typed in a Claude Code ask prompt, with origin `user_ui` and actor `user`. It is the entrypoint for that interface's own process; agents never call it. Its JSON object takes `expected_revision` (the revision the user was shown) and exactly one of:

| Field | For | Saved as |
| --- | --- | --- |
| `choice` | a question with options | the zero-based option's text |
| `text` | a question | the trimmed text |
| `approve: true` with `approval` | an approval | `approved`, with the terms, only when `approval` equals the stored target, action, `h`, scope, and rollback exactly |
| `reject: true` | an approval | `rejected` |
| `comment` alone | an approval | `commented`, with the comment; nothing approved |
| `reject: true` | an open secret | `rejected`, with no value and no `fnox set` call |

An optional `comment` rides either `approve` or `reject` as free text beside an approval's decision, or answers an approval alone as `commented` (the person replied in words; the terms stay unapproved). It is refused (exit 2) on anything but an approval, or blank, and never approves anything. It is refused the same way on a secret's `reject`.

`corrects_revision` names the current answer's revision to correct a question's answer; an approval is never corrected (ask a new question for new terms), and neither is a secret, set or rejected (ask a new secret question to set it again). Approval terms on anything but an approve are refused: a plain answer never becomes an approval. A finding is decided on the board and is refused here, as is a question without an `audience`. A secret question refuses everything but `reject: true` with `secret-needs-board`: only the board's masked `Enter` can hand a value to fnox. Unknown fields are refused.

It prints one JSON line, `{"question_id", "number", "task_id", "revision", "status": "durable" | "uncertain", "notification": {"status": "notified" | "failed" | "not_attempted"}}`, and sends the origin Thread the same identifier-only hint as `answer`. `uncertain` exits 1: read the record back before acting on it. A refusal prints `tsk ui-answer: <code>: <message>` on stderr and saves nothing; exit 2 for `usage` and `invalid-input` (bad arguments, JSON, or pick), exit 1 for `stale-revision`, `unknown-question`, `unrouted-question`, `finding-board-action`, `secret-needs-board`, `approval-refused`, `invalid-correction`, `agent-store-required`, `private-record-unavailable`, `input-unavailable`, `not-saved`, `commit-unknown`, `store-unavailable`, and `link-ambiguous`. After `stale-revision`, read the question again and show the user its current state; never resend the old pick on a new revision. After `commit-unknown`, read back before retrying.

**Trust boundary.** `ui-answer` is a local entrypoint, not authentication. Any process running as the same OS user can call it, as it can run the board. Keep agents' tool calls away from it with a hook: a Claude Code `PreToolUse` hook that refuses `tsk answer` and `tsk ui-answer` in every session but the Lead's. The Claude Code mod that shows the ask prompt calls it from its own process, outside any agent's tools.

`questions` returns full JSON even without `--json`; an optional Q address selects one record and `--task` filters by Task. Missing or corrupt private snapshots refuse reads and mutations. [Private storage and limitations](/docs/storage/#agent-questions-and-private-records).

## Commands

| Command | Action |
| --- | --- |
| `tsk` | Open the board |
| `tsk capture` | Open capture; save or discard exits |
| `tsk add` | Add a task or JSON plan |
| `tsk list` | Read tasks |
| `tsk status` | Set task status |
| `tsk edit` | Replace title, notes, or parent, or move to another project |
| `tsk ask` / `tsk answer` / `tsk route` / `tsk questions` | File, answer, route, or read task-linked questions |
| `tsk ui-answer` | Record the user's answer from another interface (not for agents) |
| `tsk steps` | Add, toggle, rename, move, or remove steps |
| `tsk archive` / `tsk unarchive` | Hide or restore a task |
| `tsk project archive` / `tsk project unarchive` | Hide or restore a project |
| `tsk trash restore` | Restore a task from trash |
| `tsk import linear` | Import Linear issues as tasks, one way |
| `tsk setup` | Configure Herdr or install an agent skill |
| `tsk update` | Upgrade an installer-managed copy, or print Homebrew guidance |
| `tsk guide` | Print the agent workflow |
| `tsk help [<command>]` | Show the CLI reference or one command's reference |
| `tsk --help` | Show the CLI reference |
| `tsk --version` / `tsk -V` | Print the installed version |

`tsk --help` is the syntax reference. `tsk help` prints the same reference, and `tsk help <command>` is identical to `tsk <command> --help`. Help always wraps at 80 columns, including redirected output. Internal Herdr launcher flags are intentionally absent.

Data commands accept `--state-dir <dir>`. Setup, update, and guide do not use that flag.

### Statuses

| Status | Meaning |
| --- | --- |
| `todo` | Captured, not yet in progress |
| `doing` | Work in progress |
| `done` | Closed |

### Task addresses

`T12`, `t12`, `12`, and a task UUID identify the same task. Direct lookup ignores the current project.

Board notice tasks paint as `N1`… (starter tasks and `What's new in tsk`). Those ids are board-only: the CLI does not accept `N1`, and default `tsk list` omits them. Agents ignore them.

### Scope

Without a scope flag, `add` and filtered `list` use the launch repository inside Git, or desk outside Git. Commands addressed to a task ignore that default. The outside-Git current directory remains available through `-p /full/path`.

| Flag | Scope |
| --- | --- |
| `--desk` | Desk |
| `-p name` / `--project name` | Existing project uniquely matching that basename, ignoring case |
| `-p /path` or `-p C:\path` | Existing absolute directory, creating a project there if needed |
| `--all` on list | All scopes |

For adds, a bare project name must match exactly one project the board already knows: a project with tasks, an archived project, or the repository you launched from. Missing and ambiguous names refuse with `unknown-project`, for example `tsk add: unknown-project: project atlss is not on the board`. A new project destination requires an existing absolute directory (`/…` or `~/…` on macOS/Linux, `C:\…` or `~\…` on Windows); relative and nonexistent paths refuse. Project-filtered reads remain permissive, so `tsk list -p typo` returns an empty list.

## add

```sh
tsk add -t "Fix login timeout" -n "Reproduce on a slow connection"
# new tasks start todo; use `tsk status T<n> doing` when work begins
tsk add -t "Buy coffee" --desk
tsk add -t "Draft release notes" -p atlas --json
```

| Option | Purpose |
| --- | --- |
| `-t`, `--title` | Required task title |
| `-n`, `--notes` | Optional notes |
| `-p`, `--project` or `--desk` | Destination |
| `--parent <task>` | Optional direct parent, addressed by task number or UUID |
| `--after <task>` | Wait for this task to be done before starting; repeat for several |
| `--json` | One result object |
| `--file <path>` or `--file -` | Read a JSON plan |
| `--state-dir <dir>` | Alternate state directory |

Duplicate detection compares the trimmed title and resolved project. A matching non-deleted task succeeds without changing the task (`outcome: existing` in JSON), even if done or individually archived. Adding into an archived project refuses.

Plain output: `added <title>` or `task already exists`.

JSON output: `outcome` (`created` or `existing`), `id`, `number`, `title`, and `project` (`null` for desk).

Values starting with `-` use equals syntax: `--title="-fix parser"`, `--notes="-5 degrees"`, `--project="-maintenance"`, `--file=...`, or `--state-dir=...`.

Blank notes are omitted. C0 control characters in titles are rejected before trimming.

### JSON plans

```json
[
  {"title": "Reproduce login timeout", "project": "atlas"},
  {"title": "Draft release notes", "notes": "Include migration instructions"}
]
```

```sh
tsk add --file plan.json
cat plan.json | tsk add
```

| Field | Meaning |
| --- | --- |
| `title` | Required string |
| `notes` | Optional notes |
| `project` omitted | Invocation default |
| `project: null` | Desk |
| `project` string | Project name or path |
| `parent` omitted or `null` | No parent |
| `parent` string | Existing parent task number or UUID |

Tasks no longer carry a thread: an item with a non-null `thread` is refused as `invalid-item`.

Output contains `created`, `existing`, and `failed` arrays. Items carry their input index `i`; failures include `code` and `error`. Successful entries include task ID, number, and title. Notes are not echoed.

Valid items persist even if another item fails. Retry only failed or confirmed-missing items.

Do not mix item flags with `--file`. Piped input is ignored when item flags are present.

A leading UTF-8 byte order mark, which Windows PowerShell 5.1 writes by default, is ignored.

## list

```sh
tsk list
tsk list T12
tsk list --all --json
tsk list -p atlas
tsk list --done --all
tsk list --done --history --all
tsk list --todo --all
tsk list --doing --all
tsk list --archived --all
tsk list --deleted --all
```

```text
tsk list [<task>] [-p <project> | --desk | --all] [--todo | --doing | --done [--history] | --deleted | --archived] [--json] [--state-dir <dir>]
```

| Filter | Result |
| --- | --- |
| Default | Todo and Doing tasks; excludes archived, deleted, and notice (`N`) tasks |
| `--todo` | Tasks with status `todo` |
| `--doing` | Tasks with status `doing` |
| `--done` | Completed tasks still in `tsk.json` (done within the last day by default) |
| `--history` | With `--done`: also the non-archived done tasks moved to the dated files under `done/`, in the same order; `-p` also matches projects known only from those files. See [storage](/docs/storage/#older-done-tasks). |
| `--archived` | Individually archived tasks and tasks in archived projects, across statuses, including tasks moved to the dated files under `done/` |
| `--deleted` | Live soft-deleted tasks and `trash.jsonl` entries, deduped by task with the live copy winning, newest deletion first. Trash is retained for 30 days, see [storage](/docs/storage/#deleted-tasks). |

Scope flags are mutually exclusive. So are `--todo`, `--doing`, `--done`, `--deleted`, and `--archived`.

A direct task address searches the main store, including done, archived, and recently deleted tasks, then the dated done files. It cannot be combined with scope or status filters. A missing task exits 2. A task already moved to `trash.jsonl` is not addressable; find it with `tsk list --deleted` (optionally `-p`), or restore it with `tsk trash restore`.

Human output groups by status in `DOING`, `TODO` order. Todo rows use oldest capture first; Doing and Done use most recent status change first. This order is separate from the board, which groups rows by project, in task-number order inside each group. JSON is that flat order. With `--all`, human output groups each status by scope: the groups are ordered by their first row, and the rows inside each group keep the board order, so interleaved scopes print in a different order from JSON; filtered rows include the task number, and `--all` adds scope labels using a unique concise trailing path or desk. List output and errors wrap to the attached terminal width with hanging indentation. Task rows, scope labels, notes, steps, and archived marks use the same wrapping behavior, supported from 50 columns. Redirected output keeps stored logical lines. Command help is reference text and instead always wraps at 80 columns.

Single-task output presents notes and steps as separate blocks. A blank line separates adjacent blocks that exist. The steps block opens with its progress, such as `Steps: 1/3 done (33%)` (rounded down; counted, not saved; it never completes the task). Human step rows show state and text without machine-oriented short IDs.

JSON returns an array with `id`, `number`, `title`, `status`, and `project`. Direct lookup returns the complete task, including `notes` (`null` when absent) and `steps` (an empty array when absent). Its fields are ordered `id`, `number`, `project`, `status`, `title`, `notes`, `steps`, `parent_task_id`, `parent`, `children_done`, `children_total`, `children`; each JSON step retains its `short_id` for step commands. Archived listings include an `archived` mark: `archived` or `project archived`.

Relationship fields add `parent_task_id` (`null` for no parent), `children_done`, and `children_total` to JSON rows. Direct lookup also returns `parent` (resolved task summary or unresolved UUID) and `children` (direct child summaries). Human direct lookup shows the same relationship blocks. Counts are calculated, not saved. Direct children include archived tasks and tasks in archived projects, across all statuses and scopes; deleted tasks and notices are excluded. A Done or archived parent remains resolved.

## status

```sh
tsk status T12 todo
tsk status T12 doing
tsk status T12 done
```

Accepts only `todo`, `doing`, and `done`. Legacy values and the `start` alias refuse with a usage error and do not write the store. Questions and Herdr execution status do not alter Task status.

This command sets the requested status directly. Repeating the same value is safe.

Output: `status T12 <status> <title>`.

## edit

```sh
tsk edit T12 --title "Fix timeout on slow connections"
tsk edit T12 --notes "Reproduced with a delayed response"
```

Requires at least one of `--title`, `--notes`, `--parent <task>`, `--no-parent`, `--after <task>`, `--no-after`, `-p <project>`, or `--desk`. Scope changes only with `-p`/`--project` or `--desk`, which move the task to that project or the desk. The destination resolves like [`add`](#scope): a bare name must match one known project, a new project needs an existing absolute directory, and an archived project refuses with `project-archived`. `-p` and `--desk` cannot be combined. Moving keeps the task's number, status, notes, steps, and parent. `--parent` replaces the direct parent; `--no-parent` clears it. These two options cannot be combined. A parent must exist and must not be deleted or a notice. Self-parenting and cycles refuse without saving any requested fields.

`--after <task>` (repeatable) replaces the wait list and `--no-after` clears it. A task with unfinished predecessors cannot start: `tsk status <task> doing` refuses with `waiting-for-after` and writes nothing, and it works once every predecessor is done (an archived done task counts). `tsk list` appends `· after T<n>` to the task's row for each predecessor not done yet, and JSON rows carry `waiting_for` (task numbers, omitted when empty). A predecessor must exist and not be deleted or a notice; the task itself and a circle refuse with `invalid-after` or `after-cycle`. It is only a condition: nothing starts by itself, and parent and children stay unordered.

```sh
tsk add -t "Implement parser" --desk --parent T10
tsk edit T12 --parent T11
tsk edit T12 --no-parent
tsk edit T12 -p ~/src/atlas
tsk edit T12 --desk
```

Each task has at most one parent, stored by UUID, not its display number. Duplicate adds leave the existing task and its parent unchanged. Changing a parent's status, archiving it, or deleting it never changes its children. A deleted or missing parent is shown as unresolved while its UUID stays on the child. Parents are never reassigned automatically. Child completion never completes a parent.

Blank notes clear the field. Notes preserve newlines and tabs. Use `--title=...` or `--notes=...` for values starting with `-`.

Output: `edited T12 <title>`. Repeating the same values is safe.

## steps

```text
tsk steps <task> add <text> [<place>] [--state-dir <dir>]
tsk steps <task> toggle <step-short-id> [--state-dir <dir>]
tsk steps <task> rename <step-short-id> <text> [--state-dir <dir>]
tsk steps <task> move <step-short-id> <place> [--state-dir <dir>]
tsk steps <task> remove <step-short-id> [--state-dir <dir>]
```

`<place>` is one of `--before <step-short-id>`, `--after <step-short-id>`, `--first`, or `--last`. `add` appends without one; `move` requires one. Other actions refuse a place as a usage error (exit 2).

Read short IDs with `tsk list T12 --json`. Full step UUIDs also work. A moved step keeps its ID, short ID, and checked state.

| Action | Output prefix | Safe to repeat unchanged? |
| --- | --- | --- |
| Add | `added <short-id> <text>` | No; creates another step |
| Toggle | `toggled <short-id> [x] <text>` | No; flips the value again |
| Rename | `renamed <short-id> <text>` | Yes |
| Move | `moved <short-id> to <position>/<count> <text>` | Yes; an unchanged position writes nothing |
| Remove | `removed <short-id> <text>` | No; refuses after removal |

Read back with `tsk list T12 --json` before retrying an uncertain result. [Using steps on the board](/docs/steps/).

## archive and unarchive

```sh
tsk archive T12
tsk unarchive T12
```

Keep the task's status and number. Archiving removes it from working views, and it remains available through `tsk list --archived` until `tsk unarchive` returns it. Repeating either command is safe.

Output: `archived T12 <title>` or `unarchived T12 <title>`.

Unknown tasks refuse with `T12 is not on the board`; deleted tasks with `T12 is deleted`.

## project archive / unarchive

```sh
tsk project archive atlas
tsk project unarchive atlas
```

Accepts a project basename or path. Archiving removes the whole project from working views, while unarchiving returns its tasks in their existing statuses. Repeating an action is safe. An unknown project refuses with `no project named <name> has tasks`.

Restoring a project preserves task statuses and leaves individually archived tasks archived.

Output: `archived project <name>` or `unarchived project <name>`.

## trash

```sh
tsk list --deleted --all
tsk trash restore T12
```

Restore returns a task from trash with its original number, no soft-delete flag, a restored event, and a new revision. A task absent from trash, or already live, refuses with `T12 is not in trash`.

Recent deletions may still be in the main store; use board undo until they move to trash. [Retention and storage](/docs/storage/#deleted-tasks).

## import

```sh
export LINEAR_API_KEY=lin_api_…
tsk import linear --dry-run
tsk import linear --team ENG --desk
tsk import linear --assignee any --linear-project atlas -p atlas --json
tsk import linear --state backlog,unstarted,started,completed --update
```

```text
tsk import linear [--team <key>] [--linear-project <name>] [--assignee me|<email>|any] [--state <types>] [--limit <n>] [-p <project> | --desk] [--update] [--dry-run] [--json] [--state-dir <dir>]
```

One-way import from [Linear](https://linear.app): reads issues through Linear's GraphQL API and creates a task for each one that is not on the board yet. Nothing is ever written back to Linear. Set `LINEAR_API_KEY` to a Linear personal API key (Linear: Settings › Security & access). On macOS and Linux the request runs through the system curl (`TSK_UPDATE_CURL` overrides its path, see [storage](/docs/storage/#update-check)); the key reaches curl on stdin, never on its command line. Import ignores curl's default configuration file so trace or verbose settings cannot log the key. Transport failures report only the curl exit status, not its raw diagnostic output. Windows is not supported yet.

| Option | Purpose |
| --- | --- |
| `--team <key>` | Only this team, by key (such as `ENG`) |
| `--linear-project <name>` | Only this Linear project, by name, ignoring case |
| `--assignee me\|<email>\|any` | Whose issues; default `me` (the key's owner) |
| `--state <types>` | Comma-separated Linear state types: `triage`, `backlog`, `unstarted`, `started`, `completed`, `canceled`; default `backlog,unstarted,started` |
| `--limit <n>` | Fetch at most `n` issues, following pages; default 100 |
| `-p`, `--project` or `--desk` | Destination for new tasks, resolved like [`add`](#scope) |
| `--update` | Refresh the title and status of existing copies from Linear |
| `--dry-run` | Report what would happen; write nothing |
| `--json` | One report object |
| `--state-dir <dir>` | Alternate state directory |

`-p`/`--project` is the tsk destination, as on every other command; the Linear project filter is `--linear-project`.

**Mapping.** The title is `<IDENTIFIER> <title>`, for example `ENG-123 Fix login timeout`. Notes start with a `Linear: <IDENTIFIER> <url>` line, then a blank line and the issue description, cut after 2000 characters with a truncation note. Status follows the Linear state type:

| Linear state type | Task status |
| --- | --- |
| `triage`, `backlog`, `unstarted` | `todo` |
| `started` | `doing`, including review-named states |
| `completed` | `done` |
| `canceled` (including duplicates) | Skipped, counted in the report |

**Direct parent.** When Linear has a parent, new task notes also contain `Parent: Linear <IDENTIFIER> <url>`. If that parent's imported copy already exists, or is created in the same import batch, the new child links to its tsk UUID. Otherwise only the external reference is kept. Import never fetches parents recursively or reconciles an existing child when a parent appears later. `--update` preserves both user notes and the stored parent relation. GitHub references use `Parent: GitHub <repo>#<num> <URL>` with the same meaning; GitHub issue intake is not provided by this command.

**Existing copies.** A task whose notes contain a line starting with `Linear: <IDENTIFIER> ` is that issue's copy, in any scope and with any status, including done and archived; soft-deleted tasks do not count. An issue with a copy is never added again. By default the copy is left untouched, since the human status on the board is the source of truth. With `--update`, a copy whose title or status differs from Linear is refreshed (notes and scope stay as they are). A whole run, creates and updates together, is one store write.

Plain output starts with `fetched <n> · created <n> · updated <n> · unchanged <n> · skipped <n>`, then one line per issue. With `--dry-run` it starts with `dry run, nothing written` and says `would create` / `would update`.

JSON output is one object: `dry_run`, `fetched`, and four arrays. `created`, `updated`, and `unchanged` rows carry `identifier`, `number` (`null` for a dry-run create), `title`, and `status`; `skipped` rows carry `identifier` and the Linear `state` type.

```json
{"dry_run":false,"fetched":3,"created":[{"identifier":"ENG-123","number":14,"title":"ENG-123 Fix login timeout","status":"doing"}],"updated":[],"unchanged":[{"identifier":"ENG-120","number":9,"title":"ENG-120 Draft release notes","status":"doing"}],"skipped":[{"identifier":"ENG-99","state":"canceled"}]}
```

Every issue is fetched before anything is written, so a missing key, a network or HTTP failure, or a GraphQL error refuses with exit 1 and saves nothing. Exit 3 is reserved for a store error during the single write.

## help and version

```sh
tsk --help
tsk help list
tsk list --help
tsk --version
```

Use top-level help to find a command, then use either one-command form for its flags, examples, refusals, and exit contract. `--version` and `-V` print `tsk <version>`. They are global flags, so place them before a command.

## update

```sh
tsk update
```

For an installer-managed copy, downloads and runs the official installer for the latest stable release, then refreshes an already registered Herdr plugin and any outdated installed agent skills ([details](/docs/install/#upgrade)). Windows stages the verified executable, returns after starting a detached helper, and replaces `tsk.exe` after the running process exits, so success means installed or staged. For a Homebrew copy, it prints `brew update && brew upgrade tsk`; Homebrew remains responsible for its own upgrades. Reopen a running board after an upgrade.

## setup

```sh
tsk setup
tsk setup herdr
tsk setup agents --yes
tsk setup pi
tsk setup --skill-dir /path/to/skills
tsk setup --detected-ids
tsk setup --skill-states
tsk setup herdr --check
```

On a TTY, bare `tsk setup` detects global agent skill roots and asks once to install or update the embedded skill for every detected agent. Without a TTY it prints guidance (and the list of named targets) and does not hang. `tsk setup --json` prints a detection report. Two plain probes exist for installers: `tsk setup --detected-ids` prints space-separated detected agent ids, and `tsk setup --skill-states` prints an `embedded <version>` line followed by one tab-separated `id state version path` line per detected agent (`missing`, `current`, `outdated`, `blocked-symlink`).

### Herdr

Requires Herdr 0.9+ on PATH; Herdr's supported `0.9.0-preview.*` Windows builds qualify. Registers the installed binary and adds **prefix+t** and **prefix+a**. A plugin command you already bound to another key is left alone; setup never adds the default chord beside it. Shortcut conflicts require confirmation; noninteractive conflicts stop before writes. `tsk setup herdr --check` prints `bound` when both plugin commands are already in the config (on any keys), otherwise `unbound`, and changes nothing. On macOS/Linux it uses `HERDR_CONFIG_PATH`, then `XDG_CONFIG_HOME/herdr/config.toml`, then `~/.config/herdr/config.toml`. On Windows it uses `HERDR_CONFIG_PATH`, then `%APPDATA%\herdr\config.toml`, with `%USERPROFILE%\AppData\Roaming\herdr\config.toml` as the fallback. A symlinked config file or config directory is followed and its target edited in place; a dangling or looping link is refused. Windows setup installs PowerShell launchers; Herdr's Windows plugin support is preview.

[Reload, upgrades, and removal](/docs/install/#herdr-setup-with-an-installed-binary).

### Agent skills

| Target | Skills directory |
| --- | --- |
| `pi` | `~/.pi/agent/skills/` |
| `omp` | Active OMP profile's user skills directory (normally `~/.omp/agent/skills/`) |
| `claude` | `~/.claude/skills/` |
| `cursor` | `~/.cursor/skills/` |
| `grok` | `~/.grok/skills/` |
| `codex` | `~/.agents/skills/` |
| `opencode` | `~/.config/opencode/skills/` |
| `--skill-dir <path>` | The supplied directory |

Setup writes `tsk-cli/SKILL.md` under the selected directory. OMP follows `OMP_PROFILE`, using legacy `PI_PROFILE` only when `OMP_PROFILE` is unset; an empty, whitespace, or `default` value explicitly selects the default profile. It also follows `PI_CONFIG_DIR` and the default profile's `PI_CODING_AGENT_DIR`; invalid profile names are refused. Skill `version:` is independent of the crate version. A matching installed version exits 1 with `skill-exists`; a missing or different version updates without `--force`. `--force` always overwrites. `tsk setup agents --yes` installs or updates every detected agent without asking; `tsk setup agents --yes --force` also rewrites copies whose version already matches. `--force` without `--yes` is a usage error for `agents`. Only global skill roots are offered (project-local roots are out of scope).

| Option | Action |
| --- | --- |
| `--yes` | With `agents`, install/update detected agents without asking |
| `--force` | Replace an existing skill even when versions match |
| `--json` | Machine-readable outcome |
| `--detected-ids` | Print detected agent ids (installer use) |
| `--skill-states` | Print each detected agent's skill state (installer use) |

Symlinks at the skills root, skill directory, or file are refused. Herdr setup does not accept these agent options.

Setup exits 0 for success/help/list/detection, 1 for setup failure or `skill-exists`, or 2 for invalid arguments.

## guide

```sh
tsk guide
```

Prints the embedded [agent skill](/docs/agents/) without YAML frontmatter. Exits 0. It is the same workflow installed by agent setup.

## Exit contract

For data commands:

| Exit | Meaning | Next step |
| --- | --- | --- |
| `0` | Success, including an already-existing task or unchanged value | Continue |
| `1` | Refusal; a plan may have saved other items | Correct refusals; retry only failed items |
| `2` | Invalid arguments or input; nothing saved | Fix the invocation |
| `3` | Storage error; a write may have committed | Read back before retrying |

After an uncertain add, inspect `tsk list --all --json`. Also check `--done` and `--archived` when a duplicate could be hidden there. If you know the task number, use direct lookup.

| Command | Refusal codes |
| --- | --- |
| Add | `empty-title`, `invalid-title`, `invalid-item`, `unknown-project`, `project-archived` |
| Steps | `empty-step-text`, `invalid-step-text`, `unknown-task`, `soft-deleted-task`, `unknown-step`, `ambiguous-step`, `same-step` |
| Status | `unknown-task`, `soft-deleted-task` |
| Edit | `unknown-task`, `soft-deleted-task`, `empty-title`, `invalid-title`, `invalid-parent`, `parent-cycle`, `unknown-project`, `project-archived` |
| Archive / unarchive | `unknown-task`, `soft-deleted-task` |
| UI answer | `stale-revision`, `unknown-question`, `unrouted-question`, `finding-board-action`, `approval-refused`, `invalid-correction`, `invalid-input` ([full list](#ui-answer)) |
| Import | `missing-api-key`, `linear-error`, `unsupported-platform`, `unknown-project`, `project-archived` |

A refusal prints as `tsk <command>: <code>: <message>` on stderr, for example `tsk status: unknown-task: T99 is not on the board`. Branch on the code; the message is for people and may change.

`--thread` is no longer an option and fails argument parsing with exit 2. Unknown or archived project refusals save nothing for that item; other valid plan items can still save.

Human-readable output escapes stored terminal control characters. JSON retains the underlying text.

## Herdr helper

`tsk --find-board-pane` reads Herdr `pane list` JSON from stdin and prints the ID of the pane labelled `tsk`. It exits 1 if none matches.
