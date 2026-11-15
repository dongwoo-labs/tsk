#!/usr/bin/env python3
"""finding 실행을 release binary의 실제 TUI로 검증한다.

준비: cargo build --release
실행: uv run --with-requirements scripts/parity-requirements.txt python \
      scripts/finding-pty-smoke.py <새 출력 디렉터리>
격리된 sample store와 기존 PTY harness를 사용한다. hc/Herdr는 fixture CLI다.
실제 runtime 전송, 실제 Task 변경, 사용자 답변 대행은 실행하지 않는다.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

REPO = Path(__file__).resolve().parents[1]
BINARY = REPO / "target/release/tsk"
spec = importlib.util.spec_from_file_location("machine_pty", REPO / "scripts/machine-pty-smoke.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)

FIXTURE = '''import json, os, sys
from pathlib import Path
root = Path(os.environ["FINDING_FIXTURE_ROOT"])
cfg = json.loads((root / "runtime.json").read_text())
args = sys.argv[1:]
with (root / "runtime-calls.jsonl").open("a") as f:
    f.write(json.dumps({"program":Path(sys.argv[0]).name,"args":args,"session":os.getenv("HC_SESSION")})+"\\n")
if cfg["mode"] == "unavailable":
    sys.exit(1)
if Path(sys.argv[0]).name == "hc":
    terminal = {"address":"t7", "name":"owner-thread", "state":"live", "cwd":str(root),
                "labels":{"task":"t1", "owner_lead":"lead-sample", "thread":"owner-thread",
                          "machine":"alpha", "worktree":str(root), "sha":"a"*40}}
    if cfg["mode"] == "gone":
        terminal["state"] = "finished"
    if cfg["mode"] == "mismatch":
        terminal["labels"]["task"] = "t2"
    if args[0] == "get":
        terminal["address"] = args[1]
        print(json.dumps(terminal))
    elif args[0] == "ls":
        rows = [] if cfg["mode"] == "mismatch" else [{"address":"t7"}]
        if cfg["mode"] == "ambiguous":
            rows.append({"address":"t8"})
        print(json.dumps({"terminals":rows}))
    elif args[0] == "send":
        print('{}')
    else:
        sys.exit(2)
else:
    if args[:1] == ["--session"]:
        assert args[1] == "example"
        args = args[2:]
    if args == ["agent", "list"]:
        rows = [] if cfg["mode"] == "gone" else [{"name":"owner-thread", "agent_status":"working",
                "workspace_id":"w1", "tab_id":"w1:t1", "pane_id":"w1:p1"}]
        if cfg["lead"]:
            rows.append({"name":"lead-sample", "agent_status":"idle", "workspace_id":"w1", "tab_id":"w1:t2", "pane_id":"w1:p2"})
        print(json.dumps({"result":{"agents":rows}}))
    elif args[:2] == ["agent", "prompt"]:
        print('{}')
    else:
        sys.exit(2)
'''


def run_case(output, name, backend="hc", mode="live", choice=b"1", link="exact", lead=False, done=False):
    sample = base.Sample(output / name)
    root = sample.output
    state = root / "state"
    (root / "runtime.json").write_text(json.dumps({"mode": mode, "lead": lead}))
    bindir = root / "bin"
    bindir.mkdir()
    for program in ["hc", "herdr"]:
        path = bindir / program
        path.write_text(f"#!{sys.executable}\n" + FIXTURE)
        path.chmod(0o700)
    sample.env.update(PATH=str(bindir) + os.pathsep + sample.env["PATH"],
                      FINDING_FIXTURE_ROOT=str(root), HERDR_BIN_PATH=str(bindir / "herdr"),
                      HC_SESSION="sender-core")
    ref = "owner-thread hc:example - hc:example:t7" if backend == "hc" else "owner-thread w1 w1:t1 w1:p1"
    notes = "Owner lead: alpha example lead-sample\nThread host: alpha example\nHold: sample must survive"
    if link != "none":
        notes += "\nThread: " + ("owner-thread" if link == "partial" else ref)
        if link == "duplicate":
            notes += "\nThread: " + ref
    sample.cli(state, "add", "--desk", "-t", "Existing sample task", "-n", notes, "--json")
    sample.cli(state, "status", "T1", "blocked", "--why", "Sample block stays")
    sample.cli(state, "add", "--desk", "-t", "Unrelated sample task", "--json")
    source = json.loads(sample.cli(state, "list", "T1", "--json"))[0]
    unrelated = json.loads(sample.cli(state, "list", "T2", "--json"))[0]
    draft = {"task_id": source["id"], "kind": "finding", "audience": "user",
             "summary": "Sample finding", "text": "PRIVATE SAMPLE BODY", "origin_thread": "owner-thread",
             "recipient": "lead-sample", "finding": {"kind": "issue", "detail": "PRIVATE SAMPLE DETAIL"}}
    if lead:
        draft["lead_recipient"] = "lead-sample"
    payload = root / "finding.json"
    payload.write_text(json.dumps(draft))
    sample.cli(state, "ask", "--file", str(payload))
    if done:
        sample.cli(state, "status", "T1", "done", keep=0)
        sample.env["TSK_DONE_KEEP_DAYS"] = "0"
        source = json.loads(sample.cli(state, "list", "T1", "--json"))[0]
    record = json.loads(sample.cli(state, "questions", "--json"))[0]
    calls_file = root / "runtime-calls.jsonl"
    calls_before = len(calls_file.read_text().splitlines()) if calls_file.exists() else 0
    refused = choice == b"1" and (mode in ("ambiguous", "mismatch", "unavailable") or link in ("partial", "duplicate"))
    reused = choice == b"1" and link == "exact" and mode == "live"
    j = base.Board(sample, state, "screens")
    try:
        j.wait("01-board", lambda s: "2 questions" in s)
        before = json.loads(sample.cli(state, "list", "--all", "--json"))
        j.key(b"2", "02-findings", lambda s: "FINDINGS" in s)
        j.key(b"\r", "03-open", lambda s: "Create the task and run it" in s)
        j.key(choice, "04-pick", lambda s: "nothing saved" in s if refused else "Q1 saved" in s)
        after = json.loads(sample.cli(state, "list", "--all", "--json"))
        answered = json.loads(sample.cli(state, "questions", "--json"))[0]
        current = json.loads(sample.cli(state, "list", "T1", "--json"))[0]
        assert json.loads(sample.cli(state, "list", "T2", "--json"))[0] == unrelated
        assert current["status"] == source["status"] and current.get("block") == source.get("block")
        if refused:
            assert after == before and answered == record, "거절은 Task와 Q를 모두 보존한다"
        else:
            answer = answered["answers"][-1]
            assert answer["action"] == ("create_and_run" if choice == b"1" else "create_only")
            if reused:
                assert len(after) == len(before), "live Thread가 있는 Task는 중복 생성하지 않는다"
                assert answer["created_task_id"] == source["id"]
                marker = f'Finding: Q1 id={record["reference"]["id"]}'
                assert current["notes"] == notes + "\n" + marker
                assert "PRIVATE SAMPLE" not in current["notes"]
                if done:
                    retained = [json.loads(line)["task"]["id"] for path in (state / "done").glob("*.jsonl")
                                for line in path.read_text().splitlines()]
                    assert source["id"] in retained, "완료 Task가 실제 history로 이동해야 한다"
                calls = [json.loads(line) for line in calls_file.read_text().splitlines()[calls_before:]]
                sends = [c for c in calls if c["args"][0] == "send" or "prompt" in c["args"]]
                hints = [str(c["args"]) for c in sends if "action=create_and_run" in str(c["args"])]
                assert len(hints) == 1 and source["id"] in hints[0]
                assert "PRIVATE SAMPLE" not in str(sends)
                if backend == "hc":
                    assert all(c["session"] == "example" and c["args"][1] == "t7"
                               for c in sends if c["args"][0] == "send")
                    assert all(c["program"] == "hc" for c in calls)
                # 완료된 finding을 닫는 correction도 연결과 notes를 보존한다.
                j.key(b"z", "05-closed", lambda s: "Sample finding" in s)
                shown = j.key(b"\r", "06-answered", lambda s: "c corrects" in s)
                assert "→ T1" in shown and "a missing task" not in shown, "연결된 기존 Task 번호가 보여야 한다"
                j.key(b"r", "06-resent", lambda s: "run request resent" in s)
                assert json.loads(sample.cli(state, "list", "--all", "--json")) == after
                assert json.loads(sample.cli(state, "questions", "--json"))[0] == answered
                run_hints = [json.loads(line) for line in calls_file.read_text().splitlines()
                             if "action=create_and_run" in line]
                assert len(run_hints) == 2 and all(source["id"] in str(call["args"]) for call in run_hints)
                j.key(b"c", "07-correct", lambda s: "Create the task and run it" in s)
                j.key(b"3", "08-close", lambda s: "Q1 saved" in s)
                corrected = json.loads(sample.cli(state, "questions", "--json"))[0]
                assert corrected["answers"][-1]["created_task_id"] == source["id"]
                assert json.loads(sample.cli(state, "list", "T1", "--json"))[0]["notes"] == current["notes"]
            else:
                assert len(after) == len(before) + 1
                created = next(t for t in after if t["id"] == answer["created_task_id"])
                assert created["title"] == "Sample finding"
                assert created["status"] == ("doing" if choice == b"1" else "todo")
                assert current["notes"] == notes
                if lead:
                    calls = [json.loads(line) for line in calls_file.read_text().splitlines()[calls_before:]]
                    run_hints = [c for c in calls if "action=create_and_run" in str(c["args"])]
                    assert len(run_hints) == 1
                    assert "lead-sample" in run_hints[0]["args"] and created["id"] in str(run_hints[0]["args"])
        (root / "readback.json").write_text(json.dumps({"source": current, "question": answered, "tasks": after}, indent=2))
    finally:
        j.capture("final")
        # PTY 출력을 읽지 않은 채 wait하면 macOS의 자식 종료가 멈출 수 있다.
        os.close(j.master)
        if j.child.poll() is None:
            j.child.kill()
        j.child.wait(timeout=10)


def main(output):
    output.mkdir(parents=True, exist_ok=False)
    cases = [dict(name="hc-live"), dict(name="herdr-live", backend="herdr"),
             dict(name="hc-done-history", done=True),
             dict(name="no-thread", link="none"), dict(name="hc-gone", mode="gone"),
             dict(name="herdr-gone", backend="herdr", mode="gone"),
             dict(name="new-task-lead", backend="herdr", mode="gone", lead=True),
             dict(name="create-only", choice=b"2"), dict(name="hc-ambiguous", mode="ambiguous"),
             dict(name="hc-mismatch", mode="mismatch"), dict(name="hc-unavailable", mode="unavailable"),
             dict(name="partial-thread", link="partial"), dict(name="duplicate-thread", link="duplicate")]
    results = []
    for case in cases:
        try:
            run_case(output, **case)
            results.append({"name": case["name"], "verdict": "PASS"})
        except Exception as error:
            results.append({"name": case["name"], "verdict": "FAIL", "error": str(error)})
        (output / case["name"] / "case-result.json").write_text(json.dumps(results[-1], indent=2))
        print(json.dumps(results[-1]), flush=True)
    report = {"head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
              "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(), "data": "sample",
              "runtime": "fixture CLI only; live delivery NOT_RUN", "cases": results}
    (output / "result.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
    return 0 if all(r["verdict"] == "PASS" for r in results) else 1


if __name__ == "__main__":
    raise SystemExit(main(Path(sys.argv[1]).resolve()))
