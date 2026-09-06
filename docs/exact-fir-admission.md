# Exact admission for native and Q1.63-coefficient banks

Both `PolyphaseFirQ63` and `PolyphaseFirBigQ63` now admit banks using their
[exact signed intervals](exact-accumulator-intervals.md), rather than requiring
a symmetric absolute-L1 estimate to fit. All phases must pass. A valid bank
can therefore use the full negative endpoint; an attainable positive overflow
still fails before execution. Multiplication, tap order, checked accumulation,
one final rounding, clipping and automatic SIMD selection are unchanged.

## Native i128 boundary

Four positive Q2.62 unity coefficients with Q1.63 input have exact raw interval

```text
[-2^127, 2^127 - 2^64]
```

It fits i128. The preceding constructor rejected this bank because its
symmetric estimate required 129 bits. The negative-unity version can reach
`+2^127` and is still rejected. A mixed-sign vector with raw coefficients
`[i64::MAX, 2, i64::MIN]` can attain both `i128::MIN` and `i128::MAX`; it too
is now admitted. The proof covers every prefix sum, not just the final sum.

`minimum_accumulator_bits()` exposes the exact maximum requirement across
phases. Native construction guarantees this is at most 128. Output outside
Q1.63 is still an error or explicit saturation when that output API is used;
`convolve_wide` preserves the wider result. Allowing the accumulator endpoint
does not change clipping policy or silently wrap.

## Q65.63 needs its own proof

For the GMP Q1.63 bank, extending an exact baseline width by 64 is not always
sufficient for Q65.63 input. The endpoint asymmetry can change which width is
minimal as the input range widens. The constructor therefore also computes
`minimum_wide_accumulator_bits()` across all phases. Wide execution uses

```text
max(configured_Q1.63_accumulator_bits + 64, minimum_wide_accumulator_bits)
```

The public baseline width remains unchanged. Existing explicit reserves are
retained; width overflow and the bigint safety ceiling still fail explicitly.
The wide proof is computed once at bank construction, not once per output.

A regression uses Q2.96 raw coefficients `[c, -c, -3]`, `c = 2^95 + 1`.
Its exact Q1.63 requirement is 160 bits, but Q65.63 requires 225, not 224.
For wide raw samples `[i128::MAX, i128::MIN, i128::MAX]`, the first two taps
exceed the 224-bit positive endpoint. The third tap cancels enough for the
final, once-rounded result to fit:

```text
raw sum = 2^223 - 2^127 - 2^95 + 2
rounded Q65.63 raw output = 2^127 - 2^31
```

The test demonstrates that a 224-bit control fails on the second tap, while
the admitted bank matches an independently unbounded GMP MAC through output.
This prevents an intermediate overflow from being mistaken for final clipping.

## Cache and report compatibility

`required_accumulator_bits()` and the corresponding baseline designer-report
fields retain their legacy **symmetric L1 estimate**. Their names predate the
exact API. They may now be one bit larger than a newly admitted bank's actual
configured width. New execution callers should use
`minimum_accumulator_bits()`; the wide input API has its separate exact method.
The field documentation states this distinction explicitly.

This preserves serialized report values, coefficient identities, cache keys,
and file schemas. Cache loading recomputes both the exact admission proof and
the legacy estimate, comparing the latter to its stored value. Existing cache
integrity checks are not relaxed. The CLI's actual-signal execution report,
introduced previously, continues to use the exact runtime requirement rather
than the legacy baseline report.

A real three-tap unity Kaiser design/cache regression checks native minimum
126 versus legacy 127, and Q2.96 GMP minimum/configuration 160 versus legacy
161. The first design stores successfully, the second lookup is a cache hit,
and the complete loaded design equals the original. This is an actual designer
output, not a fabricated cache-report fixture.

## Verification

The native endpoint test executes scalar, AVX2 and AVX-512 on this host. Banks
are padded to eight taps so the AVX-512 check actually enters its vector loop,
not only a scalar tail. Raw and rounded endpoints agree; saturation/error
remain explicit and an unsafe second phase is rejected. Both ARM endian
profiles execute the scalar path, the new wide-input regression and the real
cache roundtrip regression. No ARM SIMD capability is claimed.

All 456 native release workspace/all-target tests pass (one heavy opt-in
ignored), with libsndfile availability required. Warnings-denied Clippy passes
on native and both ARM target profiles. Formatting passes. Logs and build
identities are retained under `target/native-admission-qualified`.
Complete root/Q/rate library runs pass 125 tests on each ARM endian profile.

The ordinary cross-architecture corpus uses the established QEMU-only toolchain
and a fresh result directory:

```sh
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_QEMU_AARCH64=qemu-aarch64_be-static \
SEX_REPRO_CODECS=0 SEX_REPRO_UNTIL=0 SEX_REPRO_OPTIMIZED=1 \
scripts/validate-cross-architecture.sh target/release/sex \
  scripts/cross/aarch64_be-sex.sh target/native-admission-corpus-qualified
```

It selects the existing fourteen cases, independent cold caches, both foreign-
cache directions and blocks 1/7/4096. The comparison is bounded to those cases;
this does not requalify Until-40k, external codecs on big-endian, physical ARM
or production linking.

The completed corpus passes 56 renders and six analyses, reciprocal cache
comparisons and the unchanged-source check. A separate baseline comparison
matches all 56 WAV files, 56 complete processor reports (excluding only the
recorded command/path line), six analyses, twelve cache files and all fourteen
[previous PCM/coefficient identity rows](big-endian-identities-2026-09-06.tsv).
The newly admitted edge banks are covered by the dedicated tests above; ordinary
existing designs retain their audio, qualification and serialized-report bytes.

Artifacts are in `target/native-admission-corpus-qualified`, and the comparison
script/log are under `target/native-admission-qualified`. The corpus source
fingerprint is `9a44d4137e4adfc97d16bc378264263ed605ba11d71323ca56da067d091c1987`
(Cargo files plus sorted src/crates/examples/scripts; excludes docs and
top-level integration tests). Executed main CLI hashes are native
`966afc195384263bc90059dc5b5bdb64720ab960ad13c7880e8164d594660462`
and big-endian
`d623ee26d86098d865115adb527d76e2b54a230b78620da8d87baa651db076f0`.
