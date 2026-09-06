#!/usr/bin/env bash
# Bounded all-image response campaign. SECOND may be an AArch64/QEMU wrapper.
set -euo pipefail
if [[ $# -lt 2 || $# -gt 3 || ! -x $1 || -e $2 || ( $# == 3 && ! -x $3 ) ]]; then
  echo 'usage: validate-harmonic-responses.sh FIRST_SEX NEW_RESULT_DIRECTORY [SECOND_SEX]' >&2
  exit 2
fi
first=$(realpath -- "$1")
second=${3:-}
if [[ -n $second ]]; then second=$(realpath -- "$second"); fi
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -- "$2"
work_dir=$(cd -- "$2" && pwd)
source_fingerprint() {
  (cd -- "$repo_dir"; { printf '%s\0' Cargo.toml Cargo.lock; rg --files -0 src crates examples scripts; } \
    | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum)
}
source_before=$(source_fingerprint)
printf '%s\n' "$source_before" > "$work_dir/source.sha256"
cargo build --locked --quiet --release --manifest-path "$repo_dir/Cargo.toml" --example repro_fixtures
"$repo_dir/target/release/examples/repro_fixtures" "$work_dir/inputs"
sha256sum "$work_dir"/inputs/* > "$work_dir/inputs.sha256"
"$first" --build-info > "$work_dir/first-build.txt"
if [[ -n $second ]]; then "$second" --build-info > "$work_dir/second-build.txt"; fi
mkdir "$work_dir/first-cache" "$work_dir/second-cache"
run_case() {
  local name=$1 input=$2 rate=$3 preset=$4 grid=${SEX_HARMONIC_GRID:-65}
  local side program log cache
  for side in first second; do
    if [[ $side == first ]]; then program=$first; else program=$second; fi
    if [[ -z $program ]]; then continue; fi
    log="$work_dir/$name-$side.log"
    cache="$work_dir/$side-cache"
    { printf 'SEX_COEFFICIENT_CACHE=%q ' "$cache";
      printf '%q ' "$program" analyze "$work_dir/inputs/$input" -r "$rate" --preset "$preset" \
        --harmonics --grid "$grid" --harmonic-work 500000000;
      printf '\n';
    } > "$work_dir/$name-$side.command"
    SEX_COEFFICIENT_CACHE="$cache" "$program" analyze "$work_dir/inputs/$input" \
      -r "$rate" --preset "$preset" --harmonics --grid "$grid" --harmonic-work 500000000 \
      > "$log" 2>&1 || { cat "$log" >&2; exit 1; }
    rg -q 'main-complex-error=true, images=true, image-L2=true, stopband=true' "$log"
  done
  if [[ -n $second ]]; then cmp "$work_dir/$name-first.log" "$work_dir/$name-second.log"; fi
  printf '%s\t%s\t%s\n' "$name" \
    "$(sha256sum "$work_dir/$name-first.log" | cut -d ' ' -f1)" \
    "$(sed -n 's/^coefficient sha256: //p' "$work_dir/$name-first.log")" | tee -a "$work_dir/results.tsv"
}
run_case sane-down3 eight32.wav 16000 sane
run_case sane-up160 stereo24.wav 48000 sane
run_case sane-down147 eight32.wav 44100 sane
run_case sane-up479 stereo24.wav 47900 sane
run_case sane-down48 eight32.wav 1000 sane
run_case high-up160 stereo24.wav 48000 high
run_case high-down2 eight32.wav 24000 high
if [[ ${SEX_HARMONIC_UNTIL:-0} == 1 ]]; then
  run_case until-up2 tiny24.wav 96000 until-40k
fi
if [[ -n $second ]]; then diff -r "$work_dir/first-cache" "$work_dir/second-cache"; fi
[[ $source_before == "$(source_fingerprint)" ]] || { echo 'source tree changed during qualification' >&2; exit 1; }
echo "Sampled harmonic campaign passed; complete logs and coefficient caches retained in $work_dir"
