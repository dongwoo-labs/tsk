#!/usr/bin/env python3
"""Run real local hc and Herdr dispatch journeys with synthetic data only."""
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
import signal
import struct
import subprocess
import tempfile
import termios
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', required=True, type=Path)
parser.add_argument('--out', required=True, type=Path)
args = parser.parse_args()
binary = args.binary.resolve()
out = args.out.resolve()
if out.exists():
    parser.error('--out must be a new directory')
out.mkdir(parents=True)
root = Path(tempfile.mkdtemp(prefix='tsk4508-', dir='/tmp'))
repo, state = root / 'repo', root / 'state'
repo.mkdir(); state.mkdir(); (root / 'home').mkdir()
installed = Path(shutil.which('tsk')).resolve()
installed_hash = hashlib.sha256(installed.read_bytes()).hexdigest()
env = {'PATH': os.environ['PATH'], 'HOME': str(root / 'home'), 'SHELL': '/bin/sh',
       'TERM': 'xterm-256color', 'LANG': 'en_US.UTF-8', 'TSK_STATE_DIR': str(state),
       'XDG_CONFIG_HOME': str(root / 'c'), 'XDG_STATE_HOME': str(root / 's'),
       'TSK_MACHINE': 'smoke-machine'}
hc_env = dict(env, TSK_THREAD_BACKEND='hc', HC_SESSION='smoke')
herdr_env = dict(env, HERDR_ENV='1', HERDR_CONFIG_PATH=str(root / 'herdr.toml'),
                 HERDR_SOCKET_PATH=str(root / 'herdr.sock'))
commands = []


def run(argv, runtime=env, expected=0):
    result = subprocess.run([str(x) for x in argv], env=runtime, text=True,
                            capture_output=True, timeout=40)
    commands.append({'argv': [str(x) for x in argv], 'exit': result.returncode,
                     'stdout': result.stdout, 'stderr': result.stderr})
    (out / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
    assert result.returncode == expected, commands[-1]
    return result.stdout


def tsk(*argv, runtime=env, expected=0):
    return run([binary, *argv, '--state-dir', state], runtime, expected)


def task(number):
    return json.loads(tsk('list', f'T{number}', '--json'))[0]


def tui(runtime, name, address, missing_task=None):
    prefix = 'hc-missing-board' if missing_task else 'hc-threads'
    pid, master = pty.fork()
    if pid == 0:
        os.execve(str(binary), ['tsk'] if missing_task else ['tsk', 'threads'], runtime)
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 120, 0, 0))
    raw = b''
    finished = False

    def observe(pattern, start=0):
        nonlocal raw
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            plain = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', raw[start:])
            if re.search(pattern, plain):
                return
            ready, _, _ = select.select([master], [], [], deadline - time.monotonic())
            if ready:
                try:
                    raw += os.read(master, 65536)
                except OSError:
                    break
        raise AssertionError(f'TUI did not render {pattern!r}')

    try:
        if missing_task:
            observe(b'missing')
            os.write(master, b'/' + name.encode() + b'\r')
            start = len(raw)
            os.write(master, b'\x04')
            observe(rb'Done\s*T' + str(missing_task).encode(), start)
            (out / (prefix + '-card.ansi')).write_bytes(raw)
            start = len(raw)
            os.write(master, b'y')
            observe(rb'workspace\s*closed', start)
            (out / (prefix + '-completed.ansi')).write_bytes(raw)
        else:
            observe(re.escape(name.encode()))
            (out / (prefix + '-initial.ansi')).write_bytes(raw)
            os.write(master, b'\r')
            observe(rb'HC_SESSION=smoke hc attach\s*' + re.escape(address.encode()))
            (out / (prefix + '-attach.ansi')).write_bytes(raw)
        os.write(master, b'\x11' if missing_task else b'q')
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            waited, status = os.waitpid(pid, os.WNOHANG)
            if waited:
                assert os.waitstatus_to_exitcode(status) == 0, status
                finished = True
                return
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                try:
                    raw += os.read(master, 65536)
                except OSError:
                    pass
        raise AssertionError('threads did not quit')
    finally:
        (out / (prefix + '-final.ansi')).write_bytes(raw)
        if not finished:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        os.close(master)
        if not finished:
            try:
                os.waitpid(pid, os.WNOHANG)
            except ChildProcessError:
                pass


agent = root / 'agent.py'
agent.write_text('''import json,os,sys

def report(state):
    print("\\033]7501;state="+state+":app=tsk-smoke\\033\\\\",end="",flush=True)
report("idle")
print("SMOKE_READY "+json.dumps({"state_dir":os.environ.get("TSK_STATE_DIR"),"backend":os.environ.get("TSK_THREAD_BACKEND"),"hc_session":os.environ.get("HC_SESSION")}),flush=True)
for line in sys.stdin:
    text=line.rstrip("\\r\\n")
    if text=="SMOKE_BLOCK":
        report("blocked");print("SMOKE_BLOCKED",flush=True)
    else:
        report("working");print("SMOKE_RECEIVED "+text,flush=True);report("idle")
''')
(state / 'config.toml').write_text(
    '[agent.smoke]\ncommand = '+json.dumps([shutil.which('python3'), '-u', str(agent)])+
    '\nprompt = ""\n[agent.smoke.env]\nTSK_STATE_DIR = "/tmp/smoke-must-not-redirect"\n')
