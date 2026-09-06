#!/usr/bin/env bash
# Tier-3 target: compile the installed Rust standard-library source.
# RUSTC_BOOTSTRAP is scoped to this experimental build, never an app setting.
set -euo pipefail
: "${SEX_CROSS_TOOLCHAIN:?set the existing pinned Zig toolchain directory}"
: "${SEX_BIG_ENDIAN_WORK:?set an isolated build/cache directory}"
[[ ${SEX_QEMU_VALIDATION_ONLY:-0} == 1 ]] || { echo 'set SEX_QEMU_VALIDATION_ONLY=1 for this experimental target' >&2; exit 2; }
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
export CC="$repo_dir/scripts/cross/aarch64_be-cc.sh"
export AR="$repo_dir/scripts/cross/aarch64-ar.sh"
export CARGO_TARGET_AARCH64_BE_UNKNOWN_LINUX_MUSL_LINKER="$CC"
export CARGO_TARGET_AARCH64_BE_UNKNOWN_LINUX_MUSL_RUNNER="$repo_dir/scripts/cross/aarch64_be-run.sh"
export CARGO_TARGET_AARCH64_BE_UNKNOWN_LINUX_MUSL_RUSTFLAGS='-Clink-self-contained=no -Ctarget-feature=+crt-static'
export GMP_MPFR_SYS_CACHE="$SEX_BIG_ENDIAN_WORK/gmp-cache"
export CARGO_HOME="$SEX_BIG_ENDIAN_WORK/cargo-home"
export RUSTC_BOOTSTRAP=1
cross_cargo_args=()
cross_extra_args=()
after_separator=false
for argument in "$@"; do
  if $after_separator; then cross_extra_args+=("$argument")
  elif [[ $argument == -- ]]; then after_separator=true; cross_extra_args+=(--)
  else cross_cargo_args+=("$argument")
  fi
done
# Keep -Z after the subcommand so external subcommands (Clippy) forward it.
cargo "${cross_cargo_args[@]}" -Z build-std=std,panic_unwind,test --manifest-path "$repo_dir/Cargo.toml" \
  --target aarch64_be-unknown-linux-musl "${cross_extra_args[@]}"
