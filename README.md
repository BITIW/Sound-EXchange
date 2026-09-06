# SeX — Sound Exchange

SeX is an implemented v0.1 offline-first audio processor. Its defining
contract is a deterministic fixed-point signal path: quality has priority over
latency, safe arithmetic never silently wraps, and a reproducible configuration
must produce identical output bytes on every supported CPU.
The [original project goal](docs/project-goal.md) preserves the user's scope.
The [completion audit](docs/completion-audit.md) maps every original requirement
to current-code and retained external evidence. The detailed
[implementation history](docs/project-status.md) keeps proof scopes explicit.
Automatic arbitrary-Q FIR accumulators now use
[exact signed intervals](docs/exact-accumulator-intervals.md), including the
asymmetric negative endpoint, to avoid unnecessary guard bits.
The [native and Q1.63 bank constructors](docs/exact-fir-admission.md) apply the
same proof, with a separate accumulator check for extended Q65.63 input.

The [shared Rust library and standalone frontends](docs/library-frontends.md)
now provide `sex-rate` for explicit rate conversion and `sex-analyze` for analysis.
They use the same parser, planner, quality gates and engine as `sex`; the
`sex` library also re-exports the existing component APIs.

[`--window`](docs/windowed-cli.md) selects Kaiser, rectangular, Hann, Blackman,
or Dolph–Chebyshev across conversion, analysis, and planning. Non-Kaiser CLI
banks use arbitrary-width coefficients and the same quality gates; a resource
or quality failure never silently selects a different window.

[`--designer global-ls|remez`](docs/optimized-cli.md) selects global least-squares
or Parks–McClellan with arbitrary Q2.C, persistent caching, whole-chain precision
planning, and actual-bank quality feedback. Analysis and conversion qualify the
same bank. Solver/resource failures remain explicit; large global designs are
not yet practical across every preset and ratio.
Selected numerical failures now support [bounded working-precision
recovery](docs/solver-precision-recovery.md), without changing the filter target
or overriding explicitly pinned precision.
Remez also supports [bit-identical cosine reuse](docs/remez-cosine-cache.md)
within the existing memory allowance; a smaller allowance keeps the original
calculation available with the same coefficient results.

The repository is intentionally being built from mathematical contracts outward:

- `sexq`: native Q1.31/Q1.63, [compile-time and runtime Qm.n](docs/compile-time-q.md)
  up to 64 logical bits, arbitrary-width
  GMP-backed Qm.n, Q2.62 FIR coefficients, checked widening arithmetic,
  explicit rounding/saturation, sign extension, rescale, and planned `i128` or
  bigint accumulator bounds, plus isolated bit-exact AVX2/AVX-512 qualification
  kernels guarded by scalar differential tests and a speed gate;
- `sexrate`: exact-rational phase scheduling, native and GMP-backed quantized
  polyphase FIR execution, runtime-format BigQ sample streams, and
  chunk-independent causal streaming through one shared timeline;
- `sexfir`: MPFR Kaiser, rectangular, Hann, Blackman, and Dolph-Chebyshev
  windowed-sinc design plus weighted least-squares and Parks-McClellan
  equiripple low-pass design; Q2.62 or arbitrary Q2.n quantization, exact
  per-phase DC correction, exact continuous-frequency amplitude certificates,
  all-image harmonic-transfer analysis, bounded feedback-driven quality refinement,
  error bounds, coefficient identities, and a
  validated atomic coefficient cache;
- `sexplan`: exact-rational quality presets and automatic taps, precision,
  guard-bit, accumulator, backend, and dither planning, plus outward-rounded
  whole-chain peak/error bounds and coupled signal/coefficient refinement;
- `sexdsp`: streaming stage composition, fixed-point gain, channel matrices,
  DC blocking, causal FIR convolution, runtime-format BigQ stages with exact
  GMP MACs and Q65.63 adapters, seeded TPDF, high-pass TPDF, exact
  first/fifth/ninth-order noise shaping, and interleaved channel state;
- `sexio`: safe streaming audio contracts plus the built-in integer PCM WAVE
  adapter, isolated from DSP;
- `sexio-sndfile`: runtime-loaded libsndfile adapter for integer PCM WAVE/AIFF
  and lossless FLAC, including typed metadata transfer;
