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


class TestRunnerTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix", "the live mirror uses Unix curl")
    def test_cargo_question_store_does_not_inherit_shadow_env(self):
        with tempfile.TemporaryDirectory(prefix="tsk-test-env-") as directory:
            root = Path(directory)
            curl = root / "curl"
            marker = root / "called"
            token = root / "fixture-token"
            token.write_text("fixture-bearer")
            curl.write_text(
                '#!/bin/sh\ncat >/dev/null\n'
                'touch "$(dirname "$0")/called"\n'
                'printf \'%s\' \'{"user_id":"fixture-user","questionId":"fixture-remote",'
                '"headRevision":"fixture-head","events":[{"revision":"fixture-head"}]}\'\n'
            )
            curl.chmod(0o700)
            env = os.environ | {
                "TSK_EVOLOOP_URL": "https://must-not-contact.invalid",
                "TSK_EVOLOOP_TOKEN_FILE": str(token),
                "TSK_EVOLOOP_ORG": "fixture-org",
                "EVOLOOP_USER_ID": "fixture-user",
                "EVOLOOP_URL": "https://must-not-contact.invalid",
                "TSK_UPDATE_CURL": str(curl),
            }
            result = subprocess.run(
                ["cargo", "test", "--lib",
                 "store::tests::evoloop_copy_keeps_a_failed_question_pending_until_it_lands",
                 "--", "--exact"],
                cwd=ROOT, env=env, text=True, capture_output=True, timeout=180,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(marker.exists(), "a test reached the configured curl")

    def run_probe(self, in_deps, cwd):
        config = tomllib.loads((ROOT / ".cargo/config.toml").read_text())
        platform = "cfg(windows)" if os.name == "nt" else "cfg(unix)"
        runner = config["target"][platform]["runner"]
        with tempfile.TemporaryDirectory(prefix="tsk-runner-probe-") as directory:
            root = Path(directory)
            # Python's executable is only a fixture. A Cargo test lives in deps.
            executable = root / ("deps" if in_deps else "debug") / Path(sys.executable).name
            executable.parent.mkdir()
            if os.name == "nt":
                import shutil
                shutil.copy2(sys.executable, executable)
            else:
                executable.symlink_to(sys.executable)
            env = os.environ | {
                "CARGO_MANIFEST_DIR": str(ROOT),
                "PYTHONHOME": sys.prefix,
                "TSK_EVOLOOP_FUTURE": "fixture",
                "EVOLOOP_FUTURE": "fixture",
                "TSK_RUNNER_KEEP": "fixture",
            }
            probe = (
                "import json,os,sys; "
                "print(json.dumps({'keys': sorted(k for k in os.environ "
                "if k.startswith(('TSK_EVOLOOP_', 'EVOLOOP_'))), "
                "'keep':os.environ.get('TSK_RUNNER_KEEP'), "
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
        self.assertEqual(observed["keep"], "fixture")
        self.assertEqual(observed["args"], ["space value", "--literal"])
