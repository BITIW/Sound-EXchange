#!/usr/bin/env bash
# Isolated, pinned validation toolchain; never installs system packages.
set -euo pipefail
if [[ $# != 1 || ! -d $1 || -L $1 || -n $(ls -A -- "$1") ]]; then
  echo 'usage: bootstrap-aarch64.sh EXISTING_EMPTY_DIRECTORY' >&2
  exit 2
fi
cross_dir=$(cd -- "$1" && pwd)
for program in curl sha256sum ar tar; do
  command -v "$program" >/dev/null || { echo "missing $program" >&2; exit 2; }
done
fetch() {
  local url=$1 file=$2 expected=$3
  curl --fail --location --retry 2 --proto '=https' --tlsv1.2 "$url" -o "$cross_dir/$file"
  printf '%s  %s\n' "$expected" "$cross_dir/$file" | sha256sum --check --strict
}
# Hash sources: https://ziglang.org/download/index.json and Debian's
# packages.debian.org/trixie/arm64/{libc6,libgcc-s1}/download pages.
fetch https://ziglang.org/download/0.15.2/zig-x86_64-linux-0.15.2.tar.xz zig.tar.xz \
  02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239
fetch https://deb.debian.org/debian/pool/main/g/glibc/libc6_2.41-12+deb13u3_arm64.deb libc6.deb \
  ff529924782d3286181188fc265a6a92e7fe28975fb3a925dc0e05c0ca66e52f
fetch https://deb.debian.org/debian/pool/main/g/gcc-14/libgcc-s1_14.2.0-19_arm64.deb libgcc.deb \
  1108bc87879833d6d9a145f22a4a15cddb34e065b4b5f4b97bee586adbac2851
tar -xJf "$cross_dir/zig.tar.xz" -C "$cross_dir" --no-same-owner --no-same-permissions
mkdir "$cross_dir/sysroot"
for archive in libc6.deb libgcc.deb; do
  member=$(ar t "$cross_dir/$archive" | awk '/^data\.tar\./ { print; exit }')
  [[ -n $member ]] || { echo "missing package data: $archive" >&2; exit 1; }
  ar p "$cross_dir/$archive" "$member" | tar -xJf - -C "$cross_dir/sysroot" --no-same-owner --no-same-permissions
done
ln -s usr/lib "$cross_dir/sysroot/lib"
"$cross_dir/zig-x86_64-linux-0.15.2/zig" version
printf 'Isolated AArch64 toolchain: %s\n' "$cross_dir"
