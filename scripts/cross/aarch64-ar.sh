#!/usr/bin/env bash
set -euo pipefail
: "${SEX_CROSS_TOOLCHAIN:?set SEX_CROSS_TOOLCHAIN to the bootstrapped directory}"
exec "$SEX_CROSS_TOOLCHAIN/zig-x86_64-linux-0.15.2/zig" ar "$@"
