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

The isolated Copilot build stamps its version into both the Cargo workspace and
lockfile so `--version` and `/status` identify the installed build. Its version is
distinct from the normal Idle build. Building this checkout does not switch the
installed CLI or Remote daemon.

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
saved model list. In the CLI, use `/model` to select either a Copilot model or
DeepSeek from the same list. Selecting a different profile continues the visible
conversation in a linked thread with the chosen model and reasoning level. The
previous thread remains resumable, and the base provider's saved defaults are
preserved. Finish the current turn before changing providers. `/handoff dsv4` and
`/handoff --base` remain available for explicit profile handoffs.

On Windows, place these files in the active user's `%USERPROFILE%\.codex` (or
`CODEX_HOME`). When DeepSeek runs on a QEMU host reachable at `10.0.2.2:8000`, an
administrator can make it available through the required local endpoint:

```powershell
netsh interface portproxy add v4tov4 listenaddress=127.0.0.1 listenport=8000 connectaddress=10.0.2.2 connectport=8000
```

Check that port 8000 is free before adding the rule. Restart the CLI after adding
the configuration; the Copilot login and its model catalog remain in place.

The local exception requires a loopback HTTP or HTTPS inference endpoint,
`requires_openai_auth = false`, and no provider API key, bearer token, command
auth, or gateway OAuth. A separate catalog URL, when configured, must also be
loopback. It continues working across account changes without inheriting
Copilot or OpenAI credentials.

## Isolation boundaries

- OpenAI and ChatGPT sign-in are removed from the CLI and terminal onboarding.
  `codex login` starts GitHub Copilot device authorization. The app-server rejects
  OpenAI API-key, ChatGPT browser/device, and externally supplied ChatGPT-token
  login requests without replacing an existing Copilot login. Saved OpenAI
  credentials and OpenAI/Codex credential environment variables are ignored.
- Direct OpenAI services are disabled for the entire distribution, including
  sessions without a Copilot account. The HTTP and WebSocket transports deny
  OpenAI, ChatGPT, staging, and asset-service domains before connection, and HTTP
  redirects cannot bypass that policy. Configuration cannot enable these routes.
  An unconfigured provider cannot fall back to the OpenAI API.
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
- OpenAI backend requests, plugin/catalog refreshes, announcement downloads,
  Guardian classifier warmup, image generation, web search, realtime, and
  backend memory requests are unavailable. The hidden OpenAI Responses proxy is
  no longer included in the CLI.
- Upstream update checks and installers, desktop-app installation, and built-in
  pet asset downloads are disabled. Cached and locally supplied pet assets remain
  usable. Upgrade this distribution with a verified local package; daemon
  `update --from-cli` remains available.
- Local logs and explicitly configured OTLP exporters remain available. Debug
  builds retain local analytics capture, without account credentials; HTTP
  capture accepts literal loopback addresses and rejects redirects. Release
  builds disable that capture path. Explicit telemetry exporters also reject
  direct OpenAI destinations and redirects to them.

These are application service boundaries. Shell commands and separately running
MCP servers retain their own network permissions. OpenAI model names in the
Copilot catalog identify models served through GitHub; they do not enable a
direct OpenAI connection.

## Validation

Run the transport suites and the distribution's auth, routing, and handoff
regressions locally, then build the CLI and its runtime helpers:

```sh
just test --locked --cargo-profile dev-small \
  -p codex-http-client -p codex-websocket-client -p codex-login \
  -p codex-model-provider-info -p codex-model-provider -p codex-backend-client \
  -p codex-otel -p codex-tui -p codex-cli --lib \
  -E 'package(codex-http-client) | package(codex-websocket-client) | test(copilot_distribution) | test(github_copilot)'
just test --locked --cargo-profile dev-small \
  -p codex-app-server --test all \
  -E 'test(copilot_distribution) | test(model_picker_profiles) | test(thread_fork_portable_profile_handoff)'
just test --locked --cargo-profile dev-small \
  -p codex-cli --bin codex -E 'test(copilot_distribution)'
(cd codex-rs && cargo build --locked --profile dev-small \
  --bin codex --bin codex-app-server --bin codex-exec-server \
  --bin codex-code-mode-host)
```

Upstream tests that require successful OpenAI authentication or backend requests
are intentionally incompatible with this distribution's disabled services.

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
