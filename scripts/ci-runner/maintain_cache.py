#!/usr/bin/env python3
"""Reclaim completed-job caches between jobs on a dedicated Linux runner."""

import argparse
import json
import os
import re
import shutil
import stat
from pathlib import Path


GIB = 1024**3
LEGACY_CACHE = re.compile(r"bazel-repo-contents-cache-[0-9]+-[A-Za-z0-9_-]+")


def remove_readonly(function, path, error):
    if not isinstance(error[1], PermissionError):
        raise error[1]
    os.chmod(path, stat.S_IRWXU, follow_symlinks=False)
    function(path)


def maintain(root: Path, cache_limit: int, min_free: int) -> dict:
    # Limit deletion to generated caches. Worktrees, toolchains, Bazel's output
    # base, repository downloads and the shared extracted-repository cache stay.
    if root.is_symlink() or root.name != "codex-ci":
        raise ValueError("expected a real codex-ci cache directory")
    root.mkdir(parents=True, exist_ok=True)
    root = root.resolve(strict=True)
    removed = []
    for path in root.iterdir():
        if LEGACY_CACHE.fullmatch(path.name) and not path.is_symlink():
            if path.is_dir():
                shutil.rmtree(path, onerror=remove_readonly)
                removed.append(path.name)

    disk_cache = root / "bazel-disk-cache"
    if disk_cache.is_symlink():
        raise ValueError("refusing a symlink for the Bazel disk cache")
    entries = []
    for name in ("ac", "cas", "tmp"):
        directory = disk_cache / name
        if directory.is_symlink():
            raise ValueError(f"refusing a symlink for cache section {name}")
        for parent, _, files in os.walk(directory, followlinks=False):
            for name in files:
                path = Path(parent) / name
                try:
                    details = path.lstat()
                except FileNotFoundError:
                    continue
                if stat.S_ISREG(details.st_mode):
                    entries.append((details.st_mtime_ns, details.st_size, path))

    total = sum(size for _, size, _ in entries)
    remaining = total
    free = shutil.disk_usage(root).free
    deleted = 0
    for _, size, path in sorted(entries):
        if remaining <= cache_limit and free >= min_free:
            break
        try:
            path.unlink()
        except FileNotFoundError:
            pass
        remaining -= size
        deleted += 1
        # Recheck actual free space: blobs can have hard links in the output
        # base, so unlinking a cache entry need not release its blocks.
        if deleted % 256 == 0 or remaining <= cache_limit:
            free = shutil.disk_usage(root).free

    free = shutil.disk_usage(root).free
    return {
        "removed_job_caches": removed,
        "disk_cache_before_bytes": total,
        "disk_cache_after_bytes": remaining,
        "deleted_cache_files": deleted,
        "free_bytes": free,
        "minimum_free_bytes": min_free,
        "ready": free >= min_free,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", type=Path, default=Path("/mnt/ci-cache/home-cache/codex-ci")
    )
    parser.add_argument("--disk-cache-gib", type=int, default=32)
    parser.add_argument("--minimum-free-gib", type=int, default=24)
    args = parser.parse_args()
    if args.disk_cache_gib < 0 or args.minimum_free_gib < 0:
        parser.error("cache and free-space limits must be nonnegative")
    result = maintain(args.root, args.disk_cache_gib * GIB, args.minimum_free_gib * GIB)
    print(json.dumps(result, sort_keys=True), flush=True)
    if not result["ready"]:
        raise SystemExit("CI cache volume needs more free space before another job")


if __name__ == "__main__":
    main()
