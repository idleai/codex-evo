# Native OpenAI + Direct SGLang DSV4 Handoff: Result

Initial implementation: 2026-08-23

Last updated: 2026-09-09

Status: complete, packaged, activated, and prepared on branch `dsv4-sglang-handoff`

## Outcome

The custom Codex build now supports both of the requested execution paths:

- The normal/base configuration continues to use Codex's native OpenAI provider and ChatGPT authentication path.
- A local `deepseek-v4-flash` deployment is reached directly through SGLang's Responses-compatible endpoint at `http://127.0.0.1:8000/v1`, without LiteLLM.
- The visible TUI workflow can move between providers with `/handoff dsv4` and `/handoff --base`.
- DSV4 can use Codex multi-agent/subagent orchestration.
- The patched app-server daemon remains compatible with Codex Remote, and manual pairing now succeeds.

The final selected CLI and daemon both report `0.149.0`, and the `dsv4` container is running on port 8000.

## Final architecture

```text
base config.toml
  native OpenAI provider + ChatGPT auth
            |
            | /handoff dsv4
            v
linked portable-history fork
  dsv4.config.toml
  deepseek-v4-flash -> SGLang :8000/v1
            |
            | /handoff --base
            v
linked portable-history fork
  native OpenAI provider + ChatGPT auth
```

`/handoff` does not mutate the provider of an existing thread in place. It creates a linked fork, projects safe conversation history into it, and replaces the currently visible TUI session with that fork. The previous provider thread remains resumable, while the interaction feels like continuing the same conversation.

## Sequence of work

### 1. Investigated Codex's existing non-OpenAI support

The codebase already supported configured model providers with custom base URLs and wire APIs. This was enough to preserve the built-in OpenAI path while adding a direct OpenAI-compatible SGLang provider.

The important gaps were:

- Provider selection and model catalogs were effectively scoped to thread startup.
- Some OpenAI-compatible servers do not accept Codex namespace tools.
- Some do not accept Codex's internal `agent_message` input item.
- Copying raw rollout history between providers would leak provider-specific reasoning, tools, IDs, and metadata into the new provider.
- The existing model picker was not a safe mechanism for changing the complete provider configuration of a running conversation.

The existing `thread/fork` path was selected as the cleanest server-side boundary for cross-provider continuation.

### 2. Added the direct SGLang provider and profile

The base configuration remains the normal OpenAI configuration. The following provider was added to `~/.codex/config.toml`:

```toml
[model_providers.sglang_dsv4]
name = "Local SGLang DSV4"
base_url = "http://127.0.0.1:8000/v1"
wire_api = "responses"
requires_openai_auth = false
supports_namespace_tools = false
supports_codex_agent_messages = false
```

The named profile `~/.codex/dsv4.config.toml` selects:

```toml
model = "deepseek-v4-flash"
model_provider = "sglang_dsv4"
model_catalog_json = "/mnt/hot/ambientlight/.codex/dsv4-models.json"
model_context_window = 1048576
model_reasoning_effort = "high"
model_reasoning_summary = "auto"
```

The local model catalog declares `multi_agent_version = "v2"`, enabling Codex's current subagent protocol for DSV4.

The existing `litellm.config.toml` was left intact but is not part of the DSV4 route.

### 3. Added provider capability controls

Two optional provider properties were added:

| Property | Purpose |
| --- | --- |
| `supports_namespace_tools` | Prevents namespace tool definitions from being sent to compatible servers that only support ordinary function tools. |
| `supports_codex_agent_messages` | Rewrites Codex-internal agent-message items into public user-message items when the target server cannot parse them. |

Omitting either property preserves the historical `true` behavior, so the native OpenAI path is unchanged.

These properties were carried through provider configuration, remote thread configuration protobufs, provider capabilities, and generated schemas.

### 4. Made model management provider-aware per thread

The thread manager previously reused the app-server's original models manager. It now creates a provider-specific models manager when the new thread's provider or model catalog differs from the app-server default.

