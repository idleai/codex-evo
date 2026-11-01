# Workflow strategy

The fork's current workflow policy is documented in [Idle CI](../CI.md).
Pull requests run hosted preflight checks, compile the shipped Linux binaries,
and run selected Rust and SDK checks. Default-branch, scheduled and full manual
runs cover the broader Linux suite.

Five-minute builds and ten-minute PR feedback are advisory performance targets.
Duration reporting does not fail `CI required`, and workflows do not set explicit
job timeouts. Failed, cancelled or unexpectedly skipped checks still block CI.
