import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


@unittest.skipUnless(
    os.name == "posix" and shutil.which("bash"), "POSIX fake Bazel executable"
)
class WindowsLocalFallbackTest(unittest.TestCase):
    def test_fallback_preserves_target_and_explicit_overrides(self) -> None:
        script = Path(__file__).with_name("run-bazel-ci.sh")
        with tempfile.TemporaryDirectory() as directory:
            fake_bazel = Path(directory) / "bazel"
            fake_bazel.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\"\n")
            fake_bazel.chmod(0o755)
            env = os.environ.copy()
            env.update(
                RUNNER_OS="Windows",
                BUILDBUDDY_API_KEY="",
                CODEX_BAZEL_BIN=str(fake_bazel),
                CODEX_BAZEL_WINDOWS_PATH="C:/Program Files/Git/bin",
            )
            for override in ([], ["--platforms=//:explicit_target"]):
                with self.subTest(override=override):
                    result = subprocess.run(
                        [
                            "bash",
                            str(script),
                            "--windows-cross-compile",
                            "--",
                            "test",
                            *override,
                            "--",
                            "//codex-rs/voice-host:voice-host-lifecycle-test",
                        ],
                        env=env,
                        capture_output=True,
                        text=True,
                        check=True,
                    )
                    args = result.stdout.splitlines()
                    self.assertIn("--host_platform=//:local_windows_msvc", args)
                    self.assertEqual(
                        [arg for arg in args if arg.startswith("--platforms=")],
                        override or ["--platforms=//:windows_x86_64_gnullvm"],
                    )
                    self.assertIn(
                        "--extra_toolchains=//:windows_gnullvm_tests_on_msvc_host_toolchain",
                        args,
                    )
                    self.assertNotIn("--host_platform=//:rbe", args)
                    self.assertNotIn("--config=ci-windows-cross", args)


if __name__ == "__main__":
    unittest.main()