- `sex`: the CLI façade;
- `sex-bench`: a deterministic native-versus-bigint execution benchmark;
- `sex-fuzz`: a replayable property driver for ratios, chunks, channels,
  arithmetic backends, PCM widths, and bounded malformed-WAVE/short-read tests
  (`--suite dsp|io|all`, direct replay via `--case-seed`).

Kaiser/windowed coefficient caches now use whole-payload integrity and v2
request keys. Existing files are retained, but those banks are redesigned once
on first use after the update; optimized caches remain compatible. See
[cache format and migration](docs/cache-payload-integrity.md).
Big-endian targets select a portable SHA-256 backend automatically; the
[experimental ARM big-endian qualification](docs/big-endian-qualification.md)
records the dependency defect, regression and bounded platform evidence.

The general streaming pipeline, bigint arithmetic, arbitrary-width coefficient
designer, and bigint polyphase execution path now exist. `high`, `absurd`,
`pointless`, and `until-40k` therefore do not fall back to Q2.62. The latter is
intentionally enormous: for 44.1 -> 48 kHz its initial 160 × 66867-tap bank is
rejected by the −600 dB gate and the accepted bank is 160 × 68197 taps. It
relies on the persistent coefficient cache and deterministic phase-parallel
response analysis for practical repeat runs.
Sample-rate input is [exact-rational](docs/exact-rational-rates.md): `-r`/`--rate`
accept `48k`, `96000/2`, `4.8e4` and decimal Hz without float conversion.
Plan/analyze also support true fractional Hz; current output adapters require
integer Hz and reject an unrepresentable rate without silently rounding it.

The CLI now carries planner-selected arbitrary-fractional samples through DSP,
resampling, normalization, dither/noise shaping, and direct PCM export. Typical
signal fractional widths are 63/63/96/160/256/512 bits across the six presets;
effect-chain bounds or an explicit `--signal-precision` can raise them further.
Conversions separately report [measured final PCM RMS/peak error](docs/final-pcm-error.md)
using exact integer aggregates. This includes final dither and clipping relative
to the immediate pre-PCM signal; it is not the `--error-floor` numerical bound
or a spectral noise-floor claim.

The native `sexq` core is independently buildable without its default `bigint`
feature. Format I/O and native DSP use that boundary, while rate/FIR and the
full executable opt into GMP explicitly; a missing cross-platform GMP toolchain
therefore does not prevent validating the platform audio adapter itself.

## Current executable path

The current CLI can perform a bit-exact same-rate integer PCM WAVE/AIFF/FLAC
pass or MPFR-designed, fixed-point rational rate conversion:

```text
sex input.wav output.wav
sex input.wav -r 48000 output.wav
sex input.wav output.wav rate 48000 --precision auto
sex input.wav output.wav rate 48000 --preset fast --window hann --harmonics
sex input.wav output.wav --gain 1/2 --dither none
sex input.wav output.wav --gain 4 --clip normalize
sex input.wav output.wav --dc-remove 65535/65536
sex stereo.wav mono.wav --mix '1/2,1/2'
sex impulse.wav response.wav --convolve '1,1/2,-1/4'
sex input.wav output.wav --block-frames 1
sex input.wav -r 48000 output.wav --signal-precision 4096 --clip normalize
sex input.wav -r 48000 --preset sane --error-floor -300dB output.wav
sex input.wav --bits 16 --dither high-pass-tpdf --seed 42 output.wav
sex input.wav -r 48000 --clip error output.wav
sex input.wav -r 48000 --clip normalize output.wav
sex input.wav -r 48000 --clip allow-headroom output.wav
sex input.wav -r 48000 --preset high output.wav
sex input.wav -r 48000 output.wav --grid 129 --certify --harmonics --harmonic-work 500000000
sex input.flac -r 48000 output.flac
sex input.wav -r 48000 output.aiff
sex plan input.wav -r 48000 --preset until-40k
sex plan input.wav -r 48000 --gain 4 --dc-remove 10/11 --convolve '1,-1/5'
sex analyze input.wav -r 48000 --grid 129
sex analyze input.wav -r 48000 --preset sane --certify
sex analyze input.wav -r 48000 --preset sane --harmonics
sex analyze input.wav -r 48000 --preset sane --refine-quality --certify --harmonics
```

