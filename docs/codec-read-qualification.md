# ARM integer-codec read qualification

The isolated AArch64 validation environment now includes libsndfile 1.2.2
and its codec dependencies. This closes the runtime-availability gap recorded
in the initial [streaming-read checkpoint](streaming-read-contract.md): the
tests no longer return early because libsndfile cannot be loaded.

## Isolated setup and strict execution

The existing `scripts/cross/bootstrap-codecs.sh` fetched eight pinned Debian
ARM64 packages, verified each against `scripts/cross/arm64-codecs.sha256`, and
extracted them only into `/tmp/sex-cross-toolchain.kdS7JT/sysroot`. It did not
install system packages. Downloaded archives occupy about 3.7 MiB. The package
manifest SHA-256 is
`4e528d21888203e6323ed6b1c07760e6056cc63e3ea99837d8ec287a767f46c3`.

The strict run was:

```sh
SEX_REQUIRE_SNDFILE_TESTS=1 \
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_QEMU_VALIDATION_ONLY=1 \
scripts/cross/build-aarch64.sh test --release -p sex -p sexio-sndfile --lib
```

All 30 root-library tests and all 10 libsndfile tests pass. The runtime-required
flag forbids unavailable-library skips. This includes actual WAVE/AIFF/FLAC
integer round trips, UTF-8 metadata, the native-library read error, and huge
upper-bound requests on tiny files, not just the mocked reader state machine.
The log is `target/codec-arm-required-qualified-tests.log`.

Select the root `sex` package too: the cross build wrapper enables its
`cross-gmp` feature. A preceding invocation selecting only `sexio-sndfile`
failed Cargo feature selection before executing tests; that log is retained
as `target/codec-arm-required-tests.log` and is not passing evidence.

## Actual multiblock AIFF reading

The existing roughly 94 KiB AIFF fixture from the streaming-read checkpoint
was decoded on ARM with blocks 1 and 4294967295. Both resulting WAVE files
match each other and the existing x86-64 output byte for byte:

`a8ad65abf0739d30b37aaed6792b9d1d2b5a32fb0bb24a8c29b1cbbfc86924be`.

This reaches multiple bounded native scratch reads without generating a large
input. Results and backend-version logs are under
`target/codec-large-block-qualified`, about 204 KiB. The source/reference files
remain in `target/read-boundary-cli-qualified`.

## Focused codec corpus

The architecture harness now supports `SEX_REPRO_CODECS_ONLY=1`. It also
enables tagged fixture generation, skips the unrelated base preset corpus,
and rejects simultaneous `SEX_REPRO_OPTIMIZED_ONLY=1` before creating results.
With neither only-mode set, the previous default corpus is unchanged.

```sh
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_REPRO_CODECS_ONLY=1 \
scripts/validate-cross-architecture.sh target/release/sex \
  scripts/cross/aarch64-sex.sh NEW_RESULT_DIRECTORY
```

Each format uses independent native and ARM designs, then reads each other's
coefficient cache. It compares complete output bytes, actual coefficient
identities, complete cache files and the bank-bound qualification block.
Fixtures include typed UTF-8 metadata. Outputs use High 44100→48000,
PCM16, seed 42, noise-shaped-5 and normalization, with input blocks 1/7/4096.

The first focused attempt omitted the fixture-generator switch and stopped
at a missing tagged WAVE before conversion. This was fixed in the harness;
`target/read-boundary-codecs-qualified` is retained as a failed pilot, not a
successful codec result. Successful results must be reported separately.

### Completed codec result

The corrected run in `target/read-boundary-codecs-v2-qualified` completed with
exit 0. All twelve renders match within their format: independent native/ARM
designs and both foreign-cache directions, at all three block sizes. All twelve
bank-bound qualification blocks match too. The entire first/second cache
directories compare equal, and the harness confirms its source fingerprint is
unchanged across the run. Artifacts occupy about 2.5 MiB.

Durable [output/coefficient/report identities](codec-read-identities-2026-09-06.tsv)
record the three formats. The accepted bank is High, 160 phases × 555 taps,
C=96, MPFR=256, Q65.96 signal, minimum accumulator 259 bits. It passes the
existing -160 dB target at 65 points per band in one candidate; sampled stopband
is about -167.087 dB. This report is a sampled filter gate, not a continuous
proof, MPFR design enclosure or measurement of final noise-shaped PCM error.

Cache-file SHA-256:
`e085e6bec51501264f3b8a8c5918b7525baa60e387a768e8b0aeab799dd7c77d`.
Source fingerprint (sorted Cargo files, src/crates/examples/scripts; excludes
docs and top-level integration tests):
`3a39a76077ab7c25a64138de198116998d6a300273d990d2545eab3da6f53908`.
Executed binary SHA-256:

- x86-64: `0d31567d9f4eac3519cb159ac847232c6a36f53b326916678cec9bf449a3878b`;
- AArch64: `51710b51e40052b77ced0a84de913b24a4f55418feff0671b5f6206af24ad53e`.

Both runtime logs identify libsndfile 1.2.2. The conflicting-only-mode check
also returns status 2 before creating an output directory
(`target/codec-conflict-check.log`); shell syntax validation passes. Production
Rust sources did not change in this qualification step, so the preceding
416-test native checkpoint remains the applicable full-suite result; this
step adds strict actual ARM codec execution and complete codec output evidence.

## Limits

This is QEMU Cortex-A53 on an AArch64 Linux sysroot using the existing
validation-only linking configuration. It does not qualify physical ARM,
production linking, big-endian or 32-bit ABIs, arbitrary decoder versions,
malformed compressed-file fuzzing, or full multi-gigabyte traversal.
