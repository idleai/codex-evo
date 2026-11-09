# Isolated GitHub Copilot build

`idle/github-copilot-auth` layers direct GitHub Copilot authentication and
service isolation onto the Idle fork. The November 8 update includes `idle-base`
through `1d0b0889ad`, including attributed shared sessions and runner interruption.
This follows the 0.166 source baseline synchronized into `origin/main` at
`67f0994b53`.

The idle profile picker, named/default subagent profiles, portable handoffs,
provider catalogs, and Responses compatibility settings remain available. While
GitHub Copilot is signed in, its authentication takes precedence over configured
remote providers and environment API keys. An explicitly configured local,
unauthenticated provider keeps its own endpoint and model catalog. GitHub
credentials are never attached to its requests.

## Sign in without replacing another installation's credentials

Build the CLI from this checkout, then use a separate Codex home:

```sh
cd codex-rs
cargo build --bin codex
export CODEX_HOME="$HOME/.codex-copilot"
./target/debug/codex login github-copilot
./target/debug/codex login status
./target/debug/codex
```

The sign-in flow uses GitHub device authorization, checks Copilot entitlement,
and discovers the account's enabled OpenAI Responses-compatible models. Use
`--client-id` or `GITHUB_COPILOT_CLIENT_ID` to override the public OAuth application
identifier. App-server clients can use `account/login/start` with
`{"type":"githubCopilot"}`.

The development branch retains the `0.0.0` Cargo workspace version. Fork
releases use `codex-evo-v<version>` tags on separate commits that stamp both the
Cargo workspace and lockfile. The isolated Copilot build has its own release
version, distinct from the normal Idle build. Building this checkout does not
switch the installed CLI or Remote daemon.

## Local DeepSeek alongside the Copilot catalog

Keep the default profile on the account's Copilot catalog. Add DeepSeek through
`model_picker_profiles`, using the same complete `dsv4.config.toml` profile,
`dsv4-models.json` catalog, and prompt file as the base installation. Keep
`model_catalog_json` in the local profile so its metadata applies only to
DeepSeek.

In the isolated Copilot home's `config.toml`, add:

```toml
[model_picker_profiles]
deepseek-v4-flash = "dsv4"

[model_providers.sglang_dsv4]
name = "Local DeepSeek V4.1 Flash"
base_url = "http://127.0.0.1:8000/v1"
wire_api = "responses"
requires_openai_auth = false
supports_namespace_tools = false
supports_codex_agent_messages = false
```

The corresponding `dsv4.config.toml` is:

```toml
model = "deepseek-v4-flash"
model_provider = "sglang_dsv4"
model_catalog_json = "dsv4-models.json"
model_instructions_file = "prompts/dsv4-instructions.md"
model_context_window = 524288
model_reasoning_effort = "xhigh"
model_reasoning_summary = "auto"
service_tier = "default"
```

Use the exact model ID returned by `http://127.0.0.1:8000/v1/models` in the picker
mapping, profile, and catalog `slug`. The existing local deployment advertises
`deepseek-v4-flash` for DeepSeek V4.1 Flash; use that ID until the server advertises
`deepseek-v41-flash`. The display name can remain `DeepSeek V4.1 Flash (local)`.

After Copilot sign-in, the picker contains every enabled OpenAI model advertised
by the account with Responses support, plus the local entry. New Copilot model
IDs do not require a code allowlist. Signing in again refreshes the account's
saved model list. Select DeepSeek for a new chat, or use `/handoff dsv4` to carry
visible conversation text into a new local thread. `/handoff --base` returns to
the Copilot profile. Existing chats retain their provider.

The local exception requires a loopback HTTP or HTTPS inference endpoint,
`requires_openai_auth = false`, and no provider API key, bearer token, command
auth, or gateway OAuth. A separate catalog URL, when configured, must also be
loopback. It continues working across account changes without inheriting
Copilot or OpenAI credentials.

## Isolation boundaries

- Copilot inference uses the authenticated HTTPS `githubcopilot.com` endpoint and only
  models advertised by that account. Unsupported models and account changes
  fail instead of falling back to OpenAI or another configured provider.
  Responses HTTP requests reject redirects rather than forwarding prompts to
  a different endpoint.
- GitHub credentials are not exposed as general-purpose Codex backend tokens.
  OpenAI internal request metadata and Codex-specific agent message items are
  not sent to the Copilot endpoint.
- Built-in OpenAI analytics, Statsig export, Sentry feedback delivery, and
  background turn-cost enrichment are disabled in this distribution.
- Copilot sessions do not start OpenAI plugin/catalog refreshes, announcement
  downloads, Guardian classifier warmup, image generation, web search, realtime,
  or memory-summary requests.
- Local logs and explicitly configured OTLP exporters remain available. Debug
  builds retain local analytics capture, without account credentials; HTTP
  capture accepts literal loopback addresses and rejects redirects. Release
  builds disable that capture path.

This is provider and built-in-service isolation, not a general network firewall.
User-configured MCP servers, shell commands, tools, and explicit telemetry
destinations retain their configured network permissions.

## Validation

Run the affected local suites and build all three entrypoints from the repository
root:

```sh
(cd codex-rs && cargo build --locked --profile dev-small \
  -p codex-rmcp-client --bin test_stdio_server)
just test --locked --cargo-profile dev-small \
  -p codex-login -p codex-model-provider -p codex-app-server \
  -p codex-app-server-protocol -p codex-extension-api --lib
just test --locked --cargo-profile dev-small \
  -p codex-core -p codex-app-server --test all \
  -E 'test(github_copilot) | test(model_picker_profiles) | test(portable) | test(subagent_profiles) | test(subagent_service_tier) | test(idle_runtime) | test(idle_runner)'
(cd codex-rs && cargo build --locked --profile dev-small \
  -p codex-cli -p codex-app-server -p codex-exec-server-cli --bins)
```

Sandboxed Idle runner cases require Bubblewrap on `PATH` and host support for
user and network namespaces. Report host policy denials separately from passing
tests. The protocol suite checks that generated exports include both the Idle
and Copilot APIs.

The Linux release workflow runs repository checks, the selected Rust and SDK
suites, and package assembly before publishing checksummed CLI and app-server
archives. Both packages include the execution server required by the current
runtime and the pinned prebuilt V8 host.

Focused regression coverage includes stored Copilot authentication taking
precedence over environment keys and remote provider profiles, account-specific model
catalogs, credential changes during a session, and redirect rejection. It also
covers switching from configured, Bedrock, and Google Cloud providers into a
Copilot account, combined Copilot/local model discovery, local requests without
account headers, and restoring the local provider after an app-server restart.
Local analytics capture must omit credentials, and built-in
OpenAI telemetry and feedback delivery remain disabled.

Release verification uses temporary Codex homes, fake credentials, local mock
servers, and the downloaded release archives. Live GitHub authorization and
billable inference require an actual account and are not exercised by these
automated checks. User-configured tools retain their own network permissions.
