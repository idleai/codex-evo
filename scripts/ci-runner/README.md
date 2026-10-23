# Local Linux GitHub Actions runner

Run the runner inside an Ubuntu 24.04 KVM guest. The guest has its own root disk,
credentials, Docker daemon and build cache. No host directory or Docker socket
is mounted into it. Its SSH listener binds only to host loopback.

The initial allocation is 16 virtual CPUs, 20 GiB guest memory and a sparse
400 GiB cache disk. The user service caps the VM at 24 GiB resident memory and
16 CPU equivalents. Adjust these limits together with `vm.json` and the Bazel
job limits after measuring capacity alongside other host workloads.

## Prepare

The host needs a QEMU installation, `genisoimage`, a SHA-256-verified Ubuntu
Noble amd64 cloud image, and read/write access to `/dev/kvm`. `--tools-dir`
accepts an extracted Ubuntu package tree containing `usr/bin` and `usr/lib`.
Run `prepare_vm.py --help` for the required paths. It creates the disks, SSH key,
cloud-init seed, launch configuration and `codex-ci-vm.service`. Existing disks
are preserved. The base cloud image must remain at its original path.

Install the generated service in `~/.config/systemd/user/`, reload the user
manager, then enable and start `codex-ci-vm.service`. Enable user lingering if
the VM should start before interactive login. The service launches `run_vm.py`
from the state directory. The guest console is recorded in `console.log` there.

Connect as `runner` on `127.0.0.1:22061` using the generated `ssh-key` and a
dedicated known-hosts file. Wait for `/home/runner/ci-bootstrap-complete` and
successful `cloud-init status --wait`. The `runner` account has sudo inside the
guest so the existing CI setup can configure Linux namespaces and install tools.

## Register and activate

Register `/home/runner/actions-runner` against this repository with label
`codex-linux-x64`, using a short-lived repository registration token. Keep the
token out of terminal output and source files. Select `/mnt/ci-cache/work` as
the work directory. Install and start the runner's `svc.sh` service as `runner`.

For a public repository, require approval for workflow runs from all external
contributors before connecting the runner. Fork PRs retain GitHub-hosted
routing in these workflows. The VM also separates job files from the host.

Set repository variable `CODEX_LINUX_PILOT_RUNNER=codex-linux-x64`, then run
`self-hosted-linux-check.yml` on this branch. After the actual build and test
pass, set `CODEX_LINUX_RUNNER=codex-linux-x64` to route the Linux x64 Bazel test,
lint, release verification, V8 and argument-comment-lint jobs to it. Other
platforms and lightweight metadata jobs retain their current runners.

Delete `CODEX_LINUX_RUNNER` to route subsequent jobs back to GitHub-hosted Linux.
Already queued jobs keep their original assignment. Stop the VM with
`systemctl --user stop codex-ci-vm.service` after its runner is idle. Preserve
`cache.qcow2` to retain compiled outputs; its 400 GiB size is an upper bound,
not an immediate allocation.
