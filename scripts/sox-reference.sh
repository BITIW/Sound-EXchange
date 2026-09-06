#!/usr/bin/env bash
set -euo pipefail
: "${SEX_SOX_RUNTIME:?set SEX_SOX_RUNTIME to the extracted SoX reference directory}"
export LD_LIBRARY_PATH="$SEX_SOX_RUNTIME/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$SEX_SOX_RUNTIME/usr/bin/sox" "$@"
