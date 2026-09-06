#!/usr/bin/env bash
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
exec "$repo_dir/scripts/cross/aarch64_be-run.sh" \
  "$repo_dir/target/aarch64_be-unknown-linux-musl/release/sex" "$@"
