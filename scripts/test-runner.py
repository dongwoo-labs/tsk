"""Isolate Cargo test executables, but preserve the environment for cargo run."""
import os
from pathlib import Path
import subprocess
import sys

command = sys.argv[1:]
env = os.environ.copy()
# Cargo puts test and benchmark executables in deps, also with a custom target-dir.
if Path(command[0]).parent.name == "deps":
    env = {key: value for key, value in env.items()
           if not key.startswith(("TSK_EVOLOOP_", "EVOLOOP_"))}
if os.name == "posix":
    os.execvpe(command[0], command, env)
# Windows exec does not retain the waited-on process and its final exit code.
raise SystemExit(subprocess.run(command, env=env).returncode)
