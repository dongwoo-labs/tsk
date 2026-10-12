#!/usr/bin/env python3
"""Run the actual CLI against new isolated stores. No unit or legacy E2E suite.

python3 -I scripts/parent-cli-smoke.py --binary target/release/tsk \
  --evidence .artifacts/logs/DONGWOO-2696/replay-1 [--live-linear]

Synthetic Linear pages use the existing TSK_LINEAR_FIXTURE boundary. --live-linear
uses the existing Linear login in subprocess memory and makes read-only queries.
It never prints or saves a credential. All task writes remain below --evidence.
"""

import argparse
import json
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--live-linear", action="store_true")
    args = parser.parse_args()
    binary = args.binary.resolve()
    root = args.evidence.resolve()
    root.mkdir(parents=True, exist_ok=False)
    log = (root / "commands.log").open("w")
    base_env = dict(os.environ)
    for key in ("TSK_STATE_DIR", "TSK_LINEAR_FIXTURE", "LINEAR_API_KEY"):
        base_env.pop(key, None)

    def run(store, *argv, expected=0, data=None, extra=None):
        env = dict(base_env, TSK_STATE_DIR=str(root / store))
        env.update(extra or {})
        result = subprocess.run([str(binary), *argv], env=env, input=data,
                                text=True, capture_output=True)
        # argv and output contain task data only; environment is never recorded.
        log.write(f"$ tsk {' '.join(argv)} [isolated {store}]\n")
        log.write(f"exit={result.returncode}\n{result.stdout}{result.stderr}\n")
        log.flush()
        assert result.returncode == expected, (argv, result.returncode, result.stderr)
        return result

    def detail(store, number):
        return json.loads(run(store, "list", f"T{number}", "--json").stdout)[0]

    def add(store, title, *options):
        return json.loads(run(store, "add", "-t", title, "--desk", "--json", *options).stdout)

    def state(store):
        return json.loads((root / store / "tsk.json").read_text())

    def snapshot(store):
        return {path.name: path.read_bytes() for path in (root / store).iterdir() if path.is_file()}

    def checkpoint(message):
        print(f"PASS {message}", flush=True)
        log.write(f"PASS {message}\n")
        log.flush()

    for command in ("add", "edit", "list", "status", "archive", "project", "import", "trash"):
        run("cli", "help", command)
    parent = add("cli", "parent")
    child = add("cli", "child", "--parent", "T1")
    second = add("cli", "second parent")
    independent = add("cli", "independent")
    assert detail("cli", 4)["parent_task_id"] is None
    assert detail("cli", 2)["parent_task_id"] == parent["id"]
    assert detail("cli", 1)["children_total"] == 1
    checkpoint("parentless and parented creation; direct child lookup after process restart")

    run("cli", "edit", "T2", "--parent", second["id"], "--title", "renamed child")
    assert detail("cli", 2)["parent_task_id"] == second["id"]
    assert detail("cli", 1)["children_total"] == 0
    assert detail("cli", 3)["children_total"] == 1
    run("cli", "edit", "T2", "--no-parent")
    assert detail("cli", 2)["parent_task_id"] is None
    run("cli", "edit", "T2", "--parent=T1")
    before = snapshot("cli")
    run("cli", "edit", "T2", "--parent", "T2", expected=1)
    run("cli", "edit", "T1", "--parent", "T2", expected=1)
    run("cli", "edit", "T2", "--parent", "T999999", expected=1)
    run("cli", "edit", "T2", "--parent", "T1", "--no-parent", expected=2)
    assert snapshot("cli") == before, "refused operations wrote store files"
    checkpoint("replace/unset/reassign by number and UUID; self/cycle/invalid parent refusal")

    run("cli", "status", "T2", "done")
    run("cli", "archive", "T2")
    project = root / "project-child"
    project.mkdir()
    project_child = json.loads(run("cli", "add", "-t", "project child", "-p", str(project),
                                  "--parent", "T1", "--json").stdout)
    run("cli", "project", "archive", str(project))
    children = detail("cli", 1)
    assert (children["children_done"], children["children_total"]) == (1, 2)
    assert {row["id"] for row in children["children"]} == {child["id"], project_child["id"]}
    run("cli", "status", "T1", "done")
    run("cli", "archive", "T1")
    assert detail("cli", project_child["number"])["status"] == "open"
    assert not next(task for task in state("cli")["tasks"] if task["id"] == project_child["id"]).get("archived", False)
    assert detail("cli", 2)["parent"]["resolved"]
    checkpoint("counts include archived tasks/projects; parent Done/archive never cascades")

    plan = [{"title": "plan child", "project": None, "parent": "T3"},
            {"title": "bad parent", "project": None, "parent": "T999999"},
            {"title": "plan independent", "project": None, "parent": None}]
    result = json.loads(run("cli", "add", data=json.dumps(plan), expected=1).stdout)
    assert len(result["created"]) == 2 and result["failed"][0]["code"] == "invalid-parent"
    assert detail("cli", result["created"][0]["number"])["parent_task_id"] == second["id"]
    checkpoint("JSON plan parent/null and valid-sibling persistence")

    # Deleted/missing parent states are explicit fixture boundaries. This does not
    # verify the TUI delete action, which is owned by the UI acceptance task.
    add("unresolved", "parent")
    add("unresolved", "child", "--parent", "T1")
    path = root / "unresolved" / "tsk.json"
    document = state("unresolved")
    parent_id = document["tasks"][0]["id"]
    document["tasks"][0]["soft_deleted"] = True
    path.write_text(json.dumps(document))
    assert detail("unresolved", 2)["parent"] == {
        "id": parent_id, "resolved": False, "number": None, "title": None}
    run("unresolved", "edit", "T2", "--notes", "retained after deleted parent")
    assert detail("unresolved", 2)["parent_task_id"] == parent_id
    document = state("unresolved")
    document["tasks"] = [task for task in document["tasks"] if task["id"] != parent_id]
    path.write_text(json.dumps(document))
    run("unresolved", "edit", "T2", "--title", "retained after missing parent")
    assert detail("unresolved", 2)["parent_task_id"] == parent_id
    assert not detail("unresolved", 2)["parent"]["resolved"]
    run("unresolved", "edit", "T2", "--no-parent")
    assert detail("unresolved", 2)["parent_task_id"] is None
    checkpoint("deleted/missing fixture UUID retained through real CLI saves; explicit clear")

    fixture_source = Path(__file__).resolve().parents[1] / "tests/fixtures/current_store_v5.json"
    original = fixture_source.read_bytes()
    migration_dir = root / "migration"
    migration_dir.mkdir()
    (migration_dir / "tsk.json").write_bytes(original)
    historical = json.loads(original)
    run("migration", "add", "-t", "post migration", "--desk", "--json")
    migrated = state("migration")
    assert migrated["format_version"] == 6
    assert (migration_dir / "tsk.json.v5").read_bytes() == original
    assert migrated["tasks"][0] == historical["tasks"][0]
    assert migrated["undo_stack"] == historical["undo_stack"]
    checkpoint("v5 -> v6 original backup; historical task/history/step/undo unchanged")
    # Unknown wire fields and future formats refuse without changing live bytes.
    for name, mutate in (("unknown", lambda doc: doc["tasks"][0].update(unowned=True)),
                         ("future", lambda doc: doc.update(format_version=7))):
        directory = root / name
        directory.mkdir()
        document = json.loads(json.dumps(migrated))
        mutate(document)
        (directory / "tsk.json").write_text(json.dumps(document))
        bytes_before = (directory / "tsk.json").read_bytes()
        run(name, "list", "--all", "--json", expected=3)
        run(name, "add", "-t", "must not write", "--desk", expected=3)
        assert (directory / "tsk.json").read_bytes() == bytes_before
    checkpoint("deny_unknown_fields and future version refusal; live bytes unchanged")

    # Historical trash can still deserialize without parent_task_id.
    old_task = json.loads(original)["tasks"][0]
    trash_dir = root / "old-trash"
    trash_dir.mkdir()
    line = {"task": old_task, "deleted_at": old_task["updated_at"]}
    (trash_dir / "trash.jsonl").write_text(json.dumps(line) + "\n")
    old_rows = json.loads(run("old-trash", "list", "--deleted", "--all", "--json").stdout)
    assert old_rows[0]["parent_task_id"] is None and old_rows[0]["id"] == old_task["id"]
    checkpoint("old trash defaults to no parent")

    def issue(identifier, parent=None, status="unstarted", title=None):
        return {"identifier": identifier, "title": title or identifier,
                "url": f"https://linear.app/sample/issue/{identifier}",
                "description": "sample description", "parent": parent,
                "state": {"type": status, "name": "Done" if status == "completed" else "Todo"},
                "team": {"key": "SAMPLE"}}

    def parent_ref(identifier):
        return {"identifier": identifier, "url": f"https://linear.app/sample/issue/{identifier}"}

    def import_page(store, nodes, *options):
        fixture = root / f"{store}-page.json"
        fixture.write_text(json.dumps({"data": {"issues": {"nodes": nodes,
                             "pageInfo": {"hasNextPage": False, "endCursor": None}}}}))
        return json.loads(run(store, "import", "linear", "--desk", "--assignee", "any", "--json",
                              *options, extra={"TSK_LINEAR_FIXTURE": str(fixture),
                                               "LINEAR_API_KEY": "fixture-only-not-a-credential"}).stdout)

    nodes = [issue("SAMPLE-2", parent_ref("SAMPLE-1")), issue("SAMPLE-1"),
             issue("SAMPLE-3", parent_ref("SAMPLE-99")), issue("SAMPLE-4")]
    report = import_page("import", nodes)
    assert len(report["created"]) == 4
    assert detail("import", 1)["parent_task_id"] == detail("import", 2)["id"]
    assert detail("import", 2)["parent_task_id"] is None
    external = detail("import", 3)
    assert external["parent_task_id"] is None
    assert "Parent: Linear SAMPLE-99 https://linear.app/sample/issue/SAMPLE-99" in external["notes"]
    assert detail("import", 4)["parent_task_id"] is None
    report = import_page("import", nodes)
    assert not report["created"] and len(report["unchanged"]) == 4
    assert len(state("import")["tasks"]) == 4
    checkpoint("fixture import: no-parent, reversed same-batch parent, external-only parent, dedupe")

    report = import_page("import", [issue("SAMPLE-5", parent_ref("SAMPLE-1")), issue("SAMPLE-99")])
    assert len(report["created"]) == 2
    assert detail("import", 5)["parent_task_id"] == detail("import", 2)["id"]
    assert detail("import", 3)["parent_task_id"] is None
    original_notes = detail("import", 1)["notes"] + "\nUser notes must stay."
    run("import", "edit", "T1", "--notes", original_notes, "--parent", "T4")
    saved_parent = detail("import", 1)["parent_task_id"]
    changed = [issue("SAMPLE-2", parent_ref("SAMPLE-99"), "completed", "updated title"),
               issue("SAMPLE-3", parent_ref("SAMPLE-99"), "completed", "updated external child")]
    report = import_page("import", changed, "--update")
    assert len(report["updated"]) == 2
    updated = detail("import", 1)
    assert updated["title"] == "SAMPLE-2 updated title" and updated["status"] == "done"
    assert updated["notes"] == original_notes and updated["parent_task_id"] == saved_parent
    assert detail("import", 3)["parent_task_id"] is None
    checkpoint("fixture import: existing parent; no later reconciliation; --update preserves user Notes/parent")

    if args.live_linear:
        # Capture only in memory. No token, environment, or auth command output is logged.
        auth = subprocess.run(["linear", "auth", "token"], text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        if auth.returncode or not auth.stdout.strip():
            print("BLOCKED live Linear importer: existing login cannot supply credential")
            raise SystemExit(1)
        key = auth.stdout.strip()
        report = json.loads(run("live-linear", "import", "linear", "--desk", "--team", "DONGWOO",
                                "--assignee", "any", "--state", "triage,backlog,unstarted,started,completed",
                                "--limit", "100", "--json", extra={"LINEAR_API_KEY": key}).stdout)
        del key, auth
        document = state("live-linear")
        assert report["fetched"] > 0 and report["created"]
        linked = [task for task in document["tasks"] if task.get("parent_task_id")]
        references = [task for task in document["tasks"] if "\nParent: Linear " in (task.get("notes") or "")]
        assert references and linked, "real response did not exercise parent metadata/linking"
        for task in linked:
            parent = next(item for item in document["tasks"] if item["id"] == task["parent_task_id"])
            parent_identifier = parent["notes"].splitlines()[0].split()[1]
            assert f"\nParent: Linear {parent_identifier} " in task["notes"]
        checkpoint(f"live Linear importer: fetched={report['fetched']}, parent references={len(references)}, UUID links={len(linked)}")
    else:
        print("UNKNOWN NOT_RUN live Linear importer (omit --live-linear)")
    print("UNKNOWN NOT_RUN actual TUI delete action (fixture boundary only)")
    print("BLOCKED NOT_RUN Native acceptance (no project config)")
    log.close()


if __name__ == "__main__":
    main()
