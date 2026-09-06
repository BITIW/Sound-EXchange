# Four-engine spectral reference campaign

This is a bounded execution comparison, not a claim that different resamplers
implement the same filter. The SeX processing path remains fixed-point;
MPFR fixture synthesis and floating-point measurements run in a separate
validation executable, `examples/spectral_probe.rs`.

The default harness now also runs a separate
[complete guarded passband sweep](padded-sweep-campaign.md), without cropping
its high-frequency end. Set `SEX_SPECTRAL_SUITES=signals` to run only the
multichannel fixture described below, or `sweep` to run only the new fixture.
The sweep also supports [preset-aware qualification](spectral-preset-campaign.md);
the legacy multichannel suite remains Sane-specific.
The opt-in [complete input-phase suite](full-input-phase-campaign.md), selected
with `SEX_SPECTRAL_SUITES=phases`, covers every residue of the input-time period
in bounded multichannel impulse batches. It retains the existing amplitude,
phase, area and timing gates without changing the default suites.

## Reproduce

```bash
reference_dir=$(mktemp -d /tmp/sex-sox-reference.XXXXXX)
scripts/bootstrap-sox-reference.sh "$reference_dir"
export SEX_SOX_RUNTIME="$reference_dir"
export SEX_SOX="$PWD/scripts/sox-reference.sh"
scripts/validate-spectral-references.sh target/spectral-qualified
```

