#!/usr/bin/env bash
set -euo pipefail
: "${SEX_CROSS_TOOLCHAIN:?set SEX_CROSS_TOOLCHAIN to the bootstrapped directory}"
export ZIG_GLOBAL_CACHE_DIR="$SEX_CROSS_TOOLCHAIN/zig-cache"
# Rust 1.98 adds this hardware-erratum link flag; Zig 0.15.2 rejects it.
# Do not silently weaken an ARM hardware build. This escape hatch is ONLY
# for the explicitly declared QEMU validation executable, never distribution.
cross_args=()
for argument in "$@"; do
  if [[ $argument == -Wl,--fix-cortex-a53-843419 ]]; then
    if [[ ${SEX_QEMU_VALIDATION_ONLY:-0} != 1 ]]; then
      echo 'Zig cannot retain the Cortex-A53 erratum workaround; use a production cross toolchain, or explicitly set SEX_QEMU_VALIDATION_ONLY=1 for emulator-only tests' >&2
      exit 2
    fi
  else
    cross_args+=("$argument")
  fi
done
exec "$SEX_CROSS_TOOLCHAIN/zig-x86_64-linux-0.15.2/zig" cc \
  -target aarch64-linux-gnu.2.28 -mcpu=baseline \
  -L "$SEX_CROSS_TOOLCHAIN/sysroot/usr/lib/aarch64-linux-gnu" "${cross_args[@]}"
