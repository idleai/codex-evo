"""Open or update one remediation issue for a failed asynchronous candidate."""

import argparse
import json
import os
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen


FAILURES = {"failure", "timed_out", "startup_failure", "action_required"}


def api(resource, *, method="GET", body=None):
    request = Request(
        "https://api.github.com/" + resource,
        data=json.dumps(body).encode() if body is not None else None,
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
            "Authorization": "Bearer " + os.environ["GH_TOKEN"],
            "User-Agent": "idle-async-ci",
        },
    )
    with urlopen(request, timeout=30) as response:
        return json.load(response)


def pages(resource, field=None):
    page = 1
    while True:
        result = api(
            f"{resource}{'&' if '?' in resource else '?'}per_page=100&page={page}"
        )
        rows = result[field] if field else result
        yield from rows
        if len(rows) < 100:
            break
        page += 1


def trusted_candidate(repository, run):
    if (run.get("head_repository") or {}).get("full_name") != repository or run.get(
        "event"
    ) not in {"push", "schedule", "workflow_dispatch", "workflow_run"}:
        return False
    branch = run.get("head_branch")
    if branch == api(f"repos/{repository}")["default_branch"]:
        return True
    if run["event"] == "push" and branch:
        try:
            api(f"repos/{repository}/git/ref/tags/{quote(branch, safe='')}")
            return True
        except HTTPError as error:
            if error.code != 404:
                raise
    return False


def report(repository, run_id, *, dry_run=False):
    # Read current state so a delayed failure event cannot report a retry that
    # has already succeeded. This job never checks out candidate source.
    run = api(f"repos/{repository}/actions/runs/{int(run_id)}")
    if (
        run["status"] != "completed"
        or run["conclusion"] not in FAILURES
        or not trusted_candidate(repository, run)
    ):
        print("No failed asynchronous release candidate to report.")
        return None
    marker = f"<!-- idle-async-ci:{run['workflow_id']}:{run['head_sha']} -->"
    jobs = [
        job
        for job in pages(
            f"repos/{repository}/actions/runs/{run_id}/jobs?filter=latest", "jobs"
        )
        if job["conclusion"] in FAILURES
    ]
    details = []
    for job in jobs:
        details.append(f"- [{job['name']}]({job['html_url']}): {job['conclusion']}")
        for step in job.get("steps", []):
            if step.get("conclusion") in FAILURES:
                details.append(f"  - Step: {step['name']} ({step['conclusion']})")
    body = "\n".join(
        [
            marker,
            "",
            "Asynchronous CI or release validation failed.",
            "",
            f"Commit: [{run['head_sha']}](https://github.com/{repository}/commit/{run['head_sha']})",
            f"Workflow: [{run['name']}]({run['html_url']})",
            f"Run attempt: {run['run_attempt']}",
            "",
            *(
                details
                or [
                    "The workflow failed before a job reported a test result; inspect the run link."
                ]
            ),
            "",
            "Remediation:",
            "1. Inspect the failed jobs and their retained logs or artifacts.",
            "2. Fix or revert the failing change and rerun complete validation.",
            "3. Publish only after the candidate passes; close this issue after checking the successful run.",
        ]
    )
    title = f"[CI failure] {run['name']} at {run['head_sha'][:12]}"
    existing = next(
        (
            issue
            for issue in pages(
                f"repos/{repository}/issues?state=all&sort=created&direction=desc"
            )
            if "pull_request" not in issue and marker in (issue.get("body") or "")
        ),
        None,
    )
    if dry_run:
        print(
            json.dumps(
                {
                    "action": "update" if existing else "create",
                    "title": title,
                    "body": body,
                },
                indent=2,
            )
        )
        return None
    payload = {"title": title, "body": body}
    if existing:
        if (
            all(existing.get(key) == value for key, value in payload.items())
            and existing["state"] == "open"
        ):
            return existing
        issue = api(
            f"repos/{repository}/issues/{existing['number']}",
            method="PATCH",
            body=dict(payload, state="open"),
        )
    else:
        issue = api(f"repos/{repository}/issues", method="POST", body=payload)
    print(f"Remediation issue: {issue['html_url']}")
    return issue


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repository", default=os.environ.get("GITHUB_REPOSITORY"), required=False
    )
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--dry-run", action="store_true")
    arguments = parser.parse_args()
    if not arguments.repository:
        parser.error("a repository is required")
    report(arguments.repository, arguments.run_id, dry_run=arguments.dry_run)


if __name__ == "__main__":
    main()
