# Shared Linux GitHub Actions runners

Run the runner inside an Ubuntu 24.04 KVM guest. The guest has its own root disk,
credentials, Docker daemon and working disk. Completed Bazel action outputs and
Rust compiler outputs are also stored in persistent cache services on the host.
No host directory or Docker socket is mounted into the guest. SSH binds only to
host loopback.

The pool has eight identical guests, each with 64 virtual CPUs and 28 GiB memory.
Each VM service has a 32 GiB memory ceiling, including QEMU and disk-cache
overhead. `idle-ci.slice` applies a 240 GiB memory high watermark and a 256 GiB
hard limit to all VMs and the cache service. Guest memory totals 224 GiB; the
remaining allowance covers VM overhead and the shared service. Individual
service limits are ceilings, not reservations. The slice caps CPU use at 128
logical CPU equivalents and disables host swap for its processes.
The host has 64 physical cores with two hardware threads per core. The guests
share all 128 logical CPUs with equal weights. Each VM's CPU quota permits up to
64 logical CPU equivalents. The 512 configured vCPUs allow two busy guests to
use up to 64 CPU equivalents each, while eight equally busy guests share
approximately 16 each, subject to other host workloads. Workers
01–04 serve Codex PRs through `codex-pr-linux-x64`, 05–06 serve client PRs through
`idle-pr-linux-x64`, and 07–08 serve integration and release work through
`idle-background-linux-x64`. Legacy labels remain for existing workflows.

`configure_routes.py --record-directory <directory>` previews these assignments.
Add `--apply` to update labels and the seven participating repositories' runner
variables. It does not alter VM sizing. The plan is validated before any update;
the directory retains the original runner labels and the applied assignments.

## PR feedback and asynchronous validation

PR feedback has a ten-minute target, measured from the current workflow attempt
starting through `CI required`, including setup and queue time. The terminal
check fails if that budget is exceeded. Codex allows four minutes for preflight,
five for each parallel build/smoke job, and one for the terminal check. Client
repositories allow two minutes for preflight, seven for checks, and one for the
terminal check. Cold compilation or runner congestion can still exceed the
target; a timeout never counts as a successful check.

Codex PRs retain formatting, dependency policy, shipped-binary compilation,
eleven Rust smoke targets, and scoped SDK smoke tests. Default-branch, scheduled
and tagged-release validation run the complete Linux and SDK suites. Tagged
releases cannot publish until that validation succeeds. The coordinated product
publisher also requires successful CI for each exact component revision.
Client PRs retain their Rust quality checks. App-core's bindings and consumer
integration, and web-ui's complete browser matrix, run after merge; web-ui PRs
exercise one relevant browser fixture.

PR updates cancel superseded PR runs. Default-branch validation and coordinated
product builds finish their current run, with a newer pending candidate kept by
GitHub's concurrency queue. Reserved PR workers keep those longer jobs out of
the PR runner queue.

`async-failures.yml` watches completed default-branch and release workflows and
calls `report-async-failure.yml`. Its hosted job reads workflow/job metadata and
opens or updates a remediation issue per workflow and source commit. It uses
reporting code from `idle-base`, never candidate source or build artifacts.
Failed retries update the same issue; PR failures, superseded cancellations and
successful retries do not open issues. A failing candidate remains unpublished.
The issue includes its revision, run attempt, failed jobs and steps; close it
after verifying the successful remediation run. The reporting workflow's manual
`dry_run` mode previews the issue without writing it.

