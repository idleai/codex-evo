# Isolated GitHub Copilot build

`idle/github-copilot-auth` layers direct GitHub Copilot authentication and
service isolation onto the Idle fork. The November 2026 update follows the
0.166 source baseline synchronized into `origin/main` at `67f0994b53`, together
with the current `idle-base` integration.

The idle profile picker, named/default subagent profiles, portable handoffs,
provider catalogs, and Responses compatibility settings remain available. While
GitHub Copilot is signed in, its authentication takes precedence over configured
providers and environment API keys: profiles cannot redirect that credential to
another provider.

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

## Isolation boundaries

- Inference uses the authenticated HTTPS `githubcopilot.com` endpoint and only
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

The Linux release workflow runs repository checks, the selected Rust and SDK
suites, and package assembly before publishing checksummed CLI and app-server
archives. Both packages include the execution server required by the current
runtime and the pinned prebuilt V8 host.

Focused regression coverage includes stored Copilot authentication taking
precedence over environment keys and provider profiles, account-specific model
catalogs, credential changes during a session, and redirect rejection. It also
covers switching from configured, Bedrock, and Google Cloud providers into a
Copilot account. Local analytics capture must omit credentials, and built-in
OpenAI telemetry and feedback delivery remain disabled.

Release verification uses temporary Codex homes, fake credentials, local mock
servers, and the downloaded release archives. Live GitHub authorization and
billable inference require an actual account and are not exercised by these
automated checks. User-configured tools retain their own network permissions.
