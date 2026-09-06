# Exact signed accumulator intervals

Automatic arbitrary-Q FIR construction now computes the **minimum signed
accumulator width** for the complete declared input range and actual
coefficients. It no longer adds a bit merely because the negative endpoint has
the same magnitude as the next positive power of two. The CLI's actual-bank
execution requirement uses the same calculation, including the extended native
Q65.63 stream. This changes a range proof, not products, accumulation order,
coefficients, rounding, dither or sample precision.

## Formula and minimality

For a W-bit signed sample, raw input is `[-S, S-1]`, where `S = 2^(W-1)`.
Let P be the sum of positive raw coefficients and N the sum of the magnitudes
of negative raw coefficients. Zero coefficients contribute nothing. Then

```text
A = S * (P + N)
minimum_raw = -A + N
maximum_raw =  A - P
bits = 1 + max(bit_length(max(-minimum_raw - 1, 0)),
               bit_length(maximum_raw))
```

`bit_length(0) = 0`; an empty or all-zero sum therefore needs one signed bit.
All quantities are integers. Fractional widths determine the product's binary
point but do not change the raw signed-range argument.

The extrema are attainable: for a minimum, choose the negative sample endpoint
at positive coefficients and the positive endpoint at negative coefficients;
reverse those choices for a maximum. Every term's possible interval contains
zero, so the complete interval contains every prefix interval too. The selected
width therefore protects intermediate sums, not merely a cancellation-dependent
final sum. One fewer bit fails at an attainable endpoint unless the width is
already one. Correlations in a particular recording could give tighter bounds;
this is the minimum for independent inputs over the full declared format.

Examples:

| Configuration | Previous symmetric width | Exact signed width |
| --- | ---: | ---: |
| Q1.63 sample × one positive Q2.62 unity coefficient | 127 | 126 |
| Q1.63 × four positive Q2.62 unity coefficients | 129 | 128 |
| Q1.63 × four negative unity coefficients | 129 | 129 |
| W=8192 input × raw coefficient +1 | 8193 | 8192 |
| W=8192 input × raw coefficient -1 | 8193 | 8193 |

In the second case `-2^127` is representable by i128, whereas `+2^127` in the
third case is not. Saturation or wrap is not used to make a narrow width fit.

## API and integration

`NativeMac`, `MacQ125`, `MacQ126` and `BigMac` provide additive
`exact_requirements_for` APIs. Native `ExactAccumulatorRequirements` exposes
the negative/positive magnitudes as u128 and a minimum signed width;
`BigExactAccumulatorRequirements` exposes GMP `minimum_raw`, `maximum_raw`
and the width. The native API reports overflow if its interval calculation
exceeds u128 storage; it does not truncate. It works without the bigint feature.

`PolyphaseFirBig` checks all phases with the exact API. With
`BigFirSpec::accumulator_bits = None`, it selects the maximum of their minimum
widths. Explicit widths remain unchanged but must contain every phase interval.
All MAC operations still check overflow after each exact addition and round
once at output. The existing 8196-bit cancellation regression remains valid.

The CLI uses this same bank validation for bigint execution. For its extended
native stream it promotes one coefficient phase at a time to compute the exact
requirement, without copying the complete FIR bank. The execution backend stays
checked-i128/GMP-fallback; its mathematical interval can exceed i128, and the
fallback still handles those values exactly.

The legacy `requirements_for` APIs and baseline native/Q1.63 designer-bank
reports retain their documented conservative symmetric L1 contract. Cache
validation and serialized report bytes therefore remain compatible; no cache
key or file-format migration is needed. A subsequent
[native/Q1.63 admission update](exact-fir-admission.md) now also uses the exact
API for those constructors, with a separate proof for extended input.
Cheap `plan` still reserves a
pre-design bound, and explicit CLI/preset accumulator reserves are not reduced.
The actual-bank `analyze`/conversion **required** width is exact; **planned**
width can intentionally be larger. Neither is a measured audio peak.

## Evidence

An exhaustive oracle covers all 125 three-tap coefficient tuples in `[-2,2]`
and every sample tuple for signed widths 1 through 4. It checks attainable
minimum/maximum, every partial sum, one-bit-smaller rejection and agreement
between native and GMP interval APIs. Further tests check format mismatches,
empty/all-zero banks, native overflow, i128's asymmetric endpoint, and attained
GMP endpoint sums through 8192-bit sample formats.

The resampler test checks automatic and explicitly insufficient widths,
positive versus negative phases, and endpoint-preserving output at F=0/31/63/
4096. A root execution-summary test checks all phases and exact requirements
for Q65.63 and Q65.4096. The five new tests execute on native x86-64 and both
little- and big-endian AArch64 QEMU. These are bounded emulated-platform tests,
not physical-hardware or all-ABI qualification.

Build/test/qualification artifacts are retained in `target/exact-mac-qualified`.
The full native release workspace/all-target run passes 453 tests (one heavy
opt-in ignored), with actual libsndfile availability required. The two native
interval tests also pass with bigint disabled. Warnings-denied workspace/
all-target Clippy passes on all three target profiles; formatting passes too.
Complete root/arithmetic/rate library runs pass 123 tests on each ARM endian
profile, including the pre-existing streaming and wide-cancellation regressions.

The independent native/big-endian corpus is reproducible with the established
experimental toolchain and a new output directory:

```sh
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_QEMU_AARCH64=qemu-aarch64_be-static \
SEX_REPRO_CODECS=0 SEX_REPRO_UNTIL=0 SEX_REPRO_OPTIMIZED=1 \
scripts/validate-cross-architecture.sh target/release/sex \
  scripts/cross/aarch64_be-sex.sh target/exact-mac-corpus-qualified
```

This selects fourteen cases: default presets through Pointless, the 4096-bit
stateful chain and amplified feedback, plus LS/Remez/amplified-LS. It retains
independent cold caches, reciprocal cache reads and blocks 1/7/4096. Until-40k
and big-endian external codecs are not rerun by this corpus.

The completed run passes all 56 renders and six optimized analyses. Independent
CPU output/cache/qualification comparisons and the unchanged-source check pass.
A separate baseline comparison also checks every rendered WAV, every complete
processor report (excluding only its recorded command/path line), all six
analyses, and twelve complete cache files against the preceding big-endian
campaign: all bytes match. All fourteen rows of the
[existing identity table](big-endian-identities-2026-09-06.tsv) are retained.
These ordinary designed filters keep the same reported width; the deliberately
constructed endpoint tests above demonstrate the newly smaller exact widths.

Artifacts occupy about 6.3 MiB in `target/exact-mac-corpus-qualified`; baseline
comparison commands and successful output are in
`target/exact-mac-qualified/compare-baseline.sh` and `baseline-comparison.log`.
The corpus source fingerprint is
`5b4bf0e6e9db5f82d3a78904228042719cb71cc8ab2f01e4ded86af7173338da`
(same source-manifest scope as the preceding campaign).
Executed main CLI hashes are native
`3e5df96d2d06a20975be89e58d3603da70fc17f6a49732336869e1420038e681`
and big-endian
`e6a7116554b6c0eda0f483c662793ef75811cdbdf446144e9a72e9e489f6d4a1`.
