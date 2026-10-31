---
title: Storage
description: Where tsk keeps tasks and settings, config.toml, git backups and restore, deleted tasks, and the update check.
---

Where tsk keeps your tasks and settings, how to configure it, and how to back it up. The board, the CLI, and the Herdr plugin share one store.

## Location

| Platform | State directory |
| --- | --- |
| macOS, Linux | `~/.tsk` |
| Windows | `%LOCALAPPDATA%\tsk`, or `%USERPROFILE%\.tsk` when `LOCALAPPDATA` is not set |

- `TSK_STATE_DIR` moves it. `--state-dir <dir>` moves it for one data command.
- With no home directory and no `TSK_STATE_DIR`, tsk refuses to run rather than pick a directory.
- Inside Herdr, tsk uses the same directory, so the plugin and a bare terminal show the same board.
- Keep it on a local disk. Network drives and synced folders such as Dropbox, iCloud Drive, or OneDrive are not supported; use a [git backup](#git-backups) instead.
- It must be a real directory, not a symlink or a Windows junction.

## Files

| File | Contains |
| --- | --- |
| `tsk.json` | Your tasks, including archived tasks and projects |
| `tsk.json.1` | The previous good copy of `tsk.json` |
| `tsk.json.v<N>` | The store as it was before tsk upgraded its format ([store version](#store-version)) |
| `tsk.json.pre-restore-<time>` | The store a [restore](#restore) replaced; the newest five are kept |
| `config.toml` | Your [settings](#configuration) |
| `trash.jsonl` | [Deleted tasks](#deleted-tasks) |
| `done/` | [Older done tasks](#older-done-tasks) |
| `trash.jsonl.pre-v16`, `done/*.jsonl.pre-v16` | Original archive bytes before the first rewrite |
| `*.after-migration.json` | Planned removed UUID links beside each original migration backup ([store version](#store-version)) |
| `delivery.json` | Which [starter tasks](#starter-tasks) and release notes this directory has received |
| `backup-status.json` | Each backup target's last success, last attempt, and last error |
| `machine-id` | This directory's id, which names its folder in a backup repository. Set once, from the host name |
| `backup-repos/` | Local clones of backup targets |
| `update.json` | The cached [update check](#update-check) |
| `fetch-stamps.json` | When each repository's remote was last fetched. Dispatch, the base picker, and cleanup reuse a fetch from the last 60 seconds. Safe to delete |
| `launchers/` | Dispatch launchers. Each one deletes itself once its agent starts |
| `cleanups/` | Marks of worktree removals under way. After an interrupted removal, the next cleanup refuses and says how to finish by hand |

## Read-only consumers

These queries work with a readable store that has no write access. They require no extra flag.

| Command | Reads |
| --- | --- |
| `tsk list --all --json` | Live tasks across projects and the desk |
| `tsk reports --json` | Completion reports |
| `tsk report T12 --json` | One completion report |
| `tsk questions --inventory --json --state-dir <Agent-dir>` | Questions, including markers for unavailable private records |

Reads do not create files, change permissions, or acknowledge reports. Each query reads published snapshots; separate queries can observe different revisions. Writes still require write access.

An unreadable store returns an error, not an empty result. A missing store returns an empty list; looking up a missing report refuses.

## Machines

New tasks record the current machine. Existing tasks without a machine stay unspecified, including after edits or restore. Common notices have no machine.

The default name is the host name's first label, lowercased and limited to 32 characters. Non-ASCII letters and punctuation become hyphens. `TSK_MACHINE` overrides it with 1–63 ASCII letters, digits, dots, or hyphens, starting with a letter or digit. Names are lowercase.

Machines with the same host name need distinct overrides. Renaming a host or changing the override does not move existing tasks. Use [`tsk edit --machine`](/docs/cli/#edit) to move them explicitly. The backup directory's `machine-id` is separate from task machine names.

## Configuration

`config.toml` in the state directory holds agent profiles, project settings, the merge method, board defaults, and [backup targets](#settings).

- The first full board open writes a starter file when none exists. It lists every setting of that version with its options and default, and holds commented examples that take effect only when you uncomment them.
- Quick capture, the CLI, and setup never create the file. tsk never replaces an existing file, even an empty one.
- tsk never edits profiles or other settings: edit the file yourself. Only the [backup verbs](#git-backups) write to it, adding and removing `[[backup]]` entries.

### Agent profiles

Each `[agent.<name>]` table is a profile you can [assign](/docs/dispatch/#assign) to a task.

```toml
[agent.implementer]
command = ["pi"]
prompt = "Work on T{number}: {title}\n\n{notes}\n\n{steps}"

[agent.implementer.env]
PI_PROVIDER = "anthropic"
```

| Key | Required | Value |
| --- | --- | --- |
| `command` | Yes | The program and its arguments, as a list of strings |
| `prompt` | No | The agent's first message. Without it, tsk uses its default prompt |
| `prompt_append` | No | Text added after the default prompt. A profile sets `prompt` or `prompt_append`, not both |
| `env` | No | Environment variables for this agent only, as strings |

- A name follows the thread rules: lowercase letters, digits, `-` and `.`, starting with a letter or digit, up to 32 characters. Quote a name with dots: `[agent."review.strict"]`.
- tsk sets `TSK_AGENT` to the profile name, so the agent's blocks, reviews, and replies carry that name.

### Prompt and placeholders

The prompt goes to the command as its last argument. `command`, `prompt`, and both `prompt_append` keys can use these placeholders; tsk replaces nothing else.

| Placeholder | Value |
| --- | --- |
| `{number}` | Task number, without the `T` |
| `{title}` | Title |
| `{notes}` | Notes |
| `{steps}` | Steps, one per line as `[ ]` or `[x]` |
| `{worktree}` | The task's worktree path |
| `{branch}` | The task branch |
| `{base}` | The short base branch name, such as `dispatch` for `origin/dispatch` |
| `{on}` | Who the agent hands review to: the project's [`review_on`](#project-settings), or `you` |

Without `prompt`, tsk sends this default. A profile's own `prompt` replaces it entirely. To keep the default and add to it, use `prompt_append`.

```text
You were dispatched to T{number} ({title}) in worktree {worktree} on branch {branch}, based on {base}.

1. Run `tsk guide`, then `tsk list {number} --json`. The task notes are your brief.
2. Read the repo's agent instructions (AGENTS.md or CLAUDE.md) if present.
3. Work only on {branch}. Run the project's checks before saying you are done.
4. Push and open a pull request into {base}. Never merge it.
5. Set the task to review with what you did and what to check: `tsk status {number} review --on {on} --done "…" --check "…"`, one `--check` per thing to verify, adding `--next` for what comes after. When you need a decision, block it with your question: `tsk status {number} blocked --on {on} --why "…"`, adding `--needs` and one `--option` per choice, then stop. If your notes name someone else to hand it to, use that name instead of {on}.
6. On a relaunch, first read whichever record closed last (`closed_at`) in `tsk list {number} --json`: the `replies` of the last `past_blocks` entry, or the `feedback` and failed `checks` of the last `past_reviews` entry when it was `sent_back`.
7. A message `[tsk T{number} sent back] …` is review feedback, with any failed checks: address it, then set review again.
8. Never set the task to done; the person who reviews it does.
```

The prompt an agent gets is built in this order, with a blank line between parts:

1. The profile's `prompt`, or the default.
2. The project's `prompt_append`, for a task in that project.
3. The profile's `prompt_append`.

[`tsk dispatch T12 --show-prompt`](/docs/cli/#dispatch) prints the result for one task without launching anything.

### Project settings

A `[project."<path>"]` table holds dispatch settings for one repository. It lives only in your `config.toml`, so your workflow never lands in the repository.

```toml
[project."~/code/my-app"]
review_on = "coordinator"
prompt_append = "Run `make check` before review."
```

| Key | Value |
| --- | --- |
| `review_on` | Who agents hand review to, the `{on}` placeholder. `you` (the default), an agent profile's name for that agent, or any other name, as [`--on`](/docs/cli/#status) reads it |
| `prompt_append` | Text added after the prompt, before the profile's own `prompt_append` |

- The key is the repository's path. `~` is your home folder, and a trailing slash or a symlinked spelling of the same folder still matches.
- When two keys name the same repository, the first one in the file wins.
- Desk tasks have no project, so they get no project settings and `{on}` is `you`.
- An unknown key, or a `review_on` that is not a valid name, is an [error](#errors) like a broken profile.

How tsk launches the command:

- **macOS and Linux:** the command and `env` go into a private launcher file that runs once, then deletes itself. The pane shows only `/bin/sh "…/launchers/dispatch-….sh"`. The command starts with your login shell's environment, so any shell works. No task text or `env` value is typed into your shell or its history.
  - The program must be an executable on your login shell's `PATH`. Shell functions and aliases are not used.
  - `env` names must be variable names: letters, digits, and `_`, not starting with a digit.
  - Dispatch refuses a [state directory](#location) whose path contains `$`, `` ` ``, `\`, `"`, or `!`.
- **Windows ([preview](/docs/dispatch/#windows-preview)):** the same command and `env` run through Windows PowerShell. Write paths as TOML literal strings so backslashes stay as typed: `command = ['C:\Users\you\.local\bin\claude.exe']`.

### Merge method

`m` on the [cleanup card](/docs/dispatch/#merge-the-pull-request), and `--merge` on [`tsk status … done`](/docs/cli/#status) and [`tsk clean`](/docs/cli/#clean), merge the task's pull request with the GitHub CLI. The `[merge]` table picks how:

```toml
[merge]
method = "squash"   # "merge" (the default), "squash", or "rebase"
```

### Board defaults

The `[board]` table controls [live lines](/docs/board/#rows) under task rows and the name the board calls you.

```toml
[board]
messages = "hidden"       # shown (the default) or hidden
me = "sam"                # your name on the board; the default is you
```

| Key | Effect |
| --- | --- |
| `messages` | Sets live-line visibility when a board opens; `z` toggles it for the session |
| `me` | Replaces `@you` in assignee labels, pickers, waiting-on labels, replies, and the paper trail |
| `group`, `sort`, `project_group`, `project_sort` | Accepted for configuration compatibility, but do not change this board's layout |

- Sentences addressed to you keep "you". `tsk list --json` still says `you`.
- The CLI takes `you`, `me`, or your name for yourself, in `--assignee` and `--on`.
- The name follows the [thread name](/docs/capture/#thread-names) rules.
- A name that is also an agent profile is an error: the board reports it and calls you `you`.

### Errors

An unknown key or a bad value in one table never blocks the others. A file that is not valid TOML breaks them all.

| Mistake | What happens |
| --- | --- |
| Unknown top-level key or table | Ignored, so a file written for a newer tsk still loads |
| Unknown key inside `[agent.<name>]`, `[project."<path>"]`, `[merge]`, `[board]`, or `[[backup]]` | That table is an error |
| Broken profile or `[project."<path>"]` | The board opens without profiles and shows the error on its status row. `tsk add` and `tsk edit` with an assignee exit 2 without saving |
| Broken `[merge]` | Only merging refuses: the cleanup card says why, and the CLI refuses with `merge-config` |
| Broken `[board]` | The board shows live lines by default and reports the error. A bad `me` leaves visibility unchanged and names you `you` |
| Broken `[[backup]]` | No backup runs. Every full board open shows `backup off:` and the error on its status row. `tsk backup`, `tsk backup list`, and `tsk restore` exit 1 with the error |

## Git backups

`tsk.json.1` and `tsk.json.v<N>` sit on the same disk as the store. A backup target keeps copies in a git repository somewhere else, such as a private GitHub repository.

```sh
tsk backup setup                                  # make (or reuse) a private GitHub repository and add it
tsk backup add git@gitlab.com:me/tsk-backup.git   # any other git repository
tsk backup list                                   # targets, last backup, last error
tsk backup                                        # back up to every target now
tsk backup remove 2                               # by number in list, URL, or path
```

Every flag: [`tsk backup`](/docs/cli/#backup).

### Set up with GitHub

`tsk backup setup` needs the [GitHub CLI](https://cli.github.com) (`gh`), installed and logged in with `gh auth login`. On Windows, install it with `winget install GitHub.cli`.

It uses `<your GitHub user>/tsk-backup`, or the repository you name with `--repo`: `name`, `owner/name`, or `host/owner/name` for GitHub Enterprise.

| The repository | Setup |
| --- | --- |
| Does not exist | Creates it, private and empty |
| Is private and empty, or already holds tsk backups | Uses it |
| Is private and holds other content | Refuses |
| Is internal (enterprise) | Uses it, and notes that everyone in the enterprise can read it |
| Is public | Refuses: backups hold your task notes |

Setup then adds the repository like `tsk backup add`, using the clone URL that matches your `gh` git protocol. Running it again changes nothing. On a new machine with an empty store, it says when the repository holds other machines' backups and suggests `tsk restore`. It never restores by itself.

### Add any repository

`tsk backup add` takes a git URL (`git@host:me/repo.git`, `https://…`, `ssh://…`, `file://…`) or the path of a local repository.

- It runs the first backup straight away and prints where it went. With nothing to back up yet, it still clones the repository, so a wrong URL shows at once.
- If the first backup fails, the target stays in `config.toml` and the command exits 1. Fix the problem, then run `tsk backup`.
- Adding a target that already exists only updates its `every`.

A local bare repository inside a Dropbox, iCloud Drive, OneDrive, or Google Drive folder works for **one machine only**. Two machines pushing to the same synced folder can corrupt it. For several machines, use a hosted repository.

### Settings

The backup verbs write `[[backup]]` entries to `config.toml`. You can also edit them by hand:

```toml
[[backup]]
url = "git@github.com:me/tsk-backup.git"
every = "save"             # "save" (the default) or "daily"
```

| Key | Default | Meaning |
| --- | --- | --- |
| `url` | Required | A git URL or a local path |
| `every` | `save` | `save`: after every save. `daily`: on the first save or full board open 24 hours after the last backup; a failed attempt is retried ten minutes later |
| `kind` | `git` | Optional. `git` is the only kind |

### What is pushed

Each machine writes its own folder, `machines/<machine-id>/`, such as `machines/devbox-664a1c4a/`: the host name plus eight random characters. The name is set once and stays when the host is renamed. Two machines with the same host name get different folders.

The folder holds:

- `tsk.json`, byte for byte.
- `config.toml`, with every value under `[agent.<name>.env]` replaced by `"<not backed up>"` and credentials removed from backup URLs. Fill the values in by hand after copying a profile over.
- `machine.toml`: the host name and tsk version.

Each backup is one commit on the repository's default branch, so its history holds every version. Several machines can share one repository.

- Backups are not encrypted and contain your task notes: keep the repository private.
- Trash, dated done history, `questions-private/`, `reports.json`, and migration audit files are not backed up.
- A full Agent board recovery needs a separate, complete state-directory copy, including private question records and reports.
- A store or `config.toml` that does not parse is not pushed. A store unchanged since the last backup is not pushed again.
- tsk never deletes a machine folder that stopped receiving backups, or a clone under `backup-repos/`. Delete them by hand when you no longer need them.

Once the store holds 10 tasks and no target is set, one board open says `No backup yet: run tsk backup setup` on the status row. It says this once per state directory.

### Troubleshooting

Saves and board opens never wait for a backup: when a target is due, tsk backs it up in the background.

- A failure shows in `tsk backup list`, and on the status row at every full board open until the target succeeds.
- A broken `[[backup]]` table stops every backup until you fix it ([errors](#errors)).
- `tsk backup` runs every target in the foreground and exits 1 if any failed.
- Background backups have no terminal, so git must reach the repository without a prompt. After a first backup that worked, `add` and `setup` check that a background backup can reach it too, and say so when it cannot.
- The usual cause is an ssh key with a passphrase and no ssh-agent the board can see, for example a board started in Herdr from a shell without `SSH_AUTH_SOCK`. Load the key into an agent every shell sees (on macOS, `UseKeychain yes` and `AddKeysToAgent yes` in `~/.ssh/config`), or use https with a credential helper.
- tsk runs your own `git` with your credentials. Background backups commit as `tsk`, without signing or hooks.

### Credentials

Prefer ssh or a credential helper to a token in a backup URL. A URL such as `https://me:TOKEN@github.com/me/tsk-backup.git` works, with these limits:

- `config.toml` keeps the URL as you wrote it, token included.
- Backups never push the token: the pushed `config.toml` has it removed.
- Every command prints the URL with its credentials as `***`, such as `https://***@github.com/me/tsk-backup.git`. `tsk backup remove` takes that form too, unless two targets share it: then give the number or the full URL.

## Restore

```sh
tsk restore             # list versions, then choose one
tsk restore 3f9a1c2e    # restore one version without asking
```

`tsk restore` lists the versions every target holds, newest first, with an id, time, host name, task count, and target. It shows the newest 20 per machine; `--all` shows every one. An id restores the version it names, shown or not. In a terminal it asks which to restore.

- Quit every board and quick capture on the store first: restore refuses while one is open.
- A command still running when the restore lands, such as a `tsk dispatch` mid-launch, keeps nothing and says the store was restored. When its launch was undone, run it again. When an agent it started may still run, check or stop it first.
- The store it replaces is kept as `tsk.json.pre-restore-<time>`, in UTC. The newest five copies are kept.
- A version from an older tsk migrates on the next open. One from a newer tsk is refused until you update.
- Task numbers never go backwards: new tasks continue after the highest number the replaced store or trash held.
- Restore refuses with exit 1 if the snapshot's question references differ from the current store, or a referenced private record is unavailable. It changes nothing.
- Task restore does not roll questions back or restore private answers. Use a separately preserved full state-directory copy for complete Agent board recovery.
- Restore never changes `config.toml`. It prints the `git show` command for the `config.toml` saved with that version, so you can copy settings over by hand.
- A task number such as `T12` is not a version. To bring back a deleted task, use `tsk trash restore T12`.

For a task-only board, add the same repository on a new machine, then restore:

```sh
tsk backup setup        # or: tsk backup add <git-url>
tsk restore
```

## Store version

The board, CLI, and Herdr plugin share one store. The integrated fork writes format v17, which adds optional machine names without assigning older tasks.

Fork v10 and upstream v6 (including upstream dev formats v7–v15) share version numbers but not schemas. Both histories remain supported.

Existing `todo` and `doing` task values keep those wire spellings; upstream `open` and `started` read as aliases. `ready`, `blocked`, and `review` remain distinct. The legacy CLI JSON view keeps `todo`/`doing`/`done` for Agent consumers.

Fork UUID `after` references use the referenced task's store-global number from live tasks, trash, or dated done history. Numeric links stay numeric. A UUID absent from all three is treated as a deleted prerequisite and removed. Reads migrate in memory only and leave original bytes unchanged.

- The first live save keeps `tsk.json.v<N>` before writing the new format.
- The first archive rewrite keeps `trash.jsonl.pre-v16` or `done/YYYY-MM-DD.jsonl.pre-v16`. Retries keep the first copy unchanged.
- `<original-backup>.after-migration.json` records planned removals before replacement: each owning task's number and original UUID, or an empty list. It does not prove replacement succeeded.
- Questions, private snapshots, reports, task UUIDs, revisions, history, steps, archive flags, and parent links are retained by migration.

Stop every writer before upgrading a shared board. To roll back, stop writers again and restore the complete pre-upgrade state-directory copy with the previous binary. Binaries that support only v16 cannot read v17. Do not publish a migrated shared store until every reader uses a compatible binary.

Each task can store one optional `parent_task_id` UUID. Missing fields in older tasks and trash entries mean no parent. Relationships use the same locked, revision-checked atomic save path as other task fields; child counts are calculated and are not stored.


## Deleted tasks

A deleted task moves to `trash.jsonl` once undo can no longer bring it back, or after seven days. Trash is purged 30 days after deletion.

```sh
tsk list --deleted --all
tsk trash restore T12
```

`tsk list --deleted` shows recent deletions and trash together. `tsk trash restore` reads trash only: use `ctrl+u` on the board for a deletion that has not moved there yet.

## Starter tasks

The first full board open puts four starter tasks on your desk, painted `N1`…. They arrive once per state directory: mark one done, archive it, or delete it, and it never returns.

After an upgrade, the first board open adds one `What's new in tsk` task to your desk, in review under **NEEDS YOU**, when the new version has release notes you have not seen. Every missed release lands in that one task, newest first, with a link to the changelog. A new install shows none. Clear it like a starter task.

## Update check

When the board opens, tsk asks GitHub for the latest release if its last check is more than 24 hours old. A newer version shows on the status row as `vX.Y.Z available, run tsk update`. The check sends no task data, and a failed check stays silent.

| Setting | Effect |
| --- | --- |
| `TSK_NO_UPDATE_CHECK` | Set it to turn off the check and the notice |
| `TSK_UPDATE_CURL=/path/to/curl` | macOS and Linux: the curl the check and `tsk update` use, as an absolute path. The default is `/usr/bin/curl`. Windows uses a built-in HTTPS client |

[Upgrade tsk](/docs/install/#upgrade).

## Completion report documents

`reports.json` is a separate versioned document (`format_version: 1`) beside `tsk.json`. It stores Task identifiers and display metadata, every Markdown revision, and the last revision opened on the board. It uses the same store lock as Tasks and atomic replacement with file and directory synchronization. A corrupt or unsupported report file refuses the operation instead of being replaced with an empty list. Task migrations leave question storage unchanged.

Preserve `reports.json` in separate full state-directory copies; automatic `tsk backup` does not include it. Report documents are shareable and are not Git-excluded, unlike `questions-private/`. Never copy authentication secrets or private question originals into reports. A report keeps its Task number and title so it remains readable after the Task moves into dated done history. Existing report history survives changes and read acknowledgements. Older binaries ignore the separate report document and cannot display or register reports.

`unread` means the latest revision differs from `read_revision`. CLI reads leave it alone. A document opened on the board acknowledges the exact displayed revision under the store lock; a concurrent newer revision remains unread. A document too long for the board page (past 65,535 wrapped rows) is kept in full and stays unread. A failed acknowledgement remains unconfirmed and is reported on the document page.


## Agent questions and private records

Question commands require an explicit Agent `--state-dir` or `TSK_STATE_DIR` that is not the personal default directory. They never fall back to the personal board. The personal-root names (`.tsk` under `HOME` or `USERPROFILE`, and `tsk` under `LOCALAPPDATA`) and their ASCII case variants are reserved, including child paths when that root does not exist yet. Existing symlinks and parent components resolve before the isolation check. Shared `tsk.json` holds only a question UUID, display number, task UUID, kind, shareable summary, and immutable private-snapshot reference. Choose a non-sensitive summary: it is not automatically redacted.

Full questions, options, recommendations, impact, blocking scope, origin Thread, recipient, audience, lead recipient, finding details/path, finding action, approval terms, answers, source metadata, and optional permission context stay in `questions-private/`. This directory is excluded from Git. There is no new chmod protection, PIN, ACL, encryption, signature, or user authentication. A process running as the same OS user can impersonate a UI caller; confirmation records are not cryptographic identity evidence.

The existing store lock serializes task and question writes. A new private snapshot and its directory entries are synced before the shared reference is replaced. Missing or corrupt referenced records refuse mutation. Replies and corrections use the expected question revision; corrections append a new answer linked by `corrects_revision`. A question does not change its task's status. A route (`tsk route`) changes only the audience and lead recipient under the same Q number, as a new revision checked against the expected one. A private record with fields an older binary does not support (such as audience, route, finding action, or permission context) is refused rather than read with fields dropped. One unsupported record makes an unfiltered question list, every Q-number lookup in that store, and the board's questions tab unavailable in that binary; direct UUID access to a supported record still works. Upgrade every CLI reader and quit and reopen every board using that Agent store before enabling a relay that writes permission context. A running board keeps its old binary until it is restarted. A record without an audience shows as target unknown and is never counted. Imported actors, history and approvals never issue current approval authority.

A saved reply is separate from its Herdr notification. Registration notifies the recipient; answers and corrections notify the origin Thread. If that Thread ended or notification failed, the Lead can read and resolve the same Q UUID from the same Agent store. Read back the revision before acting; do not apply the same answer twice. A visible post-rename readback after a sync failure is **durability uncertain**, not durable success. Retry notification separately, without saving another answer. No delivery database, message server, or Linear comment replica is maintained.

For deployment or irreversible changes, approval binds to the exact target, action, Git H (40 hexadecimal digits) or plan digest (64 hexadecimal digits), scope, and rollback description. Only a confirmed User UI choice or a Lead record that preserves and verifies the original user-chat approval issues authority. An ordinary Agent CLI reply does not approve an action.

A secret question's snapshot holds only its fnox target (key, profile, config path) and the answer `set`. The value the user types goes from the board to `fnox set` on standard input and is never written under the state directory. [Secret questions](/docs/cli/#secret).


## Older done tasks

A done task stays in `tsk.json` for one day after it is finished. The next save after that moves it to `done/YYYY-MM-DD.jsonl`, one file per UTC completion date, so the live file holds recent work only. Set `TSK_DONE_KEEP_DAYS` to keep done tasks longer (whole days; `0` moves them on the next save). A done task stays live while dispatch cleanup is pending, an undo entry or open question names it, or a live task names it as parent. Answered and closed questions do not hold it: their records stay readable and still name the moved task (a record that cannot be read counts as open). Its number is never reused.

```sh
tsk list --done --history --all
tsk list T12
```

The board's done drawer shows only the tasks still in `tsk.json`. `tsk list --done --history` adds the moved non-archived tasks in the usual done order. `tsk list --archived` includes moved tasks that are individually archived or belong to an archived project, without `--history`. `tsk list T<n>` finds a moved task by number with its parent and children. Project names and child counts include moved tasks. Moved tasks are read-only: reopening one is not supported yet. Each file is plain JSON lines (`done_at` and the task), written atomically before the live file loses the tasks.


## Stall checks

The board's [stall checks](/docs/board/#stall-checks) store nothing: the Herdr agent snapshot and when each task was last asked about live only in the open board session.

| Setting | Purpose |
| --- | --- |
| `TSK_STALE_MINUTES` | Minutes a Doing task must stay unchanged, with its thread not working, before the board marks it stalled (a positive whole number; default `30`) |
