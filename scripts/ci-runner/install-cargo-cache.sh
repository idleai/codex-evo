#!/bin/bash
set -euo pipefail

scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
state="$HOME/.local/share/codex-ci-runner"
units="$HOME/.config/systemd/user"
version=2.11.7
archive="caddy_${version}_linux_amd64.tar.gz"
checksum=727b91701a392de6ebc5027509f548bf39979e5216340d0faed8fa5e69c84f8b
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

test -x "$state/bin/bazel-remote"
install -d -m 0700 /mnt/hot/ci-cache/cargo
mkdir -p "$state/bin" "$units"
curl --fail --silent --show-error --location --retry 3 \
  "https://github.com/caddyserver/caddy/releases/download/v${version}/${archive}" \
  --output "$scratch/$archive"
printf '%s  %s\n' "$checksum" "$scratch/$archive" | sha256sum --check --status
tar -xzf "$scratch/$archive" -C "$scratch" caddy
"$scratch/caddy" validate --config "$scripts/cargo-cache.Caddyfile" --adapter caddyfile
install -m 0755 "$scratch/caddy" "$state/bin/caddy"
install -m 0644 "$scripts/cargo-cache.Caddyfile" "$state/cargo-cache.Caddyfile"
install -m 0644 "$scripts/idle-ci-cargo-store.service" "$scripts/idle-ci-cargo-cache.service" "$units/"
systemctl --user daemon-reload
systemctl --user enable idle-ci-cargo-cache.service
systemctl --user restart idle-ci-cargo-store.service idle-ci-cargo-cache.service
