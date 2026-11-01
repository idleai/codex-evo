#!/usr/bin/env python3
"""Guard the fork's Linux CI boundary during upstream imports."""

from pathlib import Path
import re


WORKFLOWS = Path(__file__).resolve().parents[1] / "workflows"
RETIRED = {
    "v8-canary.yml",
    "rusty-v8-release.yml",
    "rust-ci.yml",
    "rust-ci-full.yml",
    "rust-ci-full-nextest-platform.yml",
    "postmerge-ci.yml",
    "rust-release-windows.yml",
    "rust-release-provisioned-macos.yml",
    "rust-release-argument-comment-lint.yml",
}


def main():
    failures = []
    for path in WORKFLOWS.glob("*.y*ml"):
        if path.name in RETIRED:
            failures.append(f"{path.name}: retired upstream workflow restored")
        text = path.read_text()
        for line in text.splitlines():
            if re.search(r"(?:runs-on|runner|os):.*(?:macos|windows)", line, re.I):
                failures.append(f"{path.name}: non-Linux runner: {line.strip()}")
        if re.search(r"--config[= ]ci-v8|v8_from_source[= ]true", text):
            failures.append(f"{path.name}: V8 source build restored")
        for reference in re.findall(r"uses: \./\.github/workflows/(\S+)", text):
            if not (WORKFLOWS / reference).is_file():
                failures.append(f"{path.name}: missing reusable workflow {reference}")
    if failures:
        raise SystemExit("\n".join(failures))
    print("Idle Linux workflow policy passed.")


if __name__ == "__main__":
    main()
