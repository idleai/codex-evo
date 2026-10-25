#!/usr/bin/env python3
"""Assign the existing eight workers to Codex, client PRs and background work."""

import argparse
import json
from pathlib import Path
import subprocess


ROLE_LABELS = {"codex-pr-linux-x64", "idle-pr-linux-x64", "idle-background-linux-x64"}
REPOSITORIES = (
    "codex-evo",
    "editchain",
    "host-tools",
    "app-core",
    "web-ui",
    "vscode-extension",
    "offstage",
)


def routing_plan(runners):
    by_name = {runner["name"]: runner for runner in runners}
    names = [f"idle-linux-local-{index:02}" for index in range(1, 9)]
    missing = set(names) - by_name.keys()
    if missing:
        raise ValueError(
            f"expected eight registered workers; missing {sorted(missing)}"
        )
    plan = []
    for index, name in enumerate(names, 1):
        runner = by_name[name]
        role = (
            "codex-pr-linux-x64"
            if index <= 4
            else "idle-pr-linux-x64"
            if index <= 6
            else "idle-background-linux-x64"
        )
        labels = {
            label["name"] for label in runner["labels"] if label["type"] == "custom"
        }
        # Keep legacy labels for existing workflows and repositories outside
        # this rollout. Repository variables select the new allocations.
        labels = (labels - ROLE_LABELS) | {role}
        plan.append(
            {"id": runner["id"], "name": name, "role": role, "labels": sorted(labels)}
        )
    return plan


def api(path, *, method="GET", body=None):
    command = ["gh", "api", "--method", method, path]
    if body is not None:
        command.extend(["--input", "-"])
    result = subprocess.run(
        command,
        input=None if body is None else json.dumps(body),
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout) if result.stdout else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--organization", default="idleai")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--record-directory", type=Path, required=True)
    args = parser.parse_args()
    runners = api(f"orgs/{args.organization}/actions/runners?per_page=100")["runners"]
    plan = routing_plan(runners)
    print(json.dumps(plan, indent=2))
    if not args.apply:
        return
    args.record_directory.mkdir(parents=True, exist_ok=True)
    before = args.record_directory / "runners-before.json"
    if not before.exists():
        before.write_text(json.dumps(runners, indent=2) + "\n")
    variables_before = args.record_directory / "variables-before.json"
    if not variables_before.exists():
        variables = {}
        for repository in REPOSITORIES:
            selected = {
                "CODEX_LINUX_RUNNER",
                "IDLE_LINUX_RUNNER",
                "IDLE_BACKGROUND_RUNNER",
            }
            rows = api(f"repos/{args.organization}/{repository}/actions/variables")[
                "variables"
            ]
            variables[repository] = {
                row["name"]: row["value"] for row in rows if row["name"] in selected
            }
        variables_before.write_text(json.dumps(variables, indent=2) + "\n")
    for runner in plan:
        api(
            f"orgs/{args.organization}/actions/runners/{runner['id']}/labels",
            method="PUT",
            body={"labels": runner["labels"]},
        )
    for repository in REPOSITORIES:
        name = (
            "CODEX_LINUX_RUNNER" if repository == "codex-evo" else "IDLE_LINUX_RUNNER"
        )
        label = (
            "codex-pr-linux-x64" if repository == "codex-evo" else "idle-pr-linux-x64"
        )
        for variable, value in (
            (name, label),
            ("IDLE_BACKGROUND_RUNNER", "idle-background-linux-x64"),
        ):
            subprocess.run(
                [
                    "gh",
                    "variable",
                    "set",
                    variable,
                    "--repo",
                    f"{args.organization}/{repository}",
                    "--body",
                    value,
                ],
                check=True,
            )
    (args.record_directory / "routing.json").write_text(
        json.dumps(plan, indent=2) + "\n"
    )
    print(
        "Configured four Codex PR workers, two client PR workers and two background workers."
    )


if __name__ == "__main__":
    main()
