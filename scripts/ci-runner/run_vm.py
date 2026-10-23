#!/usr/bin/env python3
"""Run the prepared CI guest without mounting host directories into it."""

import ctypes
import json
import os
from pathlib import Path


def main() -> None:
    state = Path(__file__).resolve().parent
    config = json.loads((state / "vm.json").read_text())
    if not os.access("/dev/kvm", os.R_OK | os.W_OK):
        raise SystemExit(
            "The runner service account needs read/write access to /dev/kvm."
        )
    # Avoid synchronous huge-page compaction on a host shared with inference.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(41, 1, 0, 0, 0) != 0:  # PR_SET_THP_DISABLE
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))
    tools = Path(config["tools_dir"])
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = str(tools / "usr/lib/x86_64-linux-gnu")
    env["QEMU_MODULE_DIR"] = str(tools / "usr/lib/x86_64-linux-gnu/qemu")
    command = [
        str(tools / "usr/bin/qemu-system-x86_64"),
        "-name",
        "codex-ci-linux",
        "-machine",
        "q35,accel=kvm",
        "-cpu",
        "EPYC-v4",
        "-smp",
        str(config["cpus"]),
        "-m",
        str(config["memory_mib"]),
        "-L",
        str(tools / "usr/share/qemu"),
        "-bios",
        str(tools / "usr/share/seabios/bios-256k.bin"),
        "-nodefaults",
        "-device",
        f"VGA,romfile={tools / 'usr/share/seabios/vgabios-stdvga.bin'}",
        "-drive",
        f"file={state / 'root.qcow2'},format=qcow2,if=virtio",
        "-drive",
        f"file={state / 'cache.qcow2'},format=qcow2,if=virtio",
        "-drive",
        f"file={state / 'seed.iso'},media=cdrom,readonly=on",
        "-netdev",
        f"user,id=ci,hostfwd=tcp:127.0.0.1:{config['ssh_port']}-:22",
        "-device",
        "virtio-net-pci,netdev=ci,romfile=",
        "-display",
        "none",
        "-serial",
        f"file:{state / 'console.log'}",
        "-monitor",
        "none",
        "-qmp",
        f"unix:{state / 'qmp.sock'},server=on,wait=off",
        "-sandbox",
        "on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny",
    ]
    os.execve(command[0], command, env)


if __name__ == "__main__":
    main()