Every guest has a sparse 32 GiB root disk and a 256 GiB private working disk,
retaining the filename `cache.qcow2` and mount point `/mnt/ci-cache`. Each guest
has 8 GiB of swap on that disk, 64 concurrent Bazel/Cargo build jobs and two
concurrent Bazel test jobs. Hooks before and after each job keep Bazel's disk
cache below 32 GiB and require at least 24 GiB of free space before continuing.
When the shared host cache is configured, CI disables the extra per-VM action
cache. Bazel only collects that cache between commands, so a full dependency
build could otherwise exhaust the working disk before collection starts.
They remove extracted-repository caches left by older per-job paths and keep
compiled outputs, repository downloads, toolchains and worktrees. New jobs reuse
one extracted-repository cache in each VM. Output directories, downloads and
toolchains consume additional disk space. The shared host caches have separate
512 GiB Bazel and 256 GiB Cargo limits. Combined maximum disk capacity is 3,072
GiB before image and filesystem overhead.
Sparse disk sizes are upper bounds, not immediate host allocations. Monitor
both host and guest free space as the pool fills its caches.

## Persistent shared cache

`idle-ci-cache.service` runs bazel-remote 2.6.4 from
`~/.local/share/codex-ci-runner/bin/bazel-remote`. The Linux amd64 release binary
has SHA-256 `6a6b64666c2238ec6b49b7b29f5e41e4149de09417a0028897c9bffe40376876`.
It stores artifacts in `/mnt/hot/ci-cache/bazel`, outside every VM state directory,
and evicts least-recently-used entries at 512 GiB. VM deletion, recreation and
re-registration do not remove these files.

The HTTP listener binds to host loopback at `127.0.0.1:18500`. QEMU user-mode
networking exposes it to guests at `http://10.0.2.2:18500`. Pass that URL through
`prepare_vm.py --remote-cache-url`; preparation writes `/etc/bazel.bazelrc`, which
survives setup actions rewriting the runner's home configuration. The service
has a 2 GiB memory ceiling inside the overall CI slice. Its `/status` endpoint
reports the current and maximum cache sizes.

Checkouts, Cargo `target` directories and Bazel output bases stay private to
each VM. The cache API shares completed artifacts and coordinates concurrent
reads and writes. The cache is for the existing trusted organization jobs;
fork jobs retain their hosted routing. Stop only the VMs during replacement,
and leave the cache service and its data directory in place.

## Shared Cargo compiler cache

