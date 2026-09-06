#!/usr/bin/env bash
set -euo pipefail
: "${SEX_CROSS_TOOLCHAIN:?set SEX_CROSS_TOOLCHAIN to the bootstrapped directory}"
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
export CC="$repo_dir/scripts/cross/aarch64-cc.sh"
export AR="$repo_dir/scripts/cross/aarch64-ar.sh"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$CC"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="$repo_dir/scripts/cross/aarch64-run.sh"
export GMP_MPFR_SYS_CACHE="$SEX_CROSS_TOOLCHAIN/gmp-cache"
cross_cargo_args=()
cross_extra_args=()
after_separator=false
for argument in "$@"; do
  if $after_separator; then
    cross_extra_args+=("$argument")
  elif [[ $argument == -- ]]; then
    after_separator=true
    cross_extra_args+=(--)
  else
    cross_cargo_args+=("$argument")
  fi
done
cargo "${cross_cargo_args[@]}" --manifest-path "$repo_dir/Cargo.toml" \
  --target aarch64-unknown-linux-gnu --features cross-gmp "${cross_extra_args[@]}"