The result directory must not exist. A system SoX can be used instead by setting
`SEX_SOX` to its executable; the pinned bootstrap is optional and Linux x86-64
only. It extracts two Debian packages, verifies their published SHA-256 values,
and executes no package installation/maintainer scripts. Its required runtime
dependencies must already be available. The packages are from Debian's
[SoX](https://packages.debian.org/trixie/amd64/sox/download) and
[libsox3](https://packages.debian.org/trixie/amd64/libsox3/download) records.
This old SoX is a reference processing generated fixtures, not a proposed
production parser for untrusted audio.

The harness additionally needs Cargo, jq, a C compiler, FFmpeg built with libsoxr,
the libsamplerate runtime, and standard shell tools. It installs no Python
packages. Raw float reference transport currently requires a little-endian
host. Set `SEX_SPECTRAL_RATIOS='48000:16000 44100:47900'` for a selected matrix.
The probe limits its one-second fixtures to rates 1000..192000 Hz; this is not
a limit on SeX's scheduler or processor.

## Signal and measurement definitions

Each input has `input_rate + 7` frames and signed 32-bit PCM. The original
fixture (v1) has 16 or 18 interleaved channels. The current v2 fixture appends
up to seven independent impulse channels, for at most 25 channels.
Oscillators are evaluated at MPFR 192-bit precision and
rounded once to the nearest integer, ties to even. The channel manifest and
input SHA-256 are retained. Channels contain:

- exact silence, quarter-scale DC, a half-scale centered impulse, and alternating
  signed PCM endpoints;
- a positive-full-scale sine and a half-scale 1 Hz tone;
- half-scale tones near 5%, 12.5%, 25%, 50%, 75%, 85%, and 89% of the lower Nyquist;
- three tones at 7%, 27%, and 71% of lower Nyquist, each amplitude 1/8;
- a logarithmic sweep with `f(t) = exp(log(f_end) * t)`, starting at 1 Hz and
  reaching approximately 85% of lower Nyquist at one second;
- a tone one hertz below input Nyquist, and, where available, two tones above
  output Nyquist. The middle stop tone deliberately avoids folding to DC or
  Nyquist in the default matrix: a zero-phase sine that folds exactly to DC
  can misleadingly disappear even without adequate filtering.

For reduced output/input ratio `L/M`, v2 includes impulses at input frames
`floor(input_rate/2) + j`, for `j = 0 .. min(M, 8)-1`. Their fractional output
centers are distinct: `(input_frame * L) mod M`, with denominator `M`.
This covers every input-impulse phase when `M <= 8`, and eight consecutive
input positions otherwise; it does **not** claim all phases for large `M`.
The period here is `M`, not the engine's `L` output coefficient phases.
Each impulse has its own channel, so responses never overlap each other.
The original channels and centered impulse retain their indices and names;
extra channels are named `impulse-offset-1`, etc. The harness records the
fixture version and actual channel definitions alongside every run.

All engines emit signed 32-bit PCM without dither. SeX uses `sane`, explicit
final saturation, and 257-frame blocks, then repeats with 4096-frame blocks;
the two complete files must match. Full-scale signals may clip and their
warnings/counters are retained. They are stress cases, excluded from the
half-scale passband-ripple aggregate.

For steady measurements, 0.2 seconds are removed from each end. This also
means the sweep's highest-frequency end is not included in its reported SNR;
the separate steady-tone probes cover the upper passband. Wider sweep-window
qualification is provided by the separate guarded fixture linked above; these
historical logarithmic-sweep measurements retain their original interval.
No output is shifted or amplitude-normalized
to improve a measurement. Impulse analysis uses the full file and its exact
rationally mapped input center.

The metrics have explicit meanings:

- Tone gain and phase are fitted by least squares to sine, cosine, and DC at
  the requested frequency, using absolute sample coordinates. Whole cycles
  and zero DC are not assumed. Residual RMS after this fit is reported separately.
- Waveform SNR is `20 log10(RMS(analytic signal) / RMS(output - analytic signal))`.
  It includes gain/phase response and is not a computational-rounding bound.
  It is undefined for silence, stop tones, and transition-band diagnostics.
- The tone ripple is the maximum minus minimum fitted half-scale tone gain in
  dB. It is not a continuous-band maximum.
- Stop-tone RMS and fitted alias peak are absolute dBFS. The folded frequency
  is `min(f % output_rate, output_rate - f % output_rate)`. These measurements
  include fixture/PCM quantization; a zero output is not infinite intrinsic
  filter rejection.
- The impulse report gives peak timing, normalized area, and 129 direct-DFT
  response samples through output Nyquist. Its complex response is rephased
  by the known impulse center and divided by `input_amplitude * output/input`.
  Each impulse gets separate `<signal>-summary.tsv` and
  `<signal>-response.tsv` files. The summary also records the input frame,
  exact center numerator/denominator and fractional phase numerator; the
  floating-point center is only a display/DFT measurement coordinate.
  Neither the observed peak nor a fitted delay replaces the expected center.
- Error spectra use a centered power-of-two segment, at most 4096 samples,
  and a periodic Hann window. One-sided values are peak-amplitude dBFS,
  normalized by the sum of window weights; DC/Nyquist are not doubled.
  They are neither PSD nor averaged broadband noise estimates.
- Pairwise SeX/reference files report sample-aligned difference RMS, peak,
  and spectra over the common settled interval. A difference is not evidence
  that either implementation is wrong: their frequency responses differ.

`NaN` in TSV output means an undefined/not-applicable statistic; `-inf` denotes
exactly zero measured amplitude. No pass/fail gate accepts NaN accidentally.
The probe has independent direct-DFT, calibrated Hann amplitude, non-coherent
tone/phase/DC, PCM-resolution oscillator, and invalid-value regression tests.

## Gates and their scope

SeX must emit exactly the nearest/ties-to-even rational duration. References
may differ by one frame, recorded explicitly; samples are not inserted or
removed to conceal this difference. Every engine must preserve exact silence,
DC within 1e-6 FS, impulse peak within one output frame, and normalized impulse
area within 1e-4, separately for every impulse position. Sampled impulse
passband gain/phase limits through 85% of
lower Nyquist are 0.01 dB / 0.001 radians.

Steady pass tones have the same gain/phase limits. SeX must additionally have
at least 85 dB **uncorrected waveform SNR**. The references instead require
85 dB fitted-residual SNR, while their uncorrected waveform SNR remains in the
report. This distinction is deliberate: SoX's measured 18743 Hz tone at
44.1→48 kHz has about -0.00101 dB gain, 78.67 dB waveform SNR, and -190.4 dBFS
fitted residual. Imposing SeX's flatter response on that different filter would
confuse response shape with nonlinear/numerical error. No fitted correction
relaxes the SeX waveform gate.

Multitone/sweep waveform SNR must exceed 80 dB, and stop-tone RMS must be below
-95 dBFS for every engine. These are bounded cross-engine regression limits,
not certification of the preset's -110 dB filter target. The independent MPFR
quantized-bank gate checks that target; final 32-bit PCM and libsamplerate's
float interface cannot certify arbitrary -300..-600 dB signal-path floors.

Reference configurations are SoX `-R -D ... rate -v -L`, FFmpeg
`aresample=RATE:resampler=soxr:precision=33:dither_method=none`, and
libsamplerate `SRC_SINC_BEST_QUALITY` through its
[simple API](https://libsndfile.github.io/libsamplerate/api_simple.html).
The latter processes the entire small fixture in one call, not independent
chunks. FFmpeg's [resampler documentation](https://ffmpeg.org/ffmpeg-resampler.html)
defines the selected options. Bandwidth/rolloff defaults are not forced to
match across engines, so this is not a universal quality ranking.

## Recorded result: 2026-09-05

All 20 engine/rate combinations passed, as did five repeated SeX renders.
Versions: SoX 14.4.2 (Debian `14.4.2+git20190427-5+b3`), FFmpeg 8.1.2,
libsoxr 0.1.3, libsamplerate 0.2.2, Rust 1.98.1, MPFR 4.2.2.
Conversion commands, input/channel manifests, versions, executable/source
hashes, complete audio, per-engine metrics, and pairwise spectra are retained
in `target/spectral-qualified` (about 106 MiB, no large-track fixture).

Measured half-scale tone ripple, in dB:

| Input → output Hz | SeX sane | SoX -v | libsoxr 33 | libsamplerate best |
| --- | ---: | ---: | ---: | ---: |
| 48000 → 16000 | 0.000002082179 | 0.000000005156 | 0.000000236851 | 0.000000910573 |
| 44100 → 48000 | 0.000001988100 | 0.002190131832 | 0.004912179590 | 0.000001024557 |
| 48000 → 44100 | 0.000002337089 | 0.001426086457 | 0.003649099123 | 0.000001064213 |
| 44100 → 47900 | 0.000001987452 | 0.002180638703 | 0.004896052451 | 0.000001025844 |
| 48000 → 1000 | 0.000002004759 | 0.000000006572 | 0.000000142574 | 0.000001034155 |

SeX multitone waveform SNR ranged from 142.12 to 144.65 dB across these cases;
the settled logarithmic-sweep interval measured 145.43 to 147.09 dB.
These are measured fixture results, not a promised PCM-independent floor.
Libsamplerate emitted one extra frame relative to the SeX rounding contract
for all three downsampling cases; all other recorded durations agreed.

The source fingerprint was
`7b7a17797a5810648f5233516ad5ce122f666e39a44f7a8f73782a04a0fef99f`.
The SeX binary SHA-256 was
`747973b796bb90f1d8fb512a69bf277c717f710264b4603492ed03f1b8d45167`,
the probe SHA-256 was
`a916361624f17ec0a10f944b9fca7c48dca3537d846f39949e63cf50f8380295`,
and Cargo.lock remained
`fe3d46afca93fcfb62f6c19bb6e18a34ab6f8042c620904c3642de7a089ecbb8`.

## Multi-input-phase extension: 2026-09-06

The v2 campaign passed all 20 engine/rate combinations and five byte-identical
SeX block-size repeats. The ratios are unchanged from v1. There are three
input-impulse positions for 48000→16000 (all three phases) and eight each for
44100→48000, 48000→44100, 44100→47900 and 48000→1000: 35 responses per engine,
140 total. Every response passed the unchanged timing, area and sampled
gain/phase gates. The original fixture's non-impulse channels remain present;
their gates and the three pairwise comparison reports also ran in every case.

Worst observed errors over the 35 responses for each engine:

| Engine | Peak timing error (frames) | Absolute normalized-area error | Passband gain magnitude (dB) | Passband phase magnitude (rad) |
| --- | ---: | ---: | ---: | ---: |
| SeX sane | 0.4875 | 6.40750e-7 | 5.565e-6 | 5.76626e-7 |
| SoX -v | 0.4875 | 3.42727e-7 | 9.77604e-4 | 6.16883e-7 |
| libsoxr 33 | 0.4875 | 5.96046e-7 | 2.440628e-3 | 4.90937e-7 |
| libsamplerate best | 0.4875 | 1.892447e-6 | 2.0121e-5 | 5.44884e-7 |

Gain/phase maxima use only DFT bins at or below 85% of lower Nyquist and
include final PCM quantization. Fractional expected centers explain nonzero
integer peak timing errors; they are not fitted or rounded before the DFT.
The larger passband gain variation of some references is a filter-shape
difference within the existing 0.01 dB limit, not a numerical-precision ranking.

As a negative control, FFmpeg delayed the 48000→16000 SeX output by two frames
and trimmed it back to the original duration. The analyzer rejected it at the
impulse gate: observed peak 8002, expected center 8000. Thus duration matching
alone does not hide a timing failure. The modified audio and rejection log are
retained alongside the successful campaign.

Artifacts are in `target/spectral-phases-qualified` (about 143 MiB). The
source fingerprint is
`ecaf502d9638b74dcc12832218bff7d006560be570bafb15bafbdb6f8069f562`,
SeX binary SHA-256 is
`37295b27f4b6855950b25f78915564070351776f07d25710ae5b7c83dcc447f4`,
and probe SHA-256 is
`b1a7bdbaecd78a976d01ca492ef0caf45689e661de2027b91b043c3d2af805c1`.
The harness checked that source inputs did not change during execution.
Versions and input/output/tool/package hashes are recorded there. The actual
SeX binary is unchanged from the frontend checkpoint: this extension changes
validation, not audio arithmetic or filter coefficients.

The native release workspace/all-target suite passed 344 tests, with the
existing opt-in heavy test ignored; strict all-target Clippy passed. All 13
measurement-probe tests also passed on emulated AArch64. The two added tests
check distinct residues, complete small-denominator coverage, bounded fixture
positions, and fractional-center DFT rephasing without observed-peak alignment.
The latter execution uses the existing QEMU-only link accommodation, not a
production ARM linking qualification. Logs are retained as
`target/spectral-phases-workspace-tests.log` and
`target/spectral-phases-arm-tests.log`.

Reproduce with the command above and a fresh result directory such as
`target/spectral-phases-qualified`; the current probe generates v2 by default.
The v1 artifacts remain historical evidence and are not overwritten. This
extension does not provide full large-denominator impulse-phase coverage,
padded full-band sweep evidence, other-preset measurements or continuous
complete-image bounds.

## Defect found and fixed

The pilot uncovered a real Sane preset defect: its original 129-point bank
qualification missed response peaks. At 48→16 kHz, 513 points measured
-106.34696 dB stopband peak, failing the declared -110 dB floor. The fixed
planner reserves 12 dB design margin without changing that acceptance target.
The FIR grows from 769 to 865 taps at this ratio, and its 1025-point result is
-126.28844 dB. The regression retains the old failing case and checks the new
plan at 1/3, 3/2, and 1/48. Details are in
[precision-planner.md](precision-planner.md).

The actual 160/147 CLI bank was additionally checked at 1025 points/band for
all 160 phases, at MPFR 192. All three -110 dB gates passed: maximum passband
deviation 6.084399488982508e-7, ripple 1.014297179767891e-5 dB, and input-Nyquist
endpoint -120.2500519484514 dB. That last value is not a certificate of the
entire expanded anti-imaging band. Its coefficient identity is
`90515b7fc9fc8be293bc360dc65803fb19629a3d8bea9069679ced22198c8ac2`.
Both full CLI reports are retained as `sane-down3-response-1025.log` and
`sane-up160-response-1025.log` in the result directory. Reproduce with:

```text
sex analyze input.wav -r 48000 --preset sane --error-floor -110dB --grid 1025
```

The input above must have the qualified 44100 Hz source rate; its PCM values
do not participate in coefficient-response analysis.

Remaining qualification includes other presets in this execution campaign,
wider guarded-sweep and rate/impulse-phase combinations, continuous-band
error enclosures, and comprehensive periodically time-varying anti-imaging
bounds. A finite matrix does not establish universal quality or reproducibility.
