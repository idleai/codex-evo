"""Measure a PR attempt from scheduling through its terminal merge check."""

from datetime import datetime, timezone
import json
import os
from pathlib import Path
from urllib.request import Request, urlopen


def elapsed_seconds(run, now):
    # A rerun gets its own budget; the original run creation can be days old.
    start = datetime.fromisoformat(run["run_started_at"].replace("Z", "+00:00"))
    elapsed = (now - start).total_seconds()
    if elapsed < 0:
        raise ValueError("workflow start is later than the current clock")
    return elapsed


def main():
    if os.environ["GITHUB_EVENT_NAME"] != "pull_request":
        return
    resource = (
        f"{os.environ['GITHUB_API_URL']}/repos/{os.environ['GITHUB_REPOSITORY']}"
        f"/actions/runs/{int(os.environ['GITHUB_RUN_ID'])}"
        f"/attempts/{int(os.environ['GITHUB_RUN_ATTEMPT'])}"
    )
    request = Request(
        resource,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": "Bearer " + os.environ["GH_TOKEN"],
            "User-Agent": "idle-pr-budget",
        },
    )
    with urlopen(request, timeout=20) as response:
        run = json.load(response)
    elapsed = elapsed_seconds(run, datetime.now(timezone.utc))
    message = (
        f"PR feedback: {elapsed:.0f}s (advisory target: 600s), "
        "including setup and queue time."
    )
    print(message)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(summary).open("a") as stream:
            stream.write(message + "\n")
    if elapsed > 600:
        print(
            "::notice::The ten-minute PR feedback target was exceeded; "
            "inspect queue and job timings."
        )


if __name__ == "__main__":
    main()
