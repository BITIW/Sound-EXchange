# Streaming DSP pipeline contract

Status: normative for the native, runtime-format BigQ, and Q65.63 adapter
pipelines in `sexdsp`. The CLI uses the runtime-format BigQ pipeline.

## Block invariance and failure semantics

A native stage consumes and produces interleaved Q1.63 samples; `WidePipeline`
uses interleaved Q65.63 (`WideQ63`). `BigPipeline` accepts a runtime
`BigQFormat(m, n)` and retains it in all stages and histories. Block boundaries are
transport details: concatenating the output from any sequence of complete-frame
chunks must equal processing their concatenation in one call. Stateful stages
retain only the history needed across calls.

Every reported fallible block is atomic with respect to both stage state and the caller's
output vector. On an arithmetic, clipping, alignment, counter, or allocation
error, the failed block appends nothing, commits no history, and poisons the
stage. A later call therefore returns `Poisoned` rather than continuing from an
ambiguous state. Signal arithmetic never wraps.
This describes returned errors, not recovery from a process-wide allocator abort.

`Pipeline` permits variable-length stages. During finalization, each upstream
tail is passed through every downstream stage before that downstream stage is
itself drained. Pipeline statistics count external input samples, final output
samples, and all reported saturations.

The CLI exposes `--gain N/D`, `--dc-remove R`, `--mix MATRIX`, and
`--convolve TAPS` as its first generic effects. Every coefficient is supplied
as a reduced exact rational and quantized once with nearest/ties-to-even.
Gain accepts non-negative `u64/u64` ratios and uses Q65.C; DC, matrix, and
convolution coefficients use Q2.C. C=62 for the baseline F=63 signal, otherwise
C=F. The DC radius is constrained to `0..1` and must remain below one after
quantization; its recursive error bound feeds automatic precision selection.
The fixed order is gain, DC removal, channel matrix, causal convolution, then
rational rate conversion. The same ordering and fresh initial state are used
in both passes of `--clip normalize`, so repeated output remains byte-identical.
Normalization also verifies that both passes consume the same input frame count
and decoded PCM digest, with unchanged format, typed metadata and decoder.
A mismatch prevents publication; see [normalization replay](normalization-input-replay.md)
for the streaming identity and exact concurrency limits.

## Fixed-headroom adapter and CLI selection

`WideStage` provides gain, channel mix, DC removal, and convolution. Products
and accumulators use exact GMP integers. Coefficients in its library API may
have arbitrary supported fractional width; a row must use one common format.
The DC coefficient format is extended to at least two integer bits to represent
+1 without changing its binary point. For each row the constructor proves a
signed accumulator width from `2^127 * sum(abs(coefficient_raw))`, covering
the entire declared Q65.63 input range, not only ordinary audio levels.
Each completed MAC rounds once to 63 fractional bits. Histories preserve
the same wide representation. Exceeding Q65.63 itself is an error; wrapping
and intermediate saturation are forbidden.

Gain or mixing may exceed full scale and a later stage may restore the level
without losing those peaks. The CLI selects a runtime Qm.n from the signal
planner and uses that format in the resampler, normalization, and dither.
All four CLI clipping policies act downstream, never between these stages.
Completion reports include the signal format and maximum pre-rate DSP
accumulator width. `--signal-precision auto|BITS` controls fractional signal
precision independently of MPFR filter design precision. The scan and render
passes use one shared streaming-pass implementation, with fresh stage,
resampler, and dither state where appropriate.

The coupled numerical planner propagates reference peaks, stage rounding,
effect coefficient error, and FIR coefficient error through this same order.
It uses the actual rational-to-Q coefficient differences and the quantized DC
pole gap, and increases precision before processing when the combined bound
exceeds the target. See [numerical-error-budget.md](numerical-error-budget.md)
for the exact reference, recurrence proofs, and exclusions.

The wide implementation is enabled with `sexdsp/bigint`. The original native
stage API remains available without GMP, with the explicit overflow rules below.

## Runtime-format signal pipeline

