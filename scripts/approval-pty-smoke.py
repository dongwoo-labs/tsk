#!/usr/bin/env python3
"""복수 승인 수락 검사. 실제 release binary와 격리된 샘플 store를 사용한다.

준비: cargo build --release
실행: uv run --with-requirements scripts/parity-requirements.txt python \
      scripts/approval-pty-smoke.py <새 출력 디렉터리>
ANSI, 화면 셀, CLI 결과와 binary SHA-256을 보존한다.
Herdr 알림과 실제 배포는 실행하지 않는다. 승인 대상은 샘플이다.
"""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

REPO = Path(__file__).resolve().parents[1]
BINARY = REPO / "target/release/tsk"
spec = importlib.util.spec_from_file_location("machine_pty", REPO / "scripts/machine-pty-smoke.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)


def run(output):
    sample = base.Sample(output)
    state = output / "state"
    sample.cli(state, "add", "--desk", "-t", "Multiple approval smoke", "--json")
    task = json.loads(sample.cli(state, "list", "T1", "--json"))[0]["id"]
    terms = [
        {"target": target, "action": "deploy sample", "h": str(n) * 40,
         "scope": scope, "rollback": "restore sample " + scope}
        for n, target, scope in [(1, "T12", "web"), (2, "T202", "worker"), (3, "T205", "scheduler")]
    ]
    for name, approval in [("Approve sample", terms), ("Reject sample", terms),
                           ("Comment sample", terms), ("Legacy sample", terms[0])]:
        draft = {"task_id": task, "kind": "approval", "audience": "user", "summary": name,
                 "text": "Sample only. No deployment runs.", "origin_thread": "approval-smoke",
                 "recipient": "approval-smoke", "approval": approval}
        file = output / (name.replace(" ", "-") + ".json")
        file.write_text(json.dumps(draft))
        sample.cli(state, "ask", "--file", str(file))

    def records():
        return json.loads(sample.cli(state, "questions", "--json"))

    j = base.Board(sample, state, "screens", width=78, height=24)
    try:
        j.wait("01-board", lambda s: "approval 4" in s)
        j.key(b"2", "02-list", lambda s: "NEEDS APPROVAL" in s)
        top = j.key(b"\r", "03-terms-first", lambda s: "terms 1 / 3" in s)
        assert "target    T12" in top and "restore sample web" in top
        # 각 PageDown은 한 화면 행을 이동한다. 마지막 조건을 실제로 읽는다.
        bottom = j.key(b"\x1b[6~" * 16, "04-terms-last",
                       lambda s: "restore sample scheduler" in s)
        for field in ["terms 3 / 3", "deploy sample", "T205", "scheduler", "3" * 40]:
            assert field in bottom, field
        j.key(b"m", "05-comment-field", lambda s: "comment" in s.lower())
        j.key(b"sample checked\x1b", "06-staged-comment",
              lambda s: "Comment · sample checked" in s)
        j.key(b"1", "07-approved", lambda s: "Q1 saved" in s)
        record = records()[0]
        answer = record["answers"][-1]
        assert answer["approval"] == terms == record["draft"]["approval"]
        assert answer["origin"] == "user_ui" and answer["comment"] == "sample checked"
        assert answer["text"] == "approved"

        j.key(b"\r", "08-reject-open", lambda s: "Q2 · approval · open" in s)
        j.key(b"2", "09-rejected", lambda s: "Q2 saved" in s)
        answer = records()[1]["answers"][-1]
        assert answer["text"] == "rejected" and answer["approval"] is None

        j.key(b"\r", "10-comment-open", lambda s: "Q3 · approval · open" in s)
        j.key(b"m", "11-comment-field", lambda s: "comment" in s.lower())
        j.key(b"not yet\r", "12-commented", lambda s: "Q3 saved" in s)
        answer = records()[2]["answers"][-1]
        assert answer["text"] == "commented" and answer["approval"] is None
        assert answer["comment"] == "not yet"

        legacy = j.key(b"\r", "13-legacy-open", lambda s: "Q4 · approval · open" in s)
        assert "target    T12" in legacy and "terms 1 /" not in legacy
        j.key(b"1", "14-legacy-approved", lambda s: "Q4 saved" in s)
        assert records()[3]["answers"][-1]["approval"] == terms[0]
        j.key(b"z", "15-closed", lambda s: "Q1" in s and "Q4" in s)
        result = records()
        (output / "readback.json").write_text(json.dumps(result, indent=2))
        j.key(b"\x1b", "16-exit", lambda s: j.child.poll() is not None)
        assert records() == result, "종료 후에도 전체 조건과 이력을 보존한다"
    finally:
        j.close()


if __name__ == "__main__":
    output = Path(sys.argv[1]).resolve()
    if output.exists():
        raise SystemExit("기존 증거를 덮어쓰지 않도록 새 출력 디렉터리를 지정하십시오.")
    result = {"verdict": "UNKNOWN", "data": "sample", "notifications": "disabled",
              "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
              "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest()}
    try:
        run(output)
        result["verdict"] = "PASS"
    except Exception as error:
        result.update(verdict="FAIL", error=str(error))
        raise
    finally:
        if output.is_dir():
            (output / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result))
