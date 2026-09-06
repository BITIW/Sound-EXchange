# Benchmark and property-test tools

SeX ships two small engineering frontends in addition to the ordinary unit and
integration test suites. Neither changes the signal algorithm or relaxes its
determinism rules.

## `sex-fuzz`

Run the default deterministic batch with:

```text
cargo run -p sexfuzz --release -- --cases 1000
```

Use `--seed U64` to replay an exact root sequence. Failures print a complete
`sex-fuzz --suite NAME --case-seed U64` command for the actual failing case.
`--case-seed` bypasses the root generator, and cannot be combined with `--seed`
or `--cases`. Duplicate options are rejected. The default `--suite dsp`
preserves the original sequence and checksum. Its properties compare:

- one-shot and randomly chunked native Q1.63/i128 execution;
- native and GMP execution using the same quantized coefficients;
- exact available AVX2/AVX-512 kernels versus scalar narrow-input output;
- Q65.63 inputs up to the full wide range, comparing whole/chunked checked-i128
  plus GMP fallback against bigint-bank execution, including products beyond i128;
- runtime BigQ streams with 63, 127, 257, or 4096 fractional bits, including
  perturbations below the Q*.63 grid, versus whole/chunked execution and an
  independent absolute-coordinate raw-integer FIR oracle;
- mono through four-channel interleaved ordering and state;
- exact finite output length under nearest/ties-to-even duration rounding;
- signed PCM import/export for widths from 1 through 32 bits.

Ratios currently draw both numerator and denominator from `1..=16`, FIRs use
odd lengths from 3 through 15, and files use 1 through 128 frames. This tool is
a deterministic property driver, not a replacement for coverage-guided
instrumented fuzzing; the latter remains future work.

### Bounded WAVE I/O properties

The [streaming write contract](streaming-write-contract.md) adds byte-level
write/seek/flush fault injection, poisoning and report-accounting tests, classic
RIFF overflow guards, and truncated-input transactional CLI cleanup. It brings
the native suite to 425 passing tests, without multi-gigabyte fixtures.

The [streaming read contract](streaming-read-contract.md) records the subsequent
production fixes for eager allocation, interrupted reads and terminal decode
errors, with 416 passing native tests and explicitly separated ARM/runtime
coverage. Require installed libsndfile assertions with
`SEX_REQUIRE_SNDFILE_TESTS=1 cargo test --release -p sexio-sndfile`.

```text
sex-fuzz --suite io --cases 10000 --seed 42
sex-fuzz --suite all --cases 100 --seed 42
sex-fuzz --suite io --case-seed 16294208416658607535
```

`io` generates independent minimal integer-PCM WAVE fixtures (8/16/24/32 bits,
1–4 channels, 0–128 frames), including signed endpoints and random samples.
An integer-shift oracle checks exact Q1.63 decoding. Production writer output
must be byte-identical for randomized Q1.63 chunks and one-shot signed PCM;
both decode back to the oracle with exact frame accounting and no saturation.
No audio or corpus files are written by the driver.

Every incomplete 44-byte-header prefix must be rejected. Additional cases
truncate payloads, zero or fill each structural field with ones, and flip
random header/payload bits. Mutated inputs may be valid: the requirement is
identical acceptance, spec and successful decoded PCM with whole versus short
reads and independently varied frame chunks. Rejected streams are compared
as rejection, not by error text or the partial prefix returned before failure.
Zero-frame requests consume no input; EOF remains stable. Parser panics are
failures, not swallowed errors.

Each decode is capped at 65,536 source-read calls and 4,096 accumulated samples;
exceeding either fails the property rather than counting as safe rejection.
Generated data are at most 2,092 bytes; mutated large channel counts force
one-frame requests (at most 65,535 samples in a temporary decoder chunk).
The driver never sizes input storage from a mutated chunk length. These are
harness limits, not a new sandbox or production parser resource guarantee.

A separate regression declares 4,294,967,288 PCM bytes while providing only a
52-byte header-plus-frame fixture. With one-byte reads, construction consumes
44 bytes, reading one stereo PCM32 frame consumes eight more, and requesting
the missing next frame returns an error. This checks on-demand behavior and
truncation at large declared lengths, **not** a complete 4 GB traversal, RF64,
libsndfile/FLAC malformed-input qualification, or hundred-gigabyte output.

