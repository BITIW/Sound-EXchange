#!/usr/bin/env bash
# Compare independently executable SeX builds. SECOND may be a QEMU/SSH wrapper.
set -euo pipefail
if [[ $# != 3 || ! -x $1 || ! -x $2 || -e $3 ]]; then
  echo 'usage: validate-cross-architecture.sh FIRST_SEX SECOND_SEX_OR_WRAPPER NEW_RESULT_DIRECTORY' >&2
  exit 2
fi
if [[ ${SEX_REPRO_CODECS_ONLY:-0} == 1 && ${SEX_REPRO_OPTIMIZED_ONLY:-0} == 1 ]]; then
  echo 'CODECS_ONLY and OPTIMIZED_ONLY select different corpora; choose one' >&2
  exit 2
fi
if [[ ${SEX_REPRO_CODECS_ONLY:-0} == 1 ]]; then
  # The fixture generator uses CODECS to enable tagged container inputs too.
  export SEX_REPRO_CODECS=1
fi
first=$(realpath -- "$1")
second=$(realpath -- "$2")
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -- "$3"
work_dir=$(cd -- "$3" && pwd)
source_fingerprint() {
  (cd -- "$repo_dir"; { printf '%s\0' Cargo.toml Cargo.lock; rg --files -0 src crates examples scripts; } \
    | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum)
}
source_before=$(source_fingerprint)
printf '%s\n' "$source_before" > "$work_dir/source.sha256"
cargo build --locked --quiet --release --manifest-path "$repo_dir/Cargo.toml" --example repro_fixtures
"$repo_dir/target/release/examples/repro_fixtures" "$work_dir/inputs"
sha256sum "$work_dir"/inputs/* > "$work_dir/inputs.sha256"
mkdir "$work_dir/first-cache" "$work_dir/second-cache"
feedback_options=()
if [[ ${SEX_REPRO_FEEDBACK:-0} == 1 ]]; then feedback_options+=(--refine-quality); fi
"$first" --build-info | tee "$work_dir/first-build.txt"
"$second" --build-info | tee "$work_dir/second-build.txt"
first_arch=$(sed -n 's/.*arch=\([^;]*\);.*/\1/p' "$work_dir/first-build.txt")
second_arch=$(sed -n 's/.*arch=\([^;]*\);.*/\1/p' "$work_dir/second-build.txt")
if [[ -z $first_arch || -z $second_arch || $first_arch == "$second_arch" ]]; then
  echo 'qualification requires identifiable, different executable architectures' >&2
  exit 1
fi
rustc -Vv > "$work_dir/rustc.txt"
if [[ -n ${SEX_CROSS_TOOLCHAIN:-} ]]; then
  { printf 'CPU model: %s\n' "${SEX_QEMU_CPU:-cortex-a53}";
    "${SEX_QEMU_AARCH64:-qemu-aarch64-static}" --version;
    "$SEX_CROSS_TOOLCHAIN/zig-x86_64-linux-0.15.2/zig" version;
  } > "$work_dir/emulator.txt"
fi
sha256sum "$repo_dir/Cargo.lock" > "$work_dir/lock.sha256"

run_case() {
  local name=$1 input=$2
  local extension=${SEX_REPRO_EXTENSION:-wav}
  shift 2
  local side program cache block out log
  for side in first second cross reverse; do
    case $side in
      first) program=$first; cache="$work_dir/first-cache"; block=1 ;;
      second) program=$second; cache="$work_dir/second-cache"; block=7 ;;
      cross) program=$second; cache="$work_dir/first-cache"; block=4096 ;;
      reverse) program=$first; cache="$work_dir/second-cache"; block=4096 ;;
    esac
    out="$work_dir/$name-$side.$extension"
    log="$work_dir/$name-$side.log"
    printf 'SEX_COEFFICIENT_CACHE=%q ' "$cache" > "$log"
    printf '%q ' "$program" "$work_dir/inputs/$input" "$out" --block-frames "$block" --seed 42 "${feedback_options[@]}" "$@" >> "$log"
    printf '\n' >> "$log"
    SEX_COEFFICIENT_CACHE="$cache" "$program" "$work_dir/inputs/$input" "$out" \
      --block-frames "$block" --seed 42 "${feedback_options[@]}" "$@" >> "$log" 2>&1 || { cat "$log" >&2; exit 1; }
    sed -n '/^qualified FIR sha256: /,/^FIR qualification scope: /p' "$log" > "$work_dir/$name-$side.qualification"
    if rg -q 'coefficients sha256' "$log" && [[ ! -s $work_dir/$name-$side.qualification ]]; then
      echo "$name: conversion omitted its bank-bound qualification report" >&2
      exit 1
    fi
    if [[ $side == cross || $side == reverse ]] && rg -q 'coefficients sha256' "$log"; then
      rg -q 'coefficient cache: hit' "$log" || { echo "$name: cross-architecture cache miss" >&2; exit 1; }
    fi
  done
  cmp "$work_dir/$name-first.$extension" "$work_dir/$name-second.$extension"
  cmp "$work_dir/$name-first.$extension" "$work_dir/$name-cross.$extension"
  cmp "$work_dir/$name-first.$extension" "$work_dir/$name-reverse.$extension"
  for side in second cross reverse; do
    cmp "$work_dir/$name-first.qualification" "$work_dir/$name-$side.qualification"
  done
  # Compare coefficient identity AND the complete cache files, not only PCM.
  local a b
  a=$(sed -n 's/.*coefficients sha256 //p' "$work_dir/$name-first.log")
  b=$(sed -n 's/.*coefficients sha256 //p' "$work_dir/$name-second.log")
  [[ $a == "$b" ]] || { echo "$name: coefficient identities differ" >&2; exit 1; }
  printf '%s\t%s\t%s\n' "$name" "$(sha256sum "$work_dir/$name-first.$extension" | cut -d ' ' -f1)" "${a:-no-filter}" | tee -a "$work_dir/results.tsv"
}

