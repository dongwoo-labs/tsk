#!/usr/bin/env python3
"""Replay the compact-board journey in real PTYs; ANSI/text are not screenshots.

Install scripts/parity-requirements.txt first. Supply an approved, preserved store
snapshot, not a live store. Each invocation retains fresh state copies and evidence
under --output. No question answers or cross-thread focus commands are sent.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time

import pyte
from wcwidth import wcswidth


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Terminal:
    def __init__(self, binary, state, output, width, rows):
        self.output = output
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, width, 0, 0))
        env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'TSK_'))}
        env.update(TSK_STATE_DIR=str(state), TSK_NO_UPDATE_CHECK='1',
                   HERDR_SOCKET_PATH=str(output / 'no-herdr.sock'), TERM='xterm-256color')
        self.child = subprocess.Popen(
            [str(binary)], stdin=slave, stdout=slave, stderr=slave,
            cwd=output, env=env, start_new_session=True,
            preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
        )
        os.close(slave)
        self.screen = pyte.Screen(width, rows)
        self.stream = pyte.ByteStream(self.screen)
        self.transcript = bytearray()

    def row(self, y, end):
        # pyte.display indexes empty cells after CJK repaints. Standalone empty
        # cells are blanks; only the second cell of a wide glyph is skipped.
        cells, x = [], 0
        while x < end:
            data = self.screen.buffer[y][x].data or ' '
            cells.append(data)
            x += max(1, wcswidth(data))
        return ''.join(cells)

    def text(self):
        return '\n'.join(self.row(y, self.screen.columns) for y in range(self.screen.lines))

    def list_rows(self):
        # Slice terminal cells, not Python character offsets: CJK cells are wide.
        divider = next((x for x in range(self.screen.columns)
                        if self.screen.buffer[0][x].data == '│'), self.screen.columns)
        return [self.row(y, divider) for y in range(self.screen.lines)]

    def wait(self, predicate, label):
        deadline = time.monotonic() + 8
        last_data = time.monotonic()
        while time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.05)[0]:
                try:
                    data = os.read(self.master, 65536)
                except OSError:
                    break
                self.transcript.extend(data)
                self.stream.feed(data)
                last_data = time.monotonic()
            # A frame spans multiple reads. Match only after the PTY settles.
            if time.monotonic() - last_data >= 0.15 and predicate():
                self.save(label)
                return
        self.save(label + '-FAIL')
        raise AssertionError(f'{label}: expected screen not observed; see {self.output}')

    def key(self, key, predicate, label):
        os.write(self.master, key)
        self.wait(predicate, label)

    def save(self, label):
        (self.output / f'{label}.txt').write_text(self.text())
        (self.output / 'transcript.ansi').write_bytes(self.transcript)

    def quit(self):
        os.write(self.master, b'\x11')
        deadline = time.monotonic() + 5
        # Drain shutdown output before waitpid; an unread PTY can block exit.
        while self.child.poll() is None and time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.05)[0]:
                try:
                    data = os.read(self.master, 65536)
                except OSError:
                    break
                self.transcript.extend(data)
                self.stream.feed(data)
        self.child.wait(timeout=5)
        assert self.child.returncode == 0, self.child.returncode
        (self.output / 'transcript.ansi').write_bytes(self.transcript)

    def close(self):
        os.close(self.master)
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--snapshot', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--expected-doing', type=int, required=True)
    parser.add_argument('--detail-task', type=int, required=True,
                        help='Visible Doing task with Blocked notes and at least one step')
    parser.add_argument('--widths', type=int, nargs='+', default=[200, 78])
    parser.add_argument('--rows', type=int, default=82)
    args = parser.parse_args()
    source, binary, output = (p.resolve() for p in (args.snapshot, args.binary, args.output))
    assert source != Path.home() / '.local/state/agent-home/tsk', 'Use a preserved snapshot'
    assert output != source and source not in output.parents, 'Keep output outside the snapshot'
    source_hash = digest(source / 'tsk.json')
    document = json.loads((source / 'tsk.json').read_text())
    doing = [t for t in document['tasks'] if t['status'] == 'doing'
             and not t.get('archived') and not t.get('soft_deleted')]
    assert len(doing) == args.expected_doing
    expected = {t['number'] for t in doing}
    task = next(t for t in doing if t['number'] == args.detail_task)
    assert task.get('steps'), 'Choose a task with a stored step'
    blocked = next(line.split(':', 1)[1].strip() for line in task['notes'].splitlines()
                   if line.lower().startswith('blocked:'))
    output.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    repo = Path(__file__).resolve().parents[1]
    report = {
        'kind': 'real PTY replay; ANSI/text only, not GUI screenshot',
        'binary': str(binary), 'binary_sha256': digest(binary),
        'snapshot': str(source), 'snapshot_sha256': source_hash,
        'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
        'dirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=repo)),
        'expected_doing': args.expected_doing, 'runs': [],
        'boundary': 'Herdr socket disconnected; origin-thread focus and question answers NOT_RUN',
        'visual': 'BLOCKED unless separately captured and inspected in the real GUI',
    }
    try:
        for width in args.widths:
            evidence = run / str(width)
            evidence.mkdir()
            state = evidence / 'state'
            shutil.copytree(source, state)
            terminal = Terminal(binary, state, evidence, width, args.rows)
            result = {'size': [width, args.rows], 'verdict': 'FAIL', 'checks': []}
            report['runs'].append(result)
            try:
                terminal.wait(lambda: 'DOING' in terminal.text() and 'DONE' in terminal.text(), 'initial')

                def compact():
                    lines = terminal.list_rows()
                    start = next(i for i, line in enumerate(lines) if 'DOING ─' in line)
                    end = next(i for i, line in enumerate(lines) if 'TODO ─' in line)
                    body = lines[start + 1:end]
                    ids = [int(m.group(1)) for line in body
                           if (m := re.match(r'^\s*▸?\s*T(\d+)\s', line))]
                    assert len(ids) == len(expected) and set(ids) == expected, ids
                    assert not any(re.match(r'^ {8,}\S', line) for line in body), body
                    assert not any('blocked:' in line.lower() for line in body)
                    assert any('!' in line for line in body), 'approval badge missing'
                    assert any('?1' in line for line in body), 'question badge missing'
                    assert any(re.search(r'✓\d+/\d+', line) for line in body), 'step count missing'
                    markers = [char for row in terminal.screen.buffer.values()
                               for x, char in row.items()
                               if x < (width * 2 // 5 if width >= 110 else width) and char.data == '×']
                    assert markers and all(c.bold and c.fg == 'default' for c in markers)

                compact()
                result['checks'].append('all Doing rows compact; blocked reasons hidden; bold mono marker and badges')
                terminal.key(b'v', lambda: 'blocked:' in '\n'.join(terminal.list_rows()), 'status-on')
                assert blocked[:20] in '\n'.join(terminal.list_rows())
                terminal.key(b'v', lambda: 'blocked:' not in '\n'.join(terminal.list_rows()), 'status-off')
                compact()
                result['checks'].append('v on/off')
                terminal.key(b'?', lambda: 'search keys or actions' in terminal.text(), 'help')
                terminal.key(b'status', lambda: 'show / hide status lines' in terminal.text(), 'help-status')
                terminal.key(b'\x1b', lambda: 'search keys or actions' in terminal.text(), 'help-cleared')
                terminal.key(b'\x1b', lambda: 'search keys or actions' not in terminal.text(), 'help-closed')
                result['checks'].append('searchable v help')
                for step in range(len(doing) + 2):
                    lines = terminal.list_rows()
                    selected = next(i for i, line in enumerate(lines) if '▸' in line)
                    target = next(i for i, line in enumerate(lines) if re.search(rf'\bT{task["number"]}\s', line))
                    if selected == target:
                        break
                    previous = lines[selected]
                    terminal.key(b'j' if selected < target else b'k',
                                 lambda: previous not in terminal.list_rows(), f'select-{step}')
                else:
                    raise AssertionError('Could not select detail task')
                terminal.key(b'\r', lambda: 'esc close' in terminal.text(), 'detail')
                normalized = ''.join(terminal.text().split())
                # In wide mode use the detail cells only, so list text cannot satisfy notes.
                if width >= 110:
                    normalized = ''.join(''.join(terminal.screen.buffer[y][x].data
                                         for y in range(terminal.screen.lines)
                                         for x in range(width * 2 // 5 + 1, width)).split())
                for line in task['notes'].splitlines():
                    assert ''.join(line.split()) in normalized, 'Full note line absent from detail'
                result['checks'].append('Enter full notes, including Blocked reason and final Thread line')
                terminal.key(b'v', lambda: 'esc close' in terminal.text(), 'detail-v')
                if width >= 110:
                    compact()
                terminal.key(b'\t', lambda: any('▸' in line and task['steps'][0]['text'][:12] in line
                                               for line in terminal.text().splitlines()), 'step-selected')
                terminal.key(b'e', lambda: 'enter step · shift+enter save' in terminal.text(), 'editor')
                terminal.key(b'v', lambda: ''.join(task['steps'][0]['text'].split()) + 'v'
                             in ''.join(terminal.text().split()), 'editor-v')
                terminal.key(b'\x1b', lambda: 'enter step · shift+enter save' not in terminal.text(), 'editor-cancel')
                terminal.key(b'\x1b', lambda: 'ctrl+e edit' in terminal.text(), 'draft-cancel')
                terminal.key(b'\x1b', lambda: 'enter open' in terminal.text(), 'list-return')
                compact()
                result['checks'].append('v is inert on detail and text in editor; draft canceled')
                terminal.key(b'2', lambda: 'questions ·' in terminal.text(), 'questions')
                terminal.key(b'\r', lambda: 'o thread' in terminal.text(), 'question-page')
                result['checks'].append('questions tab and page-only o thread hint')
                terminal.key(b'\x1b', lambda: '1 board' in terminal.text(), 'question-back')
                terminal.key(b'1', lambda: 'DOING' in terminal.text(), 'board-return')
                terminal.key(b'v', lambda: 'blocked:' in '\n'.join(terminal.list_rows()), 'before-restart')
                terminal.quit()
                terminal.close()
                # Restart the same copied store: status visibility must not persist.
                restart = evidence / 'restart'
                restart.mkdir()
                terminal = Terminal(binary, state, restart, width, args.rows)
                terminal.wait(lambda: 'DOING' in terminal.text() and 'DONE' in terminal.text(), 'restart')
                compact()
                terminal.quit()
                saved = json.loads((state / 'tsk.json').read_text())
                assert saved['tasks'] == document['tasks'], 'A read-only journey changed tasks'
                assert saved['questions'] == document['questions'], 'Questions changed'
                result['checks'].append('restart defaults hidden; tasks and questions unchanged')
                result['verdict'] = 'PASS'
            finally:
                terminal.close()
        assert digest(source / 'tsk.json') == source_hash, 'Preserved snapshot changed'
    finally:
        (run / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        print(run / 'result.json')


if __name__ == '__main__':
    main()
