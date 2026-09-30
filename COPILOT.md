# Isolated GitHub Copilot build

`idle/github-copilot-auth` starts from `origin/idle` at `74c21eb5f`, the
0.161.0-alpha.1 source baseline, and ports the direct GitHub Copilot authentication
and isolation changes from `origin/solmax/github-copilot-auth` at `4d3d1c2cd`.

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

The source retains idle's upstream `0.0.0` Cargo workspace version. A release
package must apply the same 0.161.0-alpha.1 version stamp as idle; the package
assembler's `--package-version` option alone does not stamp the executable.
Building this checkout does not switch the installed release CLI or Remote daemon.

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

## Validation notes

The port is validated on macOS with Rust 1.95.0. Across the executed suites,
1,535 distinct scoped tests pass, with the additional limits listed below:

- CLI compilation and debug CLI/app-server builds pass. Release-mode checks for
  analytics, feedback, and OTEL also pass, covering the non-debug isolation paths.
- All 110 model-provider unit tests pass, covering 307/308 redirect rejection,
  Copilot account transitions, gateway OAuth, workspace routing, exact provider
  metadata, and catalog refresh after credential changes.
  Provider-info, analytics, OTEL, feedback, and app-server protocol tests also pass
  (two tests are skipped by the test runner).
- All 820 backend-client, models-manager, protocol, and config unit tests pass;
  all 42 login integration tests pass.
- Eight focused runtime tests cover request headers, plugin refresh suppression,
  stale image/web tool removal, Guardian startup suppression, and the Copilot
  status snapshot. All pass, as does the core metrics-default regression.
- Fourteen core/app-server integration tests pass, covering auth precedence,
  unsupported models, provider capabilities, subagent profiles, profile pickers,
  and portable handoffs. The separate native-discovery test has the intermittent
  failure noted below and is excluded from the final focused rerun.
- A CLI/app-server smoke test uses fake credentials, a temporary `CODEX_HOME`,
  and a rejecting outbound proxy. Stored Copilot auth wins over both OpenAI key
  environment variables; account reads and the advertised model catalog work;
  the proxy observes no requests during these operations.
- Stable and experimental Rust/JSON/TypeScript protocol exports are regenerated
  and checked. The Python account/login bindings are ported and round-trip
  validated with Pydantic 2.12.5.
- Scoped `just fix` completes for the 20 affected crates other than login.
  Unrelated automatic unused-import cleanups are reverted to keep the port focused.

Known validation limits:

- The login unit-test target and its `just fix` are blocked by an existing idle
  fixture in
  `codex-rs/login/src/auth_env_telemetry.rs`: its `ModelProviderInfo` initializer
  lacks `supports_namespace_tools` and `supports_codex_agent_messages`. The
  device-flow unit tests therefore have not run on this checkout; the unrelated
  fixture is left unchanged.
- The pinned Python generator/formatter dependencies cannot be downloaded from
  `files.pythonhosted.org` in this environment. `just fmt` completes Rust, Just,
  and Bazel/Starlark formatting but cannot complete its Python stages. Python
  code generation is not independently reproduced; the directly ported bindings
  have syntax and runtime serialization coverage instead.
- The unchanged `picker_profile_appends_to_live_native_discovery` integration test
  passes intermittently but fails in a repeat run: its one-shot `/models` mock
  returns 404 when startup refresh and the explicit model-list request race.
  The fixture and background refresh code are unchanged from idle; this is left
  visible rather than weakening the test or changing unrelated refresh behavior.
- The complete workspace suite, other platforms, live GitHub authorization,
  and billable inference are not exercised. The smoke test is not a proof that
  arbitrary user-configured tools cannot make network requests.
