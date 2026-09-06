#!/usr/bin/env bash
# Measurement-only four-engine campaign. Never claims identical reference filters.
set -euo pipefail
main() {
if [[ $# != 1 || -e $1 ]]; then
  echo 'usage: validate-spectral-references.sh NEW_RESULT_DIRECTORY' >&2
  exit 2
fi
for program in cargo ffmpeg cc sha256sum rg jq; do
  command -v "$program" >/dev/null || { echo "missing $program" >&2; exit 2; }
done
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
sox_bin=${SEX_SOX:-sox}
command -v "$sox_bin" >/dev/null || { echo 'set SEX_SOX to a working SoX binary or wrapper' >&2; exit 2; }
sox_bin=$(command -v "$sox_bin")
if [[ $sox_bin != /* ]]; then sox_bin="$PWD/$sox_bin"; fi
ffmpeg_bin=$(command -v ffmpeg)
preset=${SEX_SPECTRAL_PRESET:-sane}
case "$preset" in
  fast|sane|high|absurd|pointless|until-40k) ;;
  *) echo 'unknown SEX_SPECTRAL_PRESET' >&2; exit 2 ;;
esac
export SEX_SPECTRAL_PRESET="$preset"
default_suites=sweep
if [[ $preset == sane ]]; then default_suites='signals sweep'; fi
suites=${SEX_SPECTRAL_SUITES:-$default_suites}
for suite in $suites; do
  if [[ ( $suite == signals || $suite == phases ) && $preset != sane ]]; then
    echo 'the legacy multichannel signals suite is Sane-specific; use the preset-aware sweep suite' >&2
    exit 2
  fi
done
mkdir -- "$1"
work_dir=$(cd -- "$1" && pwd)
printf '%s\n' 'spectral-v5: Sane signals/full input-phase batches and/or preset-aware guarded sweep' > "$work_dir/fixture-version.txt"
source_fingerprint() {
  (cd -- "$1"; { printf '%s\0' Cargo.toml Cargo.lock; rg --files -0 src crates examples scripts; } \
    | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum)
}
source_before=$(source_fingerprint "$repo_dir")
printf '%s\n' "$source_before" > "$work_dir/source.sha256"
cargo build --quiet --locked --release --message-format=json --manifest-path "$repo_dir/Cargo.toml" --bin sex --example spectral_probe > "$work_dir/build.jsonl"
# Consume Cargo's actual artifacts, including CARGO_TARGET_DIR/configured targets;
# never assume target/release contains the binary just built.
artifact() {
  jq -ser --arg name "$1" --arg kind "$2" '
    [.[] | select(.reason == "compiler-artifact" and .target.name == $name
      and (.target.kind | index($kind)) and .executable != null) | .executable]
    | unique | if length == 1 then .[0] else error("expected one executable artifact") end
  ' "$work_dir/build.jsonl"
}
mkdir "$work_dir/tools" "$work_dir/source"
cp -- "$(artifact sex bin)" "$work_dir/tools/sex"
cp -- "$(artifact spectral_probe example)" "$work_dir/tools/spectral_probe"
cp -a -- "$repo_dir/src" "$repo_dir/crates" "$repo_dir/examples" "$repo_dir/scripts" \
  "$repo_dir/Cargo.toml" "$repo_dir/Cargo.lock" "$work_dir/source/"
sex_bin="$work_dir/tools/sex"
probe="$work_dir/tools/spectral_probe"
src_reference="$work_dir/tools/libsamplerate-reference"
cc -std=c11 -O2 -Wall -Wextra -Werror "$repo_dir/scripts/libsamplerate-reference.c" -ldl -lm -o "$src_reference"
[[ $source_before == "$(source_fingerprint "$repo_dir")" && $source_before == "$(source_fingerprint "$work_dir/source")" ]] || {
  echo 'source tree changed during build/snapshot preparation' >&2; exit 1;
}
{ "$sex_bin" --build-info; "$sox_bin" --version; "$ffmpeg_bin" -version; "$src_reference" --version; "$src_reference" --soxr-version; rustc -Vv; cc --version; jq --version; } > "$work_dir/versions.txt"
(cd -- "$work_dir"; sha256sum tools/sex tools/spectral_probe tools/libsamplerate-reference source/Cargo.lock) > "$work_dir/tools.sha256"
sha256sum "$sox_bin" "$ffmpeg_bin" > "$work_dir/reference-entries.sha256"
"$sex_bin" --build-info | rg -q 'endian=little' || { echo 'raw f32 reference adapter currently requires little endian' >&2; exit 2; }
if [[ -n ${SEX_SOX_RUNTIME:-} ]]; then
  sha256sum "$SEX_SOX_RUNTIME/sox.deb" "$SEX_SOX_RUNTIME/libsox3.deb" "$SEX_SOX_RUNTIME/usr/bin/sox" >> "$work_dir/reference-entries.sha256"
fi
echo "Snapshot ready: project executables and their source are preserved in $work_dir"
export SEX_COEFFICIENT_CACHE="$work_dir/coefficient-cache"
export LC_ALL=C
export OMP_NUM_THREADS=1
log_run() {
  local log=$1; shift
  printf '%q ' "$@" >> "$log"
  printf '\n' >> "$log"
  "$@" >> "$log" 2>&1
}

# Input/output rates are integer Hz; length is input_rate + 7 frames.
# Impulse channels cover all input phases when M <= 8, eight otherwise.
# The nonstandard 479/441 ratio intentionally exercises many distinct phases.
matrix=${SEX_SPECTRAL_RATIOS:-'48000:16000 44100:48000 48000:44100 44100:47900 48000:1000'}
[[ $matrix =~ [^[:space:]] && $suites =~ [^[:space:]] ]] || {
  echo 'spectral ratios and suites must select at least one case' >&2; exit 2;
}
printf 'ratios=%s\nsuites=%s\npreset=%s\nguard_quarters=%s\n' "$matrix" "$suites" "$preset" "${SEX_SPECTRAL_GUARD_QUARTERS:-auto}" > "$work_dir/matrix.txt"
for pair in $matrix; do
  from=${pair%:*}; to=${pair#*:}
  [[ $from =~ ^[0-9]+$ && $to =~ ^[0-9]+$ ]] || { echo 'invalid rate pair' >&2; exit 2; }
  for suite in $suites; do
  case "$suite" in
    signals) suffix='' ;;
    sweep) suffix='-sweep' ;;
    phases) suffix='-phases' ;;
    *) echo 'SEX_SPECTRAL_SUITES must contain signals, phases and/or sweep' >&2; exit 2 ;;
  esac
  phase_count=1
  if [[ $suite == phases ]]; then phase_count=$("$probe" input-phase-count "$from" "$to"); fi
  for ((phase_start=0; phase_start<phase_count; phase_start+=32)); do
  phase_options=()
  case_dir="$work_dir/$from-$to$suffix"
  if [[ $suite == phases ]]; then
    batch_count=$((phase_count-phase_start)); if (( batch_count > 32 )); then batch_count=32; fi
    phase_options=("$phase_start" "$batch_count")
    case_dir+="-$phase_start"
  fi
  mkdir "$case_dir"
  "$probe" "generate$suffix" "$case_dir/input.wav" "$from" "$to" "${phase_options[@]}" > "$case_dir/input-spec.tsv"
  read -r frames channels < "$case_dir/input-spec.tsv"
  sha256sum "$case_dir/input.wav" >> "$work_dir/inputs.sha256"
  preflight_taps=0
  preflight_hash=''
  if [[ $suite == sweep ]] && (( 10#$from != 10#$to )); then
    # Qualify/cache the actual bank BEFORE an expensive render. Feedback can
    # outgrow a fixture guard even if the initial planner length fitted it.
    log_run "$case_dir/design-preflight.log" "$sex_bin" analyze "$case_dir/input.wav" -r "$to" --preset "$preset"
    preflight_taps=$(awk '$1=="filter:" && $4=="phases" && $6 ~ /^[0-9]+$/ {n++; taps=$6} END {if(n!=1)exit 1; print taps}' "$case_dir/design-preflight.log")
    preflight_hash=$(awk '$1=="coefficient" && $2=="sha256:" && $3 ~ /^[0-9a-f]{64}$/ {n++; hash=$3} END {if(n!=1)exit 1; print hash}' "$case_dir/design-preflight.log")
    "$probe" check-sweep-guard "$from" "$to" "$preflight_taps" > "$case_dir/guard-preflight.tsv"
  fi
  log_run "$case_dir/sex.log" "$sex_bin" "$case_dir/input.wav" "$case_dir/sex.wav" \
    -r "$to" --preset "$preset" --bits 32 --dither none --clip saturate --block-frames 257
  if [[ $suite == phases ]] && (( 10#$from != 10#$to )); then
    actual_taps=$(awk '$1=="filter:" && $2 ~ /^[0-9]+$/ {n++; taps=$2} END {if(n!=1)exit 1; print taps}' "$case_dir/sex.log")
    (( (actual_taps-1)/2 <= 10#$from/2 )) || {
      echo 'accepted FIR exceeds the phase fixture half-second context' >&2; exit 1;
    }
  fi
  if [[ $suite == sweep ]]; then
    actual_taps=0
    if (( 10#$from != 10#$to )); then
      actual_taps=$(awk '$1=="filter:" && $2 ~ /^[0-9]+$/ {n++; taps=$2} END {if(n!=1)exit 1; print taps}' "$case_dir/sex.log")
      actual_hash=$(awk '$1=="filter:" && $NF ~ /^[0-9a-f]{64}$/ {n++; hash=$NF} END {if(n!=1)exit 1; print hash}' "$case_dir/sex.log")
      [[ $actual_taps == "$preflight_taps" && $actual_hash == "$preflight_hash" ]] || {
        echo 'rendered FIR differs from the preflight-qualified bank' >&2; exit 1;
      }
    fi
    "$probe" check-sweep-guard "$from" "$to" "$actual_taps" > "$case_dir/guard-check.tsv"
  fi
  log_run "$case_dir/sex-repeat.log" "$sex_bin" "$case_dir/input.wav" "$case_dir/sex-repeat.wav" \
    -r "$to" --preset "$preset" --bits 32 --dither none --clip saturate --block-frames 4096
  cmp "$case_dir/sex.wav" "$case_dir/sex-repeat.wav"
  if [[ $suite == sweep ]] && (( 10#$from != 10#$to )); then
    repeat_hash=$(awk '$1=="filter:" && $NF ~ /^[0-9a-f]{64}$/ {n++; hash=$NF} END {if(n!=1)exit 1; print hash}' "$case_dir/sex-repeat.log")
    [[ $repeat_hash == "$preflight_hash" ]] || { echo 'repeat FIR differs from preflight' >&2; exit 1; }
  fi
  log_run "$case_dir/sox.log" "$sox_bin" -R -D "$case_dir/input.wav" -b 32 -e signed-integer \
    "$case_dir/sox.wav" rate -v -L "$to"
  log_run "$case_dir/soxr.log" "$ffmpeg_bin" -hide_banner -loglevel error -nostdin -i "$case_dir/input.wav" \
    -af "aresample=$to:resampler=soxr:precision=33:dither_method=none" -c:a pcm_s32le "$case_dir/soxr.wav"
  log_run "$case_dir/samplerate.log" "$ffmpeg_bin" -hide_banner -loglevel error -nostdin -i "$case_dir/input.wav" \
    -f f32le -acodec pcm_f32le "$case_dir/input.f32"
  printf '%q ' "$src_reference" "$from" "$to" "$channels" "$frames" >> "$case_dir/samplerate.log"
  printf '< %q > %q\n' "$case_dir/input.f32" "$case_dir/output.f32" >> "$case_dir/samplerate.log"
  "$src_reference" "$from" "$to" "$channels" "$frames" \
    < "$case_dir/input.f32" > "$case_dir/output.f32" 2>> "$case_dir/samplerate.log"
  log_run "$case_dir/samplerate.log" "$ffmpeg_bin" -hide_banner -loglevel error -nostdin \
    -f f32le -ar "$to" -ac "$channels" -i "$case_dir/output.f32" \
    -af aresample=dither_method=none -c:a pcm_s32le "$case_dir/samplerate.wav"
  for engine in sex sox soxr samplerate; do
    options=()
    if [[ $engine == sex ]]; then options+=(strict); fi
    "$probe" "analyze$suffix" "$case_dir/$engine.wav" "$from" "$to" "${phase_options[@]}" "$case_dir/$engine-metrics" "${options[@]}" \
      | tee -a "$work_dir/results.txt"
    sha256sum "$case_dir/$engine.wav" >> "$work_dir/outputs.sha256"
  done
  if [[ $suite != phases ]]; then
  for engine in sox soxr samplerate; do
    "$probe" "compare$suffix" "$case_dir/sex.wav" "$case_dir/$engine.wav" "$from" "$to" "$case_dir/sex-vs-$engine"
  done
  fi
  done
  done
done
[[ $source_before == "$(source_fingerprint "$work_dir/source")" ]] || { echo 'preserved source snapshot changed during qualification' >&2; exit 1; }
(cd -- "$work_dir"; sha256sum --check --strict tools.sha256) > "$work_dir/snapshot-check.log"
sha256sum --check --strict "$work_dir/reference-entries.sha256" >> "$work_dir/snapshot-check.log"
echo "Spectral-reference campaign passed; all commands, signals, spectra, and caches retained in $work_dir"
}

# Parse the entire campaign before executing it. Later edits of this workspace
# script cannot replace commands that a long-running shell has yet to read.
{
  main "$@"
  exit
}
