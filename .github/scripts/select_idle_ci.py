#!/usr/bin/env python3
"""Select Idle's Linux checks from the candidate diff without resolving Cargo."""

import argparse
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[2]
EXCLUDED = (
    "codex-rs/code-mode/",
    "codex-rs/code-mode-host/",
    "codex-rs/code-mode-protocol/",
    "codex-rs/code-mode-runtime/",
    "codex-rs/v8-poc/",
    "third_party/v8/",
)
CRITICAL = (
    "codex-rs/ext/idle-runtime",
    "codex-rs/core",
    "codex-rs/app-server",
    "codex-rs/app-server-daemon",
    "codex-rs/execpolicy",
    "codex-rs/linux-sandbox",
)
BUILD_FILES = {
    "MODULE.bazel",
    "MODULE.bazel.lock",
    "defs.bzl",
    ".bazelrc",
    ".bazelversion",
    "BUILD.bazel",
    "codex-rs/Cargo.toml",
    "codex-rs/Cargo.lock",
    "codex-rs/rust-toolchain.toml",
}


def select(paths, *, full=False, root=ROOT):
    paths = [path for path in paths if not path.startswith(EXCLUDED)]
    rust_paths = [path for path in paths if path.startswith("codex-rs/")]
    build = any(
        path in BUILD_FILES
        or path.startswith(("bazel/", "workspace_root_test_launcher."))
        for path in paths
    )
    ci = any(path.startswith(".github/") for path in paths)
    rust = full or build or ci or bool(rust_paths)
    packages = set()
    if rust:
        packages.update(
            f"//{path}:all"
            for path in CRITICAL
            if (root / path / "BUILD.bazel").is_file()
        )
    for path in rust_paths:
        found = False
        for directory in (root / path).parents:
            if directory in (root, root / "codex-rs"):
                break
            if (directory / "BUILD.bazel").is_file():
                packages.add(f"//{directory.relative_to(root).as_posix()}:all")
                found = True
                break
        if not found:
            build = True
    # Changes to shared build inputs, deleted crates, and integration runs need
    # the full Linux Rust suite. Ordinary edits retain their package boundaries.
    if full or build:
        packages = {"//codex-rs/..."}
    query = "tests(set(" + " ".join(sorted(packages)) + "))"
    query += ' except attr(tags, "manual", //codex-rs/...)'
    query += ' except filter("code[-_]mode|v8-poc|windows-cross|windows_sandbox|wine-exec", //codex-rs/...)'
    return {
        "rust": rust,
        "sdk": full or any(path.startswith("sdk/") for path in paths),
        "full": full,
        "query": query,
    }


def changed_paths(base, head, *, root=ROOT):
    if not base or set(base) == {"0"}:
        raise ValueError("a valid base revision is required for scoped CI")
    output = subprocess.check_output(
        ["git", "diff", "--name-only", "--no-renames", "-z", base, head], cwd=root
    )
    return [os.fsdecode(path) for path in output.split(b"\0") if path]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default=os.environ.get("BASE_SHA", ""))
    parser.add_argument("--head", default=os.environ.get("HEAD_SHA", "HEAD"))
    parser.add_argument("--full", action="store_true")
    args = parser.parse_args()
    paths = [] if args.full else changed_paths(args.base, args.head)
    plan = select(paths, full=args.full)
    print(json.dumps(plan, indent=2))
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as stream:
            for key, value in plan.items():
                stream.write(
                    f"{key}={str(value).lower() if isinstance(value, bool) else value}\n"
                )


if __name__ == "__main__":
    main()
