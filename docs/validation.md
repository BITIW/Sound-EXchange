# Resampler validation

The validation strategy has four separate layers:

1. Arithmetic and scheduler unit tests prove exact local invariants: no silent
   wrap, one final FIR rounding, exact-rational phase sequences, accumulator
   width, clipping visibility, and chunk-boundary independence.
2. CLI integration tests pass deterministic multichannel integer fixtures
   through the complete application and require byte-identical repeated output.
   Cross-architecture qualification additionally requires independent designs
   and execution to agree, including encoded files, metadata, and cache bytes.
3. External reference runs compare observable response against independent
   implementations. They are quality comparisons, not golden-byte tests,
   because those implementations intentionally use different arithmetic and
   filters.
4. Exact continuous-frequency certificates use the actual quantized bank to
   bound whole passband/stopband intervals. They are independent of the sampled
   response grid and distinguish exact violation from an exhausted proof budget.
   Their per-phase amplitude scope excludes complete anti-imaging, effects,
   PCM noise, and MPFR designer error; see
   [continuous-response-certificate.md](continuous-response-certificate.md).

The current designed-pipeline torture set includes silence, steady DC, positive
and negative impulses, and alternating full-scale endpoints. Silence remains
bit-exact zero, the settled DC region is bit-exact unity gain, impulse polarity
is exactly antisymmetric, and the near-Nyquist alternating sequence does not
silently overflow.

A second spectral execution matrix interleaves six independent channels:
silence, a full-scale sine, 1 Hz, a tone one hertz below Nyquist, a three-tone
mixture, and a linear sweep from 1 Hz to one hertz below Nyquist. Test fixtures
are quantized with MPFR; the processing path still receives only Q1.63. A
single-block render and an irregular `1, 17, 3, 257, 64, 5, 1023` frame chunk
sequence must match in every sample and statistic. The silence channel must
remain zero and every signal channel must remain nonzero.

`sex-fuzz` complements those named fixtures with replayable generated cases.
Each case chooses a reduced rational ratio, odd FIR length, one to four
channels, full-range fixed-point input, exact-unity quantized phase
coefficients, and randomized complete-frame chunks. It requires native
one-shot, native chunked, and GMP chunked execution to produce identical
samples and statistics, then checks exact signed-PCM round trips at randomly
selected widths from 1 through 32 bits. A failure prints the seed needed to
reproduce that case.

On x86, the same generated case is also rendered through every available exact
AVX2/AVX-512 multiplication backend and compared to scalar output and
statistics. SIMD availability never relaxes the native-versus-GMP oracle.

The finite-stream unit suite separately locks the scheduler edge where a
downsampling phase is ready before nearest duration rounding admits it. The
phase is emitted later from indexed retained history; no chunk boundary may
change its three-tap window and no already-emitted frame may need retraction.

Duration edge tests include one-sample 44.1→48 kHz and 48→16 kHz files. Under
nearest/ties-to-even they produce exactly one and zero frames respectively.
An integer-only 47999/44117 phase-clock test proves a deliberately awkward rate
without converting it to a binary float.

Long-stream coverage does not manufacture a disposable hundred-gigabyte file.
It processes one million generated frames at ratio 1/1001 while inspecting the
internal retained-history bound, then separately schedules the frame count of
a virtual 100 GiB, eight-channel, 32-bit payload with `u128` rational products.
This validates stream-state and counter behaviour without claiming RF64 or
other large-container support that the I/O layer has not yet implemented.

## Cross-architecture execution

Window selection has native and AArch64-QEMU library regressions covering the
corrected fractional Dolph interpolation, all-window Q2.62-versus-bigint
coefficient/measurement identity, C=4096, cache round trips/corruption, and
separate coefficient/length feedback. Native CLI tests match full analysis
qualification blocks to actual Hann, Blackman-with-amplification, and High
Dolph PCM at blocks 1/7/4096. Resource and warm-cache inconclusive-proof failures
preserve previous output. See [windowed-cli.md](windowed-cli.md).

The [shared gate tests](shared-quality-gates.md) bind analysis to actual
conversion: complete qualification blocks and coefficient identities must
agree for the same settings. Native Sane, bigint High, and amplification-driven
precision refinement all execute with requested continuous/harmonic checks at
blocks 1/7/4096. Warm-cache proof/limit failures and later PCM clipping failures
must preserve the previous output. Matching FIR reports do not turn the final
PCM noise floor into a certified resampler-response bound.

The preceding [mandatory quality qualification](quality-refinement.md) passed a
14-case / 56-render corpus without an opt-in flag. Every output and cache byte
matches the preceding `--refine-quality`-enabled corpus. Absurd and Pointless
at 1/2 both require a second filter-shape candidate; all four renderings and
independently generated caches agree for each case. Their output identities
intentionally change relative to unchecked preset execution, while the other
twelve scenarios retain their prior identities. The expensive Until-40k case
was not repeated on AArch64 in this checkpoint; its separate native default-
feedback regression passes with the same bank and block-identical PCM.

