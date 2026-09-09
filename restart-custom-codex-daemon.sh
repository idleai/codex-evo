#!/usr/bin/env bash
set -euo pipefail

custom_release="/mnt/hot/ambientlight/.codex/packages/standalone/releases/0.154.0-alpha.11-codex-evo-idle.1-20260909-x86_64-unknown-linux-gnu"
standalone_dir="/mnt/hot/ambientlight/.codex/packages/standalone"
current_link="$standalone_dir/current"
updater_pid_file="/mnt/hot/ambientlight/.codex/app-server-daemon/app-server-updater.pid"
package_codex="$custom_release/bin/codex"
managed_codex="$custom_release/codex"
installed_codex="/mnt/hot/ambientlight/.local/bin/codex"
expected_cli_version="codex-cli 0.154.0-alpha.11"

if [[ ! -x "$package_codex" ]]; then
    echo "Custom Codex binary is missing or not executable: $package_codex" >&2
    exit 1
fi

actual_cli_version="$("$package_codex" --version)"
if [[ "$actual_cli_version" != "$expected_cli_version" ]]; then
    echo "Custom Codex version mismatch: expected '$expected_cli_version', got '$actual_cli_version'" >&2
    exit 1
fi

ensure_managed_entrypoint() {
    if [[ -e "$managed_codex" || -L "$managed_codex" ]]; then
        resolved_managed_codex="$(readlink -f "$managed_codex" 2>/dev/null || true)"
        if [[ "$resolved_managed_codex" != "$package_codex" ]]; then
            echo "Managed daemon entrypoint resolves unexpectedly: $managed_codex -> $resolved_managed_codex" >&2
            exit 1
        fi
        return
    fi

    echo "Adding the managed daemon entrypoint..."
    ln -s "bin/codex" "$managed_codex"
}

stop_official_updater() {
    if [[ ! -f "$updater_pid_file" ]]; then
        return
    fi

    updater_pid="$(sed -nE 's/.*"pid":([0-9]+).*/\1/p' "$updater_pid_file")"
    if [[ ! "$updater_pid" =~ ^[0-9]+$ ]] || [[ ! -r "/proc/$updater_pid/cmdline" ]]; then
        return
    fi

    updater_command="$(tr '\0' ' ' < "/proc/$updater_pid/cmdline")"
    if [[ "$updater_command" != *"app-server daemon pid-update-loop"* ]]; then
        return
    fi

    echo "Stopping official Codex updater (PID $updater_pid)..."
    kill "$updater_pid"
    for _ in {1..50}; do
        if ! kill -0 "$updater_pid" 2>/dev/null; then
            return
        fi
        sleep 0.1
    done

    echo "Updater did not stop; refusing to continue." >&2
    exit 1
}

# The updater would replace `current` with the stock release on its next pass.
stop_official_updater
ensure_managed_entrypoint

echo "Selecting custom Codex release..."
next_link="$standalone_dir/current.custom-next.$$"
ln -s "$custom_release" "$next_link"
mv -Tf "$next_link" "$current_link"

resolved_codex="$(readlink -f "$installed_codex")"
if [[ "$resolved_codex" != "$custom_release/"* ]]; then
    echo "Installed Codex did not resolve to the custom release: $resolved_codex" >&2
    exit 1
fi

selected_managed_codex="$current_link/codex"
resolved_managed_codex="$(readlink -f "$selected_managed_codex")"
if [[ "$resolved_managed_codex" != "$package_codex" ]]; then
    echo "Managed daemon entrypoint did not resolve to the custom binary: $resolved_managed_codex" >&2
    exit 1
fi

echo "Restarting the patched app-server daemon..."
"$selected_managed_codex" app-server daemon restart

echo "Ensuring remote control is enabled..."
"$selected_managed_codex" app-server daemon enable-remote-control

# `remote-control start` bootstraps the auto-updater when it is absent, so the
# helper intentionally uses the daemon-level command above. Assert that an
# updater did not survive or reappear before returning success.
stop_official_updater

echo
echo "Resolved CLI: $resolved_codex"
"$installed_codex" --version
"$installed_codex" app-server daemon version
