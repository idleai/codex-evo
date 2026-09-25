"""Exercise the release helper without touching the running Remote daemon."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


HELPER = Path(__file__).resolve().parents[1] / "restart-custom-codex-daemon.sh"
VERSION = "0.158.0-alpha.14"
MOCK_CODEX = r"""#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shutil
import sys

home = Path(os.environ["CODEX_HOME"])
args = sys.argv[1:]
with (home / "commands.jsonl").open("a") as log:
    log.write(json.dumps(args) + "\n")
version = os.environ.get("TEST_CLI_VERSION", "0.158.0-alpha.14")
if args == ["--version"]:
    print(f"codex-cli {version}")
elif args == ["app-server", "daemon", "update", "--from-cli", "--yes"]:
    if os.environ.get("TEST_UPDATE_FAIL"):
        sys.exit(1)
    destination = home / "packages/app-server-daemon/current"
    shutil.copytree(os.environ["CODEX_CUSTOM_RELEASE"], destination)
elif args == ["app-server", "daemon", "version"]:
    print(json.dumps({
        "status": "running",
        "cliVersion": version,
        "managedCodexVersion": version,
        "appServerVersion": os.environ.get("TEST_SERVER_VERSION", version),
    }))
elif args not in (["app-server", "daemon", "start"],
                  ["app-server", "daemon", "enable-remote-control"]):
    raise SystemExit(f"Unexpected command: {args}")
"""


@unittest.skipUnless(sys.platform == "linux", "the release helper targets GNU/Linux")
class RestartCustomCodexTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name)
        self.package = self.home / "packages/standalone/releases/custom"
        self.package.mkdir(parents=True)
        for name in (
            "bin/codex",
            "bin/codex-code-mode-host",
            "codex-resources/bwrap",
            "codex-path/rg",
        ):
            binary = self.package / name
            binary.parent.mkdir(parents=True, exist_ok=True)
            binary.write_text(
                MOCK_CODEX if name == "bin/codex" else "#!/bin/sh\nexit 0\n"
            )
            binary.chmod(0o755)
        (self.package / "codex-package.json").write_text(
            json.dumps(
                {
                    "version": VERSION,
                    "target": "x86_64-unknown-linux-gnu",
                    "entrypoint": "bin/codex",
                }
            )
        )
        self.current = self.home / "packages/standalone/current"
        self.previous = self.package.parent / "previous"
        self.previous.mkdir()
        self.current.symlink_to(self.previous)
        self.environment = {
            **os.environ,
            "CODEX_HOME": str(self.home),
            "CODEX_CUSTOM_RELEASE": str(self.package),
        }

    def run_helper(self, *args, **overrides):
        return subprocess.run(
            ["bash", str(HELPER), *args],
            env={**self.environment, **overrides},
            capture_output=True,
            text=True,
            timeout=10,
        )

    def commands(self):
        return [
            json.loads(line)
            for line in (self.home / "commands.jsonl").read_text().splitlines()
        ]

    def test_check_leaves_both_installations_untouched(self):
        result = self.run_helper("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.commands(), [["--version"]])
        self.assertEqual(self.current.resolve(), self.previous)
        self.assertFalse((self.home / "packages/app-server-daemon").exists())

    def test_activation_pins_then_starts_and_enables_remote_before_selecting_cli(self):
        result = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.commands(),
            [
                ["--version"],
                ["app-server", "daemon", "update", "--from-cli", "--yes"],
                ["app-server", "daemon", "start"],
                ["app-server", "daemon", "enable-remote-control"],
                ["app-server", "daemon", "version"],
            ],
        )
        self.assertEqual(self.current.resolve(), self.package)

    def test_failed_install_preserves_cli_selection(self):
        result = self.run_helper(TEST_UPDATE_FAIL="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.current.resolve(), self.previous)
        self.assertEqual(len(self.commands()), 2)

    def test_wrong_running_version_preserves_cli_selection(self):
        result = self.run_helper(TEST_SERVER_VERSION="0.154.0-alpha.11")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("version verification failed", result.stderr)
        self.assertEqual(self.current.resolve(), self.previous)

    def test_wrong_package_version_never_restarts(self):
        result = self.run_helper(TEST_CLI_VERSION="0.0.0")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.commands(), [["--version"]])
        self.assertEqual(self.current.resolve(), self.previous)


if __name__ == "__main__":
    unittest.main()
