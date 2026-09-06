# Measured final PCM error, separate from the numerical budget

Successful conversions now report full-file RMS and peak absolute error at the
final PCM boundary. The reference is the actual signal immediately before PCM
quantization, **after normalization when selected**. The measurement includes
TPDF/high-pass TPDF, noise shaping, final rounding and any saturation. It does
not include earlier FIR approximation, coefficient error, DSP roundoff or the
gain change introduced by normalization itself.

This is separate from `--error-floor`: a -300 dB pre-PCM numerical target does
not imply -300 dB PCM16 output error. The reported error is a full-band finite
file measurement, not a spectral noise density, passband noise floor,
per-channel SNR, worst-case future-input bound or proof of dither statistics.
Noise shaping may increase this broadband RMS while reducing error in another
frequency region; this metric alone cannot rank shaping modes.

## Exact measurement model

The optional-bigint library module `sexdsp::pcm_error` exports `PcmErrorMeter`
and `PcmErrorStats`. Let the signal have F fractional bits and target signed PCM
have B bits. Use W = max(F, B-1). For each matching interleaved sample:

```text
e_i = (pcm_i << (W-(B-1))) - (signal_raw_i << (W-F))
N   = number of interleaved samples
P   = max(abs(e_i))
S   = sum(e_i * e_i)
peak error = P / 2^W
mean-square error = S / (N * 2^(2W))
```

Every subtraction, square and sum uses GMP integers without intermediate
rounding. Only the final CLI display uses 256-bit MPFR logarithms, rendered to
16 significant decimal digits: `20 log10(P / 2^W)` and
`10 log10(S / (N * 2^(2W)))`. These decimal dB values are approximations, not
directed-rounding certificates; the library retains the exact aggregates.
One FS means amplitude 1, including when a saturated error exceeds that level.
There is no floating-point feedback into audio or canonical sample storage.

Empty input reports `n/a (empty)`, because no RMS/peak has been observed. For
nonempty exact output, both values are `-inf`. The same-depth no-effect PCM
bypass reports known exact zero error and the actual sample count without
unnecessarily reconstructing bigint samples. It still distinguishes empty input.

The meter retains only aggregates, not an error track. Storage grows with the
declared sample width and logarithm of the sample count, not with track length.
It adds one integer error square per quantized sample. Counts are checked u64;
invalid sample counts, formats, PCM ranges or counter overflow reject the whole
observed block without modifying prior statistics. Such blocks can be corrected
and retried. Process-wide allocation abort recovery is not promised.

The CLI measures only the rendering pass and emits the report only after the
writer and transactional publication succeed. It does not double-count the
normalization scan or alter PRNG consumption. Its reference includes any
normalization rounding because that is already part of the pre-PCM signal.

## Tests

Exact hand-computed ties and saturation check P and S. A reconstructed-PCM
oracle checks all six dithering modes, B=1/8/16/24/32, F=0/4/63/96/4096, and
blocks 1/2/7/4096. Invalid final samples and counter exhaustion prove atomic,
retryable error handling. Display tests cover empty, exact, and half-FS errors,
including cancellation of a 4096-bit scale and the N divisor in RMS.

A CLI integration test reconstructs errors independently from exported PCM16
and source PCM24 for every dither mode at blocks 1/7/4096. It checks output bytes,
reported aggregates converted to dB, saturation, exact normalization endpoints,
empty files and same-depth bypass. Test-only floating-point logarithms compare
the independent small-integer oracle to the displayed decimal values; production
measurement arithmetic remains integer-only.

## Qualification checkpoint

All 439 native release workspace/all-target tests pass (one existing heavy
test remains opt-in). The three meter regressions and dB display regression
execute on emulated AArch64. Warnings-denied native/cross Clippy and formatting
pass; `sexdsp` also still builds without default features or bigint/MPFR.

Twenty-four conversions compare native x86-64 and emulated Cortex-A53: six dither
modes, F=63/4096, stereo PCM24 to PCM16, gain 4, normalization, seed 42, native
block 1 versus ARM block 7. Both output bytes and complete PCM-error report lines
agree for every matching configuration. Precision changes are different
configurations and are not claimed to consume the same PRNG sequence.

A separate pair of tagged-WAVE High 160/147 renders at F=96, PCM16,
noise-shaped-5, seed 42 and normalization retains the historical complete WAV
SHA-256 `41c4405a6c522f3f483c1908078b5019042cbaf558d7d5092be0b8164a82ba53`.
Its measured final PCM error is RMS **-79.22309789135843 dBFS**, peak
**-70.31816575986032 dBFS**, over 158 interleaved samples. Those are the shaped
full-band errors of this short file, not its -160 dB FIR target or a passband
noise measurement. The two-line metric report matches across CPUs with SHA-256
`ad332b3a5dd0b53bfb5ce482ad34f23486ba384136f37fefc41aa55e2035349b`.

Tests, output files and per-case reports are retained under
`target/pcm-error-qualified` (about 412 KiB at this checkpoint). This evidence
uses the established QEMU-only ARM toolchain, not physical ARM or another ABI.
