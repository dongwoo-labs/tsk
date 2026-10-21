#!/usr/bin/env python3
"""Report acceptance against the release binary, not a render mock.

Prerequisites: cargo build --release; pip install -r scripts/parity-requirements.txt.
Run: python scripts/reports-pty-smoke.py [new-output-directory]
Each run retains its own sample store, ANSI transcript, decoded cells, and result.
No personal store, Herdr config, external service, or GUI screenshot is involved.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

import pyte
from wcwidth import wcswidth

REPO = Path(__file__).resolve().parents[1]
BINARY = REPO / "target/release/tsk"
MARKDOWN = """# Health check 완료

한글 보고서입니다. **검증 완료**

| Check | Result |
| --- | --- |
| HTTP | PASS |

```sh
curl http://localhost/health
```

[Runbook](https://example.com/runbook)

""" + "".join(f"검증 항목 {i:02}: 정상 동작을 확인했습니다.\n\n" for i in range(1, 41)) + "끝 END-REPORT\n"


class Journey:
    def __init__(self, output):
        self.output = output
        self.state = output / "state"
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "TSK_"))}
        self.env.update(TSK_STATE_DIR=str(self.state), TSK_NO_UPDATE_CHECK="1", TERM="xterm-256color")
        self.cli("add", "--desk", "-t", "Health check 완료 보고서", "--json")
        self.cli("status", "T1", "done")
        self.register(MARKDOWN)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 78, 0, 0))
        self.child = subprocess.Popen(
            [str(BINARY)], stdin=slave, stdout=slave, stderr=slave, cwd=REPO,
            env=self.env, start_new_session=True,
            preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
        )
        os.close(slave)
        self.screen = pyte.Screen(78, 24)
        self.stream = pyte.ByteStream(self.screen)
        self.raw = bytearray()

    def cli(self, *args):
        return subprocess.check_output([str(BINARY), *args], env=self.env, cwd=REPO, text=True)

    def record(self):
        return json.loads(self.cli("report", "T1", "--json"))

    def register(self, markdown):
        path = self.output / "report-input.md"
        path.write_text(markdown)
        return json.loads(self.cli("report", "T1", "--file", str(path), "--json"))

    def text(self):
        # pyte 0.8.2's display property indexes empty wide-character continuation
        # cells on Korean redraws. Preserve their width; an orphan empty cell is
        # blank, not a reason to shift every following cell left.
        lines = []
        for y in range(self.screen.lines):
            row = self.screen.buffer[y]
            cells = []
            for x in range(self.screen.columns):
                data = row[x].data
                continuation = x > 0 and wcswidth(row[x - 1].data) == 2
                cells.append(data if data else ("" if continuation else " "))
            lines.append("".join(cells))
        return "\n".join(lines)

    def pump(self):
        if select.select([self.master], [], [], 0.05)[0]:
            try:
                data = os.read(self.master, 65536)
            except OSError:
                return
            self.raw.extend(data)
            self.stream.feed(data)

    def capture(self, name):
        (self.output / f"{name}.ansi").write_bytes(self.raw)
        (self.output / f"{name}.txt").write_text(self.text())
        # Keep final cell styles as well as the original terminal byte stream.
        styles = [{"row": y, "column": x, "text": c.data, "bold": c.bold,
                   "underline": c.underscore, "fg": c.fg, "bg": c.bg}
                  for y, row in self.screen.buffer.items() for x, c in row.items()
                  if c.data.strip()]
        (self.output / f"{name}.cells.json").write_text(json.dumps(styles, ensure_ascii=False))

    def wait(self, name, predicate, timeout=8):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            self.pump()
            if predicate(self.text()):
                # A frame can arrive in several writes. Drain its remaining bytes
                # before taking evidence or sending the next user action.
                while select.select([self.master], [], [], 0.15)[0]:
                    if self.child.poll() is not None:
                        break
                    self.pump()
                if predicate(self.text()):
                    self.capture(name)
                    return self.text()
        self.capture(name)
        raise AssertionError(f"{name}: expected screen not reached\n{self.text()}")

    def key(self, value, name, predicate):
        os.write(self.master, value)
        return self.wait(name, predicate)

    def idle(self, name, seconds=2):
        # Observe across multiple idle refresh ticks, without sending a key.
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.pump()
        self.capture(name)
        return self.text()

    def resize(self, width, height):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        self.screen.resize(lines=height, columns=width)

    def close(self):
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait()
        os.close(self.master)


def run(j):
    j.wait("01-board", lambda s: "3 reports" in s)
    j.key(b"3", "02-unread-list", lambda s: "1 unread" in s)
    assert j.record()["unread"] is True
    j.key(b"\r", "03-open", lambda s: "revision 1" in s and "Check" in s)
    j.wait("04-ack", lambda s: "0 unread" in s)
    assert j.record()["read_revision"] == 1
    for key, name, at_top in [(b"\x1b[B", "04-down", False), (b"k", "04-k", True),
                              (b"j", "04-j", False), (b"\x1b[A", "04-up", True)]:
        j.key(key, name, lambda s: ("Health check 완료" in s.splitlines()[3]) == at_top)
    # Page keys and wheel must move the same real document viewport.
    j.key(b"\x1b[6~", "05-page-down", lambda s: "검증 항목 10" in s)
    j.key(b"\x1b[5~", "06-page-up", lambda s: "Health check 완료" in s.splitlines()[3])
    j.key(b"\x1b[<65;30;10M", "07-wheel-down",
          lambda s: "Health check 완료" not in s.splitlines()[3])
    j.key(b"\x1b[<64;30;10M", "08-wheel-up",
          lambda s: "Health check 완료" in s.splitlines()[3])
    # A report-only write refreshes the unread count, not the open snapshot.
    j.register("# NEW-REVISION\n\n새로운 버전입니다.\n")
    j.wait("09-idle-revision", lambda s: "1 unread" in s)
    assert "revision 1" in j.text() and "NEW-REVISION" not in j.text()
    assert j.record()["read_revision"] == 1

    # Inject a real read failure in this run's sample store only. Restore read
    # permission without changing the failed signature; idle recovery must work.
    path = j.state / "reports.json"
    path.chmod(0)
    stamp = path.stat()
    os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns + 1_000_000_000))
    try:
        j.wait("10-unavailable", lambda s: "reports · cannot be read" in s)
        assert "revision 1" in j.text() and "Health check 완료" in j.text()
        failed_signature = (path.stat().st_size, path.stat().st_mtime_ns)
    finally:
        path.chmod(0o600)
    assert failed_signature == (path.stat().st_size, path.stat().st_mtime_ns)
    j.wait("11-idle-recovered", lambda s: "1 unread" in s)
    assert "revision 1" in j.text()

    j.key(b"\x1b", "12-back", lambda s: "rev 2" in s)
    row = next(i for i, line in enumerate(j.text().splitlines(), 1) if "▤ T1" in line)
    mouse = f"\x1b[<0;12;{row}M\x1b[<0;12;{row}m".encode()
    j.key(mouse, "13-mouse-open", lambda s: "NEW-REVISION" in s and "0 unread" in s)
    assert j.record()["read_revision"] == 2

    # One journey crosses the compact and split layouts, rather than replaying
    # the whole scenario for a viewport matrix.
    j.resize(40, 10)
    j.wait("14-compact", lambda s: "NEW-REVISION" in s)
    j.key(b"\x1b", "15-compact-list", lambda s: "enter open" in s and "▤ T1" in s)
    j.key(b"\r", "16-compact-open", lambda s: "NEW-REVISION" in s)
    j.resize(120, 30)
    j.wait("17-wide", lambda s: "│" in s and "NEW-REVISION" in s)

    # Put the sample task back on the board to exercise the direct task shortcut.
    j.cli("status", "T1", "todo")
    j.key(b"1", "18-board", lambda s: "TODO" in s)
    j.wait("18-task-arrived", lambda s: any("T1 Health check" in line.split("│")[0]
                                           for line in s.splitlines()))
    j.key(b"/Health", "19-search", lambda s: "enter pin" in s)
    j.key(b"\r", "20-pinned", lambda s: "all · /Health" in s)
    j.key(b"r", "21-task-report", lambda s: "NEW-REVISION" in s and "j/k scroll" in s)

    # A failed acknowledgment remains visible on the snapshot. It is retried
    # only by Enter, not by silently acknowledging a background refresh.
    j.register("# RETRY-REVISION\n\n읽음 저장 재시도\n")
    j.wait("22-retry-unread", lambda s: "1 unread" in s)
    j.key(b"\x1b", "23-return-task", lambda s: "r opens the unread completion report" in s)
    j.key(b"3", "23-retry-list", lambda s: "rev 3" in s)
    path.chmod(0)
    try:
        j.key(b"\r", "24-ack-failure", lambda s: "mark read failed" in s and "enter retries" in s)
        j.idle("25-failure-retained")
        assert "mark read failed" in j.text() and "RETRY-REVISION" in j.text()
    finally:
        path.chmod(0o600)
    assert j.record()["read_revision"] == 2
    j.key(b"\r", "26-explicit-retry", lambda s: "0 unread" in s and "mark read failed" not in s)
    assert j.record()["read_revision"] == 3
    j.key(b"2", "27-questions", lambda s: "no questions" in s)
    j.key(b"3", "28-reports", lambda s: "reports · 0 unread" in s)

    # The new tab's digit must not steal a question's third choice. This is a
    # plain sample question, not an approval. HERDR_* is absent, so notification
    # delivery is deliberately outside this check.
    draft = {
        "task_id": j.record()["task_id"], "kind": "question",
        "summary": "Sample display choice", "origin_thread": "t4022-test-origin",
        "recipient": "t4022-test-recipient", "audience": "user",
        "text": "Choose a sample display. This is isolated test data.",
        "options": ["Compact", "Comfortable", "Detailed"],
        "recommendation": None, "impact": None, "blocking_scope": None,
        "approval": None, "finding": None, "source": None,
    }
    question = j.output / "question-input.json"
    question.write_text(json.dumps(draft))
    j.cli("ask", "--file", str(question))
    j.key(b"2", "29-question-list", lambda s: "Sample display choice" in s)
    j.key(b"\r", "30-question-open", lambda s: "3. Detailed" in s)
    j.key(b"3", "31-question-answer", lambda s: "Q1 saved" in s)
    assert json.loads(j.cli("questions", "Q1", "--json"))["answers"][-1]["text"] == "Detailed"
    j.key(b"\x11", "32-quit", lambda s: j.child.poll() is not None)
    j.child.wait(timeout=5)
    assert j.child.returncode == 0


def main():
    if len(sys.argv) > 1:
        output = Path(sys.argv[1]).resolve()
        output.mkdir(parents=True, exist_ok=False)
    else:
        root = REPO / ".artifacts/test-results/reports-pty"
        root.mkdir(parents=True, exist_ok=True)
        output = Path(tempfile.mkdtemp(prefix="run-", dir=root))
    result = {
        "kind": "real binary, PTY key input, CLI readback; ANSI output, not GUI screenshots",
        "data": "isolated synthetic sample; no mocks for the Rust app or persistence",
        "binary": str(BINARY), "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
        "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
        "dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=REPO)),
        "command": sys.argv, "output": str(output), "verdict": "FAIL",
    }
    journey = None
    try:
        journey = Journey(output)
        run(journey)
        result["verdict"] = "PASS"
    except Exception as error:
        result["error"] = str(error)
        raise
    finally:
        if journey:
            journey.capture("final")
            journey.close()
        (output / "result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
        print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
