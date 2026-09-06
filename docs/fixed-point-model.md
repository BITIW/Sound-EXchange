# Fixed-point arithmetic contract

Status: normative for the native Q1.31, Q1.63, runtime Qm.n, and arbitrary-width
Qm.n backends implemented in `sexq`.

## Representation

For a signed raw integer `r` with `F` fractional bits, the represented value is
exactly `r / 2^F`. PCM import and the narrow API use Q1.63 (`i64`, `F = 63`) with range
`[-1, 1 - 2^-63]`. Native FIR coefficients use Q2.62 with range
`[-2, 2 - 2^-62]`, because a Q1.63 coefficient cannot represent unity. Signed
PCM import is an exact sign-preserving left shift.

The specialized Q1.31 backend occupies exactly four bytes and widens its
products to exact Q2.62 in `i64`. Conversion between Q1.31 and Q1.63 is an exact
left shift in the widening direction and an explicitly rounded narrowing in the
other direction.

`QFormat(m, n)` defines a runtime native format with `m >= 1`, where `m`
includes the sign bit, and `m + n <= 64`. `NativeQ` stores the format beside its
signed `i64` raw value. Addition and subtraction require identical formats;
rescale and multiplication require an explicit destination format and rounding
policy. There is no implicit binary-point alignment.
Compile-time configurable [ConstQ/ConstMac](compile-time-q.md) now expose the
same native contracts with I/F in the Rust type. `ConstQ<I,F>` stores one i64
without per-sample format metadata; mixed types require explicit conversion.
This is separate from the specialized four-byte Q1.31 representation.

Negative values must be sign-extended. Code must not infer a rounding rule from
a signed right shift: discarding fractional bits always calls an explicit
rounding operation.

Importing a two's-complement storage word rejects bits above the declared width,
then sign-extends from exactly that width. Export masks back to the same width,
making the rule independently testable for nonstandard formats such as Q3.4.

`BigQFormat(m, n)` follows the same definition but stores the raw integer in
GMP through `rug::Integer`. The public construction boundary currently caps a
single value at 1,048,576 bits to reject accidental resource explosions; this
is a safety limit rather than a numerical limitation of the arithmetic model.
It includes the 8192-bit accumulator required by the initial `until-40k`
contract with ample margin.

The `sexq` crate exposes this layer through its default `bigint` feature. Its
native Q1.31/Q1.63/Qm.n, checked MAC, wide Q65.63, rounding, and exact SIMD
surface also compile with `--no-default-features`; that configuration has no
GMP dependency. Format adapters and the native DSP pipeline deliberately use
that smaller surface. `sexrate`, `sexfir`, and the complete CLI request
`bigint` explicitly because wide FIR and exact-rational normalization are part
of their contract. This is a dependency boundary only: disabling bigint never
selects floating point or changes native arithmetic.

`WideQ63` is Q65.63 backed by `i128`. It shares Q1.63's binary point but keeps
64 additional integer/headroom bits in the baseline CLI configuration and
fixed-width library API.
Native and bigint FIR kernels can finish directly into this representation;
there is still exactly one post-MAC rounding operation. Returning to Q1.63 is
an explicit error-or-saturate boundary. Exact-rational normalization uses GMP
for the multiply/divide so a wide endpoint cannot overflow an intermediate.
The CLI now selects a runtime Qm.n and retains it through DSP, resampling,
normalization, dither, and final PCM rounding. High/absurd/pointless/until-40k
select at least 96/160/256/512 fractional signal bits. `--signal-precision` can
raise the automatic minimum. The Q65.63/native-coefficient branch remains an
exact optimization; selecting a wider signal uses BigQ without narrowing.

`BigQ::from_signed_pcm` interprets a B-bit signed code as Q1.(B-1), then
rescales with an explicit rounding/overflow policy. `BigQ::to_signed_pcm`
rounds directly from the actual signal binary point to Q1.(B-1); it does not
first round through Q*.63. This matters even for 16-bit output: a value just
above a PCM halfway case can lose its tie-breaking low bit at an intermediate
63-bit boundary. `BigQ::scale_ratio` multiplies by an exact signed integer
numerator and divides by a positive integer denominator with one final
rounding at the original signal binary point.

## Overflow and clipping

Safe arithmetic has no wrapping mode. A representational boundary selects one
of:

