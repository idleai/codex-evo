# Shared Linux GitHub Actions runners

Run the runner inside an Ubuntu 24.04 KVM guest. The guest has its own root disk,
credentials, Docker daemon and build cache. No host directory or Docker socket
is mounted into it. Its SSH listener binds only to host loopback.

The pool has four guests, each with 32 virtual CPUs and 32 GiB memory. Each
service has a 35 GiB memory ceiling, including QEMU and disk-cache overhead.
`idle-ci.slice` caps the complete pool at 153,769,803,776 bytes: the previous
24 GiB allowance plus 128 decimal GB released from inference. Guest memory
totals 128 GiB; the remaining allowance covers VM overhead. The slice caps CPU
use at 128 logical CPU equivalents and disables host swap for the VM processes.
The host has 64 physical cores with two hardware threads per core. CPU quotas
limit aggregate execution time; the guests are not pinned to particular cores.

New guests have a sparse 32 GiB root disk and 192 GiB cache disk. The first
guest retains its existing 48 GiB root and 400 GiB cache disks. Each guest has
8 GiB of swap on its cache disk, twelve concurrent Bazel/Cargo build jobs and two
concurrent Bazel test jobs. Hooks before and after each job keep Bazel's disk
cache below 32 GiB and require at least 24 GiB of free space before continuing.
They remove extracted-repository caches left by older per-job paths and keep
compiled outputs, repository downloads, toolchains and worktrees. New jobs reuse
one extracted-repository cache in each VM. Output directories, downloads and
toolchains consume additional disk space.
Sparse disk sizes are upper bounds, not immediate host allocations. Monitor
both host and guest free space as the pool fills its caches.

## Prepare

The host needs a QEMU installation, `genisoimage`, a SHA-256-verified Ubuntu
Noble amd64 cloud image, and read/write access to `/dev/kvm`. `--tools-dir`
accepts an extracted Ubuntu package tree containing `usr/bin` and `usr/lib`.
Run `prepare_vm.py --help` for the required paths. It creates the disks, SSH key,
cloud-init seed and launch configuration. Give each guest a distinct
`--state-dir`, `--name`, `--service-name` and `--ssh-port`. For example, the new
workers use names `idle-ci-linux-02` through `idle-ci-linux-04`, service names
`idle-ci-vm-02` through `idle-ci-vm-04`, and ports 22062 through 22064. The first
guest retains service `codex-ci-vm` and port 22061.

Existing disks and SSH keys are preserved, and the command refuses to rewrite
the configuration of a running VM. Disk-size arguments only affect newly
created disks. Keep each guest's name stable because it also identifies the
cloud-init instance. The base cloud image must remain at its original path.

Install `idle-ci.slice` and each generated service in
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
unique names `idle-linux-local-01` through `idle-linux-local-04`. Use a
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
`cache.qcow2` to retain compiled outputs. Guest `fstrim` can return unused
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

The Bazel wrapper also passes its disk-cache collection settings explicitly.
Do not rely on `~/.bazelrc` for those settings: the setup action rewrites it.
