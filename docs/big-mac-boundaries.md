# Actual 8192-bit MAC boundaries and constant range reuse

`BigMac` now constructs its signed minimum/maximum once, when the accumulator
is created, and reuses those same integers after every exact product addition.
Previously it constructed both endpoints on every tap. No comparison,
arithmetic order, range, error policy, rescale, or rounding rule has changed.
Unbounded accumulators still have no finite range check. `clear` resets the
sum and poisoned state but retains the immutable width and endpoints.

The bounded contract remains:

```text
minimum = -2^(width-1)
maximum =  2^(width-1)-1
sum += sample_raw * coefficient_raw
if sum < minimum or sum > maximum: error; poison accumulator
```

Neither endpoint reuse nor a caller's eventual output-saturation policy permits
partial-sum wrapping, clipping or rounding. Finishing a poisoned accumulator
still fails.

## Stronger arithmetic evidence

The older test configuring 8192 accumulator bits uses much smaller actual
values; configuration size alone is not evidence of near-boundary arithmetic.
Three new regressions strengthen that evidence:

1. Signed endpoints and one-step overflows are checked at widths
   1/2/31/32/63/64/127/128/8192. This includes the asymmetric negative endpoint,
   poison propagation, cloning, explicit reset, and preservation of the width.
2. Q3.4096 sample raw value `H=2^4097-1` times Q2.4096 coefficient raw value
   `K=2^4094-1` has 8191 magnitude bits and requires **8192 signed bits**.
   A width of 8191 rejects it for both signs. At width 8192, adding `H*K` and
   `H*(-K)` cancels exactly in either order. Two further products, each half of
   one output-grid LSB, sum to one retained LSB at final ties-to-even rounding.
   Independently rounding those last two products would incorrectly yield zero.
3. An actual five-tap arbitrary-width FIR uses coefficients
   `[K, -K, 2^4095, 2^4095, 0]` and input `[H, H, 1, 1, 0]`.
   Automatic planning uses full-format input peak `2^4098` and coefficient raw
   L1 `3*2^4095-2`, selecting **8196 signed bits**. An explicitly requested 8195
   bits fails construction. The valid FIR returns exactly one output LSB, with
   no saturation, despite the cancellation of the much larger terms.

The automatic width is the conservative full-input-range L1 contract, not a
promise that every programme signal will actually reach that magnitude. It
must exceed the width needed for this particular safe cancellation sequence.

## Qualification

All 419 native release workspace/all-target tests pass, with the existing
heavy Until-40k test still opt-in. All three new regressions execute on emulated
AArch64. Native and cross all-target Clippy with warnings denied, plus formatting,
pass. Logs: `target/big-mac-boundary-qualified/{workspace,arm}-tests.log`.

The existing deterministic DSP property corpus at 1000 cases, root seed 42,
passes with identical native and ARM checksum `502236096e75323a`. Complete
stdout SHA-256 on both is
`e4074bd80ab6a64c5597d18b2c00ec8375cf2f672153f0ceed483921bee3d9ee`,
recorded in `target/big-mac-boundary-qualified/{native,arm}-fuzz.txt`.
This exercises scalar,
available exact SIMD, GMP, chunk layouts, headroom and arbitrary signal widths;
it is not coverage-guided fuzzing or a replacement for the explicit boundary
oracles above.

## Bounded execution measurement

On the same host, the existing benchmark was run before and after endpoint
reuse with bigint backend, ratio 160/147, stereo, 4096 input frames, 513 taps
per phase and three timed iterations:

```text
before: GMP-192/Q2.62, 2.520 s
after:  GMP-192/Q2.62, 1.258 s
both:   output 8916 samples, checksum 43e3773fbfa0e91d
```

This observed batch is about 2× faster. It is a small 192-bit execution
measurement, not an 8192-bit throughput claim, an entire Until-40k campaign,
or a portable/statistical performance guarantee. The original benchmark
outputs are `target/big-mac-boundary-qualified/{before,after}.txt`.

The optimization is purely reuse of exact constants. It introduces no SIMD-only
path, floating point, relaxed overflow check, or approximate arithmetic.
