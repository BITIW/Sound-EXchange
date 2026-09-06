#!/usr/bin/env bash
# Validation-only SoX, extracted locally; no package installation or maintainer scripts.
set -euo pipefail
if [[ $# != 1 || ! -d $1 || -L $1 || -n $(ls -A -- "$1") ]]; then
  echo 'usage: bootstrap-sox-reference.sh EXISTING_EMPTY_DIRECTORY' >&2
  exit 2
fi
[[ $(uname -m) == x86_64 ]] || { echo 'this pinned runtime is Linux x86-64 only' >&2; exit 2; }
reference_dir=$(cd -- "$1" && pwd)
# SHA-256 from packages.debian.org/trixie/amd64/{sox,libsox3}/download.
for package in sox libsox3; do
  case $package in
    sox) expected=918cef8e8675b2de562af0266b2dab1b9c4609c24590bcdfe8e39ce7e8e8aacb ;;
    libsox3) expected=063e0162c4c1998b992e5807191ea6c010f7747238d2c31a4945853f6a0b4701 ;;
  esac
  archive="$reference_dir/$package.deb"
  curl --fail --location --silent --show-error --retry 2 --proto '=https' \
    "https://deb.debian.org/debian/pool/main/s/sox/${package}_14.4.2+git20190427-5+b3_amd64.deb" -o "$archive"
  printf '%s  %s\n' "$expected" "$archive" | sha256sum --check --strict
  member=$(ar t "$archive" | awk '/^data\.tar\./ { print; exit }')
  [[ -n $member ]] || exit 1
  ar p "$archive" "$member" | tar -xJf - -C "$reference_dir" --no-same-owner --no-same-permissions
done
LD_LIBRARY_PATH="$reference_dir/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  "$reference_dir/usr/bin/sox" --version
printf 'Use SEX_SOX_RUNTIME=%q with scripts/sox-reference.sh\n' "$reference_dir"