`sexdsp::arbitrary::{BigStage, BigPipeline}` implements gain, channel mix,
DC removal, and convolution on `BigQ` samples. Constructors take the signal
format explicitly. A pipeline requires every stage and input sample to use
that format; even a no-effect pipeline rejects a mismatched binary point.
Coefficient formats are independent of the signal format, but each coefficient
row must be homogeneous. The safe MAC width is derived from
`2^(m+n-1) * sum(abs(coefficient_raw))`. Completed stage outputs round once
back to Qm.n, with an error if its integer range is exceeded. No intermediate
conversion to Q1.63 or Q65.63 is permitted.

`WideStage` and `WidePipeline` are now exact Q65.63 adapters over this shared
implementation. They do not have a second copy of the stateful algorithm.
The arbitrary pipeline keeps the same transactional block and downstream-tail
contracts. Returned errors roll back block state and output, then poison the
pipeline. Convolution tails retain the declared signal format throughout.

The cross-library integration test imports real 24-bit PCM into Q65.512,
attenuates by 2^-200, resamples 160/147 with MPFR-designed Q2.96 coefficients,
and amplifies by 2^200 before direct PCM export. Its output matches the
unattenuated reference across block sizes. Every attenuated sample would be
zero if narrowed to Q65.63, so this detects a hidden fixed-width staging path.

## Native multiply-accumulate rule

Gain, mixing, DC blocking, and convolution multiply Q1.63 samples by Q2.62
coefficients. Each product is an exact Q3.125 `i128`; all terms for one output
sample are accumulated before a single selected rounding back to Q1.63. The
chosen error-or-saturate policy is applied only after that rounding.

At construction, every coefficient row is checked against a full-scale input:

```text
2^63 * sum(abs(coefficient_raw))
```

If the signed result needs more than 128 bits, construction fails. No stage
depends on cancellation or typical programme level to make an unsafe native
accumulator appear safe.

## Channel matrix

`ChannelMixStage` stores `output_channels * input_channels` Q2.62 coefficients
in output-row-major order. For input frame `x` and output channel `o`:

```text
y[o] = round_once(sum_i x[i] * matrix[o,i])
```

The stage can change sample count because it changes channel count, but it
always consumes and emits complete frames. Matrix dimensions and the native
accumulator proof are validated before processing.

The CLI syntax separates input columns with commas and output rows with
semicolons. Coefficients are signed reduced rationals in the Q2.62 range; the
requested fractions and quantized raw values are both reported. Channel count
changes happen before rate conversion, so the resampler, dither state, and
output container are all constructed for the resulting layout. The CLI's wide
matrix preserves Q1.63 overshoot until final PCM conversion. Library callers
using the native `ChannelMixStage` still choose its Q1.63 overflow policy.

## DC blocker

`DcBlockStage` keeps independent input/output histories per channel and applies:

```text
y[n] = x[n] - x[n-1] + R y[n-1]
```

`R` is exact Q2.62 constrained to `0 <= R < 1`; this places the recursive pole
inside the unit circle. The three terms share one exact accumulator and one
rounding. Channel histories never interact.

## Causal convolution

For Q2.62 taps `h[0...(T-1)]`, `ConvolutionStage` computes independently for
each channel:

```text
y[n] = round_once(sum_(k=0..T-1) x[n-k] h[k])
```

Missing pre-roll samples are exact zero. With tail draining enabled, finishing
after at least one real input frame processes exactly `T-1` zero-input frames;
with it disabled, output ends with the final supplied frame. Repeated finish is
idempotent. Per-channel delay lines and all output remain invariant under chunk
partitioning.

The native generic stages use Q2.62 coefficients. Wide stages reuse `BigMac`
with the same exact-sum/one-rounding rule and never silently narrow coefficients.

The CLI enables tail draining for `--convolve`. Comma-separated signed rational
taps are reduced and quantized once to the selected Q2.C. The pipeline's `T-1` tail is fed
to the finite resampler as real post-effect input before its own aligned finish;
therefore output duration is exactly
`round_even((input_frames + T - 1) * up / down)` for a non-empty source. The
same tail contributes to the normalize scan and render passes and precedes
dither. CLI convolution retains wide headroom and reports an error only if the
completed value exceeds the selected signal Qm.n.

## CLI block invariance

`--block-frames N` selects the positive input read size; 4096 is the default.
It is not part of the numerical configuration and may not alter output. The
integration suite compares blocks of 1, 7, and 4096 frames across gain, DC
state, channel layout change, convolution and its tail, rational resampling,
two-pass normalization, and fifth-order noise-shaped dither.