- `Error`: return an overflow error without a sample;
- `Saturate`: clamp to the closest endpoint and return `saturated = true`.

Accumulator overflow is always an error. Saturating or rounding a partial FIR
sum would make the final answer depend on tap order and violate the exact-MAC
contract. `normalize` and `allow-headroom` belong to the pipeline layer and do
not alter these primitive rules.

## Rounding

Supported modes are toward zero, floor, ceiling, nearest/ties-to-even, and
nearest/ties-away-from-zero. Nearest/ties-to-even is the default because it is
unbiased under symmetric repeated quantization. A chosen mode is part of the
reproducibility configuration and cache key.

## Multiplication and FIR accumulation

General Q1.63 multiplication yields exact Q2.126. The native FIR multiplies a
Q1.63 sample by a Q2.62 coefficient to produce exact Q3.125 in `i128`. It:

1. multiplies every sample/coefficient pair without narrowing;
2. adds every Q3.125 product to the same checked `i128` accumulator;
3. rounds once by 62 fractional bits back to Q1.63;
4. applies the selected output overflow policy once.

An `i128` accumulator is only valid when the planner proves its bound. For
samples bounded by raw magnitude `S` and coefficient raw integers `c[k]`, the
condition `S * sum(abs(c[k])) <= 2^127 - 1` is sufficient, but not necessary.
For the full signed sample range `[-S,S-1]`, let P/N sum positive/negative
coefficient magnitudes. The exact attainable raw interval is
`[-S*(P+N)+N, S*(P+N)-P]`. Admission requires its lower endpoint to be at least
`-2^127` and its upper endpoint to be at most `2^127-1`. Every prefix interval
is contained in the full interval, since each term can be zero. A wider backend
is required if that exact proof fails; ordinary programme material is not a
substitute for a proof. One positive Q2.62 unity coefficient with Q1.63 input
needs exactly 126 signed accumulator bits, leaving two bits in i128.

The native polyphase bank performs this proof independently for every phase at
construction. A phase whose exact signed interval requires 129 bits is rejected
before processing begins; a legacy symmetric estimate of 129 is not itself a
rejection criterion. The `minimum_accumulator_bits()` accessor reports the exact
minimum, while `required_accumulator_bits()` preserves the older cache estimate.
If exact admission fails,
later planners may instead choose an input-headroom contract or a bigint
backend.

The Q65.63-input resampler shares the same scheduler, but its declared raw
sample range is `[-2^127,2^127-1]` instead of `[-2^63,2^63-1]`. Its exact
signed width is proved separately; it is not always the narrow exact width +64.
The native-coefficient bank first tries checked i128 products and sums. If
either overflows, it discards that attempt and recomputes the entire dot
product with GMP at its precomputed `minimum_wide_accumulator_bits()`. This
can require 193 bits for a newly admitted native bank. No partial result is
rounded or reused. Q1.63/bigint-coefficient banks use the larger of their
configured width +64 and their separately proved wide minimum. Both round
once into Q65.63, with error on final
Q65.63 overflow. For general Qm.n, CLI accumulator plans extend the baseline
conservative Q1.63-input reserve by `m+n-64` bits, then validate the actual
bank's exact range. Reports distinguish the baseline filter plan, signal format,
extended reserve, exact execution minimum and legacy symmetric cache estimate.
See [native wide fallback proof](native-wide-fallback-proof.md).

`PolyphaseFirBig` instead takes a `BigFirSpec` containing independent input,
output, and coefficient formats. For each phase it proves the exact full declared
input-range signed interval and either selects its minimum accumulator width
or rejects an insufficient explicit width. Every product retains
`input_fractional_bits + coefficient_fractional_bits`; the only rescale is
from that completed sum to the declared output format. `None` in the bank
spec requests an automatically proved finite width, not unchecked accumulation.
The existing MPFR/cache coefficient bank can be shared without copying its
GMP values, while native coefficients can be promoted losslessly once.

`CausalResamplerBig` and `InterleavedResamplerBig` retain owned BigQ samples,
including exact pre-roll zeros at the input format. They share the same
`Timeline` implementation with the native and wide façades. The phase clock,
finite prefix limit, deferred-phase history, pruning, and tail schedule are
therefore sample-domain independent. A mono processing error may leave an
already-completed output prefix and poisons the stream. The interleaved BigQ
adapter stages all channels' output before appending; any processing error
poisons that adapter. Neither API silently accepts mixed signal formats.

