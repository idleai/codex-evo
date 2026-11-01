import tempfile
import unittest
from pathlib import Path
import subprocess

from select_idle_ci import changed_paths, select


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for crate in ("core", "app-server", "ext/idle-runtime", "utils/absolute-path"):
            path = self.root / "codex-rs" / crate / "BUILD.bazel"
            path.parent.mkdir(parents=True)
            path.touch()

    def plan(self, paths, **options):
        return select(paths, **options)

    def test_docs_do_not_allocate_a_rust_runner(self):
        self.assertFalse(self.plan(["docs/start.md"])["rust"])

    def test_changed_leaf_selects_bounded_runtime_smoke_checks(self):
        plan = self.plan(["codex-rs/utils/absolute-path/src/lib.rs"])
        self.assertIn(
            "//codex-rs/ext/idle-runtime:idle-runtime-grants-test", plan["query"]
        )
        self.assertIn(
            "//codex-rs/cli:cli-exec_server_websocket_auth-test", plan["query"]
        )
        self.assertNotIn("//codex-rs/core:all", plan["query"])
        self.assertNotIn("//codex-rs/app-server:all", plan["query"])
        self.assertFalse(plan["full"])

    def test_build_changes_and_removed_packages_keep_the_pr_budget(self):
        for path in [
            "codex-rs/Cargo.lock",
            "codex-rs/removed-crate/src/lib.rs",
            "MODULE.bazel",
        ]:
            with self.subTest(path=path):
                plan = self.plan([path])
                self.assertTrue(plan["rust"])
                self.assertNotIn("tests(set(//codex-rs/...))", plan["query"])

    def test_async_validation_always_selects_the_full_suite_and_sdks(self):
        plan = self.plan([], full=True)
        self.assertTrue(plan["rust"])
        self.assertTrue(plan["sdk"])
        self.assertIn("tests(set(//codex-rs/...))", plan["query"])

    def test_code_mode_only_edits_do_not_add_rust_checks(self):
        for crate in (
            "code-mode",
            "code-mode-host",
            "code-mode-protocol",
            "code-mode-runtime",
        ):
            self.assertFalse(self.plan([f"codex-rs/{crate}/src/lib.rs"])["rust"])
        self.assertFalse(self.plan(["third_party/v8/BUILD.bazel"])["rust"])

    def test_full_run_excludes_dedicated_code_mode_and_cross_tests(self):
        plan = self.plan([], full=True)
        self.assertTrue(plan["rust"])
        self.assertIn(
            "code[-_]mode|v8-poc|windows-cross|windows_sandbox|wine-exec",
            plan["query"],
        )

    def test_sdk_is_scoped_separately(self):
        plan = self.plan(["sdk/typescript/src/client.ts"])
        self.assertTrue(plan["sdk"])
        self.assertFalse(plan["rust"])
        self.assertTrue(self.plan(["pnpm-lock.yaml"])["sdk"])

    def test_ci_edits_validate_the_gate(self):
        self.assertTrue(self.plan([".github/workflows/blocking-ci.yml"])["rust"])

    def test_renames_include_old_and_new_paths(self):
        def git(*args):
            return subprocess.check_output(
                ["git", *args], cwd=self.root, text=True
            ).strip()

        git("init", "-q")
        git("config", "user.email", "ci@example.invalid")
        git("config", "user.name", "CI test")
        source = self.root / "codex-rs/core/old.rs"
        source.write_text("// fixture\n")
        git("add", ".")
        git("commit", "-qm", "before")
        base = git("rev-parse", "HEAD")
        source.rename(self.root / "codex-rs/app-server/new.rs")
        git("add", ".")
        git("commit", "-qm", "after")
        self.assertEqual(
            set(changed_paths(base, "HEAD", root=self.root)),
            {"codex-rs/core/old.rs", "codex-rs/app-server/new.rs"},
        )

    def test_missing_base_fails_closed(self):
        with self.assertRaises(ValueError):
            changed_paths("", "HEAD", root=self.root)


if __name__ == "__main__":
    unittest.main()