Qualification: 10,000 cases at root seed 42 pass on native x86-64 and emulated
AArch64 with checksum `222161181ef0c15b`. The complete stdout SHA-256 is
`643ed41d6326c60f702fd39940b0290ff242c975b952cc45ff9cfc5dd5d0a64e`
on both, recorded in `target/io-property-{native,arm}-10000.txt`. Three I/O unit
tests also pass on emulated AArch64. Native CLI tests verify direct replay
against the independently known first SplitMix64 output for all three suites.
This uses the existing QEMU-only cross-linking configuration, not physical
ARM hardware qualification. The full native suite passes 406 tests, with one
existing heavy opt-in test ignored; native/cross strict Clippy and formatting
checks pass. Logs: `target/io-property-workspace-tests.log` and
`target/io-property-arm-tests.log`.

## Arbitrary-width global designer qualification

For arbitrary-width global LS/Remez library coverage, independently of the CLI:

```text
cargo test --release -p sexfir optimized
cargo run --release --example optimized_probe
cargo run --release --example optimized_probe -- --cache-directory PATH
```

The [optimized-designer contract](optimized-designers.md) distinguishes
coefficient-only refinement, spectral qualification, and known solver failures.
The v2 corpus covers thirteen real integer banks/streams, including the repaired
large Remez case at two working precisions. It does not claim optimized-method
CLI support. The cache option exercises persistent library bank/report reuse,
including precision-search candidates, on cold and repeated runs; response
and proof checks are repeated after hits. The preceding v1 corpus retains its
historical failure record.

## Explicit Until-40k CLI qualification

```text
cargo test --release --test cli_reproducibility until_40k_real_filter_meets_floor_and_streams_reproducibly -- --ignored --nocapture
```

This deliberately opt-in test designs a 133733-tap, 512-fractional-bit Kaiser
bank at MPFR precision 1024 for ratio 1/2. It requires the unchanged -600 dB
passband/stopband/coefficient gate on 65 points per band, with feedback enabled
for analysis and every conversion. It checks that the original bank passes in
one candidate, then checks Q65.512
conversion, coefficient-cache reuse, and byte equality at block sizes 1, 7,
and 4096. The nine-frame PCM fixture and roughly 9 MB temporary cache are
removed after the test. It requires CPU time, not a huge audio file. Passing
this sampled single-ratio test is not a continuous-band or all-ratio proof.

## Continuous response qualification

```text
cargo test -p sexfir certificate
cargo test --test cli_reproducibility analyze_
sex analyze input-48k.wav -r 16000 --preset sane --certify
sex analyze input-48k.wav -r 24000 --preset high --certify --certificate-taps 2049
```

The fourteen certifier unit tests include independent exact-rational oracles,
F=4096 coefficient codes, peaks hidden from an endpoint grid, explicit limits,
and regressions for the old Sane/High failures followed by corrected-bank
certificates. CLI tests check proof reports, nonzero inconclusive status, and
Until-40k's pre-design tap-limit rejection without creating a cache. The
[certificate contract](continuous-response-certificate.md) specifies bounds,
commands, limitations, and measured work counts. Resource-limited exact
certification must never silently fall back to a grid-only quality pass.

## Feedback-driven quality qualification

```text
cargo test -p sexfir refinement
cargo test -p sexplan refinement
cargo test --bin sex response_feedback
cargo test --test cli_reproducibility default_
SEX_REPRO_CODECS=1 SEX_REPRO_FEEDBACK=0 scripts/validate-cross-architecture.sh target/release/sex scripts/cross/aarch64-sex.sh NEW_RESULT_DIRECTORY
```

The [feedback contract and regression evidence](quality-refinement.md) cover
independent coefficient-width and Kaiser-shape growth, exact certification
after a legacy failure, coupled whole-chain replanning, cold/hot cache
identity, resource failures, and existing-output preservation. The optional
cross-corpus switch can still exercise the redundant `--refine-quality` alias;
feedback now runs by default even when `SEX_REPRO_FEEDBACK=0`. The corpus checks
byte-identical audio and independently generated caches even when a preset
needs multiple assessed candidates. It does not itself request continuous or
all-image analysis, and Until-40k remains a separate opt-in case.

## Shared conversion/analysis qualification

```text
cargo test --test cli_reproducibility shared_
cargo test --bin sex gate_configuration
SEX_REPRO_CODECS=1 SEX_REPRO_SHARED_GATES=1 SEX_REPRO_SHARED_UNTIL=1 scripts/validate-cross-architecture.sh target/release/sex scripts/cross/aarch64-sex.sh NEW_RESULT_DIRECTORY
```

The [shared gate contract](shared-quality-gates.md) explains how the report is
bound to the actual bank consumed by conversion. The harness now compares
qualification blocks for every resampling case as well as full PCM/cache bytes.
The first optional switch adds three cases with continuous and harmonic checks,
including bigint and amplified/normalized processing. The second adds a real
Until-40k 2/1 bank with sampled harmonic checks. Each added case is rendered
four ways and separately analyzed on both architectures; all six qualification
blocks must agree. There is no continuous long-FIR or all-image proof implied
by that Until-40k sampled check.

