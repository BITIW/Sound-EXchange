#!/usr/bin/env bash
set -euo pipefail
exec "${SEX_QEMU_AARCH64_BE:-qemu-aarch64_be-static}" -cpu "${SEX_QEMU_CPU:-cortex-a53}" "$@"
