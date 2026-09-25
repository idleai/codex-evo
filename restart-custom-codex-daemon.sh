#!/usr/bin/env bash
set -euo pipefail

codex_home="${CODEX_HOME:-/mnt/hot/ambientlight/.codex}"
export CODEX_HOME="$codex_home"
release_name="0.158.0-alpha.14-codex-evo-idle.1-20260925-x86_64-unknown-linux-gnu"
standalone_dir="$codex_home/packages/standalone"
custom_release="${CODEX_CUSTOM_RELEASE:-$standalone_dir/releases/$release_name}"
package_codex="$custom_release/bin/codex"
expected_version="0.158.0-alpha.14"

case "${1:-}" in
    ""|--check) ;;
    *) echo "Usage: $0 [--check]" >&2; exit 2 ;;
esac
if (( $# > 1 )); then
    echo "Usage: $0 [--check]" >&2
    exit 2
fi

for binary in bin/codex bin/codex-code-mode-host codex-resources/bwrap codex-path/rg; do
    if [[ ! -x "$custom_release/$binary" ]]; then
        echo "Custom Codex package is missing an executable: $custom_release/$binary" >&2
        exit 1
    fi
done
actual_cli_version="$("$package_codex" --version)"
if [[ "$actual_cli_version" != "codex-cli $expected_version" ]]; then
    echo "Custom Codex version mismatch: expected 'codex-cli $expected_version', got '$actual_cli_version'" >&2
    exit 1
fi
python3 - "$custom_release/codex-package.json" "$expected_version" <<'PY'
import json
import sys
from pathlib import Path

manifest = json.loads(Path(sys.argv[1]).read_text())
expected = {
    "version": sys.argv[2],
    "target": "x86_64-unknown-linux-gnu",
    "entrypoint": "bin/codex",
}
for key, value in expected.items():
    if manifest.get(key) != value:
        raise SystemExit(f"Custom Codex package {key} mismatch: {manifest.get(key)!r} != {value!r}")
PY

if [[ "${1:-}" == --check ]]; then
    echo "Ready: $custom_release ($actual_cli_version)"
    exit 0
fi

# This command copies the complete package into packages/app-server-daemon,
# pins it against production updates, and restarts an already-running daemon.
# Run from SSH or a local terminal: replacing the daemon disconnects Remote.
echo "Installing and pinning the custom daemon package..."
"$package_codex" app-server daemon update --from-cli --yes
"$package_codex" app-server daemon start

echo "Ensuring remote control is enabled..."
"$package_codex" app-server daemon enable-remote-control

daemon_dir="$codex_home/packages/app-server-daemon"
if [[ -e "$daemon_dir/auto-update-version" ]]; then
    echo "Daemon is still following production updates; refusing to report success." >&2
    exit 1
fi
if ! cmp -s "$package_codex" "$daemon_dir/current/bin/codex"; then
    echo "Managed daemon package does not contain the selected custom binary." >&2
    exit 1
fi

version_json="$("$package_codex" app-server daemon version)"
python3 - "$version_json" "$expected_version" <<'PY'
import json
import sys

status = json.loads(sys.argv[1])
expected = sys.argv[2]
if status.get("status") != "running" or any(
    status.get(key) != expected
    for key in ("cliVersion", "managedCodexVersion", "appServerVersion")
):
    raise SystemExit(f"Custom daemon version verification failed: {status}")
PY

# Select the CLI only after the dedicated daemon has started successfully.
next_link="$standalone_dir/current.custom-next.$$"
trap 'rm -f -- "$next_link"' EXIT
ln -s "$(realpath "$custom_release")" "$next_link"
mv -Tf "$next_link" "$standalone_dir/current"
echo "Selected CLI: $(readlink -f "$standalone_dir/current/bin/codex")"
echo "$version_json"
