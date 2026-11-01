#!/usr/bin/env python3
"""Keep PR smoke checks bounded and select the complete suite after merge."""

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
SMOKE = (
    "//codex-rs/app-server-protocol:app-server-protocol-unit-tests",
    "//codex-rs/cli:cli-execpolicy-test",
    "//codex-rs/cli:cli-exec_server_websocket_auth-test",
    "//codex-rs/execpolicy:execpolicy-basic-test",
    "//codex-rs/execpolicy:execpolicy-unit-tests",
    "//codex-rs/ext/idle-runtime:idle-runtime-grants-test",
    "//codex-rs/ext/idle-runtime:idle-runtime-unit-tests",
    "//codex-rs/ext/idle-runtime:idle-runtime-workspaces-test",
    "//codex-rs/protocol:protocol-unit-tests",
    "//codex-rs/utils/absolute-path:absolute-path-unit-tests",
    "//codex-rs/utils/cargo-bin:cargo-bin-unit-tests",
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


def select(paths, *, full=False):
    paths = [path for path in paths if not path.startswith(EXCLUDED)]
    rust_paths = [path for path in paths if path.startswith("codex-rs/")]
    build = any(
        path in BUILD_FILES
        or path.startswith(("bazel/", "workspace_root_test_launcher."))
        for path in paths
    )
    ci = any(path.startswith(".github/") for path in paths)
    rust = full or build or ci or bool(rust_paths)
    # The full suite also covers changed and deleted packages and shared build
    # inputs. It runs after merge; none of those paths expands the PR budget.
    targets = ("//codex-rs/...",) if full else SMOKE if rust else ()
    query = "tests(set(" + " ".join(targets) + "))"
    query += ' except attr(tags, "manual", //codex-rs/...)'
    query += ' except filter("code[-_]mode|v8-poc|windows-cross|windows_sandbox|wine-exec", //codex-rs/...)'
    return {
        "rust": rust,
        "sdk": full
        or any(
            path.startswith("sdk/")
            or path
            in {
                "package.json",
                "pnpm-lock.yaml",
                "pnpm-workspace.yaml",
                ".github/workflows/sdk.yml",
            }
            for path in paths
        ),
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
