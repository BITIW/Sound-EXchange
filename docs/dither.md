# Deterministic dithering contract

Status: normative for native and arbitrary-precision PCM boundaries.

## Placement and target scale

Dither is applied only at the final fixed-point-to-integer-PCM boundary, after
rate conversion and other DSP. For signal fractional precision `F` and target
width `B`, one PCM least significant bit is exactly `2^(F-B+1)` raw units
(`2^(64-B)` at F=63). The implementation never
uses a floating-point noise sample.

The CLI exposes:

```text
--bits 16
--signal-precision auto|4096
--dither none|tpdf|high-pass-tpdf|noise-shaped-1|noise-shaped-5|noise-shaped-9
--seed 42
--clip saturate|error|normalize|allow-headroom
```

Rate conversion uses the preset's policy when `--dither` is absent. A same-rate,
same-width copy remains untouched unless dither or a new target width is
explicitly requested.

## Portable random sequence

The generator is SplitMix64 with fixed constants and unsigned modular operations.
This wrapping is intentional and isolated PRNG arithmetic; signal arithmetic
continues to reject or explicitly saturate overflow. The user seed and channel
index deterministically derive one independent generator per channel. Chunk
boundaries do not reset or otherwise alter generator state.

The BigQ quantizer draws `ceil((F-B+1)/64)` little-endian 64-bit uniform limbs,
masking unused high bits in the last limb. A zero-width draw still consumes
one generator value, matching the native contract. Thus F=63 is bit-identical
to native for all modes, PCM widths, and rounding modes. Greater precision
changes the noise grid and PRNG consumption, so F is part of the reproducibility
configuration, not a transport option. Library inputs with F below B-1 are
promoted exactly to that binary point; they introduce no discarded-bit noise.

## Modes

For `U()` uniformly distributed over the integer interval
`0..2^(F-B+1)-1`:

```text
TPDF[n]          = U1[n] - U2[n]
high-pass TPDF[n] = U[n] - U[n-1]
```

Both have a discrete triangular marginal distribution bounded by one target
LSB in either direction. High-pass TPDF additionally introduces the intended
negative correlation between adjacent samples.

For order `N` in `{1,5,9}`, the noise shaper computes:

```text
v[n] = x[n] + TPDF[n] + sum(k=1..N, (-1)^k C(N,k) e[n-k])
y[n] = quantize(v[n])
e[n] = reconstruct(y[n]) - v[n]
```

This gives the exact error transfer `(1-z^-1)^N`. The binomial feedback
coefficients are small integers, so coefficient quantization is absent. All
terms are checked `i128` values at Q1.63 scale in the native API, or exact
GMP integers at the selected binary point in `BigDitherQuantizer`. Feedback
history never passes through Q*.63 in the latter. On saturation, the complete
feedback history is reset so a clipped endpoint cannot poison later samples.
Errors poison the quantizer instance; continuing after a consumed random value
is forbidden. Presets `high`, `absurd`, `pointless`, and `until-40k` select the
implemented fifth- or ninth-order modes directly.

## Reproducibility tests

Tests cover fixed-seed repetition, different-seed divergence, per-channel state,
chunk invariance, endpoint saturation reporting, no-dither equivalence, and the
complete CLI path when converting 24-bit input to a 16-bit output WAVE.
They also compare native versus BigQ at F=63, test 4096-bit noise/feedback,
and require block-invariant output through the complete 4096-bit CLI chain.
`BigInterleavedDither` commits all channel states and output atomically per
block. A returned error rolls back that block and poisons the complete instance.

## Clipping boundary

All processed CLI paths preserve the selected Qm.n through generic DSP, FIR
output, normalization, and dither. Direct PCM rounding never stages through Q*.63.
`saturate` clamps only at final target-bit-depth PCM quantization and counts
every clamped sample. `error` uses that same boundary but aborts rather than
emitting a wrapped or clamped sample. There is no `wrap` policy.

`allow-headroom` preserves intermediate peaks through final quantization.
Integer PCM still has no headroom, so a value that remains
outside its representable range fails at that final boundary; the policy is not
an alias for saturation.

`normalize` performs two deterministic passes. The first runs the complete
wide FIR and measures absolute peak magnitude. The second multiplies every wide
sample by one exact non-negative rational using GMP and ties-to-even rounding.
For target depth `B`, the target raw peak is
`(2^(B-1) - 1) * 2^(F-B+1)` minus a conservative bound for the selected dither
and noise-shaping feedback. This is the actual positive PCM endpoint expressed
at the signal's binary point; using `i64::MAX` instead could round to the
unrepresentable positive PCM code `2^(B-1)`. Final quantization uses the
error policy, so a violated bound is visible. The CLI reports measured raw peak,
dither guard, and exact scale numerator/denominator.