Float WAVE is rejected at the format boundary rather than becoming a canonical
floating-point path. The libsndfile backend likewise admits only integer source
subtypes; lossy/float-decoded formats do not silently enter the canonical path.
Conversion output is written to a uniquely reserved sibling file and published
only after the audio writer has finalized and synchronized it. A processing or
codec error therefore removes the temporary file and leaves any existing output
untouched.
Two-pass normalization verifies decoded PCM, frame counts and input properties
across both passes; a changed source fails without publishing output. See the
[input replay contract](docs/normalization-input-replay.md) for its scope.
Every completion report names the input and output adapters. External
AIFF/FLAC or tagged-WAVE endpoints include libsndfile's runtime version string,
so a byte-reproducibility claim is tied to an identifiable codec backend.
Rate-changing output defaults to the numerically specified
`sane` preset; `--preset`, `--error-floor`, and `--precision` feed the exact
planner rather than a floating-point configuration path.
Bounded [quality feedback](docs/quality-refinement.md) is mandatory for every
rate-changing conversion and every analyzed FIR, even without `--error-floor`.
The optional floor selects the target; `--refine-quality` remains a compatible,
redundant alias. Failed candidates increase coefficient precision or FIR
length/beta and are checked again, without relaxing the target or band edges.
Resource exhaustion fails explicitly. A grid-only pass remains sampled, and
`plan` still reports an unqualified initial candidate without designing a bank.
It shows the initial work estimate and any required budget increases. A cached
bank is checked again, not trusted as a certificate. Same-rate conversion has
no resampler FIR to qualify. The initial Absurd/Pointless candidates at 1/2
fail their targets; ordinary conversion now automatically selects stronger
banks instead of requiring an opt-in flag.
Conversion, `analyze`, and `plan` now accept the same
[quality gate settings](docs/shared-quality-gates.md). With matching settings,
conversion executes the very bank that was qualified and prints the same
bank-bound report as analysis. Same-rate conversion rejects supplemental FIR
checks explicitly because it bypasses the resampler.
`--gain N/D` and `--dc-remove R` are the first generic pipeline effects exposed
by the CLI. Gain is a non-negative exact linear `u64/u64` ratio, quantized once
at the selected effect precision before any rate conversion. Amplification
preserves intermediate headroom instead of clipping to Q1.63.
DC removal uses `y[n] = x[n] - x[n-1] + R*y[n-1]`, requires exact rational
`0 <= R < 1` and one post-MAC rounding to the selected signal Q format. A
near-unity radius increases the signal precision budget rather than being
rejected solely because it would round to one in Q2.62.
`--mix` accepts semicolon-separated output rows and comma-separated input
columns, so `'1/2,1/2'` maps stereo to mono while `'1,0;0,1'` is a two-channel
identity. Every signed rational is reduced and quantized once, every
row uses one exact GMP MAC, and the input column count must match the file.
The fixed CLI order is gain, DC removal, channel mix, causal convolution, then
rational rate conversion. The selected Qm.n is retained through resampling and
dither, with clipping applied only at final PCM quantization. At Q65.63 with
native coefficients, FIR products use checked i128 with exact GMP recomputation
on overflow. Wider signal or coefficient formats use a bounded GMP accumulator.
The report names the actual signal and effect-coefficient formats. Filter design
precision (`--precision`) and signal fractional precision (`--signal-precision`)
are separate controls; neither may undercut its automatic minimum.
The planner propagates reference peaks and rounding/effect-coefficient/FIR-
coefficient errors through the chain. It increases signal or FIR precision as
needed and reports the resulting numerical bound. With `--certify-design`,
the accepted Kaiser bank's rigorous joint design/quantization error replaces
the generic FIR allowance and the whole-chain budget is rechecked. Ordinary
runs still exclude MPFR design error. Both modes exclude ideal-filter
approximation and final PCM/dither/clipping; the precise
reference and proof rules are in [numerical-error-budget.md](docs/numerical-error-budget.md).
`--convolve` accepts a comma-separated causal FIR in the same exact-rational
coefficient syntax. It emits the full `T-1` tail; those frames are included in
same-rate output, the exact rational rate schedule, both normalization passes,
and the continuous per-channel dither sequence.
`--block-frames` changes only transport granularity (default 4096), not the
algorithm. End-to-end tests require identical output for 1-, 7-, and
4096-frame blocks through the complete stateful CLI pipeline.
The analyze mode measures every stored phase using MPFR and reports passband
ripple, stopband peak, coefficient error, accumulator width, and coefficient
identity. Sane has dense 1025-point regressions at 1/3, 3/2, and 1/48 after
the original sparse grid missed response peaks. No sampled grid establishes
universal or continuous-band preset bounds. The optional
[`--certify` mode](docs/continuous-response-certificate.md) uses exact rational
polynomials and Bernstein subdivision to cover entire specified bands. It has
explicit resource limits and distinguishes proven compliance, exact violations,
and inconclusive results. Sane at 1/3 and 160/147 and corrected High at 1/2 have
continuous per-phase amplitude certificates; this is not a full anti-imaging
or Until-40k qualification.
The separate [`--harmonics` mode](docs/harmonic-response-analysis.md) samples
all output components for every input frequency in a pass/transition/stop grid.
It includes exact folded image locations, wanted-component complex/phase error,
aggregate image power, and stopband leakage. This covers periodically varying
images missing from individual phase magnitudes, but is not a continuous proof.

