#!/usr/bin/env python3
"""Prepare an isolated Linux KVM guest for a GitHub Actions runner."""

import argparse
import json
import os
import re
import shlex
import shutil
import socket
import subprocess
from pathlib import Path


RUNNER_VERSION = "2.338.1"
RUNNER_SHA256 = "08e9ccb3604d2462c24b5e33604dc30810296ca15cc65f23cffd4317eb144c37"


def positive_int(value: str) -> int:
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return number


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", required=True, type=Path)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--tools-dir", required=True, type=Path)
    parser.add_argument("--name", default="codex-ci-linux")
    parser.add_argument("--service-name", default="codex-ci-vm")
    parser.add_argument("--ssh-port", type=positive_int, default=22061)
    parser.add_argument("--cpus", type=positive_int, default=32)
    parser.add_argument("--memory-mib", type=positive_int, default=32768)
    parser.add_argument("--root-disk-gib", type=positive_int, default=32)
    parser.add_argument("--cache-disk-gib", type=positive_int, default=192)
    parser.add_argument("--bazel-jobs", type=positive_int, default=12)
    parser.add_argument("--test-jobs", type=positive_int, default=2)
    parser.add_argument("--cargo-jobs", type=positive_int, default=12)
    args = parser.parse_args()
    for name in (args.name, args.service_name):
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,62}", name):
            parser.error("names must use lowercase letters, digits and hyphens")
    if not 1024 <= args.ssh_port <= 65535:
        parser.error("--ssh-port must be between 1024 and 65535")
    if args.root_disk_gib < 8 or args.cache_disk_gib < 12:
        parser.error("root and cache disks need at least 8 GiB and 12 GiB respectively")
    state = args.state_dir.resolve()
    tools = args.tools_dir.resolve()
    image = args.image.resolve(strict=True)
    state.mkdir(parents=True, exist_ok=True)
    os.chmod(state, 0o700)
    with socket.socket(socket.AF_UNIX) as monitor:
        try:
            monitor.connect(str(state / "qmp.sock"))
        except (FileNotFoundError, ConnectionRefusedError):
            pass
        else:
            parser.error("stop this VM before changing its configuration")
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = str(tools / "usr/lib/x86_64-linux-gnu")
    env["QEMU_MODULE_DIR"] = str(tools / "usr/lib/x86_64-linux-gnu/qemu")
    qemu_img = str(tools / "usr/bin/qemu-img")
    key = state / "ssh-key"
    if not key.exists():
        subprocess.run(
            ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)],
            check=True,
        )
    public_key = key.with_suffix(".pub").read_text().strip()
    root_disk = state / "root.qcow2"
    if not root_disk.exists():
        subprocess.run(
            [
                qemu_img,
                "create",
                "-f",
                "qcow2",
                "-F",
                "qcow2",
                "-b",
                str(image),
                str(root_disk),
                f"{args.root_disk_gib}G",
            ],
            env=env,
            check=True,
        )
    cache_disk = state / "cache.qcow2"
    if not cache_disk.exists():
        subprocess.run(
            [
                qemu_img,
                "create",
                "-f",
                "qcow2",
                str(cache_disk),
                f"{args.cache_disk_gib}G",
            ],
            env=env,
            check=True,
        )
    archive = f"actions-runner-linux-x64-{RUNNER_VERSION}.tar.gz"
    download = f"https://github.com/actions/runner/releases/download/v{RUNNER_VERSION}/{archive}"
    bootstrap = f"""#!/bin/bash
set -euo pipefail
mountpoint -q /mnt/ci-cache
if [[ ! -e /mnt/ci-cache/swapfile ]]; then
  install -m 0600 /dev/null /mnt/ci-cache/swapfile
  fallocate -l 8G /mnt/ci-cache/swapfile
  mkswap /mnt/ci-cache/swapfile
fi
if ! swapon --show=NAME --noheadings | grep -Fxq /mnt/ci-cache/swapfile; then
  swapon /mnt/ci-cache/swapfile
fi
if ! grep -Fq '/mnt/ci-cache/swapfile none swap' /etc/fstab; then
  echo '/mnt/ci-cache/swapfile none swap sw,nofail 0 0' >> /etc/fstab
fi
sysctl -w vm.swappiness=10
install -d -o runner -g runner /mnt/ci-cache/home-cache /mnt/ci-cache/cargo /mnt/ci-cache/rustup /mnt/ci-cache/toolcache /mnt/ci-cache/work
install -d -o runner -g runner /home/runner/actions-runner
for mapping in .cache:home-cache .cargo:cargo .rustup:rustup; do
  source=/home/runner/${{mapping%%:*}}
  destination=/mnt/ci-cache/${{mapping#*:}}
  if [[ -d "$source" && ! -L "$source" ]]; then
    cp -a "$source/." "$destination/"
    rm -rf -- "$source"
  fi
  ln -sfn "$destination" "$source"
done
cat > /home/runner/.bazelrc <<'BAZELRC'
build --experimental_disk_cache_gc_max_size=32G
build --experimental_disk_cache_gc_idle_delay=1s
BAZELRC
chown runner:runner /home/runner/.bazelrc
curl --fail --location --retry 3 {shlex.quote(download)} -o /tmp/runner.tar.gz
printf '%s  %s\\n' {RUNNER_SHA256} /tmp/runner.tar.gz | sha256sum --check --status
tar -xzf /tmp/runner.tar.gz -C /home/runner/actions-runner
rm /tmp/runner.tar.gz
cat > /home/runner/actions-runner/.env <<'ENV'
RUNNER_TOOL_CACHE=/mnt/ci-cache/toolcache
AGENT_TOOLSDIRECTORY=/mnt/ci-cache/toolcache
CODEX_BAZEL_JOBS={args.bazel_jobs}
CODEX_BAZEL_TEST_JOBS={args.test_jobs}
CARGO_BUILD_JOBS={args.cargo_jobs}
ACTIONS_RUNNER_HOOK_JOB_STARTED=/usr/local/lib/idle-ci-runner/maintain-cache.sh
ACTIONS_RUNNER_HOOK_JOB_COMPLETED=/usr/local/lib/idle-ci-runner/maintain-cache.sh
ENV
chown -R runner:runner /home/runner/actions-runner
touch /home/runner/ci-bootstrap-complete
"""
    config = {
        "hostname": args.name,
        "manage_etc_hosts": True,
        "ssh_pwauth": False,
        "disable_root": True,
        "groups": ["docker"],
        "users": [
            {
                "name": "runner",
                "shell": "/bin/bash",
                "groups": ["sudo", "docker"],
                "sudo": ["ALL=(ALL) NOPASSWD:ALL"],
                "lock_passwd": True,
                "ssh_authorized_keys": [public_key],
            }
        ],
        "package_update": True,
        "package_upgrade": False,
        "packages": [
            "build-essential",
            "ca-certificates",
            "cmake",
            "curl",
            "docker.io",
            "fonts-liberation",
            "gh",
            "git",
            "gnupg",
            "gnupg2",
            "jq",
            "libasound2-dev",
            "libcurl4-openssl-dev",
            "libgbm1",
            "libgtk-3-0t64",
            "libicu74",
            "libncurses-dev",
            "libnss3",
            "libpython3-dev",
            "libssl-dev",
            "libudev-dev",
            "libxml2-dev",
            "libxss1",
            "libz3-dev",
            "ninja-build",
            "openssh-server",
            "pkg-config",
            "python3",
            "python3-venv",
            "ripgrep",
            "rsync",
            "unzip",
            "xvfb",
            "xz-utils",
            "zip",
            "zlib1g-dev",
            "zstd",
        ],
        "disk_setup": {
            "/dev/vdb": {"table_type": "gpt", "layout": True, "overwrite": False},
        },
        "fs_setup": [
            {
                "label": "ci-cache",
                "filesystem": "ext4",
                "device": "/dev/vdb1",
                "overwrite": False,
            }
        ],
        "mounts": [
            ["LABEL=ci-cache", "/mnt/ci-cache", "ext4", "defaults,nofail", "0", "2"]
        ],
        "write_files": [
            {
                "path": "/usr/local/lib/idle-ci-runner/maintain_cache.py",
                "permissions": "0644",
                "content": Path(__file__).with_name("maintain_cache.py").read_text(),
            },
            {
                "path": "/usr/local/lib/idle-ci-runner/maintain-cache.sh",
                "permissions": "0755",
                "content": Path(__file__).with_name("maintain-cache.sh").read_text(),
            },
            {
                "path": "/etc/sysctl.d/60-ci-runner-memory.conf",
                "permissions": "0644",
                "content": "vm.swappiness=10\n",
            },
            {
                "path": "/usr/local/sbin/prepare-ci-runner",
                "permissions": "0700",
                "content": bootstrap,
            },
        ],
        "runcmd": [["/usr/local/sbin/prepare-ci-runner"]],
    }
    (state / "user-data").write_text(
        "#cloud-config\n" + json.dumps(config, indent=2) + "\n"
    )
    (state / "meta-data").write_text(
        f"instance-id: {args.name}-1\nlocal-hostname: {args.name}\n"
    )
    subprocess.run(
        [
            str(tools / "usr/bin/genisoimage"),
            "-quiet",
            "-output",
            str(state / "seed.iso"),
            "-volid",
            "cidata",
            "-joliet",
            "-rock",
            str(state / "user-data"),
            str(state / "meta-data"),
        ],
        env=env,
        check=True,
    )
    vm = {
        "state_dir": str(state),
        "tools_dir": str(tools),
        "name": args.name,
        "cpus": args.cpus,
        "memory_mib": args.memory_mib,
        "ssh_port": args.ssh_port,
    }
    (state / "vm.json").write_text(json.dumps(vm, indent=2) + "\n")
    shutil.copyfile(Path(__file__).with_name("run_vm.py"), state / "run_vm.py")
    launch = shlex.join(["/usr/bin/python3", str(state / "run_vm.py")])
    service = f"""[Unit]
Description=Isolated Linux CI VM ({args.name})
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Slice=idle-ci.slice
ExecStart=/usr/bin/sg kvm -c {shlex.quote(launch)}
Restart=on-failure
RestartSec=15
CPUQuota={args.cpus * 100}%
CPUWeight=20
MemoryMax={args.memory_mib + 3072}M
MemorySwapMax=0
IOWeight=20
Nice=5
KillSignal=SIGTERM
TimeoutStopSec=120

[Install]
WantedBy=default.target
"""
    (state / f"{args.service_name}.service").write_text(service)
    print(f"Prepared VM files in {state}; no runner was registered or started.")


if __name__ == "__main__":
    main()