Cargo jobs use sccache 0.19.0 through `RUSTC_WRAPPER=/usr/local/bin/sccache`.
The runner sets `CARGO_INCREMENTAL=0` so compiled Rust libraries can be reused
between VMs. Existing Cargo target directories still retain local outputs.
`CARGO_BUILD_BUILD_DIR=/mnt/ci-cache/cargo/build` gives every repository the same
intermediate build path inside its VM. Cargo 1.97 keeps final outputs in each
repository's target directory. This also gives generated dependency files the
same paths across jobs, allowing the remote cache to reuse them. The build
directory is private to each VM; Cargo manages its local locking and freshness.
Cache keys include the compiler, source contents, features, target and flags;
matching a package version alone does not guarantee a hit. Rust compilation
also requires matching build paths, which the CI workspaces and coordinated
product checkouts keep stable across workers. Final linking, build scripts and
tests still run. See [sccache's Rust support](https://github.com/mozilla/sccache/blob/v0.19.0/docs/Rust.md).

The coordinated product builder inherits the ordinary Cargo home and build
directory. It temporarily adds source dependency patches to its owned checkouts
and restores those configurations when it exits. A separate Cargo home or an
extra `CARGO_TARGET_DIR` value would change the compiler cache keys even for
otherwise identical dependencies. Different dependency selections, feature
sets, target platforms and compiler flags continue to produce separate entries.

`idle-ci-cargo-store.service` uses the installed bazel-remote binary to keep
compiler results in `/mnt/hot/ci-cache/cargo`, bounded to 256 GiB with automatic
eviction. This service accepts opaque sccache entries on host loopback port 18502. Its HTTP action-result validation is disabled because these entries are
compiler archives. The existing Bazel service keeps its own validation and
512 GiB cache.

`idle-ci-cargo-cache.service` runs Caddy 2.11.7 on host loopback port 18501.
`cargo-cache.Caddyfile` maps sccache's three directory components and cache key
to the store's action-cache URL, and forwards the client's storage check.
Both services run inside `idle-ci.slice`; their memory ceilings are 2 GiB and
512 MiB respectively. Guests connect at `http://10.0.2.2:18501` without a host
filesystem mount. The endpoint is for the same trusted organization jobs as
the Bazel cache.

Run `bash scripts/ci-runner/install-cargo-cache.sh` on the host after installing
bazel-remote. It verifies the pinned Caddy archive, installs the configuration
and services, and starts the endpoint. Pass
`--cargo-cache-url http://10.0.2.2:18501` when preparing a new VM. Preparation
installs the pinned sccache binary with an archive checksum check and writes
the runner environment. The client disables WebDAV directory creation because
the cache protocol only needs reads and writes. It can fall back to rustc if
its local daemon cannot be reached.

For existing VMs, drain the runner, install sccache with
`install-sccache.sh`, and add these values to its runner `.env` before restarting
the listener:

```sh
RUSTC_WRAPPER=/usr/local/bin/sccache
CARGO_INCREMENTAL=0
CARGO_BUILD_BUILD_DIR=/mnt/ci-cache/cargo/build
SCCACHE_WEBDAV_ENDPOINT=http://10.0.2.2:18501
SCCACHE_WEBDAV_DISABLE_CREATE_DIR=true
SCCACHE_IGNORE_SERVER_IO_ERROR=1
```

Verify reuse by building the same Cargo inputs on two workers with empty local
target directories at matching paths. Compare `sccache --show-stats` before and
after the second build; it must report remote cache hits. Run the resulting
tests as well. The shared cache cannot import previously compiled Cargo target
directories: jobs populate it as they compile through sccache.

`check_cargo_cache.py` exercises the remote store from independent consumer
workspaces, clearing local intermediate files and restarting its isolated
cache client between builds. It checks reuse, changed source and generated
inputs, feature and compiler-flag changes, and restoration of the original
inputs. It also executes the resulting binaries. The self-hosted runner check
runs this validation with Cargo 1.97.

## Prepare

The host needs a QEMU installation, `genisoimage`, a SHA-256-verified Ubuntu
Noble amd64 cloud image, and read/write access to `/dev/kvm`. `--tools-dir`
accepts an extracted Ubuntu package tree containing `usr/bin` and `usr/lib`.
Run `prepare_vm.py --help` for the required paths. It creates the disks, SSH key,
cloud-init seed and launch configuration. Give each guest a distinct
`--state-dir`, `--name`, `--service-name` and `--ssh-port`. For example, the new
workers use names `idle-ci-linux-01` through `idle-ci-linux-08`, service names
`idle-ci-vm-01` through `idle-ci-vm-08`, and ports 22061 through 22068.
Defaults match the uniform pool: 64 vCPUs, 28 GiB guest RAM, 32 GiB host limit,
32 GiB root disk, 256 GiB working disk, and 64 build workers.

Existing disks and SSH keys are preserved, and the command refuses to rewrite
the configuration of a running VM. Disk-size arguments only affect newly
created disks. Keep each guest's name stable because it also identifies the
cloud-init instance. The base cloud image must remain at its original path.

Install `idle-ci.slice`, `idle-ci-cache.service` and each generated VM service in
`~/.config/systemd/user/`, reload the user manager, then enable and start the
services. Enable user lingering if the VMs should start before interactive
login. Each service launches `run_vm.py` from its state directory. The guest
console is recorded in `console.log` there. Size the slice and services together
when changing the pool; check available host memory alongside other workloads.

Connect as `runner` on the guest's loopback port using its generated `ssh-key`
and a dedicated known-hosts file. Verify the host key against the public key
or fingerprint in that guest's console log before connecting. Wait for
`/home/runner/ci-bootstrap-complete` and
successful `cloud-init status --wait`. The `runner` account has sudo inside the
guest so the existing CI setup can configure Linux namespaces and install tools.

## Register and activate

Register `/home/runner/actions-runner` against `https://github.com/idleai` in
the `Default` runner group, with labels `idle-linux-x64,codex-linux-x64` and
unique names `idle-linux-local-01` through `idle-linux-local-08`. Use a
short-lived organization registration token; keep it out of terminal output
and source files. Select `/mnt/ci-cache/work` as the work directory. Install
the runner's `svc.sh` service as `runner`.

Before starting each guest service, add a systemd drop-in for the unit named
in `/home/runner/actions-runner/.service`:

```ini
[Service]
Restart=on-failure
RestartSec=15
OOMPolicy=continue
```

Reload the guest's systemd manager and start the runner service. An individual
job can still fail if it exhausts memory; the listener should remain available
to report the result and accept subsequent jobs. Inspect such failures before
raising build concurrency.

Limit the runner group's repository access to `codex-evo`, `app-core`,
`editchain`, `host-tools`, `offstage`, `vscode-extension`, `web-ui` and `memos`.
The group must allow the selected public repositories. Organization runner
administration requires an appropriately scoped organization credential.

For a public repository, require approval for workflow runs from all external
contributors before connecting the runner. Fork PRs retain GitHub-hosted
routing in these workflows. The VM also separates job files from the host.

For the first check, give an idle worker an additional `idle-linux-pilot` label
and set repository variable `CODEX_LINUX_PILOT_RUNNER=idle-linux-pilot`. Run
`self-hosted-linux-check.yml` on this branch. After the actual build and test
pass, set `CODEX_LINUX_RUNNER=codex-linux-x64` to route the Linux x64 Bazel test,
lint, release verification, SDK, V8 and argument-comment-lint jobs to it. Other
platforms and lightweight metadata jobs retain their current runners.

The other repositories use `IDLE_LINUX_RUNNER=idle-linux-x64` with the same
fork-PR guard and their original hosted label as a fallback. Validate each
repository's full Linux job on a branch before adopting its workflow change.

Delete a repository's routing variable to route subsequent jobs back to
GitHub-hosted Linux. Already queued jobs keep their original assignment. Before
maintenance, remove that worker's job-routing labels, wait for its GitHub busy
flag to clear, and check that its guest has no active job or Rust build. Stop
the guest runner service before unregistering or changing registration. Stop
the host VM service only after the guest has shut down cleanly. Preserve
`cache.qcow2` when retaining that VM's local outputs. The host cache survives
independently. Guest `fstrim` can return unused
blocks to the host because the virtual disks support discard.

Changing the configured CPU count requires a clean guest shutdown and restart.
Drain and resize one worker at a time so other workers continue their builds.
Update its VM configuration, service CPU quota and runner build limits together,
then verify the guest CPU count before restoring its job-routing labels.

Cloud-init runs only for a new instance. Updating the generated seed does not
upgrade an existing guest: apply package, swap, environment and service changes
there separately while its runner is drained. Re-registering the first guest
at organization scope preserves its disks and build caches.

For existing guests, install `maintain_cache.py` and `maintain-cache.sh` in
`/usr/local/lib/idle-ci-runner` and make the shell script executable. Set both
`ACTIONS_RUNNER_HOOK_JOB_STARTED` and `ACTIONS_RUNNER_HOOK_JOB_COMPLETED` to
`/usr/local/lib/idle-ci-runner/maintain-cache.sh` in the runner's `.env`, then
restart its listener while drained. Run the Python helper once before restoring
the routing labels. It reports the removed caches and free bytes as JSON and
fails if it cannot restore the free-space reserve. It never removes Bazel's
output base; investigate further disk growth before resuming a runner that
cannot recover enough space. The hooks support older workflow revisions too.

The Bazel wrapper explicitly disables the per-VM action cache when
`IDLE_CI_REMOTE_CACHE` is set, while preserving explicit command-line overrides.
Without a shared cache, it passes the disk-cache collection settings explicitly.
Do not rely on `~/.bazelrc` for these settings: the setup action rewrites it.
