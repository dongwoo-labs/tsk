#!/usr/bin/env python3
"""Machine-range acceptance against the actual release binary.

Build first. Install scripts/parity-requirements.txt in a test-local venv.
Run: <venv>/bin/python scripts/machine-pty-smoke.py <new-output-directory>
This preserves sample roots, raw ANSI, decoded cells, CLI results and provenance.
No personal state, real Herdr calls, notification delivery, product edits or baselines.
Stalled-task checks use the existing HERDR_BIN_PATH seam and a local notification sink.
Expected failures remain failures; independent cases continue after a failure.
"""
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pty
import re
import shutil
import time
import struct
import subprocess
import sys
import termios

import pyte

REPO = Path(__file__).resolve().parents[1]
BINARY = REPO / "target/release/tsk"
spec = importlib.util.spec_from_file_location("reports_pty", REPO / "scripts/reports-pty-smoke.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)


class Sample:
    def __init__(self, output):
        self.output = output
        self.output.mkdir(parents=True, exist_ok=False)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "TSK_"))}
        self.env.update(TSK_MACHINE="alpha", TSK_NO_UPDATE_CHECK="1", TERM="xterm-256color")

    def cli(self, state, *args, machine="alpha", keep=None):
        env = {**self.env, "TSK_MACHINE": machine}
        if keep is not None:
            env["TSK_DONE_KEEP_DAYS"] = str(keep)
        command = [str(BINARY), *args, "--state-dir", str(state)]
        p = subprocess.run(command, cwd=REPO, env=env, text=True, capture_output=True)
        with (self.output / "cli.jsonl").open("a") as file:
            file.write(json.dumps({"command": command, "machine": machine, "exit": p.returncode,
                                   "stdout": p.stdout, "stderr": p.stderr}) + "\n")
        if p.returncode:
            raise AssertionError(f"CLI {args}: exit {p.returncode}: {p.stderr}")
        return p.stdout

    def ask(self, state, number, summary):
        task = json.loads(self.cli(state, "list", f"T{number}", "--json"))[0]
        draft = {"task_id": task["id"], "kind": "question", "summary": summary,
                 "origin_thread": "t4239-sample-origin", "recipient": "t4239-sample-recipient",
                 "audience": "user", "text": "Isolated sample question. No sends.",
                 "options": ["One", "Two"], "recommendation": None, "impact": None,
                 "blocking_scope": None, "approval": None, "finding": None, "source": None}
        file = self.output / f"{summary}.json"
        file.write_text(json.dumps(draft))
        self.cli(state, "ask", "--file", str(file))

    def report(self, state, number, title):
        file = self.output / f"{title}.md"
        file.write_text(f"# {title}\n\nSample completion evidence.\n")
        self.cli(state, "report", f"T{number}", "--file", str(file))

    def normal(self):
        state = self.output / "state"
        for title, machine in [("Alpha active", "alpha"), ("Beta hidden blocker", "beta"),
                               ("Unspecified legacy", "alpha"), ("Alpha finished", "alpha"),
                               ("Beta finished", "beta"), ("Legacy finished", "alpha")]:
            self.cli(state, "add", "--desk", "-t", title, "--json", machine=machine)
        for n in (3, 6):
            self.cli(state, "edit", f"T{n}", "--no-machine")
        self.cli(state, "status", "T1", "blocked", "--why", "Waiting for beta dependency", "--on", "T2")
        self.cli(state, "edit", "T1", "--after", "T2", "--parent", "T2")
        for n in (4, 5, 6):
            self.cli(state, "status", f"T{n}", "done")
        for n, title in [(1, "Alpha sample Q"), (2, "Beta sample Q"), (3, "Legacy sample Q")]:
            self.ask(state, n, title)
        for n, title in [(4, "Alpha report"), (5, "Beta report"), (6, "Legacy report")]:
            self.report(state, n, title)
        return state

    def clone(self):
        producer, consumer = self.output / "producer", self.output / "consumer"
        self.cli(producer, "add", "--desk", "-t", "Foreign producer task", "--json", machine="beta")
        self.ask(producer, 1, "Foreign private Q")
        consumer.mkdir()
        shutil.copy2(producer / "tsk.json", consumer / "tsk.json")
        self.cli(consumer, "add", "--desk", "-t", "Local consumer task", "--json")
        self.ask(consumer, 2, "Local private Q")
        return consumer


