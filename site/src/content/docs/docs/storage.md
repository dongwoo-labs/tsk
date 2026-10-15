---
title: Storage
description: Task data, backups, deleted tasks, and update checks.
---

The board, CLI, and Herdr plugin share one store. The current store format is v9. Task statuses are strictly `todo`, `doing`, and `done`. The v6 → v7 migration maps open/ready to todo, started/blocked/review to doing, and done to done without adding history events or changing revisions, steps, archive flags, or parent links. The v8 → v9 migration changes no data: it marks stores that may hold a `secret` question reference, so a v8 binary refuses them instead of failing to read them. The v7 → v8 migration removes the task thread label from every live and trashed task and changes nothing else; name a working Thread with a `Thread:` line in the notes instead ([links](/docs/task-page/#questions-findings-and-links)). Each task can store one optional `parent_task_id` UUID. Missing fields in older tasks and trash entries mean no parent. Relationships use the same locked, revision-checked atomic save path as other task fields; child counts are calculated and are not stored.

## Location

| Setting | Default | Purpose |
| --- | --- | --- |
| `TSK_DONE_KEEP_DAYS` | `1` | Days a done task stays in `tsk.json` before it moves to its dated file under `done/`, see [older done tasks](#older-done-tasks) |
| `TSK_STATE_DIR` | Platform default | Task data, backups, trash, and release-check cache. The default is `~/.tsk` on macOS/Linux, `%LOCALAPPDATA%\tsk` on Windows, or `%USERPROFILE%\.tsk` when `LOCALAPPDATA` is unavailable. Without a usable platform home, tsk refuses to run rather than pick a directory |
| `--state-dir <dir>` | State directory | Override storage for a data command |

Use a local disk. NFS and synced folders such as Dropbox or iCloud Drive are unsupported. Directory roots must be real directories, not symlinks or Windows reparse points such as junctions.

Herdr's plugin-specific state/config directories do not override these locations. Removing tsk leaves its task data intact.

## Backups

| File | Contains |
| --- | --- |
| `tsk.json` | Current tasks and archived-project records |
| `tsk.json.1` | Previous valid task document |
| `tsk.json.v<N>` | Backup made when migrating an older store format, such as `tsk.json.v4` for the v4 → v5 migration |
| `delivery.json` | Which starter tasks this install has received or dismissed, and the newest release note it has seen |

An older binary refuses a newer or unversioned store instead of rewriting it. Use a compatible tsk version to open it. On first save, v5 stores migrate through v6 (no parent by default) and v7 to v8 and v9, keeping task data, history, undo, and trash. The original document is saved as `tsk.json.v5`. A v7 store is saved as `tsk.json.v7` before its first v8 save, and a v7 binary refuses the v8 store. A v8 store is saved as `tsk.json.v8` before its first v9 save, and a v8 binary refuses the v9 store. Older stores keep their own original-version backup; the v4 to v5 step still enables batch undo. Earlier stores still run through each migration in order, including the v3 to v4 move from ready to open.

Legacy trash statuses are normalized and thread labels dropped before parsing. Before the first trash rewrite, the original bytes are saved as `trash.jsonl.pre-v8` (a v7 binary saved `trash.jsonl.pre-v7`, which stays untouched); a backup failure refuses the rewrite. To roll back, stop all writers and restore the matching `tsk.json.v<N>` and trash backup together. Keep private question snapshots unchanged. A live backup alone is not a complete rollback after trash conversion.

Archived tasks stay in the task document with their existing status. [Archive and restore](/docs/board/#archive).

## Agent questions and private records

Question commands require an explicit Agent `--state-dir` or `TSK_STATE_DIR` that is not the personal default directory. They never fall back to the personal board. The personal-root names (`.tsk` under `HOME` or `USERPROFILE`, and `tsk` under `LOCALAPPDATA`) and their ASCII case variants are reserved, including child paths when that root does not exist yet. Existing symlinks and parent components resolve before the isolation check. Shared `tsk.json` holds only a question UUID, display number, task UUID, kind, shareable summary, and immutable private-snapshot reference. Choose a non-sensitive summary: it is not automatically redacted.

Full questions, options, recommendations, impact, blocking scope, origin Thread, recipient, audience, lead recipient, finding details/path, finding action, approval terms, answers and source metadata stay in `questions-private/`. This directory is excluded from Git. There is no new chmod protection, PIN, ACL, encryption, signature, or user authentication. A process running as the same OS user can impersonate a UI caller; confirmation records are not cryptographic identity evidence.

The existing store lock serializes task and question writes. A new private snapshot and its directory entries are synced before the shared reference is replaced. Missing or corrupt referenced records refuse mutation. Replies and corrections use the expected question revision; corrections append a new answer linked by `corrects_revision`. A question does not change its task's status. A route (`tsk route`) changes only the audience and lead recipient under the same Q number, as a new revision checked against the expected one. A private record that names an audience, a route, or a finding action is unreadable to an older binary: it refuses that record (the questions tab shows it unavailable) rather than dropping the fields. A record without an audience shows as target unknown and is never counted. Imported actors, history and approvals never issue current approval authority.

A saved reply is separate from its Herdr notification. Registration notifies the recipient; answers and corrections notify the origin Thread. If that Thread ended or notification failed, the Lead can read and resolve the same Q UUID from the same Agent store. Read back the revision before acting; do not apply the same answer twice. A visible post-rename readback after a sync failure is **durability uncertain**, not durable success. Retry notification separately, without saving another answer. No delivery database, message server, or Linear comment replica is maintained.

For deployment or irreversible changes, approval binds to the exact target, action, Git H (40 hexadecimal digits) or plan digest (64 hexadecimal digits), scope, and rollback description. Only a confirmed User UI choice or a Lead record that preserves and verifies the original user-chat approval issues authority. An ordinary Agent CLI reply does not approve an action.

A secret question's snapshot holds only its fnox target (key, profile, config path) and the answer `set`. The value the user types goes from the board to `fnox set` on standard input and is never written under the state directory. [Secret questions](/docs/cli/#secret).

## Starter guides and release notes

The four starter tasks (`N1`… on your desk) are seeded once per state directory on the first board open. Mark one done, archive it, or delete it and it never returns. Deleting `delivery.json` seeds any starter catalog id the task document no longer holds.

After an upgrade, the first board open adds one `What's new in tsk` task to your desk (in `doing`, under DOING) when the new version bundles release notes you have not seen. That includes upgrading from a build that only had starter tasks. Every missed release lands in that one task, newest first, with its changelog link in the notes. The notes ship inside the binary; a blank first install records them as seen and shows none. Clear it like any starter task.

## Older done tasks

A done task stays in `tsk.json` for one day after it is finished. The next save after that moves it to `done/YYYY-MM-DD.jsonl`, one file per UTC completion date, so the live file holds recent work only. Set `TSK_DONE_KEEP_DAYS` to keep done tasks longer (whole days; `0` moves them on the next save). A done task stays live while an undo entry or a question still names it, or while a task that stays live names it as parent. Its number is never reused.

```sh
tsk list --done --history --all
tsk list T12
```

The board's done drawer shows only the tasks still in `tsk.json`. `tsk list --done --history` adds the moved tasks after the live ones, newest first, and `tsk list T<n>` finds a moved task by number. Moved tasks are read-only: reopening one is not supported yet. Each file is plain JSON lines (`done_at` and the task), written atomically before the live file loses the tasks.

## Deleted tasks

Deleted tasks move to `trash.jsonl` once undo can no longer restore them, or after seven days. They are purged 30 days after deletion.

```sh
tsk list --deleted --all
tsk trash restore T12
```

The list includes both recently deleted tasks still in the main store and tasks in trash. Restore reads trash; use board undo for a recent deletion that has not moved there yet.

## Stall checks

The board's [stall checks](/docs/board/#stall-checks) store nothing: the Herdr agent snapshot and when each task was last asked about live only in the open board session.

| Setting | Purpose |
| --- | --- |
| `TSK_STALE_MINUTES` | Minutes a Doing task must stay unchanged, with its thread not working, before the board marks it stalled (a positive whole number; default `30`) |

## Update check

On launch, tsk checks for a newer release if its cached check is older than 24 hours. A newer version appears on the board's idle status row as `vX.Y.Z available, run tsk update`.

| Setting or file | Purpose |
| --- | --- |
| `TSK_NO_UPDATE_CHECK` | Set to disable the check and notice |
| `TSK_UPDATE_CURL=/absolute/path/to/curl` | macOS/Linux only: use a nonstandard, explicit curl path for the check, `tsk update`, and `tsk import linear` (default `/usr/bin/curl`). Windows uses the binary's native HTTPS client |
| `update.json` | Cached release check in the state directory |

The check requests the latest release tag from GitHub over HTTPS only. It does not upload task data. Failures are silent.

[Upgrade tsk](/docs/install/#upgrade).