This lets a handoff fork load the DSV4 catalog without replacing or weakening the base OpenAI configuration.

### 5. Added portable cross-provider history

`thread/fork` received two experimental fields:

| Field | Semantics |
| --- | --- |
| `profile` | Omitted: preserve app-server startup profile. String: load `$CODEX_HOME/<name>.config.toml`. Explicit `null`: load the base `$CODEX_HOME/config.toml`. |
| `portableHistory` | Project source history into bounded provider-neutral user and assistant text before starting the fork. |

Portable history deliberately excludes:

- reasoning payloads;
- tool calls and outputs;
- images and audio;
- provider-owned response IDs;
- passthrough metadata;
- inter-agent and other internal rollout items.

The projection is hard-bounded:

- at most 32 turns;
- at most 8 KiB of text per projected item;
- at most 64 KiB of projected history text overall.

The fork remains linked through normal thread lineage and emits the standard `thread/started` notification, so app-server and Remote subscribers discover it normally.

### 6. Added the TUI handoff workflow

New commands:

```text
/handoff dsv4
/handoff --base
```

Profile names are restricted to ASCII letters, digits, `_`, and `-`.

The TUI now:

1. requests a portable `thread/fork` using the chosen server-side profile;
2. shuts down its attachment to the old thread;
3. attaches to the linked fork;
4. updates its active model and provider configuration;
5. shows the selected profile, model, provider, and a resume hint for the previous thread.

The model picker also explains profile handoff when the target model is not present in the current provider's catalog.

### 7. Exercised DSV4 and subagents

The direct SGLang route produced successful model responses without LiteLLM.

A live DSV4 multi-agent test launched two subagents for repository exploration. Both completed useful exploration work. One malformed tool-name emission from the local model was recovered by sending a follow-up task, confirming that orchestration remained usable while also identifying an area where the local model/server can still be less strict than OpenAI models.

### 8. Packaged and activated a custom daemon build

An initial custom package was produced and selected through the standalone installation layout. A helper script was added at `restart-custom-codex-daemon.sh` to:

- stop the stock standalone updater when it is actually running;
- atomically point `standalone/current` at the custom release;
- verify that `~/.local/bin/codex` resolves into that release;
- restart the app-server daemon;
- enable Remote Control through the daemon command;
- avoid `codex remote-control start`, which would bootstrap the stock auto-updater;
- print the selected CLI and daemon versions.

Restarting the daemon from inside the Remote session terminated that same session, as expected. Subsequent restarts were therefore performed from an ordinary local/SSH shell.

### 9. Diagnosed failed Remote pairing

Web research and local source inspection found two relevant facts:

