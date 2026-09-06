#!/usr/bin/env bash
set -euo pipefail

repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/sex-reference-validation.XXXXXX")
trap 'rm -rf -- "$work_dir"' EXIT

for program in cargo ffmpeg ffprobe awk cmp cc; do
  if ! command -v "$program" >/dev/null 2>&1; then
    echo "missing required program: $program" >&2
    exit 2
  fi
done

cargo build --quiet --release --manifest-path "$repo_dir/Cargo.toml"
sex_bin="$repo_dir/target/release/sex"
src_reference="$work_dir/libsamplerate-reference"
cc -std=c11 -O2 -Wall -Wextra -Werror \
  "$repo_dir/scripts/libsamplerate-reference.c" -ldl -lm -o "$src_reference"
export SEX_COEFFICIENT_CACHE=off

make_tone() {
  local frequency=$1
  local output=$2
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "sine=frequency=${frequency}:sample_rate=48000:duration=1" \
    -c:a pcm_s24le "$output"
}

resample_soxr() {
  local input=$1
  local output=$2
  ffmpeg -hide_banner -loglevel error -y -i "$input" \
    -af "aresample=16000:resampler=soxr:precision=33" \
    -ar 16000 -c:a pcm_s24le "$output"
}

resample_libsamplerate() {
  local input=$1
  local output=$2
  ffmpeg -hide_banner -loglevel error -i "$input" \
    -f f32le -acodec pcm_f32le - \
    | "$src_reference" 48000 16000 1 48000 \
    | ffmpeg -hide_banner -loglevel error -y \
      -f f32le -ar 16000 -ac 1 -i - -c:a pcm_s24le "$output"
}

rms_db() {
  local input=$1
  local log
  log=$(ffmpeg -hide_banner -nostats -i "$input" \
    -af "atrim=start_sample=800:end_sample=15200,astats=metadata=0:reset=0" \
    -c:a pcm_s32le -f null - 2>&1)
  awk -F ': ' '/RMS level dB:/ { print $2; exit }' <<<"$log"
}

frame_count() {
  ffprobe -v error -select_streams a:0 \
    -show_entries stream=duration_ts -of default=nw=1:nk=1 "$1"
}

assert_at_most() {
  local value=$1
  local maximum=$2
  local label=$3
  if ! awk -v value="$value" -v maximum="$maximum" \
    'BEGIN { exit !(value <= maximum) }'; then
    echo "$label: $value exceeds $maximum" >&2
    exit 1
  fi
}

assert_close() {
  local left=$1
  local right=$2
  local tolerance=$3
  local label=$4
  if ! awk -v left="$left" -v right="$right" -v tolerance="$tolerance" \
    'BEGIN { difference = left - right; if (difference < 0) difference = -difference; exit !(difference <= tolerance) }'; then
    echo "$label: $left versus $right exceeds tolerance $tolerance" >&2
    exit 1
  fi
}

stop_input="$work_dir/stop-11731.wav"
pass_input="$work_dir/pass-1000.wav"
make_tone 11731 "$stop_input"
make_tone 1000 "$pass_input"

"$sex_bin" "$stop_input" -r 16000 "$work_dir/sex-stop-a.wav" >/dev/null
"$sex_bin" "$stop_input" -r 16000 "$work_dir/sex-stop-b.wav" >/dev/null
"$sex_bin" "$pass_input" -r 16000 "$work_dir/sex-pass.wav" >/dev/null
cmp --silent "$work_dir/sex-stop-a.wav" "$work_dir/sex-stop-b.wav"

resample_soxr "$stop_input" "$work_dir/soxr-stop.wav"
resample_soxr "$pass_input" "$work_dir/soxr-pass.wav"
resample_libsamplerate "$stop_input" "$work_dir/samplerate-stop.wav"
resample_libsamplerate "$pass_input" "$work_dir/samplerate-pass.wav"

sex_stop_rms=$(rms_db "$work_dir/sex-stop-a.wav")
soxr_stop_rms=$(rms_db "$work_dir/soxr-stop.wav")
samplerate_stop_rms=$(rms_db "$work_dir/samplerate-stop.wav")
sex_pass_rms=$(rms_db "$work_dir/sex-pass.wav")
soxr_pass_rms=$(rms_db "$work_dir/soxr-pass.wav")
samplerate_pass_rms=$(rms_db "$work_dir/samplerate-pass.wav")
sex_frames=$(frame_count "$work_dir/sex-stop-a.wav")
samplerate_frames=$(frame_count "$work_dir/samplerate-stop.wav")
sex_version=$("$sex_bin" --version)
ffmpeg_version=$(ffmpeg -version | awk 'NR == 1 { print; exit }')
samplerate_version=$("$src_reference" --version)

if [[ "$sex_frames" != "16000" ]]; then
  echo "SeX emitted $sex_frames frames instead of 16000" >&2
  exit 1
fi
if [[ "$samplerate_frames" != "16000" ]]; then
  echo "libsamplerate emitted $samplerate_frames frames instead of 16000" >&2
  exit 1
fi
assert_at_most "$sex_stop_rms" -95 "SeX stopband RMS"
assert_at_most "$soxr_stop_rms" -95 "libsoxr stopband RMS"
assert_at_most "$samplerate_stop_rms" -95 "libsamplerate stopband RMS"
assert_close "$sex_pass_rms" "$soxr_pass_rms" 0.02 "SeX/libsoxr passband RMS difference"
assert_close "$sex_pass_rms" "$samplerate_pass_rms" 0.02 "SeX/libsamplerate passband RMS difference"

printf 'SeX external-reference validation passed\n'
printf '  versions: %s; %s; %s\n' "$sex_version" "$ffmpeg_version" "$samplerate_version"
printf '  11731 Hz -> 16 kHz interior RMS: SeX %s dBFS, libsoxr %s dBFS, libsamplerate %s dBFS\n' \
  "$sex_stop_rms" "$soxr_stop_rms" "$samplerate_stop_rms"
printf '  1000 Hz -> 16 kHz interior RMS:  SeX %s dBFS, libsoxr %s dBFS, libsamplerate %s dBFS\n' \
  "$sex_pass_rms" "$soxr_pass_rms" "$samplerate_pass_rms"
printf '  output frames: %s; repeated SeX outputs: byte-identical\n' "$sex_frames"