# Apply identical gate/effect options to analysis and actual PCM conversion.
run_checked_case() {
  local name=$1 input=$2 side program cache log
  shift 2
  run_case "$name" "$input" "$@"
  for side in first second; do
    local analysis_prefix=(analyze)
    if [[ $side == first ]]; then
      program=$first
      if [[ -n ${SEX_REPRO_FIRST_ANALYZER:-} ]]; then program=$(realpath -- "$SEX_REPRO_FIRST_ANALYZER"); analysis_prefix=(); fi
    else
      program=$second
      if [[ -n ${SEX_REPRO_SECOND_ANALYZER:-} ]]; then program=$(realpath -- "$SEX_REPRO_SECOND_ANALYZER"); analysis_prefix=(); fi
    fi
    cache="$work_dir/$side-cache"
    log="$work_dir/$name-$side-analysis.log"
    printf 'SEX_COEFFICIENT_CACHE=%q ' "$cache" > "$log"
    printf '%q ' "$program" "${analysis_prefix[@]}" "$work_dir/inputs/$input" --seed 42 "$@" >> "$log"
    printf '\n' >> "$log"
    SEX_COEFFICIENT_CACHE="$cache" "$program" "${analysis_prefix[@]}" "$work_dir/inputs/$input" \
      --seed 42 "$@" >> "$log" 2>&1 || { cat "$log" >&2; exit 1; }
    sed -n '/^qualified FIR sha256: /,/^FIR qualification scope: /p' "$log" > "$work_dir/$name-$side-analysis.qualification"
    cmp "$work_dir/$name-first.qualification" "$work_dir/$name-$side-analysis.qualification"
  done
}

