# Idle CI policy

Preserve the Linux CI policy in `CI.md` when importing upstream changes.
Do not restore macOS/Windows job matrices, V8 source builds, or dedicated
V8/Code Mode checks. Application code and platform implementations are separate
from this CI policy.

Keep hosted preflight ahead of expensive jobs and preserve the `CI required`
result. A skipped job is acceptable only when the selector explicitly excludes
it. Keep the shipped CLI/app-server compile check and the core Idle runtime
checks when changing target selection.

Validate workflow edits with actionlint and `verify_idle_ci.py`. Run the
selection/result helper tests when changing selection or the required result.
