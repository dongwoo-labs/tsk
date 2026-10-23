#!/usr/bin/env python3
"""Replay the verified notes journey on a new copy of an authorized store.

python3 -I scripts/notes-cli-smoke.py --binary .artifacts/isolated-install/bin/tsk \
  --source-state-dir /path/to/authorized/state --task T4142

Only the selected Task in the copy is changed. Each run retains a private directory
under .artifacts. Logs contain command arguments and assertions, not Task contents.
The source tsk.json is read only to create the copy. No external services are used.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def expected_set(notes, prefix, value):
    """Apply the documented line contract to the copied notes, including endings."""
    pattern = re.compile(
        r"^[ \t]*(?:- )?[ \t]*" + re.escape(prefix[:-1])
        + r"[ \t]*:[^\n]*(?:\n|$)", re.M | re.I | re.A,
    )
    if not pattern.search(notes):
        return notes + ("" if not notes or notes.endswith("\n") else "\n") + prefix + " " + value
    first = True

    def replacement(match):
        nonlocal first
        if not first:
            return ""
        first = False
        ending = "\r\n" if match[0].endswith("\r\n") else "\n" if match[0].endswith("\n") else ""
        return prefix + " " + value + ending

    return pattern.sub(replacement, notes)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source-state-dir", type=Path, required=True)
    parser.add_argument("--task", required=True)
    args = parser.parse_args()
    require(re.fullmatch(r"[Tt]?\d+|[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", args.task),
            "task must be a number or UUID")
    binary = args.binary.resolve(strict=True)
    source = (args.source_state_dir / "tsk.json").resolve(strict=True)
    worktree = Path(__file__).resolve().parents[1]
    artifacts = worktree / ".artifacts"
    artifacts.mkdir(exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="notes-cli-smoke-", dir=artifacts))
    root.chmod(0o700)
    state = root / "state"
    state.mkdir(mode=0o700)
    store = state / "tsk.json"
    require(source != store and source.parent not in state.parents, "copy must be outside source root")
    store.write_bytes(source.read_bytes())
    store.chmod(0o600)
    before = json.loads(store.read_bytes())
    env = dict(os.environ, TSK_STATE_DIR=str(state))
    evidence = {
        "mode": "scripted replay", "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "source_read_only": str(source), "state_dir": str(state), "task": args.task,
        "data": "real store copy with synthetic note values; no mocks",
        "commands": 0, "checks": [],
    }
    print(f"EVIDENCE={root}", flush=True)
    with (root / "commands.jsonl").open("w") as log:
        def run(*argv, code=0, help_command=False):
            command = [str(binary), *argv]
            if not help_command:
                command += ["--state-dir", str(state)]
            result = subprocess.run(command, env=env, text=True, capture_output=True, timeout=30)
            log.write(json.dumps({"argv": command, "TSK_STATE_DIR": str(state),
                                  "exit": result.returncode}) + "\n")
            log.flush()
            evidence["commands"] += 1
            require(result.returncode == code, "unexpected CLI exit; see command log")
            require(not result.stderr if code == 0 else not result.stdout and bool(result.stderr),
                    "unexpected CLI output channel")
            return result.stdout, result.stderr

        def checkpoint(message):
            evidence["checks"].append({"verdict": "PASS", "assertion": message})
            print("PASS " + message, flush=True)

        def snapshot():
            return {p.name: (p.read_bytes(), p.stat().st_mtime_ns, p.stat().st_ino)
                    for p in state.iterdir() if p.is_file()}

        def detail(address=args.task):
            rows = json.loads(run("list", address, "--json")[0])
            require(isinstance(rows, list) and len(rows) == 1, "expected exactly one Task")
            return rows[0]

        def projection(flag, name, expected):
            stable = snapshot()
            require(json.loads(run("list", args.task, flag, name, "--json")[0]) == expected,
                    "JSON projection differs from expected value")
            plain = run("list", args.task, flag, name)[0]
            if isinstance(expected, (list, dict)):
                require(json.loads(plain) == expected and len(plain.splitlines()) == 1,
                        "structured plain projection must be compact JSON")
            else:
                require(plain == (str(expected) if expected is not None else "") + "\n",
                        "plain scalar projection differs from expected value")
            require(snapshot() == stable, "projection wrote store files")

        try:
            evidence["version"] = run("--version", help_command=True)[0].strip()
            notes_help = run("help", "notes", help_command=True)[0]
            require(notes_help == run("notes", "--help", help_command=True)[0], "notes help routes differ")
            list_help = run("help", "list", help_command=True)[0]
            require(all(term in notes_help for term in ("set", "remove", "append", "NOT safe", "Exit:")),
                    "notes help lacks action or retry guidance")
            require(all(term in list_help for term in ("--field", "--note", "--json", "mutually")),
                    "list help lacks projection guidance")
            require(all(len(line) <= 80 for line in (notes_help + list_help).splitlines()),
                    "help exceeds documented 80-column width")
            checkpoint("CLI help routes, syntax, retry guidance and reference width")
            current = detail()
            original = next(t for t in before["tasks"] if t["id"] == current["id"])
            require(current["notes"] == original.get("notes"), "CLI did not read copied notes")
            require(detail(original["id"]) == current, "number and UUID lookup differ")
            checkpoint("installed binary reads the real copied Task by address and UUID")

            def write(*argv):
                output = run("notes", args.task, *argv)[0]
                require(output.startswith(f'notes T{original["number"]} ')
                        and len(output.splitlines()) == 1, "mutation acknowledgement is not one task line")
                if argv[0] in ("set", "append"):
                    require(argv[-1] not in output, "acknowledgement exposed note text")

            notes = current["notes"] or ""
            changes = 0
            for prefix, value in (("Now:", "E2E live isolated notes update"),
                                  ("Next:", "Verify fresh CLI projection"),
                                  ("Blocked:", "E2E copy-only removal prerequisite")):
                expected = expected_set(notes, prefix, value)
                changes += expected != notes
                write("set", prefix, value)
                notes = expected
                require(detail()["notes"] == notes, "set changed other lines or failed to persist")
                projection("--note", prefix, value)
                if prefix != "Blocked:":
                    stable = snapshot()
                    write("set", prefix, value)
                    require(snapshot() == stable, "repeated set wrote store files")
            expected = re.sub(r"^[ \t]*(?:- )?[ \t]*Blocked[ \t]*:[^\n]*(?:\n|$)", "", notes,
                              flags=re.M | re.I | re.A)
            changes += expected != notes
            write("remove", "Blocked:")
            notes = expected
            require(detail()["notes"] == notes, "remove changed unrelated lines")
            projection("--note", "Blocked:", None)
            stable = snapshot()
            write("remove", "Blocked:")
            require(snapshot() == stable, "repeated remove wrote store files")
            checkpoint("set Now/Next and remove Blocked preserve lines; no-op bytes/mtime/inode stay unchanged")
            line = "E2E: isolated append reference"
            for _ in range(2):
                write("append", line)
                notes += ("" if not notes or notes.endswith("\n") else "\n") + line
                changes += 1
                require(detail()["notes"] == notes, "append changed other lines or failed to persist")
            require(notes.endswith(line + "\n" + line), "append did not duplicate on repeat")
            checkpoint("append persists and adds duplicate lines on repeat")
            current = detail()
            for field in ("id", "notes", "steps"):
                projection("--field", field, current[field])
            projection("--note", "Now:", "E2E live isolated notes update")
            checkpoint("id/notes/steps and Now return exact single values in plain and JSON output")
            unknown = "T" + str(max(t.get("number") or 0 for t in before["tasks"]) + 100)
            failures = [
                (("notes", args.task, "set", "Now", "bad"), 2),
                (("notes", args.task, "remove", "Now"), 2),
                (("list", args.task, "--note", "Now"), 2),
                (("list", args.task, "--field", "invalid-field"), 2),
                (("notes", unknown, "append", "must not persist"), 1),
                (("list", unknown, "--field", "id"), 2),
            ]
            for argv, code in failures:
                stable = snapshot()
                _, error = run(*argv, code=code)
                require(snapshot() == stable, "rejected command wrote store files")
                if code == 1:
                    require("unknown-task" in error, "unknown mutation lacks refusal code")
            checkpoint("invalid prefix/field exit 2; unknown mutation exit 1 and lookup exit 2; no writes")
            after = json.loads(store.read_bytes())
            require([t for t in before["tasks"] if t["id"] != original["id"]]
                    == [t for t in after["tasks"] if t["id"] != original["id"]], "unrelated Tasks changed")
            changed = next(t for t in after["tasks"] if t["id"] == original["id"])
            require(all(changed[k] == v for k, v in original.items()
                        if k not in ("notes", "updated_at", "revision", "history")), "other Task fields changed")
            history = original["history"]
            require(changed["history"][:len(history)] == history, "prior history changed")
            added = changed["history"][len(history):]
            require(len(added) == changes and all(e["kind"] == "edited" for e in added),
                    "expected one edit event per changed mutation")
            require(changed["revision"] != original["revision"], "edit revision did not change")
            require(all(after[k] == v for k, v in before.items() if k not in ("tasks", "undo_stack")),
                    "non-task store data changed")
            checkpoint("only selected Task and edit bookkeeping changed; argv and state override target copy")
            evidence.update(verdict="PASS", changes=changes, steps_count=len(current["steps"]))
        except Exception as error:
            # Exceptions can carry captured Task content. Record only the exception class.
            evidence.update(verdict="FAIL", failure_class=type(error).__name__)
            print(f"FAIL {type(error).__name__}; inspect command/check positions, not raw Task output")
        finally:
            (root / "result.json").write_text(json.dumps(evidence, indent=2) + "\n")
    return 0 if evidence["verdict"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
