#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"
host_target="$(rustc -vV | awk '/^host: / { print $2 }')"
target="${CODEX_TARGET:-$host_target}"
if [[ -z "$target" ]]; then
    echo "Could not determine the Rust host target; set CODEX_TARGET." >&2
    exit 1
fi

package_version="$(python3 - "$repo_root/codex-rs/Cargo.toml" <<'PY'
import pathlib
import sys
import tomllib

manifest = tomllib.loads(pathlib.Path(sys.argv[1]).read_text())
print(manifest["workspace"]["package"]["version"])
PY
)"
commit="$(git -C "$repo_root" rev-parse --short=12 HEAD)"
install_id="${commit}-$(date -u +%Y%m%dT%H%M%SZ)-$$"
package_dir="$repo_root/codex-rs/target/local-package-$install_id"
install_root="${CODEX_INSTALL_ROOT:-$HOME/.local/lib/codex-evo}"
bin_link="${CODEX_BIN_LINK:-$HOME/.local/bin/codex}"

mkdir -p -- "$install_root" "$(dirname -- "$bin_link")"
install_root="$(cd -- "$install_root" && pwd -P)"
bin_link="$(cd -- "$(dirname -- "$bin_link")" && pwd -P)/$(basename -- "$bin_link")"

if [[ -e "$bin_link" || -L "$bin_link" ]]; then
    if [[ ! -L "$bin_link" ]]; then
        echo "Refusing to replace non-symlink Codex executable: $bin_link" >&2
        exit 1
    fi
    current_binary="$(realpath -- "$bin_link")"
    case "$current_binary" in
        "$install_root"/*) ;;
        *)
            echo "Refusing to replace Codex symlink outside $install_root: $current_binary" >&2
            exit 1
            ;;
    esac
    current_package="$(dirname -- "$(dirname -- "$current_binary")")"
fi

build_args=(
    --target "$target"
    --variant codex
    --cargo-profile release
    --package-version "$package_version"
    --package-dir "$package_dir"
)
if [[ -n "${CODEX_BWRAP_BIN:-}" ]]; then
    build_args+=(--bwrap-bin "$CODEX_BWRAP_BIN")
elif [[ "$target" == "$host_target" && -x "${current_package:-}/codex-resources/bwrap" ]]; then
    build_args+=(--bwrap-bin "$current_package/codex-resources/bwrap")
fi
if [[ -n "${CODEX_RG_BIN:-}" ]]; then
    build_args+=(--rg-bin "$CODEX_RG_BIN")
elif [[ "$target" == "$host_target" && -x "${current_package:-}/codex-path/rg" ]]; then
    build_args+=(--rg-bin "$current_package/codex-path/rg")
fi

CODEX_REPO_ROOT="$repo_root" \
    python3 "$repo_root/scripts/build_codex_package.py" "${build_args[@]}"

staging="$(mktemp -d "$install_root/.codex-stage-$install_id.XXXXXX")"
link_tmp="$bin_link.tmp.$$"
cleanup() {
    if [[ -n "${staging:-}" && -d "$staging" ]]; then
        rm -rf -- "$staging"
    fi
    if [[ -n "${link_tmp:-}" && -L "$link_tmp" ]]; then
        rm -f -- "$link_tmp"
    fi
}
trap cleanup EXIT

cp -a -- "$package_dir/." "$staging/"
expected_version="codex-cli $package_version"
actual_version="$("$staging/bin/codex" --version)"
if [[ "$actual_version" != "$expected_version" ]]; then
    echo "Built Codex version mismatch: expected '$expected_version', got '$actual_version'." >&2
    exit 1
fi
"$staging/bin/codex" login github-copilot --help >/dev/null
python3 - "$staging/codex-package.json" "$package_version" "$target" <<'PY'
import json
import sys
from pathlib import Path

manifest = json.loads(Path(sys.argv[1]).read_text())
expected = {"version": sys.argv[2], "target": sys.argv[3], "variant": "codex"}
for key, value in expected.items():
    if manifest.get(key) != value:
        raise SystemExit(f"Package {key} mismatch: {manifest.get(key)!r} != {value!r}")
PY

install_dir="$install_root/$install_id"
if [[ -e "$install_dir" ]]; then
    echo "Install destination already exists: $install_dir" >&2
    exit 1
fi
mv -- "$staging" "$install_dir"
staging=""

ln -s -- "$install_dir/bin/codex" "$link_tmp"
mv -Tf -- "$link_tmp" "$bin_link"
link_tmp=""
echo "Installed $actual_version from $commit at $install_dir"
echo "Active command: $bin_link"