## Whole-chain numerical-bound qualification

```text
cargo test -p sexplan numerical
cargo test --test numerical_budget
cargo test --test cli_reproducibility amplified_cli_refines
```

The first command checks outward-rounded coefficient bounds and recursive DC
error against exact rational arithmetic. The integration test independently
computes the ideal rational gain/DC/mix/convolution/FIR/normalization chain and
requires every actual BigQ output error to fit its predicted bound, including
4096-bit signals and three block sizes. The CLI regression checks coupled
precision growth and distinct cache identities. The proved reference and
exclusions are specified in [numerical-error-budget.md](numerical-error-budget.md).

## All-image harmonic measurements

```text
cargo test -p sexfir harmonics
cargo test --test cli_reproducibility harmonic
scripts/validate-harmonic-responses.sh target/release/sex NEW_RESULT_DIRECTORY
SEX_HARMONIC_UNTIL=1 scripts/validate-harmonic-responses.sh target/release/sex NEW_RESULT_DIRECTORY scripts/cross/aarch64-sex.sh
```

The [harmonic analysis contract](harmonic-response-analysis.md) documents the
complete component model, exact rational coordinates, MPFR precision, and
budgets. The harness covers five Sane ratios, High at two ratios, and optionally
Until-40k at 2/1; a second executable must produce identical full reports and
coefficient caches. This is sampled all-image qualification, not a continuous
proof or an audio-file-size stress test.

## Four-engine spectral measurements

```text
cargo test --example spectral_probe
cargo test -p sexfir sane_default_target_is_qualified_beyond_the_old_sparse_grid
scripts/validate-spectral-references.sh target/spectral-qualified
```

The [campaign documentation](spectral-reference-campaign.md) describes the
required SoX/FFmpeg/libsamplerate runtimes, isolated SoX setup, fixture matrix,
and distinct waveform/fitted-response gates. The probe checks its FFT against
an independent direct DFT, calibrates window normalization, and rejects NaN
in gates. The dense-grid regression preserves the original Sane failure and
requires its corrected default floor without weakening the target.

## Cross-architecture reproducibility

```text
SEX_REPRO_CODECS=1 SEX_REPRO_UNTIL=1 scripts/validate-cross-architecture.sh target/release/sex scripts/cross/aarch64-sex.sh target/cross-repro-full
```

This requires two independently built frontends with different architecture
identities and a new result directory. The optional Until-40k case designs the
real 133733-tap bank twice; the audio fixtures remain tiny. Each scenario checks
four renderings, independent coefficient identities, and cache interchange in
both directions. Output files, commands, build identities, source/input hashes,
and caches are retained for inspection rather than automatically deleted.

The [isolated AArch64 setup and recorded results](cross-architecture.md) explain
the required toolchain/runtime variables and emulator-only linking limitation.
That page also records executed AArch64 library, integration, and property tests;
ordinary CLI integration tests cannot simply spawn a foreign-architecture
executable without a launcher.

`SEX_REPRO_WINDOWS=1` adds four checked window cases: Fast Hann 3/2,
Fast Blackman 1/3 with 2^60 gain/normalization, High Dolph 1/2, and Fast Dolph
3/2 (fractional phases). Each adds four PCM renders and two standalone analyses,
comparing entire bank-bound qualification blocks, certificates, all-image
reports, and cache files. They can be combined with `SEX_REPRO_SHARED_GATES=1`
and `SEX_REPRO_SHARED_UNTIL=1`. New library/CLI regressions can be run with:

```text
cargo test --release --workspace window
```

The [window contract](windowed-cli.md) records C=4096/DC/cache tests, the
signed-bin Dolph regression, and window-preserving bounded feedback.

## `sex-bench`

Run a representative comparison with:

```text
cargo run -p sexbench --release -- --backend both
```

The benchmark accepts input/output rates, odd taps per phase, channels, frames,
and iteration count. It feeds deterministic Q1.63 samples through the same
finite rational scheduler and FIR interfaces used by the CLI, then prints a
checksum with its timing. Native and GMP runs must report the same checksum for
the same configuration.

`--native-kernel auto|scalar|avx2|avx512` permits controlled qualification of
the exact native multiplication kernels. `auto` uses only a backend that has
passed the project's performance gate; CPU feature detection alone does not
promote an implementation. SIMD backends requested on unsupported CPUs fail
explicitly. The checksum must remain identical across every available choice.

Elapsed time and derived throughput are reporting data only. Floating-point
formatting of those measurements is outside the audio signal path and has no
effect on samples, coefficients, scheduling, or output bytes. Benchmark values
are machine- and build-dependent and are not quality claims.
