#!/usr/bin/env bash
set -euo pipefail
: "${SEX_CROSS_TOOLCHAIN:?set SEX_CROSS_TOOLCHAIN to the bootstrapped directory}"
exec "${SEX_QEMU_AARCH64:-qemu-aarch64-static}" \
  -L "$SEX_CROSS_TOOLCHAIN/sysroot" -cpu "${SEX_QEMU_CPU:-cortex-a53}" "$@"