if [[ ${SEX_REPRO_OPTIMIZED_ONLY:-0} != 1 ]]; then
if [[ ${SEX_REPRO_CODECS_ONLY:-0} != 1 ]]; then
run_case copy stereo24.wav --dither none --clip error
run_case empty empty24.wav -r 48000 --preset fast --dither none --clip error
run_case one one16.wav -r 16000 --preset fast --dither none --clip error
run_case one-up one16.wav -r 48000 --preset fast --dither none --clip error
run_case fast stereo24.wav -r 48000 --preset fast --bits 16 --clip normalize
run_case sane eight32.wav -r 16000 --preset sane --bits 24 --clip normalize
run_case high stereo24.wav -r 48000 --preset high --bits 16 --clip normalize
run_case absurd mono24.wav -r 22050 --preset absurd --bits 24 --clip normalize
run_case pointless mono24.wav -r 22050 --preset pointless --bits 24 --clip normalize
if [[ ${SEX_REPRO_UNTIL:-0} == 1 ]]; then
  run_case until tiny24.wav -r 24000 --preset until-40k --bits 24 --clip normalize
fi
run_case wide-chain stereo24.wav -r 48000 --preset fast --signal-precision 4096 \
  --gain 4 --dc-remove 65535/65536 --mix '1/2,1/2' --convolve '1,1/2,-1/4' \
  --bits 16 --dither noise-shaped-9 --clip normalize
run_case refined stereo24.wav -r 48000 --preset fast --gain 1152921504606846976 \
  --bits 24 --dither high-pass-tpdf --clip normalize
fi
if [[ ${SEX_REPRO_CODECS:-0} == 1 || ${SEX_REPRO_CODECS_ONLY:-0} == 1 ]]; then
  for format in wav aiff flac; do
    SEX_REPRO_EXTENSION=$format run_case "tagged-$format" "tagged.$format" \
      -r 48000 --preset high --bits 16 --clip normalize
  done
fi
if [[ ${SEX_REPRO_SHARED_GATES:-0} == 1 ]]; then
  run_checked_case certified-native stereo24.wav -r 66150 --preset sane --grid 9 \
    --certify --harmonics --dither none --clip normalize
  run_checked_case certified-high mono24.wav -r 22050 --preset high --grid 17 \
    --certify --certificate-taps 2049 --harmonics --dither none --clip normalize
  run_checked_case certified-amplified mono24.wav -r 14700 --preset fast --grid 9 \
    --gain 1152921504606846976 --certify --harmonics --dither none --clip normalize
fi
if [[ ${SEX_REPRO_SHARED_UNTIL:-0} == 1 ]]; then
  run_checked_case harmonic-until-up2 tiny24.wav -r 96000 --preset until-40k --grid 9 \
    --harmonics --dither none --clip normalize
fi
if [[ ${SEX_REPRO_WINDOWS:-0} == 1 ]]; then
  run_checked_case window-hann stereo24.wav -r 66150 --preset fast --window hann --grid 9 \
    --certify --harmonics --dither none --clip normalize
  run_checked_case window-blackman-amplified mono24.wav -r 14700 --preset fast --window blackman --grid 9 \
    --gain 1152921504606846976 --certify --harmonics --dither none --clip normalize
  run_checked_case window-dolph-high mono24.wav -r 22050 --preset high --window dolph-chebyshev --grid 9 \
    --certify --certificate-taps 4097 --harmonics --dither none --clip normalize
  run_checked_case window-dolph-fractional stereo24.wav -r 66150 --preset fast --window dolph-chebyshev --grid 9 \
    --certify --harmonics --dither none --clip normalize
fi
fi
if [[ ${SEX_REPRO_OPTIMIZED:-0} == 1 || ${SEX_REPRO_OPTIMIZED_ONLY:-0} == 1 ]]; then
  run_checked_case optimized-ls stereo24.wav -r 29400 --preset fast --designer global-ls --grid 9 \
    --certify --harmonics --dither none --clip normalize
  run_checked_case optimized-remez stereo24.wav -r 29400 --preset fast --designer remez --grid 9 \
    --certify --harmonics --dither none --clip normalize
  run_checked_case optimized-ls-amplified mono24.wav -r 14700 --preset fast --designer global-ls --grid 9 \
    --gain 1152921504606846976 --certify --harmonics --dither none --clip normalize
fi
diff -r "$work_dir/first-cache" "$work_dir/second-cache"
[[ $source_before == "$(source_fingerprint)" ]] || { echo 'source tree changed during qualification' >&2; exit 1; }
echo "Cross-architecture corpus passed; output/cache files and commands retained in $work_dir"
