#!/usr/bin/env bash
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
exec "$repo_dir/scripts/cross/aarch64-run.sh" \
  "$repo_dir/target/aarch64-unknown-linux-gnu/release/sex-rate" "$@"
