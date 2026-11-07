"""Cargo tests must not inherit a developer's EvoLoop shadow connection."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.name == "posix", "the live mirror and runner are Unix-only")
class TestRunnerTests(unittest.TestCase):
    def run_probe(self, in_deps, cwd):
        config = tomllib.loads((ROOT / ".cargo/config.toml").read_text())
        runner = config["target"]["cfg(unix)"]["runner"]
        with tempfile.TemporaryDirectory(prefix="tsk-runner-probe-") as directory:
            root = Path(directory)
            # Python's executable is only a fixture. A Cargo test lives in deps.
            executable = root / ("deps" if in_deps else "debug") / Path(sys.executable).name
            executable.parent.mkdir()
            executable.symlink_to(sys.executable)
            env = os.environ | {
                "CARGO_MANIFEST_DIR": str(ROOT),
                "PYTHONHOME": sys.base_prefix,
                "TSK_EVOLOOP_FUTURE": "fixture",
                "EVOLOOP_FUTURE": "fixture",
                "TSK_RUNNER_KEEP": "fixture",
            }
            probe = (
                "import json,os,sys; "
                "print(json.dumps({'keys': sorted(k for k in os.environ "
                "if k.startswith(('TSK_EVOLOOP_', 'EVOLOOP_'))), "
                "'keep':os.environ.get('TSK_RUNNER_KEEP'), "
                "'shadow_values': [os.environ.get(k) for k in "
                "('TSK_EVOLOOP_FUTURE','EVOLOOP_FUTURE')], "
                "'args':sys.argv[1:], 'cwd':os.getcwd()})); sys.exit(17)"
            )
            result = subprocess.run(
                [*runner, str(executable), "-c", probe, "space value", "--literal"],
                cwd=cwd, env=env, capture_output=True, text=True, timeout=30,
            )
            self.assertEqual(result.returncode, 17, result.stdout + result.stderr)
            return json.loads(result.stdout)

    def test_runner_removes_both_prefixes_and_keeps_arguments_and_cwd(self):
        observed = self.run_probe(True, ROOT / "src")
        self.assertEqual(observed["keys"], [])
        self.assertEqual(observed["keep"], "fixture")
        self.assertEqual(observed["args"], ["space value", "--literal"])
        self.assertEqual(Path(observed["cwd"]).resolve(), (ROOT / "src").resolve())

    def test_normal_cargo_run_keeps_shadow_environment(self):
        observed = self.run_probe(False, ROOT / "src")
        self.assertIn("TSK_EVOLOOP_FUTURE", observed["keys"])
        self.assertIn("EVOLOOP_FUTURE", observed["keys"])
        self.assertEqual(observed["shadow_values"], ["fixture", "fixture"])
        self.assertEqual(Path(observed["cwd"]).resolve(), (ROOT / "src").resolve())
        self.assertEqual(observed["keep"], "fixture")
        self.assertEqual(observed["args"], ["space value", "--literal"])
