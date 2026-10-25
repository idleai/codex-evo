#!/bin/bash
set -euo pipefail

exec timeout 180s python3 /usr/local/lib/idle-ci-runner/maintain_cache.py
