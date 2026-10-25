# Idle CI

This directory owns the fork's CI policy. Preserve it during upstream imports.
Do not restore upstream workflow matrices without an explicit policy change.

Pull requests run hosted preflight checks before allocating one Linux GNU Bazel
job. That job tests changed Rust packages and the core Idle runtime, app server,
daemon, authorization and Linux sandbox packages, and compiles the shipped CLI
and app server. SDK checks run when SDK files change. Documentation-only changes
do not allocate a Rust runner. `CI required` fails for failed, cancelled or
unexpectedly skipped jobs.

Pushes to `idle-base` and `main`, a daily scheduled run, and manual full runs
validate the broader Linux Rust suite. New revisions replace older runs for the
same branch or PR. Integration runs use the background runner allocation.

All Codex build workflows use Linux. Dedicated V8/Code Mode tests, V8 source
builds, and macOS/Windows jobs are excluded, including release and manual paths.
Required Linux binaries still use the existing pinned prebuilt V8 archive.
Application code and platform implementations are outside this policy change.

`CODEX_LINUX_RUNNER` selects the four Codex PR workers;
`IDLE_BACKGROUND_RUNNER` selects the two integration/release workers. An unset
variable uses hosted Linux. Busy local workers do not cause automatic hosted
overflow. The workflow routes external fork PRs to hosted runners.

Release tags build Linux GNU packages with Bazel and publish them on GitHub
after checksum verification. Existing published assets remain available. Publishing npm/Python or
other platform packages requires a separate, explicitly configured release path.