- CLI/headless Remote Control is experimental, and OpenAI's stable Remote guide and developer-command documentation currently describe conflicting setup boundaries. This documentation conflict is tracked in [openai/codex#35928](https://github.com/openai/codex/issues/35928).
- Stock Codex has an open pairing bug where the local control client stops waiting after two seconds even though the backend pairing request is allowed to take 30 seconds. This is tracked in [openai/codex#37698](https://github.com/openai/codex/issues/37698).

The enrollment request visible in the public client source sends the server name, OS, architecture, app-server version, installation ID, and authenticated account context. It does not send a binary hash or code signature. That did not prove an absence of all private backend policy, but it made blanket custom-binary rejection unlikely.

Local inspection confirmed that this checkout still used the same generic two-second control-socket timeout for `remoteControl/pairing/start`.

### 10. Fixed pairing and release identity

Pairing now has a dedicated 35-second response deadline. Fast local daemon probes and other control operations retain their two-second timeout.

A regression test delays the pairing response beyond the old two-second cutoff and verifies that the response is still received. The daemon crate test run completed with 33 of 33 tests passing.

The initial custom package also had a release-stamping problem:

- `codex --version` appeared as `0.149.0` because of package metadata;
- the app-server protocol identified the source build internally as `0.0.0` because `main` uses that Cargo workspace version.

The final binary was rebuilt using a temporary `0.149.0` Cargo workspace stamp, following the release build's versioning behavior. The source manifest and all lockfile entries were then restored, leaving no version-stamping diff in the repository.

The final binary now reports `0.149.0` both at the CLI and app-server protocol layers. Manual Remote pairing subsequently succeeded.

## Code change summary

| Area | Main result |
| --- | --- |
| `app-server-protocol` | Added experimental `thread/fork.profile` and `thread/fork.portableHistory` fields and regenerated experimental schema output. |
| `app-server` | Added request-scoped profile loading, bounded portable-history projection, portable fork construction, docs, and integration coverage. |
| `config` | Transported the two provider compatibility flags through remote thread config and protobuf representation. |
| `core` | Added agent-message rewriting, provider/catalog-specific model-manager creation, and profile-routed subagents. |
| `model-provider-info` | Added optional namespace-tool and Codex-agent-message support declarations. |
| `model-provider` | Exposed those declarations as runtime provider capabilities. |
| `tui` | Added `/handoff`, linked-fork attachment, active provider/model updates, messages, hints, and slash-command tests. |
| `app-server-daemon` | Added a dedicated 35-second pairing response timeout and delayed-response regression coverage. |
| Model prompts | Made `apply_patch` guidance reflect whether the tool is exposed as a function or free-form tool. |
| Local configuration | Added the direct `sglang_dsv4` provider, `dsv4` profile, and DSV4 model catalog. |
| Operations | Added `restart-custom-codex-daemon.sh` to select and safely restart the custom standalone daemon without re-enabling the stock updater. |

## Verification and final state

| Check | Result |
| --- | --- |
| Rust formatting | `just fmt` passed. |
| Pairing regression | `just test -p codex-app-server-daemon`: 33/33 passed. |
| Release build | Custom `codex` release build succeeded. |
| CLI version | `codex-cli 0.149.0`. |
| Internal app-server version | `0.149.0`. |
| Daemon | Running from the final custom release with `cliVersion` and `appServerVersion` both `0.149.0`. |
| Direct DSV4 | Successful through SGLang on port 8000. |
| DSV4 subagents | Two live repository-exploration subagents completed. |
| Remote pairing | Confirmed successful by the user. |
| Cargo version residue | None in `Cargo.toml` or `Cargo.lock`. |

## Initial handoff release artifact (historical)

```text
/mnt/hot/ambientlight/.codex/packages/standalone/releases/0.149.0-dsv4-handoff-sglang-agents-pairfix-20260823-x86_64-unknown-linux-gnu
```

Entrypoint SHA-256:

```text
5a1d19cec41a6125ffd6eb39cc64ab0592bc7410503ed39b426053a856aa561a
```

The optional `codex-code-mode-host` binary was reused from the preceding custom package because its separate rebuild encountered a missing upstream V8 archive. The pairing change is in the main Codex/daemon path and does not depend on that host binary.

## Operating commands

Start Codex normally with the native OpenAI base provider:

```bash
codex
```

Inside the TUI, continue through local DSV4:

```text
/handoff dsv4
```

Return through the native OpenAI base configuration:

```text
/handoff --base
```

Restart or reselect the custom daemon from an ordinary local/SSH shell, not from the Remote session being served by that daemon:

```bash
cd /mnt/hot/ambientlight/repos/codex
./restart-custom-codex-daemon.sh
```

Request a new manual pairing artifact:

```bash
codex remote-control pair --json
```

Verify the selected binary and daemon identity:

```bash
readlink -f ~/.local/bin/codex
codex --version
codex app-server daemon version
```

## Caveats and repository state

- Cross-provider continuation is implemented as a linked fork, not an in-place provider mutation.
- Portable history intentionally drops non-text and provider-specific context.
- CLI/headless Remote Control remains an experimental upstream feature.
- Running the restart helper from the Remote session it serves will disconnect that session.
- Re-enabling the stock standalone updater can replace `standalone/current`; rerun the helper if that happens.
- The source changes and this result document are preserved on branch `dsv4-sglang-handoff` in the `idleai/codex-evo` fork.
- No pairing code, access token, or other credential is recorded in this document.

## Final result

The requested configuration is operational: native OpenAI remains the default, direct local SGLang DSV4 works without LiteLLM, provider switching is available through portable handoff forks in one visible workflow, DSV4 subagents work, the custom daemon is active, and Codex Remote pairing succeeds.

## Follow-up: OpenAI parent spawning direct DSV4 children

The follow-up patch completes the remaining workflow: a native OpenAI root agent can now spawn a child whose complete model route comes from the `dsv4` profile, including the direct SGLang provider, DSV4 catalog, model instructions, context limits, and reasoning settings.

### Implemented behavior

| Area | Result |
| --- | --- |
| Spawn profile loading | `spawn_agent` can resolve a validated `$CODEX_HOME/<profile>.config.toml` model route without replacing inherited tools, permissions, environments, or workspace settings. |
| Project default | `[agents].default_subagent_profile = "dsv4"` applies whenever a spawn call omits an explicit profile. |
| Child model manager | Profile-routed children receive a provider/catalog-specific models manager, so DSV4 is validated against its own catalog instead of the OpenAI parent catalog. |
| Explicit override | Non-OpenAI parents can advertise the optional `profile` field directly. Native OpenAI parents hide it because OpenAI reserves and validates the exact `spawn_agent` schema. |
| OpenAI plaintext bridge | An OpenAI parent with `default_subagent_profile` automatically uses Codex's stock V1 multi-agent surface. V1 carries delegated task text as plaintext; OpenAI's V2 surface intentionally encrypts it for OpenAI children and a direct SGLang child cannot decrypt that payload. |
| Child protocol | The DSV4 child still resolves its own catalog and uses V2 multi-agent behavior; only the OpenAI parent-side handoff uses V1. |
| Cross-provider context | Cross-provider V2 spawns require `fork_turns = "none"`; when omitted for a profile-routed spawn it is selected automatically. V1 cross-provider spawns reject `fork_context = true`. |
| OpenAI compatibility | The native OpenAI request receives the unmodified reserved schema, avoiding the backend `Function 'collaboration.spawn_agent' ... must match the configured schema` rejection found by the first live smoke test. |

### `editchain` configuration

`/mnt/hot/ambientlight/repos/editchain/.codex/config.toml` now contains:

```toml
[agents]
default_subagent_profile = "dsv4"
```

No model or profile argument is needed in that repository. A normal OpenAI session can simply be told to spawn a subagent; the handler applies `dsv4` after the stock OpenAI tool call arrives.

### Live verification

| Check | Result |
| --- | --- |
| Native parent | `gpt-5.6-sol` used the native OpenAI provider/authentication path. |
| Packaged routing sentinel | The final packaged binary spawned exactly one child without model/profile arguments; the child returned `FINAL_DSV4_OK`. |
| Direct endpoint evidence | The `dsv4` container logged a fresh successful `POST /v1/responses` during the packaged run. |
| Repository exploration | A DSV4 child read only `Cargo.toml` and `README.md` in `editchain`, identified the `editchain` CLI package, summarized the repository's CRDT edit-chain purpose, and completed without edits. |
| Focused integration tests | `just test -p codex-core subagent_profiles`: 2/2 passed for explicit and configured-default profile routes, using separate parent and child mock endpoints. |
| Tool schema tests | The focused `spawn_agent_tool` tests passed, and the tool-plan test confirms OpenAI V1/V2 schemas omit the custom profile property. |
| Config tests | `just test -p codex-config`: 275/275 passed. |
| `editchain` validation | `./scripts/lint.sh` exited 0 with `RESULT: PASS`; formatting, check, Clippy, tests, doc tests, and cargo-deny passed. Cargo-deny emitted only its non-failing unmatched ISC allowance warning. |
| Formatting/lint | Scoped `just fix -p codex-core`, `just fix -p codex-config`, `just fmt`, and `git diff --check` completed. The config fix retained one pre-existing generated-protobuf Clippy warning. |
| Broader core run | The earlier `just test -p codex-core` run passed 3566/3567 tests with 9 skipped; the sole failure was the unrelated existing hook timeout `async_hook_finishing_while_idle_waits_for_the_next_turn`, which also timed out when isolated. |

The first live exploration attempt exposed the two backend constraints that shaped the final design: OpenAI rejects modifications to its reserved V2 tool schema, and the stock V2 delegated `message` is opaque ciphertext outside OpenAI. Keeping the exact stock schema and selecting stock plaintext V1 for a configured cross-provider default solved both issues. Subsequent sentinel and real repository-exploration runs completed successfully.

### Follow-up subagent-profile release (historical)

```text
/mnt/hot/ambientlight/.codex/packages/standalone/releases/0.149.0-dsv4-subagent-profiles-plaintext-v1-20260823-x86_64-unknown-linux-gnu
```

Entrypoint SHA-256:

```text
4651326aa49e4872dfe0c59107a102cb44c0731982312fd3a36520acd188b00f
```

The packaged CLI and app-server protocol both report `0.149.0`. The temporary Cargo release stamp was removed after building; `Cargo.toml` and `Cargo.lock` were restored to their original hashes.

This package was selected during the original follow-up. It has since been superseded by the active prompt-compatibility release documented below.

From an ordinary SSH/local shell, activate the final daemon with:

```bash
cd /mnt/hot/ambientlight/repos/codex
./restart-custom-codex-daemon.sh
```

### Final daemon activation compatibility fix

The first activation attempt stopped safely before restarting the daemon because the final
hand-built release was missing the standalone installer's root-level `codex -> bin/codex`
entrypoint. The normal CLI symlink uses `standalone/current/bin/codex`, but daemon lifecycle
management intentionally launches the fixed `standalone/current/codex` path.

The final release now includes that compatibility symlink. The restart helper also creates and
validates it before selecting a release, then invokes lifecycle commands through the exact managed
path. `bash -n restart-custom-codex-daemon.sh` passes, all managed and CLI paths resolve to the same
final binary, and a read-only daemon version query succeeds. The failed attempt left the preceding
custom daemon running, so no Remote service interruption occurred. The compatibility fix was
subsequently activated from SSH.

Then start Codex in `editchain` normally and request a child without specifying a model or profile:

```bash
cd /mnt/hot/ambientlight/repos/editchain
codex
```

For example: `Spawn one subagent to inspect Cargo.toml and README.md, wait for it, and summarize its findings.`

## Active release before the idle rebase handoff

The active standalone installation on 2026-09-03 is:

```text
/mnt/hot/ambientlight/.codex/packages/standalone/releases/0.149.0-dsv4-responses-v12-promptfix-20260902-x86_64-unknown-linux-gnu
```

Entrypoint SHA-256:

```text
b82c49e16e4eb86fa602dc22beb5224f530acd6a3a0b064317e694636c00eda8
```

Both `standalone/current` and `~/.local/bin/codex` resolve to that release, and the CLI reports
`codex-cli 0.149.0`. The restart helper is pinned to the same package.

The final source validation on this branch covered the affected crates. Provider-info (27 tests),
config (275), app-server protocol (291), and app-server daemon (33) passed completely. The full
core run passed 3,564 of 3,567 tests, with the remaining failures caused by checkout-level
`AGENTS.md` fixture leakage and the pre-existing async-hook timeout. The app-server run passed
1,271 of 1,272 tests, with one unrelated Cursor-migration fixture failure. The TUI handoff tests
passed, and there are no pending TUI snapshots; the broader TUI suite encountered unrelated
environment-dependent IDE socket and global `AGENTS.md` tests.

## Previous idle rebase and 0.151.0 release handoff

On 2026-09-03, the complete custom patch stack was replayed onto the fork's `idle` branch. The
branch now has a linear history above this exact upstream-main baseline:

```text
728cb12fe5794b0c3a8e776fb4994b1650b973a8
```

That baseline is marked by the annotated tag `codex-evo-upstream-main-20260903`. The official
annotated `rust-v0.151.0` tag is also present locally; it peels to release commit
`78c290807ce710180111df227df3b7a4fe845452`. The `idle` baseline is newer than that official
release commit, so this package is intentionally an upstream-main snapshot with a `0.151.0`
runtime identity, not a byte-for-byte derivative of the official release tag.

The replay retained all ten original custom commits, followed by two integration fixes:

| Area | Rebase result |
| --- | --- |
| Provider compatibility | Direct Responses-compatible providers retain custom header, message-role, and request-shape controls. |
| Thread-local routing | Each thread can own its provider-specific model manager and static model catalog. |
| Portable handoff | App-server and TUI profile handoffs preserve bounded user/assistant text while dropping provider-specific tool, reasoning, realtime, and retained-context records. |
| Profiled subagents | OpenAI parents can route children through named profiles such as `dsv4`, with the child's provider and model catalog created independently. |
| Service tier | Explicit spawn tier, selected-profile tier, and root preference now have stable precedence; ordinary role-local tiers cannot unexpectedly override them. |
| Remote pairing | The longer pairing-response timeout remains in the app-server daemon client. |
| Upstream adaptation | Current thread settings, task tools, MCP hydration, Windows proxy settings, originators, and newer rollout variants were integrated during the replay. |

That custom source was tagged with the annotated tag `codex-evo-v0.151.0-idle.1`. The old
`dsv4-sglang-handoff` branch remains untouched as a historical reference.

### Prepared standalone package

```text
/mnt/hot/ambientlight/.codex/packages/standalone/releases/0.151.0-codex-evo-idle.1-20260903-x86_64-unknown-linux-gnu
```

| Artifact | SHA-256 |
| --- | --- |
| `bin/codex` | `a908a0d1d25673fe3dd7da7336fffcda201d674efe5f857a189d6503f2bce56f` |
| `bin/codex-code-mode-host` | `07aa85992de9f38606b50a7a9b31e89ced276fd3d0b605c9ee44335f974a7205` |

The repository's canonical package assembler created the package with a GNU/Linux target, a
freshly compiled and checksum-backed V8 code-mode host, and the latest installed standalone
resource binaries. The root-level `codex -> bin/codex` compatibility entrypoint is present. Both
the binary and `codex-package.json` report `0.151.0`.

The release build temporarily changed the Cargo workspace version from `0.0.0` to `0.151.0`.
After packaging, `Cargo.toml` and `Cargo.lock` were restored byte-for-byte to these original hashes:

| File | Restored SHA-256 |
| --- | --- |
| `codex-rs/Cargo.toml` | `d0da87a7ea903a8bd17a61d9164cbffdbab9031d00c90f90ff040cde58feca9f` |
| `codex-rs/Cargo.lock` | `c441d9fd25810acab8b75a489f9b61ff8e585a3215eff86e4393987587194024` |

### Validation

| Check | Result |
| --- | --- |
| Compile | `cargo check` passed for core, app-server, TUI, and app-server daemon. |
| App-server protocol | 299 tests passed, 1 skipped. |
| DSV4 subagent profiles | Both explicit-profile and configured-default integration tests passed. |
| Service-tier behavior | Profile/root inheritance tests and all 40 matching service-tier tests passed. |
| Provider catalogs and role rewriting | 7 model-catalog tests and the Responses message-role rewrite test passed. |
| Portable handoff | The cross-provider app-server integration test passed. |
| TUI handoff | All 3 matching slash-handoff tests passed. |
| Remote pairing timeout | The focused daemon regression test passed. |
| Lint and formatting | Scoped Clippy fixes completed for all affected crates; `just fmt` and `git diff --check` passed. |

The restart helper is pinned to this package and verifies `codex-cli 0.151.0` before changing the
managed `standalone/current` symlink. It was deliberately not executed from the Remote session
that built it, because restarting that daemon would disconnect the session. Activate it from an
ordinary SSH or local shell:

```bash
cd /mnt/hot/ambientlight/repos/codex
./restart-custom-codex-daemon.sh
```

## Latest idle port: 0.154.0-alpha.11

On 2026-09-09, the complete 0.151.0 custom stack was replayed onto the latest local `idle`
branch. The new source branch is `feature/dsv4-subagent-profiles-v0.154.0-alpha.11`, based on
this exact idle commit:

```text
ccf470c060d1c86c9c7f8d42dc3dabf507b56c9a
```

The baseline is marked by `codex-evo-upstream-main-20260909`, and the final source is marked by
`codex-evo-v0.154.0-alpha.11-idle.1`. The official annotated `rust-v0.154.0-alpha.11` tag peels
to release commit `4236f50b7cef42a74bf69e4b8f8fd18fd3ad97c6`. That release-only commit and `idle` share source
parent `9e868bd9dc007c05e84a98e0b1f4e31dc98c5e6a`; `idle` then includes later upstream changes.
Consequently, this port uses the exact newer idle source while carrying the latest applicable
upstream alpha release identity in its branch and tag names.

The replay retained the original 13 commits and added one compatibility fix for the newer thread
manager constructor. Rebase conflicts were integrated with current upstream behavior: portable
profile forks use the latest fork/start options, `/handoff` coexists with the newer `/worktree`
flow, and the experimental app-server schema was regenerated from source.

### Latest port validation

| Check | Result |
| --- | --- |
| Experimental schema | `just write-app-server-schema --experimental` passed. |
| App-server protocol | 303 passed, 1 skipped. |
| DSV4 subagent profiles | Both explicit-profile and configured-default core integration tests passed. |
| Portable handoff | 3 app-server unit tests and the cross-provider integration test passed. |
| TUI handoff | All 3 matching slash-command tests passed. |
| Provider, config, daemon | 458 tests passed, including the delayed pairing-response regression. |
| Protocol and sample | 343 protocol tests passed; the thread-manager sample compiled successfully. |
| Lint and formatting | Scoped Clippy completed for all affected crates; `just fmt` and `git diff --check` passed. |

### Prepared alpha.11 standalone package

```text
/mnt/hot/ambientlight/.codex/packages/standalone/releases/0.154.0-alpha.11-codex-evo-idle.1-20260909-x86_64-unknown-linux-gnu
```

| Artifact | SHA-256 |
| --- | --- |
| `bin/codex` | `8ce34c8c533a914f46c1a7b605e4c5f9745ea315e9e97a93064960180adeb3f3` |
| `bin/codex-code-mode-host` | `239e74aacd9ffb28d8164900762d9a739f59c09836a048f7d75f21d712acd830` |
| `codex-resources/bwrap` | `c102c5f893faed17ed053ce6ceb9fe0bb03069b991a6f0390e54d82c85f1bca0` |

The canonical package assembler built a GNU/Linux release with a temporary upstream-style
`0.154.0-alpha.11` Cargo workspace stamp. The CLI and a fresh app-server handshake both report
`0.154.0-alpha.11`; the package manifest records the same version. The host lacked the `libcap`
development metadata needed to rebuild Bubblewrap, so the package reused the verified helper from
the preceding package through the builder's supported prebuilt override. Its source is unchanged
between the two release branches.

The package is selected through `standalone/current`, and the restart helper is pinned to it. The
already-running 0.151.0 daemon was deliberately left untouched so it can be restarted separately.