`NativeMac` generalizes the same rule to any pair of runtime formats. Every
`i64 * i64` product is exact in `i128`; all products retain the sum of the input
fractional widths; only `finish(destination)` rounds and narrows. An accumulation
error poisons the instance until an explicit `clear`, preventing callers from
continuing after a skipped term. Its `exact_requirements_for` calculator uses
the declared sample interval and actual coefficient signs. The older
`requirements_for` retains a conservative symmetric L1 estimate for compatibility;
neither assumes programme audio will be benign.

`BigMac` is the same contract over arbitrary-width integers. The planner may
give it a concrete signed accumulator width, in which case every exact addition
is range-checked and any violation poisons the accumulator. `None` selects a
mathematically unbounded accumulator. Neither form rounds or saturates a partial
sum. The final `finish` operation performs the only rescale, rounding, and
destination overflow decision. The test suite exercises a 4097-term MAC inside
an explicitly planned 8192-bit accumulator.
The [actual-boundary tests](big-mac-boundaries.md) additionally exercise products
that genuinely require 8192 signed bits, an automatically selected 8196-bit FIR
accumulator, and preservation of an output LSB after large-term cancellation.
Signed range endpoints are constructed once per `BigMac` and reused for each
check; this does not remove or weaken any check.

Arbitrary-width right shifts are rounded from an absolute magnitude, quotient,
and remainder. Consequently negative-value results do not depend on GMP's
implementation-defined-looking signed shift behavior, and match the native
backend for all five rounding modes.

## Reproducibility boundary

Analysis of an actual bank uses the same signal-format promotion and accumulator
checks as conversion. Its [reported accumulator/headroom](analyze-execution-accumulator.md)
refers to the full declared Q-format range, not to a measured programme peak or
the narrower baseline designer bank.

Bit-exact behaviour covers arithmetic, rate scheduling, coefficient bytes,
PRNG sequence, metadata transformations that are declared deterministic, and
output encoding. Filter design may initially use a noncanonical helper only if
its quantized coefficient table is serialized, hashed, reported, and reused;
Until-40k design uses MPFR with recorded precision and rounding.

## Exact rate state

Integer input/output rates are reduced by GCD to `up/down`. Output frame `t`
has the exact input coordinate `t * down / up`; the engine stores this as an
integer input index and a remainder in `[0, up)`. Chunked calls advance the same
state as single-frame calls. Neither a binary float ratio nor a floating-point
phase clock is permitted.

Finite files use an explicit frame-count policy. With nearest/ties-to-even, an
input prefix of `N` frames may expose at most `round_even(N * up / down)` output
frames. This prefix limit matters because the causal phase scheduler can make a
frame calculable before the final duration rule admits it; emitting that frame
would require an impossible retraction if the file ended at that prefix.

The finite push API therefore advances two exact states together: the rational
phase clock and the monotonically rounded prefix length. A ready phase blocked
by the prefix length is deferred. Its input anchor can then be older than the
most recently ingested frame, so history is indexed rather than assumed to be
anchored at the newest sample. Pruning retains the oldest sample required by
the next scheduled phase. Finalization recomputes the target from the total
real input count and rejects a different policy or target.

The unbounded causal API remains available for live streams and deliberately
has different duration semantics: it emits every computable phase immediately
and drains the full FIR tail. Finite and unbounded pushes cannot be mixed on one
stream instance.

## SIMD equivalence and dispatch

The scalar Q1.63 × Q2.62 dot product is the normative native implementation.
AVX2 and AVX-512 qualification backends decompose each signed 64-bit magnitude
into unsigned 32-bit limbs, form four exact partial products per lane, rebuild
the full 128-bit product, restore its sign, and add products with the same
checked `i128` operation in original tap order. They neither truncate a product
nor change the single final Q3.125-to-Q65.63 rounding boundary.

An instruction set being available is not enough to make it automatic. A SIMD
backend must pass edge/random differential tests, complete-stream native versus
bigint property tests, and a release-build throughput gate on a qualified
target. The current AVX2 and AVX-512 64×64 decomposition paths are explicit
testing/benchmark choices because they are bit-exact but did not outperform the
scalar 64×64→128 sequence on the first measured x86-64 target. Automatic
dispatch therefore remains scalar. This prevents “SIMD” from becoming a label
that silently makes the program slower or numerically different.
