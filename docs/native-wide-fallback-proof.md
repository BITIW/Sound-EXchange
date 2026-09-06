# Native-coefficient Q65.63 fallback proof

The [exact admission update](exact-fir-admission.md) proved the wider domain for
the GMP-coefficient bank, but missed the native-coefficient fallback's fixed
192-bit allocation. The native bank can now admit a narrow signed interval
whose symmetric L1 exceeds the old limit, so that old wide allocation was no
longer justified for every admitted bank.

`PolyphaseFirQ63` now computes `minimum_wide_accumulator_bits()` independently
for Q65.63 across all phases at construction, using the exact signed interval.
Scratch coefficient promotion is bounded to one phase. Checked-i128 execution
is still tried first; on overflow it discards that attempt and recomputes the
entire dot product in GMP with the proved wide width. The width is not guessed
from the narrow exact minimum or hard-coded to 192. It can be 193.

The CLI's Q65.63/native summary reuses this same stored proof. Other declared
signal formats still get their own exact calculation. Coefficients, cache
serialization, PRNG state, rounding order and output format policy do not change.

## Reproduced failure and corrected scope

Let the Q2.62 raw coefficients be `[i64::MAX, 2, i64::MIN]`. The exact Q1.63
interval fits both signed i128 endpoints, so admitting the bank is correct.
But for Q65.63 input the worst positive sum is

```text
2^191 + 2^127 - 2^63 - 1
```

It needs 193 signed bits. The regression chooses wide raw samples
`[i128::MAX, i128::MAX, i128::MIN]`. Before the fix it receives
`AccumulatorOverflow { bits: 192 }`. With the corrected proof the complete
sum is formed exactly, then final Q65.63 conversion correctly reports a
destination overflow, matching an unbounded GMP oracle.

This particular case does **not** have a representable final output; no claim
is made that this test recovers a previously rejected valid PCM file. It fixes
the accumulator invariant and the failure boundary. The preceding
GMP-coefficient 225-bit cancellation test has a representable final output and
remains separate evidence, not this native test's result.

A second regression compares both phases of a bank containing unity and the
193-bit case across all 343 triplets from seven wide endpoint/interior values,
and all five rounding modes: 3430 output/error comparisons against an unbounded
GMP MAC. It exercises the small checked-i128 path and large fallback values,
including final overflow. Zero, positive-unity and negative-unity banks report
wide minimums 1, 190 and 191 respectively. The later phase's 193-bit requirement
must control the combined bank, not only the first phase's width.

The normative [fixed-point model](fixed-point-model.md) now distinguishes
sufficient symmetric bounds from necessary-and-sufficient signed intervals,
and describes the actual fallback implementation. The stale claim of one guard
bit for unity is corrected to two: positive Q2.62 unity with Q1.63 input needs
126 signed bits, not the legacy conservative estimate of 127.

Before/after logs and qualification artifacts are retained under
`target/native-wide-proof-qualified`. The full original goal remains open;
these tests qualify the corrected fallback boundary, not all project requirements.

All 458 native release workspace/all-target tests pass (one heavy opt-in ignored),
with actual libsndfile availability required. Root/Q/rate library runs pass 127
tests on each little-/big-endian ARM QEMU profile. Warnings-denied Clippy passes
on all three target profiles, and formatting passes.

The native/big-endian corpus completes all fourteen cases: 56 renders, six
optimized analyses, reciprocal cache reads and unchanged-source checks. The
separate baseline comparison preserves every WAV, all 56 complete processor
reports (excluding only the recorded command/path line), six analyses and twelve
cache files. All previous [identity rows](big-endian-identities-2026-09-06.tsv)
remain applicable. Results are under `target/native-wide-proof-corpus-qualified`;
the comparison script/log are under `target/native-wide-proof-qualified`.

The corpus source fingerprint is
`103ff016964b3c1a30e62645c07eba3a549834c52ce5e45556e46edd6fb3abf7`
(Cargo files plus sorted src/crates/examples/scripts, excluding docs and
top-level integration tests). Executed main CLI hashes are native
`4aa00b6ededc6ab872f50eb2835884f038417bd638ab16ae828820d9d784ace8`
and big-endian
`2d43da9be30cee52a4689cda53fe3abc9afc094592f81e4d94fb6a57b95f9c6a`.
This bounded corpus does not execute the newly identified Until-40k
[default-budget gap](goal-audit-open-items.md), nor requalify external codecs
on big-endian, physical ARM or production linking.
