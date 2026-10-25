import os
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import maintain_cache


class MaintainCacheTest(unittest.TestCase):
    def test_removes_old_job_caches_but_preserves_builds_and_shared_caches(self):
        with TemporaryDirectory() as directory:
            root = Path(directory) / "codex-ci"
            names = (
                "bazel-repo-contents-cache-123-test",
                "bazel-repo-contents-cache",
                "bazel-repository-cache",
                "o",
                "cargo-target",
                "bazel-repo-contents-cache-not-a-job",
            )
            for name in names:
                (root / name).mkdir(parents=True)
                (root / name / "keep").write_text("cached data")
            result = maintain_cache.maintain(root, 1024, 0)
            self.assertEqual(result["removed_job_caches"], [names[0]])
            self.assertFalse((root / names[0]).exists())
            self.assertTrue(all((root / name / "keep").is_file() for name in names[1:]))

    def test_prunes_old_blobs_to_the_limit_and_keeps_new_blobs(self):
        with TemporaryDirectory() as directory:
            root = Path(directory) / "codex-ci"
            cache = root / "bazel-disk-cache/cas/aa"
            cache.mkdir(parents=True)
            for name, age in (("old", 1), ("new", 2)):
                (cache / name).write_bytes(b"x" * 8)
                os.utime(cache / name, (age, age))
            result = maintain_cache.maintain(root, 8, 0)
            self.assertEqual(result["disk_cache_after_bytes"], 8)
            self.assertFalse((cache / "old").exists())
            self.assertEqual((cache / "new").read_bytes(), b"x" * 8)

    def test_does_not_follow_symlinks_outside_the_cache(self):
        with TemporaryDirectory() as directory:
            root = Path(directory) / "codex-ci"
            outside = Path(directory) / "outside"
            root.mkdir()
            outside.mkdir()
            (outside / "keep").write_text("not a cache file")
            (root / "bazel-repo-contents-cache-123-test").symlink_to(outside)
            cache = root / "bazel-disk-cache/cas"
            cache.mkdir(parents=True)
            (cache / "linked-directory").symlink_to(outside)
            (cache / "linked-file").symlink_to(outside / "keep")
            maintain_cache.maintain(root, 0, 0)
            self.assertEqual((outside / "keep").read_text(), "not a cache file")
            (root / "bazel-disk-cache/ac").symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "symlink"):
                maintain_cache.maintain(root, 0, 0)

    def test_reports_insufficient_free_space_without_deleting_build_outputs(self):
        with TemporaryDirectory() as directory:
            root = Path(directory) / "codex-ci"
            (root / "o").mkdir(parents=True)
            (root / "o/binary").write_bytes(b"compiled output")
            with patch("maintain_cache.shutil.disk_usage") as usage:
                usage.return_value.free = 0
                result = maintain_cache.maintain(root, 0, 1)
            self.assertFalse(result["ready"])
            self.assertEqual((root / "o/binary").read_bytes(), b"compiled output")


if __name__ == "__main__":
    unittest.main()