(repo / 'README.md').write_text('Synthetic dispatch acceptance data.\n')
run(['git', 'init', '-b', 'main', repo])
run(['git', '-C', repo, 'add', 'README.md'])
run(['git', '-C', repo, '-c', 'user.name=smoke', '-c', 'user.email=smoke@example.invalid',
     'commit', '-m', 'Initialize synthetic dispatch fixture'])
server = None
server_log = None
summary = {'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
           'root': str(root), 'data': 'synthetic', 'model': 'NOT_RUN: Python OSC fixture, not an LLM',
           'versions': {}, 'checks': {}}
(out / 'setup.json').write_text(json.dumps(summary, indent=2)+'\n')
try:
    summary['versions']['hc'] = run(['hc', '--version']).strip()
    summary['versions']['herdr'] = run(['herdr', '--version']).strip()
    # The twentieth normal hc address is t1b. Exercise it through dispatch, not a parser stub.
    for _ in range(19):
        terminal = json.loads(run(['hc', 'run', '--bg', '--json', '--no-notify', '--', '/usr/bin/true'], hc_env))
        run(['hc', 'wait', terminal['address'], '--timeout', '10', '--json'], hc_env)
    tsk('add', '-t', 'Synthetic hc dispatch', '-n', 'Owner lead: smoke default lead-smoke\nThread: thr-smoke-hc',
        '-p', repo, '--assignee', 'smoke', '--base', 'main', '--json')
    missing = dict(hc_env); missing.pop('HC_SESSION')
    tsk('dispatch', 'T1', runtime=missing, expected=1)
    assert 'HC_SESSION' in commands[-1]['stderr'], commands[-1]
    assert task(1).get('dispatch') is None
    tsk('dispatch', 'T1', runtime=hc_env)
    first = task(1)
    record = first['dispatch']
    assert first['status'] == 'doing' and record['herdr_workspace_id'] == 'hc:smoke'
    entries = json.loads(run(['hc', 'ls', '--label', 'task=t1', '--json'], hc_env))['terminals']
    assert len(entries) == 1, entries
    address = entries[0]['address']; assert address == 't1b', address
    run(['hc', 'wait', address, '--timeout', '10', '--json'], hc_env)
    terminal = json.loads(run(['hc', 'get', address, '--json'], hc_env))
    assert terminal['labels'] == {'task':'t1', 'owner_lead':'lead-smoke', 'thread':'thr-smoke-hc',
        'worktree':record['worktree'], 'machine':'smoke-machine', 'sha':record['base_commit'][:7]}
    assert terminal['die_with_parent'] is False
    assert first['notes'].endswith('Thread: thr-smoke-hc hc:smoke - hc:smoke:t1b')
    screen = json.loads(run(['hc', 'read', address, '--json'], hc_env))['text']
    assert 'SMOKE_READY' in screen and json.dumps(str(state)) in screen
    assert 'smoke-must-not-redirect' not in screen
    rows = json.loads(run([binary, 'threads', '--json', '--all'], hc_env))
    assert len(rows) == 1 and rows[0]['pane_id'] == 'hc:smoke:t1b', rows
    tui(hc_env, 'thr-smoke-hc', address)
    summary['checks']['hc_dispatch_labels_store_threads_tui'] = 'PASS'
    tsk('dispatch', 'T1', '--again', runtime=hc_env, expected=1)
    assert 'live hc terminal' in commands[-1]['stderr'], commands[-1]
    tsk('status', 'T1', 'blocked', '--why', 'Synthetic reply test')
    assert 'sent to @smoke' in tsk('reply', 'T1', 'SMOKE_REPLY', '--send', runtime=hc_env)
    screen = json.loads(run(['hc', 'read', address, '--json'], hc_env))['text']
    assert 'SMOKE_RECEIVED [tsk T1 reply] SMOKE_REPLY' in screen
    run(['hc', 'send', address, 'SMOKE_BLOCK', '--json'], hc_env)
    run(['hc', 'wait', address, '--timeout', '10', '--json'], hc_env)
    assert 'not sent:' in tsk('reply', 'T1', 'SMOKE_MUST_NOT_SEND', '--send', runtime=hc_env)
    screen = json.loads(run(['hc', 'read', address, '--json'], hc_env))['text']
    assert 'SMOKE_MUST_NOT_SEND' not in screen
    summary['checks']['hc_reply_blocked_and_duplicate'] = 'PASS'
    wrong = dict(hc_env, HC_SESSION='other')
    tsk('clean', 'T1', '--json', runtime=wrong, expected=1)
    assert Path(record['worktree']).is_dir()
    dirty = Path(record['worktree']) / 'smoke-dirty.txt'; dirty.write_text('synthetic\n')
    tsk('clean', 'T1', '--json', runtime=hc_env, expected=1)
    assert dirty.read_text() == 'synthetic\n'
    dirty.unlink()
    clean = json.loads(tsk('clean', 'T1', '--json', runtime=hc_env))
    assert clean['worktree']['outcome'] == 'removed' and not Path(record['worktree']).exists()
    assert task(1)['dispatch']['cleaned'] is True and task(1)['status'] != 'done'
    assert json.loads(run(['hc', 'get', address, '--json'], hc_env))['state'] == 'finished'
    summary['checks']['hc_session_dirty_cleanup'] = 'PASS'
    tsk('add', '-t', 'SMOKE missing hc', '-n', 'Owner lead: smoke default lead-smoke\nThread: thr-smoke-missing',
        '-p', repo, '--assignee', 'smoke', '--base', 'main', '--json')
    tsk('dispatch', 'T2', runtime=hc_env)
    missing_record = task(2)['dispatch']
    missing_terminal = json.loads(run(['hc','ls','--label','task=t2','--json'],hc_env))['terminals'][0]['address']
    run(['hc','wait',missing_terminal,'--timeout','10','--json'],hc_env)
    checkout = Path(missing_record['worktree'])
    assert run(['git','-C',checkout,'status','--porcelain','--untracked-files=all']) == ''
    assert run(['git','-C',checkout,'ls-files']) == 'README.md\n'
    assert run(['git','-C',checkout,'ls-files','--others','--ignored','--exclude-standard']) == ''
    assert (checkout/'README.md').read_text() == 'Synthetic dispatch acceptance data.\n'
    run(['git','-C',repo,'worktree','remove',checkout])
    assert json.loads(run(['hc','get',missing_terminal,'--json'],hc_env))['state'] == 'live'
    tui(hc_env, 'SMOKE missing hc', missing_terminal, missing_task=2)
    assert task(2)['status'] == 'done' and task(2)['dispatch']['cleaned'] is True
    assert json.loads(run(['hc','get',missing_terminal,'--json'],hc_env))['state'] == 'finished'
    run(['git','-C',repo,'show-ref','--verify','refs/heads/'+missing_record['branch']])
    summary['checks']['hc_missing_worktree_board_cleanup'] = 'PASS'
    (root / 'herdr.toml').write_text('[worktrees]\ndirectory = '+json.dumps(str(root/'herdr-worktrees'))+'\n')
    server_log = (out / 'herdr-server.log').open('w')
    server = subprocess.Popen(['herdr', 'server'], env=herdr_env, stdout=server_log, stderr=subprocess.STDOUT)
    deadline = time.monotonic() + 10
    while not (root / 'herdr.sock').exists():
        assert server.poll() is None, 'test Herdr server exited'
        assert time.monotonic() < deadline, 'test Herdr server not ready'
        time.sleep(0.02)
    tsk('add', '-t', 'Synthetic default Herdr dispatch', '-p', repo, '--assignee', 'smoke', '--base', 'main', '--json')
    tsk('dispatch', 'T3', runtime=herdr_env)
    second = task(3)['dispatch']; assert not second['herdr_workspace_id'].startswith('hc:')
    panes = json.loads(run(['herdr','pane','list','--workspace',second['herdr_workspace_id']],herdr_env))['result']['panes']
    pane = panes[0]['pane_id']
    run(['herdr','pane','wait-output',pane,'--match','SMOKE_READY','--timeout','10000'],herdr_env)
    capture = run(['herdr','pane','read',pane,'--source','recent-unwrapped','--lines','40','--format','ansi'],herdr_env)
    (out/'herdr-launch.ansi').write_text(capture)
    assert json.dumps(str(state)) in capture and 'smoke-must-not-redirect' not in capture
    tsk('clean','T3','--json',runtime=herdr_env)
    assert task(3)['dispatch']['cleaned'] is True and not Path(second['worktree']).exists()
    summary['checks']['default_herdr_live_dispatch_cleanup'] = 'PASS'
    assert hashlib.sha256(installed.read_bytes()).hexdigest() == installed_hash
    summary['checks']['installed_binary_unchanged'] = 'PASS'
    summary['verdict'] = 'PASS'
except BaseException as error:
    summary['verdict'] = 'FAIL'; summary['error'] = str(error)
    raise
finally:
    # Stop only the isolated servers created above. Keep fixture files and evidence.
    if server is not None and server.poll() is None:
        run(['herdr','workspace','list'],herdr_env)
        run(['herdr','server','stop'],herdr_env)
        server.wait(timeout=15)
    if server_log is not None: server_log.close()
    inventory = json.loads(run(['hc','ls','--all','--json'],hc_env))
    (out/'hc-final-inventory.json').write_text(json.dumps(inventory,indent=2)+'\n')
    if not any(t['state']=='live' for t in inventory['terminals']):
        run(['hc','server','stop','--json'],hc_env)
        summary['runtime_cleanup'] = 'PASS'
    else:
        summary['runtime_cleanup'] = 'UNKNOWN: live synthetic terminals preserved for inspection'
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