The [AArch64 qualification](cross-architecture.md) passed 15 scenarios and 60
renders against native x86-64, including every preset, 4096-fractional-bit
stateful DSP, and tagged WAVE/AIFF/FLAC. Each architecture designs its own
coefficient cache, then reads the other's cache. Output and cache files must
agree byte-for-byte at block sizes 1, 7, and 4096. Build identities, source and
input hashes, commands, and results are retained by the harness.

The AArch64 executable runs under QEMU with an explicitly emulator-only linker
configuration. Physical ARM, production linking and 32-bit execution remain
unqualified. A subsequent [big-endian experiment](big-endian-qualification.md)
adds a separate static-musl profile; it does not retroactively extend the
historical GNU/codec runs described above. These tests establish reproducibility for the
recorded configurations; they are not filter-quality reference comparisons.

## Current external check

Run:

```text
scripts/validate-native-vs-references.sh
```

The script creates temporary 24-bit integer WAVE tones at 48 kHz, converts them
to 16 kHz with SeX, FFmpeg's libsoxr backend at precision 33, and
libsamplerate's `SRC_SINC_BEST_QUALITY`, discards 50 ms at each edge, and
measures the remaining RMS. It checks:

- a 11731 Hz stopband tone is below -95 dBFS for all three engines;
- a 1000 Hz passband tone differs from SeX by no more than 0.02 dB RMS;
- SeX emits exactly 16000 frames for one second;
- libsamplerate emits exactly 16000 frames for one second;
- two independent SeX runs are byte-identical.

The libsamplerate adapter is a validation-only C frontend that dynamically
loads its stable public simple API. This keeps reference floating-point data
outside every SeX crate and also works on systems that provide the runtime
library without `sndfile-resample` or development headers. The harness prints
the SeX, FFmpeg/libsoxr, and libsamplerate versions with every result.

The source tone is integer-quantized, so the stopband result eventually reaches
the fixture's own quantization/noise floor. This check is meant to catch gross
aliasing and gain regressions; `sex analyze` remains the high-dynamic-range
coefficient-response measurement.

The compatibility wrapper `scripts/validate-native-vs-soxr.sh` invokes the same
three-engine smoke harness. The broader campaign below additionally executes
a real SoX frontend, extracted into an isolated reference runtime.

## Four-engine spectral campaign

`scripts/validate-spectral-references.sh NEW_RESULT_DIRECTORY` compares SeX
Sane, SoX `rate -v -L`, libsoxr precision 33, and libsamplerate best quality
at 1/3, 160/147, 147/160, 479/441, and 1/48. All 20 combinations passed; SeX
also passed five full-file repeats with different input block sizes.
The [campaign report](spectral-reference-campaign.md) defines fixtures,
units, gates, commands, versions, and limitations. It records amplitude/phase,
waveform/fitted residuals, alias peaks, impulse response, and pairwise spectra.

The pilot caught a sparse-grid Sane defect. Its 129-point check missed a
-106.35 dB peak, below the promised quality of its -110 dB target. A 12 dB
design margin and 1025-point regressions now qualify the corrected candidate;
the acceptance floor is unchanged. The original sparse-grid numbers remain
historical regression data, not current Sane certification.

## Harmonic-transfer analysis

`sex analyze --harmonics` evaluates every output image of the quantized bank
over an exact rational pass/transition/stop input-frequency grid. The phase
schedule, coefficient tap order, and delay compensation are checked against
an independent interleaved-prototype sum and real fixed-point cosine/sine
execution at five ratios and three block sizes. Parseval checks total component
power independently. The zero-order-hold test exposes images invisible in
individual phase magnitude plots.

The [harmonic contract and campaign](harmonic-response-analysis.md) define
component versus aggregate metrics, exact output-frequency folding, resource
budgets, and the distinction between measured response and continuous proof.
`scripts/validate-harmonic-responses.sh` preserves full reports and can compare
independent designs and analysis reports on a second architecture. It does not
replace PCM/reference-engine execution or the continuous interval certifier.

## Required expansion

The continuous certifier has qualified Sane at 1/3 and 160/147 and corrected
High at 1/2. Its discovery of High's original -160 dB violation is retained as
a regression; design margin was increased without weakening the target. These
bounded certificates do not qualify every preset or replace the expansion below.

Before every public preset is frozen, extend this matrix to those other
presets, padded full-band sweep measurements, more impulse input phases and
pathological ratios, and stronger anti-imaging qualifications. Measured PCM
noise must remain distinct from filter response and computational error bounds.
Reference versions and conversion commands remain required for published results.
