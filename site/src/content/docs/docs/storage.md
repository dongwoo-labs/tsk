---
title: Storage
description: Task data, backups, deleted tasks, git sync, and update checks.
---

The board, CLI, and Herdr plugin share one store. The current store format is v5.

## Location

| Setting | Default | Purpose |
| --- | --- | --- |
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

An older binary refuses a newer or unversioned store instead of rewriting it. Use a compatible tsk version to open it. On first save, v4 stores migrate to v5 so one undo entry can cover a marked completion or deletion; the original document is saved as `tsk.json.v4`. Earlier stores still run through each migration in order, including the v3 to v4 move from ready to open.

Archived tasks stay in the task document with their existing status. [Archive and restore](/docs/board/#archive).

## Starter guides and release notes

The four starter tasks (`N1`… on your desk) are seeded once per state directory on the first board open. Mark one done, archive it, or delete it and it never returns. Deleting `delivery.json` seeds any starter catalog id the task document no longer holds.

After an upgrade, the first board open adds one `What's new in tsk` task to your desk (in `review`, under NEEDS YOU) when the new version bundles release notes you have not seen. That includes upgrading from a build that only had starter tasks. Every missed release lands in that one task, newest first, with its changelog link in the notes. The notes ship inside the binary; a blank first install records them as seen and shows none. Clear it like any starter task.

## Deleted tasks

Deleted tasks move to `trash.jsonl` once undo can no longer restore them, or after seven days. They are purged 30 days after deletion.

```sh
tsk list --deleted --all
tsk trash restore T12
```

The list includes both recently deleted tasks still in the main store and tasks in trash. Restore reads trash; use board undo for a recent deletion that has not moved there yet.

## Share through git

`tsk sync` shares one board between machines, and with agents that run elsewhere, through a git repository. The state directory becomes that repository: `tsk.json` and `trash.jsonl` are committed, and machine-local files (`tsk.json.lock`, `tsk.json.1`, `tsk.json.v<N>`, temp files, `delivery.json`, `update.json`) are ignored.

1. Create an empty **private** repository on your git host. The board holds your task titles and notes.
2. On the first machine, set up sync and push the board:

   ```sh
   tsk sync --init git@github.com:me/tsk-board.git
   tsk sync
   ```

3. On every other machine, or for an agent with its own checkout, clone it and run init once in the clone. Init installs the merge driver, which a clone does not carry:

   ```sh
   git clone git@github.com:me/tsk-board.git ~/tsk-board
   TSK_STATE_DIR=~/tsk-board tsk sync --init
   TSK_STATE_DIR=~/tsk-board tsk sync
   ```

4. Use the board as usual with the same `TSK_STATE_DIR` (`tsk list`, `tsk add`, `tsk status`, the board itself), and run `tsk sync` to publish your changes and pick up everyone else's. An agent runs `tsk sync` before reading and after writing.

`tsk sync --init <url>` on a machine that already has its own board joins the remote board: the next `tsk sync` merges the two, and tasks from both keep unique numbers. Re-running `tsk sync --init` is safe; it repairs `.gitignore`, `.gitattributes`, and the merge-driver config without duplicating anything. It refuses a state directory that sits inside another git repository or already tracks other files, and it never changes an existing `origin`.

There is no background sync: changes travel only when someone runs `tsk sync`. Sync holds the board's exclusive lock through commit, fetch, merge, push, and the one allowed retry. Other local writers wait until sync finishes, including while it waits for the network. Git uses your usual credentials; sync never prompts for them.

### How boards merge

When both sides changed `tsk.json`, git runs `tsk merge-driver` (configured by init, hidden from help), which merges by task:

| Case | Result |
| --- | --- |
| New on one side | Kept |
| Changed on one side | That side's version |
| Changed on both sides | The later edit (`updated_at`); a tie keeps the local version |
| Deleted (trashed) on one side, untouched on the other | Deleted |
| Deleted on one side, changed on the other | The changed task is kept |
| Two tasks with the same number | The task that had it first keeps it; the other gets the next free number, so `T` numbers stay unique |

Archived projects from both sides are kept. Undo history stays per machine. `trash.jsonl` keeps the lines of both sides.

Every machine must run the same store format. If the remote board was written by a newer tsk, or the merged board would not load, sync refuses with exit 1 and leaves your board as it was; upgrade tsk and run `tsk sync` again. The board before each merge is kept as `tsk.json.1`.

## Update check

On launch, tsk checks for a newer release if its cached check is older than 24 hours. A newer version appears on the board's idle status row as `vX.Y.Z available, run tsk update`.

| Setting or file | Purpose |
| --- | --- |
| `TSK_NO_UPDATE_CHECK` | Set to disable the check and notice |
| `TSK_UPDATE_CURL=/absolute/path/to/curl` | macOS/Linux only: use a nonstandard, explicit curl path for the check and for `tsk update` (default `/usr/bin/curl`). Windows uses the binary's native HTTPS client |
| `update.json` | Cached release check in the state directory |

The check requests the latest release tag from GitHub over HTTPS only. It does not upload task data. Failures are silent.

[Upgrade tsk](/docs/install/#upgrade).
