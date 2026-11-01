#!/bin/bash
set -euo pipefail

destination="${1:-/usr/local/bin/sccache}"
version=0.19.0
archive="sccache-v${version}-x86_64-unknown-linux-musl.tar.gz"
checksum=aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

curl --fail --silent --show-error --location --retry 3 \
  "https://github.com/mozilla/sccache/releases/download/v${version}/${archive}" \
  --output "$scratch/$archive"
printf '%s  %s\n' "$checksum" "$scratch/$archive" | sha256sum --check --status
tar -xzf "$scratch/$archive" -C "$scratch" \
  "sccache-v${version}-x86_64-unknown-linux-musl/sccache"
install -m 0755 "$scratch/sccache-v${version}-x86_64-unknown-linux-musl/sccache" "$destination"
"$destination" --version
