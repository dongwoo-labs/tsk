"""Isolate Cargo test executables, but preserve the environment for cargo run."""
import os
from pathlib import Path
import sys

command = sys.argv[1:]
env = os.environ.copy()
# Cargo puts test and benchmark executables in deps, also with a custom target-dir.
if Path(command[0]).parent.name == "deps":
    env = {key: value for key, value in env.items()
           if not key.startswith(("TSK_EVOLOOP_", "EVOLOOP_"))}
os.execvpe(command[0], command, env)
