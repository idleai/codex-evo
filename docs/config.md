# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

The interactive TUI requests detailed reasoning summaries by default, including
when a model's default is `none` and when submitting a turn to a resumed thread.
Explicit summary settings are preserved. To opt out, set this top-level value in
your TUI configuration:

```toml
model_reasoning_summary = "none"
```

This preference requests summaries from providers and models that support them;
it does not require every reasoning item to contain a summary. Copilot does not
require `features.concurrent_reasoning_summaries` for incremental summaries.

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