class Board(base.Journey):
    def __init__(self, sample, state, name, width=78, height=30):
        self.output = sample.output / name
        self.output.mkdir()
        self.state = state
        self.sample = sample
        self.env = {**sample.env, "TSK_STATE_DIR": str(state)}
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        self.child = subprocess.Popen([str(BINARY)], stdin=slave, stdout=slave, stderr=slave,
                                     cwd=REPO, env=self.env, start_new_session=True,
                                     preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        os.close(slave)
        self.screen, self.raw = pyte.Screen(width, height), bytearray()
        self.stream = pyte.ByteStream(self.screen)

    def cli(self, *args):
        return self.sample.cli(self.state, *args)

    def scope(self, selector, name):
        self.key((":machine " + selector).encode(), name + "-palette", lambda s: ":machine " + selector in s)
        return self.key(b"\r", name, lambda s: "machine " + selector in s and ":machine " not in s)

    def board(self, name):
        return self.key(b"1", name, lambda s: "machine " in s and any(x in s for x in ("DOING", "TODO", "NEEDS YOU")))


CHECKS = []
def check(name, condition, observed):
    CHECKS.append({"name": name, "verdict": "PASS" if condition else "FAIL", "observed": observed})


def core(sample, state):
    j = Board(sample, state, "core")
    try:
        s = j.wait("01-default", lambda s: "machine @local" in s and "Alpha active" in s)
        check("local rows and counts", "Beta hidden blocker" not in s and "Unspecified legacy" in s
              and "2 questions ·2" in s and "3 reports ·2" in s, s)
        j.key(b"/Beta", "02-search", lambda s: 'no tasks match "Beta"' in s and "enter pin" in s)
        s = j.key(b"\r", "03-search-pinned", lambda s: "/Beta" in s and "enter pin" not in s)
        check("search excludes foreign", "T2 Beta" not in s, s)
        j.key(b"\x1b", "04-clear-search", lambda s: "/Beta" not in s)
        s = j.key(b"d", "05-local-drawer", lambda s: "T4 Alpha finished" in s)
        check("local drawer", "T6 Legacy finished" in s and "T5 Beta finished" not in s, s)
        j.key(b"d", "06-close-drawer", lambda s: "T4 Alpha finished" not in s)
        s = j.key(b"2", "07-local-questions", lambda s: "Alpha sample Q" in s)
        check("local questions", "Legacy sample Q" in s and "Beta sample Q" not in s, s)
        s = j.key(b"3", "08-local-reports", lambda s: "T4 Alpha finished" in s)
        check("local reports", "T6 Legacy finished" in s and "T5 Beta finished" not in s, s)
        j.board("09-board")
        for selector, title, question_count in [("@all", "Beta hidden blocker", 3),
                                                ("@unspecified", "Unspecified legacy", 1),
                                                ("@current", "Alpha active", 1), ("beta", "Beta hidden blocker", 1)]:
            s = j.scope(selector, "10-" + selector.replace("@", ""))
            check("palette " + selector, title in s and f"2 questions ·{question_count}" in s, s)
        s = j.key(b"M ", "11-mark-beta", lambda s: "1 selected" in s)
        s = j.scope("@current", "12-current-reanchor")
        check("scope clears hidden selection and marks", "1 selected" not in s and "T2 Beta" not in s
              and any("▸" in line and "T1 Alpha" in line for line in s.splitlines()), s)
        j.scope("@local", "13-local")
        j.key(b"/Alpha\r", "14-alpha-search", lambda s: "/Alpha" in s and "enter pin" not in s)
        s = j.key(b"\r", "15-alpha-page", lambda s: "blocked on T2" in s and "parent" in s)
        check("hidden blocker preserved", "after T2" in s and "Beta hidden blocker" in s, s)
        # The block card is the first tab stop. The parent is the second.
        j.key(b"\t", "16-block", lambda s: "w waiting on" in s)
        j.key(b"\t", "17-parent", lambda s: any("▸" in line and "parent" in line for line in s.splitlines()))
        s = j.key(b"\r", "18-hidden-parent", lambda s: "machine beta" in s)
        check("direct foreign relation", "T2 Beta hidden blocker" in s and "machine @local" in s, s)
        j.key(b"\x1b", "19-back-alpha", lambda s: "blocked on T2" in s)
        j.key(b"\x1b", "20-clear-page-query", lambda s: "/Alpha" not in s)
        # Traverse the actual list, including unspecified notices. Foreign T2 must never be a J/K destination.
        header = lambda text: next(line for line in text.splitlines() if line.strip())
        visited = []
        for i in range(6):
            before = header(j.text())
            s = j.key(b"J", f"21-next-{i}", lambda s: header(s) != before)
            visited.append(header(s))
            if "T3 Unspecified legacy" in header(s):
                break
        check("J traversal excludes foreign", any("T3" in x for x in visited) and all("T2" not in x for x in visited), visited)
        before = header(j.text())
        s = j.key(b"K", "22-previous", lambda s: header(s) != before)
        check("K traversal excludes foreign", "T2" not in header(s) and "N4" in header(s), s)
        j.resize(120, 30)
        s = j.wait("23-wide", lambda s: "│" in s)
        check("wide layout rendered", "│" in s, s)
        # Real retention path, not a fabricated historical task.
        sample.cli(state, "edit", "T3", "--notes", "Sample retention save", keep=0)
        history = [json.loads(x)["task"] for p in (state / "done").glob("*.jsonl") for x in p.read_text().splitlines()]
        check("done history retains machine metadata", {t["number"]: t.get("machine") for t in history} == {4: "alpha", 5: "beta", 6: None}, history)
        s = j.key(b"3", "24-history-reports", lambda s: "T4 Alpha finished" in s and "reports · 2 unread" in s)
        check("retained reports use actual machines", "T6 Legacy finished" in s and "T5 Beta finished" not in s, s)
    finally:
        j.capture("final")
        j.close()


def boundaries(sample, state):
    j = Board(sample, state, "boundaries")
    try:
        j.wait("01-local", lambda s: "machine @local" in s)
        j.key(b"2", "02-questions", lambda s: "Alpha sample Q" in s)
        j.key(b"\r", "03-question", lambda s: "3. Type something" in s)
        j.key(b"3Draft must survive machine reassignment", "04-draft", lambda s: "Draft must survive" in s)
        sample.cli(state, "edit", "T1", "--machine", "beta")
        # Observe the real idle sync, without a follow-up key.
        s = j.idle("05-draft-after-sync", seconds=3)
        check("draft survives external machine reassignment", "Draft must survive machine reassignment" in s, s)
        sample.cli(state, "edit", "T1", "--machine", "alpha")
        j.idle("06-restored-machine", seconds=2)
        s = j.key(b"\x1b\r", "07-reopen-original-question", lambda s: "Q1 · question" in s and "options" in s)
        check("original question retains draft after return", "Draft must survive machine reassignment" in s, s)
    finally:
        j.capture("final")
        j.close()
    # A fresh session prevents a failed draft scenario from influencing relation navigation.
    sample.report(state, 2, "Foreign relation report")
    j = Board(sample, state, "foreign-report")
    try:
        j.wait("01-local", lambda s: "machine @local" in s)
        j.key(b"/Alpha\r", "02-search", lambda s: "/Alpha" in s and "enter pin" not in s)
        j.key(b"\r\t", "03-block", lambda s: "blocked on T2" in s)
        j.key(b"\t", "04-parent", lambda s: any("▸" in line and "parent" in line for line in s.splitlines()))
        j.key(b"\r", "05-relation", lambda s: "machine beta" in s)
        os.write(j.master, b"r")
        s = j.idle("06-direct-report", seconds=1)
        check("foreign relation direct r report", "Foreign relation report" in s, s)
    finally:
        j.capture("final")
        j.close()


def trash(sample, state):
    j = Board(sample, state, "trash")
    try:
        j.wait("01-local", lambda s: "machine @local" in s)
        j.scope("beta", "02-beta")
        # Record and inspect the sole target before deleting this sample task.
        target = json.loads(sample.cli(state, "list", "T2", "--json"))[0]
        if target["title"] != "Beta hidden blocker" or target["machine"] != "beta":
            raise AssertionError("Unexpected delete target")
        (j.output / "delete-inventory.json").write_text(json.dumps(target, indent=2))
        j.key(b"\x18", "03-delete-confirm", lambda s: "press ctrl+x again" in s)
        j.key(b"\x18", "04-deleted", lambda s: "T2 Beta hidden blocker" not in s)
        j.scope("@unspecified", "05-unspecified")
        j.key(b"/Unspecified\r", "06-selected", lambda s: "/Unspecified" in s and "enter pin" not in s)
        # CLI edits do not add undo entries. Real TUI status changes exhaust the
        # deletion's undo reachability so the normal save moves it to trash.
        for i in range(26):
            j.key(b"\x13", f"07-start-{i}", lambda s: "DOING" in s and "TODO" not in s)
            j.key(b"\x0f", f"07-open-{i}", lambda s: "TODO" in s and "DOING" not in s)
        path = state / "trash.jsonl"
        if not path.exists():
            raise AssertionError("Normal deletion did not reach trash after 52 status saves")
        history = [json.loads(x)["task"] for x in path.read_text().splitlines()]
        check("trash retains beta metadata", any(t["number"] == 2 and t.get("machine") == "beta" for t in history), history)
        j.key(b"\x1b", "08-clear-query", lambda s: "/Unspecified" not in s)
        for selector in ("@local", "@current", "@unspecified"):
            j.scope(selector, "09-" + selector[1:])
            os.write(j.master, b"2")
            s = j.idle("10-Q-" + selector[1:], seconds=0.3)
            check("trashed beta question excluded " + selector, "Beta sample Q" not in s, s)
            os.write(j.master, b"3")
            s = j.idle("11-report-" + selector[1:], seconds=0.3)
            check("trashed beta report excluded " + selector, "T2 Beta hidden blocker" not in s, s)
            j.board("12-board-" + selector[1:])
    finally:
        j.capture("final")
        j.close()


def clone(sample):
    state = sample.clone()
    j = Board(sample, state, "clone")
    try:
        j.wait("01-default", lambda s: "machine @local" in s)
        os.write(j.master, b"2")
        s = j.idle("02-local-Q", seconds=0.5)
        check("clone local Q remains readable", "Local private Q" in s and "cannot be read" not in s, s)
        check("clone foreign Q excluded", "Foreign private Q" not in s, s)
        j.board("03-board")
        j.scope("@all", "04-all")
        os.write(j.master, b"2")
        s = j.idle("05-all-Q", seconds=0.5)
        check("clone unavailable is not absent or answered", "cannot be read" in s and "nothing is shown as zero" in s, s)
    finally:
        j.capture("final")
        j.close()


def stalled(sample):
    fixture = Sample(sample.output / "stall-fixture")
    state = fixture.output / "state"
    for number, machine in enumerate(("alpha", "beta", "alpha", "alpha", "beta", "alpha"), 1):
        fixture.cli(state, "add", "--desk", "-t", f"Stall sample {number}", "--json", machine=machine)
    for number in (3, 6):
        fixture.cli(state, "edit", f"T{number}", "--no-machine")
    agents = [{"name": "lead-sample", "agent_status": "idle", "pane_id": "wS:p0",
               "workspace_id": "wS", "tab_id": "wS:t0"}]
    for number in (1, 2, 3):
        fixture.cli(state, "edit", f"T{number}", "--notes", f"Thread: sample-{number} wS wS:t1 wS:p{number}")
        fixture.cli(state, "status", f"T{number}", "started")
        agents.append({"name": f"sample-{number}", "agent_status": "idle",
                       "pane_id": f"wS:p{number}", "workspace_id": "wS", "tab_id": "wS:t1"})
    # Synthetic age is the only direct store setup; task identity, status and
    # machine came from the real CLI. No time-dependent wait is needed.
    path = state / "tsk.json"
    data = json.loads(path.read_text())
    for task in data["tasks"]:
        task["updated_at"] = [int(time.time()) - 7200, 0]
    path.write_text(json.dumps(data))
    sink = fixture.output / "prompts.jsonl"
    host = fixture.output / "fake-herdr.py"
    host.write_text(f'''#!{sys.executable}
import json
from pathlib import Path
import sys
args = sys.argv[1:]
if args == ["agent", "list"]:
    print({json.dumps({"result": {"agents": agents}})!r})
elif args[:2] == ["agent", "prompt"] and len(args) == 4:
    with Path({str(sink)!r}).open("a") as output:
        output.write(json.dumps(args) + "\\n")
    print("{{}}")
else:
    raise SystemExit(2)
''')
    host.chmod(0o700)
    fixture.env.update(HERDR_ENV="1", HERDR_BIN_PATH=str(host), TSK_STALE_MINUTES="1")
    for selector, expected, counts in (("@local", {1, 3, 4, 6}, "2 stale doing, 2 todo"),
                                       ("@all", {1, 2, 3, 4, 5, 6}, "3 stale doing, 3 todo")):
        j = Board(fixture, state, selector[1:])
        try:
            j.wait("01-board", lambda s: "machine @local" in s)
            if selector != "@local":
                j.scope(selector, "02-scope")
            screen = j.key(b"\x0c", "03-check-all", lambda s: "asked lead-sample to check" in s)
            prompt = json.loads(sink.read_text().splitlines()[-1])
            numbers = {int(n) for n in re.findall(r"\bT(\d+)\b", prompt[3])}
            check("stalled and todo candidates " + selector,
                  numbers == expected and counts in screen and prompt[2] == "lead-sample",
                  {"screen": screen, "captured_prompt": prompt, "numbers": sorted(numbers),
                   "boundary": "HERDR_BIN_PATH fake list and notification sink; no real Herdr calls"})
        finally:
            j.capture("final")
            j.close()


def main():
    output = Path(sys.argv[1]).resolve()
    sample = Sample(output)
    result = {"binary": str(BINARY), "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
              "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
              "revision_kind": "dirty working tree" if subprocess.check_output(
                  ["git", "status", "--porcelain"], cwd=REPO) else "clean committed revision",
              "data": "fresh synthetic roots; actual CLI, PTY, store, questions and reports; fake Herdr notification sink only",
              "command": sys.argv, "checks": CHECKS}
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=REPO).decode().split("\0")
    paths.append("src/machine.rs")
    result["source_sha256"] = {p: hashlib.sha256((REPO / p).read_bytes()).hexdigest()
                               for p in sorted(set(paths)) if p and (REPO / p).is_file()}
    try:
        state = sample.normal()
        for name, run in [("core", lambda: core(sample, state)), ("boundaries", lambda: boundaries(sample, state)),
                          ("trash", lambda: trash(sample, state)), ("clone", lambda: clone(sample)),
                          ("stalled", lambda: stalled(sample))]:
            try:
                run()
            except Exception as error:
                CHECKS.append({"name": name + " execution", "verdict": "FAIL", "observed": str(error)})
    finally:
        result["verdict"] = "FAIL" if any(c["verdict"] != "PASS" for c in CHECKS) else "PASS"
        (output / "result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
        print(json.dumps({"verdict": result["verdict"], "checks": [{k:v for k,v in c.items() if k != "observed"} for c in CHECKS]}, ensure_ascii=False))
    return 1 if result["verdict"] == "FAIL" else 0


if __name__ == "__main__":
    sys.exit(main())
