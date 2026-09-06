#!/usr/bin/env bash
# Experimental static-musl big-endian QEMU validation, not production linking.
set -euo pipefail
: "${SEX_CROSS_TOOLCHAIN:?set the existing pinned Zig toolchain directory}"
: "${SEX_BIG_ENDIAN_WORK:?set an isolated build/cache directory}"
[[ ${SEX_QEMU_VALIDATION_ONLY:-0} == 1 ]] || {
  echo 'big-endian cross linking is currently qualified for QEMU only' >&2; exit 2;
}
export ZIG_GLOBAL_CACHE_DIR="$SEX_BIG_ENDIAN_WORK/zig-cache"
cross_args=()
for argument in "$@"; do
  # Same Zig limitation as the existing LE ARM validation toolchain.
  if [[ $argument != -Wl,--fix-cortex-a53-843419 ]]; then cross_args+=("$argument"); fi
done
exec "$SEX_CROSS_TOOLCHAIN/zig-x86_64-linux-0.15.2/zig" cc \
  -target aarch64_be-linux-musl -mcpu=baseline -static "${cross_args[@]}"