The library also provides [rigorous Kaiser design-error certificates](docs/kaiser-design-enclosures.md):
directed MPFR enclosures, exact rational phase geometry, every phase of the
actual integer bank, automatic enclosure-precision refinement, and a joint
design/quantization/DC-correction bound. This is distinct from `--certify` above
and feeds the CLI whole-chain numerical budget. Require it with
`sex analyze input.wav -r 48000 --certify-design` or add `--certify-design` to a
rate conversion. The default joint error target follows the planner's
coefficient allowance; resource/precision limits are explicit. Certificates
are recomputed even for coefficient-cache hits. A proved coefficient-error
violation triggers bounded automatic bank redesign and repeat qualification;
an inconclusive enclosure never masquerades as a proved bank failure. Run
`cargo run --release --example kaiser_bank_certificate` for complete-bank
examples, including all 160 phases of a 44.1→48 kHz candidate.

The test suite also launches the complete CLI twice on the same 24-bit stereo
fixture and requires byte-for-byte identical rate-converted WAVE files. This
covers the combined format, designer, rational scheduler, FIR, and output path,
not only isolated arithmetic routines.
The [cross-architecture qualification](docs/cross-architecture.md) additionally
compares x86-64 with AArch64 under QEMU: 15 scenarios / 60 renders passed,
including all presets, 4096-bit fractional signals, tagged WAVE/AIFF/FLAC, and
independently generated coefficient caches exchanged in both directions.
This is emulated execution evidence, not physical ARM or production-linking
certification. `sex --build-info` reports the executable's architecture and ABI.
The [four-engine spectral campaign](docs/spectral-reference-campaign.md) passed
20 Sane/SoX/libsoxr/libsamplerate rate combinations and retains measured
gain/phase, aliasing, impulse response, and error spectra. It found and led to
fixing Sane's insufficient design margin; the target remains -110 dB, with
289 taps at unity and 865 at 1/3. Sane's coefficient/output identities therefore
change intentionally; the full cross-architecture corpus was rerun successfully.
Subsequent continuous certification found a similar High defect. High now keeps
its -160 dB target with 12 dB design margin, 555 unity-rate taps, and 1109 at
ratio 1/2. Its output/coefficient identities also change intentionally.

See [`docs/fixed-point-model.md`](docs/fixed-point-model.md) for the normative
arithmetic contract, [`docs/filter-design.md`](docs/filter-design.md) for the
designer math, and [`docs/sox-reference-audit.md`](docs/sox-reference-audit.md)
for the initial upstream audit and rewrite decisions. The layered test and
external-reference methodology is documented in
[`docs/validation.md`](docs/validation.md), and preset/planner rules in
[`docs/precision-planner.md`](docs/precision-planner.md). The deterministic PCM
quantization contract is in [`docs/dither.md`](docs/dither.md), and the durable
coefficient format in [`docs/coefficient-cache.md`](docs/coefficient-cache.md).
Generic streaming-stage semantics are specified in
[`docs/dsp-pipeline.md`](docs/dsp-pipeline.md).
The container/metadata boundary and its reproducibility scope are documented in
[`docs/io-formats.md`](docs/io-formats.md).
The benchmark and deterministic property-test entry points are documented in
[`docs/testing-tools.md`](docs/testing-tools.md).
