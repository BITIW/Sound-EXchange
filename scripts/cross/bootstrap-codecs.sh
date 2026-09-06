#!/usr/bin/env bash
# Optional integer-codec validation runtime. The manifest hashes were read
# from https://deb.debian.org/debian/dists/trixie/main/binary-arm64/Packages.xz.
set -euo pipefail
: "${SEX_CROSS_TOOLCHAIN:?set SEX_CROSS_TOOLCHAIN to the bootstrapped directory}"
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
[[ -d $SEX_CROSS_TOOLCHAIN/sysroot/usr/lib/aarch64-linux-gnu ]] || exit 2
mkdir "$SEX_CROSS_TOOLCHAIN/codec-packages"
while read -r expected relative; do
  archive="$SEX_CROSS_TOOLCHAIN/codec-packages/${relative##*/}"
  curl --fail --location --silent --show-error --retry 2 --proto '=https' \
    "https://deb.debian.org/debian/$relative" -o "$archive"
  printf '%s  %s\n' "$expected" "$archive" | sha256sum --check --strict
  member=$(ar t "$archive" | awk '/^data\.tar\./ { print; exit }')
  [[ -n $member ]] || exit 1
  ar p "$archive" "$member" | tar -xJf - -C "$SEX_CROSS_TOOLCHAIN/sysroot" --no-same-owner --no-same-permissions
done < "$script_dir/arm64-codecs.sha256"
