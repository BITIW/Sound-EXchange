# Cross-architecture reproducibility qualification

Status: x86-64 Linux versus little-endian AArch64 Linux under QEMU execution.
The historical campaigns below are not physical ARM or 32-bit ABI tests.
The separate [experimental big-endian qualification](big-endian-qualification.md)
documents a SHA-256 portability defect and the target-specific fix; its
static-musl codec limitations differ from the little-endian GNU profile.

## 2026-09-06: hardened streaming reads with actual ARM codecs

The [codec read qualification](codec-read-qualification.md) closes the missing
libsndfile runtime in the latest isolated ARM sysroot. All ten codec-library
tests execute with unavailable-library skips forbidden, alongside 30 root
library tests. The focused High 160/147 codec corpus passes twelve native/ARM
WAVE/AIFF/FLAC renders, including reciprocal cache reads and whole report/file
comparisons. Real multiblock AIFF reading at block sizes 1 and 4294967295 also
matches native output. [Exact identities](codec-read-identities-2026-09-06.tsv)
and package/binary/source fingerprints are recorded; production Rust is
unchanged from the 416-test streaming-read checkpoint.

Use `SEX_REPRO_CODECS_ONLY=1` with the existing cross-architecture harness to
repeat only this corpus. It enables tagged fixture generation and excludes
the base preset cases; it cannot be combined with `OPTIMIZED_ONLY`. See the
linked qualification for full commands and the failed-pilot distinction.

## 2026-09-06: shared library and standalone frontend qualification

All 342 native release workspace/all-target tests pass (one existing heavy
test ignored), both new frontend parser tests pass on AArch64 QEMU, and
warnings-denied all-target Clippy passes on both platforms. The original CLI
tests now exercise the single library implementation through `sex`; new tests
compare `sex-rate` conversion and `sex-analyze` reports, wrong-mode rejection,
preservation of output on failure, information commands and non-UTF-8 failure
without panic. Module movement does not duplicate the engine into three parsers.

The standalone corpus passes three cases, twelve PCM renders and six analyses,
including native/ARM independent cold designs and both foreign-cache directions.
Every PCM/coefficient row, full qualification block and cache file compares
unchanged with the preceding Remez-cosine corpus. The durable
[qualification](remez-cosine-qualification-2026-09-06.tsv),
[PCM/coefficient](optimized-cli-identities-2026-09-06.tsv), and
[cache](optimized-cli-cache-2026-09-06.tsv) identities therefore remain applicable.

```sh
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_REPRO_OPTIMIZED_ONLY=1 SEX_REPRO_CODECS=0 SEX_REPRO_UNTIL=0 \
SEX_REPRO_FIRST_ANALYZER=target/release/sex-analyze \
SEX_REPRO_SECOND_ANALYZER=scripts/cross/aarch64-sex-analyze.sh \
scripts/validate-cross-architecture.sh target/release/sex-rate \
  scripts/cross/aarch64-sex-rate.sh target/cross-repro-frontends
```

The optional analyzer overrides omit the main CLI's `analyze` prefix when
invoking a standalone analyzer; ordinary harness behavior is unchanged when
they are unset. This rate-only run uses the optimized subset because the base
corpus also has intentional rate-less copies. Use a new result directory to
repeat. Raw reports, commands, outputs and caches occupy about 456 KiB in
`target/cross-repro-frontends`. This does not repeat historical codec/Until-40k
corpora or qualify physical ARM/production linking.

Harness source fingerprint (Cargo files plus sorted src/crates/examples/scripts;
excludes docs and top-level integration tests):
`fcb585692ac0c28301de7d97a6e9ecd1e419daf358c9601c88a55c49c6b5f030`.
The [binary manifest](frontend-binaries-2026-09-06.tsv) records all three CLI
executables and the external Rust consumer on both architectures. The final
test-only non-UTF-8 regression does not change those executables; final builds
and checks are retained under `target/frontends-qualified/*-final-*`.

### Facade consumer and local installation

The separate `library_rate` example calls the public `sex::cli::run` entry.
Its x86-64 and AArch64 outputs at Remez 2/3, Fast, grid 9, certificate/harmonics,
no dither, normalization and block 4096 are equal to the retained corpus PCM.
The native complete qualification also matches. The logs and artifacts are
`example.*` and `library-aarch64.*` under `target/frontends-qualified`.

Three offline, locked local installs (root package, `sexbench`, `sexfuzz`)
succeed into `target/frontends-qualified/install`, about 8.5 MiB. All five help
commands succeed; installed `sex-rate` PCM and `sex-analyze` qualification
match the same corpus. The installed benchmark smoke uses 64 stereo frames,
17 taps, one iteration and native/GMP backends: both have checksum
`70361d9a8ed80932`. The installed fuzz smoke passes 16 cases at seed 42 with
checksum `24f863f4bf87d81c`. Install and smoke logs are retained there.

This is local packaging with dependencies already cached, not a clean-machine
bootstrap, stable C ABI, registry publication or signed binary distribution.
No system installation or PATH modification was performed. Library/command
contracts are in [library-frontends.md](library-frontends.md).

## 2026-09-06: bit-identical Remez cosine reuse

All 337 native release workspace/all-target tests pass, with one existing
heavy test ignored. Both new cosine/cache-policy tests pass under AArch64 QEMU,
including C=4096. Warnings-denied all-target Clippy passes on both architectures.
The tests compare cached/direct MPFR values, error vectors, complete solver and
quantization reports, banks, exact optional-memory thresholds, and persistent
file identity under either workspace policy. See
[remez-cosine-cache.md](remez-cosine-cache.md) for the resource contract.

The focused optimized corpus passes three cases, twelve PCM renders and six
analyses, with independent cold designs and reciprocal cache reads. The prior
[PCM/coefficient rows](optimized-cli-identities-2026-09-06.tsv) and every
[cache file](optimized-cli-cache-2026-09-06.tsv) compare unchanged. Qualification
blocks add only `cosine_cache_bytes`; removing that field reproduces the old
blocks byte-for-byte. A [new qualification manifest](remez-cosine-qualification-2026-09-06.tsv)
records the extended reports, equal across every architecture/block/cache route.

```sh
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_REPRO_OPTIMIZED_ONLY=1 SEX_REPRO_CODECS=0 SEX_REPRO_UNTIL=0 \
scripts/validate-cross-architecture.sh target/release/sex \
  scripts/cross/aarch64-sex.sh target/cross-repro-remez-cosine
```

Use a new result directory to repeat. Raw commands, PCM, reports and caches
occupy about 456 KiB in `target/cross-repro-remez-cosine`. Final test/build/check
logs are under `target/remez-cosine-qualified`. This does not repeat historical
Until-40k/codec corpora or establish physical ARM/production-link qualification.

Harness source fingerprint (Cargo files and sorted src/crates/examples/scripts;
excludes docs and top-level integration tests):
`cce3e212a3547fe1e2e9bc80685ed9cee36810726e910d59d8055052a0c2b31e`.
Executed CLI SHA-256:

- x86-64: `74c525b71eb8c3a55db2bf8550415ba67d22e0a4cb67698883a298a639b549a2`
- AArch64: `b1763976fb291c60ae5ca638024a01e59332d87a3ae9edc48c20ae23671b5bc8`

### Measured designer speed, same numerical result

The bounded `remez_cache_bench` compares complete direct and cosine-cached solves
at ratio 2/3, C=96 and P=160. Each invocation asserts equality of integer banks,
quantization reports and solver results. Recorded host: AMD Ryzen 9 9950X3D.
These are repeated wall-clock measurements, not a portable speed guarantee or
a statistical benchmark campaign; other qualification jobs were also running.

| Taps/phase | Trial | Direct seconds | Cached seconds | Speedup |
| ---: | ---: | ---: | ---: | ---: |
| 129 | 1 | 10.115786 | 1.714171 | 5.901x |
| 129 | 2 | 10.097456 | 1.707252 | 5.914x |
| 193 | 1 | 29.142251 | 5.110954 | 5.702x |
| 193 | 2 | 29.021441 | 5.098739 | 5.692x |

At T=129 the planned core/table bytes are 8,234,696 / 47,357,440; at T=193
they are 15,958,728 / 105,635,328. Each total fits the same 128 MiB allowance.
The direct path uses only the core estimate and remains available below the
optional-table threshold. Work charges and coefficient targets are identical.

The `bench-T-trial-N.log` files retain these measurements. The earlier
`bench-129.log` is an exploratory run, not included in the table. Benchmark
executable SHA-256:
`9dcb87fd5d6ec1292034eb6669c4eedae798280edc176e807198150f52778f84`.
Shared cached/direct coefficient identities are
`097f99d915983b8692399f7a330bfd6a9f5ebb87be7c2ca037be51452a299388`
for T=129 and
`ec4debf6d3989f051be9b3dbaf18a8e896ad3e8b52c785c5555b4756e4d393f9`
for T=193. These P=160/C=96 benchmark banks are not the CLI corpus's P=128/C=64
bank. Repeated cosine evaluation is reduced, but the cubic solve and selector
remain; very-long global-design scalability is still unfinished.

## 2026-09-06: unity endpoint-grid qualification

The corrected [unity prototype grid](unity-designer-grid.md) is qualified with
335 passing native release workspace/all-target tests (one existing heavy test
ignored), five passing AArch64 unity-filtered library tests (four new), and
warnings-denied all-target Clippy on both architectures. The independent
three-tap oracle cases verify exact rational minimax and LS coefficients,
native/bigint Remez agreement, and single rather than repeated endpoint weight.

Twelve CLI analyses cover two methods (global LS and Remez), each with native
cold/warm, AArch64 cold/warm and both reciprocal cache-read directions. Every
run uses a 44.1 kHz source at output rate 44100, Fast, grid 9, `--certify` and
`--harmonics`. Both methods accept T=129, C=64, P=128 on their first candidate.
LS uses 257 design points; Remez uses 1057 and converges in 16 iterations.
Complete stdout/stderr match between independently cold architectures and
between all four warm/foreign runs. Every bank-bound qualification block and
both independently generated cache files match across all six runs per method.

The retained `target/unity-grid-qualified/corpus` occupies about 248 KiB. Its
`METHOD-SIDE.out`, `.err` and `.qualification` files distinguish `first`,
`second`, `first-warm`, `second-warm`, `cross` and `reverse`. Two cache directories
are `first-cache` and `second-cache`. The
[durable identity manifest](unity-designer-identities-2026-09-06.tsv) contains
coefficient/qualification and full cache-file hashes. Reproduce each invocation
with its own or foreign cache directory and the following command shape:

```sh
SEX_COEFFICIENT_CACHE=CACHE sex analyze INPUT_44100.wav -r 44100 \
  --preset fast --designer remez --grid 9 --certify --harmonics
```

Repeat with `global-ls` and the documented AArch64 wrapper/toolchain. Actual
input is `target/cross-repro-optimized-cli/inputs/stereo24.wav`; no new large
audio fixture is generated. This is hypothetical FIR analysis, not a changed
same-rate PCM path. The reported continuous stopband upper bound of
-373.2771946233366 dB covers only the single Nyquist endpoint. The passband
deviation bounds are 3.454309648974885e-14 for LS and 1.762741429943645e-14 for
Remez. Those do not certify a finite-width stopband, full image interval, MPFR
design error or exported PCM noise floor.

Six additional warm non-unity PCM renders and six analyses repeat the preceding
three optimized CLI cases on both architectures, at blocks 1 and 7. All output
and qualification bytes retain the [earlier identities](optimized-cli-identities-2026-09-06.tsv),
and all existing cache files remain unchanged. These logs and outputs are in
`target/unity-grid-qualified/optimized-*`. This focused check does not repeat
cold non-unity CLI solves or historical Until-40k/codec corpora.

Source fingerprint (Cargo files and sorted src/crates/tests/examples/scripts;
excludes docs):
`9b35181f80d59067817a17b864169476e1b6a7b9afea8ba753051166c9a1bbb4`.
Executed CLI SHA-256:

- x86-64: `540766ff39bbbd2a75c7364f04706dc039c470ffb98016f4facc280321653a8a`
- AArch64: `2a7b90fe8df2a1f2fba553e95aa4d5d905b52f26950c7571760a045916c03629`

The QEMU-only linker accommodation and finite-platform qualification scope are
unchanged. Final test/build/check logs are `target/unity-grid-qualified/*-final-*`.

## 2026-09-06: bounded numerical solver recovery

All three solver-recovery tests pass on native x86-64 and AArch64 QEMU. The
real global-LS fixture L/M=2/3, T=129, r=3/5, weights 1/10, grid 258 per band,
C=64 fails at P=128 with singular pivot 110 and recovers at P=256. It passes
the unchanged -80 dB coefficient, phase, continuous amplitude and sampled
complete-image gates. Both architectures assert the same recovered coefficient
SHA-256:
`572cec99f26cfb24ad1aa1c71770e8db45f2ff14e632e883d1fa52e4c070f707`.
Both also compare its raw coefficients with a separate P=512 design.

The same real fixture exercises cold and warm cache recovery, exact aggregate
charges and absence of failed-bank entries. Additional tests check pinned P,
both precision caps, the shared candidate cap, cumulative work and typed
retry/non-retry classification. The Remez classifications are policy tests,
not a newly demonstrated real Remez recovery. The details and trust boundaries
are in [solver-precision-recovery.md](solver-precision-recovery.md).

All 330 native release workspace/all-target tests pass (one existing heavy
test ignored), and warnings-denied all-target Clippy passes on both platforms.
Final logs are `target/solver-retry-qualified/*-final-*.log`.

Six additional warm PCM renders and six standalone analyses repeat the three
preceding optimized CLI cases on both architectures, using the existing native
cache, block 1 on x86-64 and block 7 on AArch64. They compare against the
[preceding durable identities](optimized-cli-identities-2026-09-06.tsv): all
audio and complete qualification blocks are unchanged, including full solver
reports. The [three cache files](optimized-cli-cache-2026-09-06.tsv) remain
unchanged. Outputs and reports are in `target/solver-retry-qualified`, named
`optimized-CASE-ARCHITECTURE-MODE.log` and `optimized-CASE-ARCHITECTURE.wav`.
Use the preceding case options, but omit `--block-frames` for `analyze`, which
does not execute chunked audio. This focused check is not a repeat of independent
cold-cache CLI solves, the full historical codec/Until-40k corpus, or physical ARM.

Source fingerprint (Cargo files and sorted src/crates/tests/examples/scripts,
including top-level tests, excluding docs):
`95df8d2c7e81b5cf0946aa4fa186bc7aa97246e4899df3a19ef2ef602d22603b`.
Executed CLI SHA-256:

- x86-64: `0bfe0a3c666e1f587e3d9a49abf0f1753de705914375e92aa240175256a2eb1b`
- AArch64: `7bc940376a0073a72df00dee3dceae8324ece298a4e065860787d77c7c8a9ebe`

The prior QEMU-only linker accommodation is unchanged. Numerical recovery
remains bounded and cannot promise a successful design for every request.

## 2026-09-06: qualified global LS and Remez CLI execution

The optimized-only corpus passes three cases, twelve actual PCM renders and
six standalone analyses. Each architecture independently constructs its cache;
both reciprocal cache-read directions pass as well. Output bytes, complete
bank-bound qualification blocks (including full solver reports), coefficient
identities and all three cache files match at block sizes 1/7/4096.

| Case | Executed bank | Signal / resampler MAC | Continuous phase stopband upper bound |
| --- | --- | --- | --- |
| Global LS 2/3, stereo | 2*193 taps, C=64, P=128 | Q65.64 / 203 bits | -91.97674030384271 dB |
| Remez 2/3, stereo | 2*193 taps, C=64, P=128, 15 iterations | Q65.64 / 203 bits | -87.30748375142391 dB |
| Global LS 1/3, mono, gain 2^60 + normalize | 385 taps, C=87, P=160 | Q72.88 / 258 bits | -137.2926825562939 dB |

Every case uses Fast's unchanged -80 dB target, grid 9, `--certify`,
`--harmonics`, no dither, and normalization. All requested gates pass in one
candidate. Complete-image sampled checks include both harmonics at 2/3;
the 1/3 bank has no separate image components because L=1. These conservative
continuous per-phase bounds are not sampled image peaks or PCM noise floors.
Coefficient/shape growth is separately exercised by the real library tests,
not by these single-candidate PCM fixtures.

```sh
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_REPRO_OPTIMIZED_ONLY=1 SEX_REPRO_CODECS=0 SEX_REPRO_UNTIL=0 \
scripts/validate-cross-architecture.sh target/release/sex \
  scripts/cross/aarch64-sex.sh target/cross-repro-optimized-cli
```

Use a fresh result directory when repeating. `SEX_REPRO_OPTIMIZED=1` instead
adds these cases to the ordinary corpus; the default scenario set is unchanged.
This run deliberately does not repeat the historical Until-40k or codec corpus.
It uses built-in integer WAVE, QEMU 10.2.2 Cortex-A53, Zig 0.15.2 and the
previously documented QEMU-only linker accommodation, not production ARM
linking or physical hardware.

Raw commands, reports, outputs and caches occupy about 456 KiB in
`target/cross-repro-optimized-cli`. Durable manifests record
[PCM/coefficient/qualification identities](optimized-cli-identities-2026-09-06.tsv)
and [whole-cache-file identities](optimized-cli-cache-2026-09-06.tsv).
Harness source fingerprint (Cargo files plus src/crates/examples/scripts;
excludes docs and top-level tests):
`03a2def1375d5d0c03b78e957c6ee5e59165d3a80856adfe808baf64f8c7fa16`.
Executed CLI binary SHA-256:

- x86-64: `dc381fa8605d2d86e51baef86040a95889038f533388e27a5f1810963d6756f0`
- AArch64: `784cb594bbe5ecfc97b2418990a33eba60b3fbdb324e7e429a0d409fee63e427`

All 327 native release workspace/all-target tests pass, with the one existing
heavy test ignored. Both new optimized-refinement tests pass on AArch64, and
warnings-denied all-target Clippy passes on both architectures. Final logs are
under `target/optimized-cli-qualified`.

Six additional CLI failure checks use the warm LS 2/3 bank: certificate work,
per-candidate designer work, and cumulative designer work are each limited to
one on both architectures. Every failure preserves the existing destination,
claims no qualified-bank hash, and produces byte-identical paired diagnostics.
The certificate failure explicitly reports `Inconclusive(Work)`. The retained
`negative-*-*.log` files have paired SHA-256 values:

- Certificate work: `00378732c3cb31a52355ac8cb024d0925aebcad83521d87993287661546519ed`
- Per-candidate work: `687fd6b3726c757a6cc2479af86cead0cfa06818226794a6e1908f00d0e22105`
- Cumulative work: `7692067e85460bf18d46cf4c4f1eda54d408b60f9952c0c3844d07838f7da1dc`

This qualifies the recorded CLI paths, not every optimized preset/ratio or
automatic recovery from ill-conditioned solves. Continuous complete-image
bounds, scalable very-long global solves and broader platforms remain open.

## 2026-09-06: optimized-designer cache qualification

Kind-4 optimized cache entries now preserve arbitrary-width global-LS/Remez
coefficient banks, quantization reports and solver diagnostics. Cache mode of
`optimized_probe` exercises every precision-search candidate as well as the
thirteen inspected banks, including C=4096 and the repaired 129-tap Remez at
P=160/P=512. It repeats response, certificate and complete-image checks after
loading; no cached proof is substituted for those checks.

Six full invocations pass: x86-64 cold/warm, AArch64 cold/warm, x86-64 reading
the ARM cache and ARM reading the x86-64 cache. Each performs 39 integer-stream
renders and qualifies five banks, for 234 renders total. All six 51,466-byte
outputs compare equal. Both cold runs independently produce the same fifteen
cache files (including the two C=8 search candidates). All thirty files retain
the same names and full-file SHA-256 after reciprocal reads. The prior Remez-v2
numerical report/stream output is unchanged; only the final scope sentence drops
the former disclaimer about cache qualification.

The [cache manifest](optimized-cache-identities-2026-09-06.tsv) lists each filename
and full-file SHA-256, including its payload checksum. Coefficient and raw-stream
identities remain the [preceding v2 identities](remez-v2-identities-2026-09-06.tsv).
The six runs and both cache directories are under `target/optimized-cache-qualified`,
about 564 KiB in total. No large audio files are generated.

```sh
cargo build --release --example optimized_probe
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_QEMU_VALIDATION_ONLY=1 scripts/cross/build-aarch64.sh \
  build --release --example optimized_probe -j8
target/release/examples/optimized_probe --cache-directory NATIVE_CACHE
target/release/examples/optimized_probe --cache-directory NATIVE_CACHE
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
scripts/cross/aarch64-run.sh target/aarch64-unknown-linux-gnu/release/examples/optimized_probe \
  --cache-directory ARM_CACHE
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
scripts/cross/aarch64-run.sh target/aarch64-unknown-linux-gnu/release/examples/optimized_probe \
  --cache-directory ARM_CACHE
target/release/examples/optimized_probe --cache-directory ARM_CACHE
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
scripts/cross/aarch64-run.sh target/aarch64-unknown-linux-gnu/release/examples/optimized_probe \
  --cache-directory NATIVE_CACHE
```

NATIVE_CACHE and ARM_CACHE are distinct, initially absent result directories;
they are literal placeholders above. Actual directories are `native-cache` and
`aarch64-cache` beneath the retained result directory. Capture stdout separately
for each invocation and compare all reports and cache bytes.

Final source fingerprint (same sorted source-list method as prior records):
`1aafebd08027db367d5e66bb7a3e1e42c11fb7f368e1804ea0fcd98324f50f86`.
Executed example binary SHA-256:

- x86-64: `68dd58a9f8fe1d500c916b5b41716424e74003dd5ee0b89021f6743a2241b3bf`
- AArch64: `7393d8ac7e0236f768f1a6e14933eed9071c3f6723bed357927df14931c1212f`
- Complete output, all six runs: `81c48f80d0830a8bafb6e564f9da4317408c1ba8ff91c06cac311121e748b9fa`

All 323 native release workspace/all-target tests pass (one existing heavy test
ignored), all six new cache tests pass under AArch64 QEMU, and warnings-denied
all-target Clippy passes on both architectures. Final logs use `*-final-*`;
earlier five-cache-test logs remain separately. The final test-only addition
does not change the probe binaries, verified by rebuilding them after the runs.
Tests include resealed invalid metadata/DC, checksum and canonical word failures,
budget enforcement on hits, concurrent publication and failed-write preservation.

This is persistent library cache, solver report and integer-stream qualification.
It does not provide production CLI selection/PCM export for LS/Remez, a proof of
cached MPFR measurements, continuous complete-image bounds, physical ARM
qualification or production ARM linking. The QEMU-only linker accommodation and
the trusted-local, non-authenticating cache assumption are explicit in the
[cache contract](coefficient-cache.md#optimized-global-ls-and-remez-cache).

## 2026-09-06: Remez v2 regression repair

The retained T=129, ratio 2/3, r=9/10, C=96 Remez failure now succeeds at both
P=160 and P=512, in 12 iterations, with identical raw coefficients between
precisions and unchanged target 80 dB. Both banks pass their coefficient and
sampled phase gates, continuous phase amplitude certificates and sampled
complete-image checks. The continuous stopband upper bound is
-106.8464251509297 dB; the sampled image peak is -140.0847775290125 dB.
These are distinct scopes, and neither is an exported-PCM noise-floor claim.

The two successful banks replace the preceding corpus's failure case.
`optimized_probe` now independently designs thirteen banks on each architecture,
runs 39 Q65.160 stereo streams at chunks 1/3/4096, and qualifies five banks.
Both programs terminate successfully; their complete 51,472-byte outputs compare
equal, including coefficients, solver/quantization reports, raw integers,
precision-search histories, continuous certificates and image-grid reports.
All six global-LS report/stream blocks are unchanged from the v1 corpus.
Remez identities deliberately change to v2; old results are preserved.

The corrected signed-extremum selection preserves one-point sign lobes.
Physical-band-width initialization and a stricter alternating-ripple stopping
condition accompany it. Native and arbitrary Remez share the same work
estimator. For the repaired bank its conservative estimate is 792,998,432 units,
explicitly allowed by a one-billion-unit limit; the target, FIR length and
iteration cap are not weakened. Full contracts and diagnostic evidence are in
[optimized-designers.md](optimized-designers.md).

All 317 native release workspace/all-target tests pass with one existing heavy
test ignored, and all 17 optimized-library tests pass under AArch64 QEMU.
Warnings-denied all-target Clippy passes on both architectures. Unit tests and
the independent executable corpus are separate successful runs.

The old temporary toolchain no longer existed. Initial cross links terminated
with missing-Zig errors; they were not running jobs or numerical failures.
The pinned bootstrap restored Zig 0.15.2 and the checksum-verified ARM libc/libgcc
packages in `/tmp/sex-cross-toolchain.kdS7JT`, without installing system packages.
QEMU is 10.2.2. This toolchain is about 499 MiB and the result logs about 200 KiB;
no large audio files were generated. The retained successful rerun commands are:

```sh
cargo test --release --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --example optimized_probe
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_QEMU_VALIDATION_ONLY=1 scripts/cross/build-aarch64.sh \
  test --release --workspace --lib optimized
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_QEMU_VALIDATION_ONLY=1 scripts/cross/build-aarch64.sh \
  clippy --workspace --all-targets -- -D warnings
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
SEX_QEMU_VALIDATION_ONLY=1 scripts/cross/build-aarch64.sh \
  build --release --example optimized_probe -j8
target/release/examples/optimized_probe
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT \
scripts/cross/aarch64-run.sh target/aarch64-unknown-linux-gnu/release/examples/optimized_probe
```

Source fingerprint (same sorted source-list method as the previous record):
`34dfde5c8b961b10ee5dd5027be68a475951462118d142da3b9c9cc9329f412c`.
Executed example binary SHA-256:

- x86-64: `0408f0a0c8b9077f7129ff16032f1e52a58c9d02cc298f819693dd889688670c`
- AArch64: `0e8a4e32ec5fdce73b4cb50031680d586b9b1064cea7a9ec3a889c1dd86f7f77`
- Complete output, both: `77bb08edc029536ecb4b75b60e15182fa7d5d5d9f1bea87bf5ee335bc7dc22c1`

The [v2 manifest](remez-v2-identities-2026-09-06.tsv) records thirteen coefficient
identities and hashes of each exact `Q65.160 stereo stream ...` output line,
including its newline. Full evidence is under `target/remez-qualified`; successful
cross logs are named `aarch64-restored-*`, separately from missing-toolchain logs.
This is not optimized-method CLI/cache integration, a new PCM corpus, a
continuous complete-image proof, physical ARM qualification, or a production ARM
link. The explicit QEMU-only linker accommodation is unchanged.

## 2026-09-05: arbitrary-width global designer library qualification

`examples/optimized_probe.rs` independently constructs eleven global LS/Remez
Q2.C banks on x86-64 and AArch64 QEMU, including C=4096. Each bank executes a
Q65.160 stereo integer stream at chunks 1/3/4096: 66 actual stream renders
across the two architectures. Both runs exit successfully with byte-identical
37,205-byte output, including raw output integers, coefficient identities,
precision-search histories, quantization/solver reports and all spectral checks.

Three banks pass coefficient and sampled phase gates, exact continuous phase
amplitude certificates and sampled complete-image gates on both architectures:

| Method | Ratio | T/phase | Rolloff | Target | Continuous stopband upper bound |
| --- | --- | ---: | --- | ---: | ---: |
| Global LS | 2/3 | 31 | 3/4 | -40 dB | -68.36109048949823 dB |
| Remez | 2/3 | 31 | 3/4 | -40 dB | -70.91611208667018 dB |
| Global LS | 2/3 | 129 | 9/10 | -80 dB | -82.54646954872555 dB |

The 40 dB cases use phase grids of 129 points/band and complete-image grids of
33 points/band (97 distinct input frequencies); the 80 dB case uses 257 and 65
respectively (193 input frequencies). All use C=96/P=160. Image grids are not
continuous image bounds. The short arithmetic and coefficient-search fixtures
are explicitly not spectrally qualified.

A twelfth case reproduces the same unresolved large Remez failure on both
architectures: ratio 2/3, T=129, r=9/10, C=96/P=512, weights (1,10), density 16,
64 iterations, explicit 1-billion-unit work cap. Its estimated work is
792,913,672 units and storage 11,700,336 bytes. It fails because 130 extrema
are needed but only 129 are found, returning no bank. Matching this failure
is not successful design or an 80 dB quality claim.

```sh
cargo build --release --example optimized_probe
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.ZnEZ3u \
SEX_QEMU_VALIDATION_ONLY=1 scripts/cross/build-aarch64.sh \
  build --release --example optimized_probe -j8
target/release/examples/optimized_probe
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.ZnEZ3u \
scripts/cross/aarch64-run.sh target/aarch64-unknown-linux-gnu/release/examples/optimized_probe
```

Source fingerprint, computed by
`rg --files Cargo.toml Cargo.lock crates src examples tests scripts | LC_ALL=C sort | xargs sha256sum | sha256sum`:
`1acad909655d56641bc19f1040dc758c27b1c584900ce19ad0a94789eb4b6ddc`.
Executed example binary SHA-256:

- x86-64: `f303bf373eedc0dac493253011454fcffeb1a7beba9923a98b4689019436c514`
- AArch64: `f4cf66de0d4ca6ebbef01f7cbd51955281e95af8b8c10b672653f1192f3057c1`
- Complete output, both architectures: `c893a34bb4d65cbedde16669740ef2f80fa29f335aaf7d5412119897b717bb4b`

The [eleven coefficient identities](optimized-designer-identities-2026-09-05.tsv)
are retained separately. Full reports and build/test logs are under
`target/optimized-qualified` (about 200 KiB). Native release workspace/all-target
tests pass 313 tests with one existing opt-in heavy test ignored; the 13 new
library tests pass under AArch64 QEMU, and all-target warnings-denied Clippy
passes on both architectures. The example and test runs are distinct records.

This qualification is library design plus integer streaming, not CLI method
selection, PCM export, persistent optimized-method caches, physical ARM hardware,
or production ARM linking. It does not rerun or supersede the preceding
22-scenario/88-PCM-render window corpus. Known convergence failures, mathematical
contracts and remaining integrations are in [optimized-designers.md](optimized-designers.md).

## 2026-09-05: window selection qualification

The full window-selection corpus passed 22 scenarios / 88 actual PCM renders,
plus 16 standalone analysis runs. This includes all 18 preceding shared-gate
scenarios unchanged, four additional window cases, and the real two-candidate
Until-40k 2/1 bank. Historical PCM/coefficient rows, all 18 prior qualification
blocks, and every existing Kaiser cache file were compared with
`target/cross-repro-shared-gates` and are unchanged.

```sh
SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.ZnEZ3u \
SEX_REPRO_CODECS=1 SEX_REPRO_SHARED_GATES=1 SEX_REPRO_SHARED_UNTIL=1 \
SEX_REPRO_WINDOWS=1 SEX_REPRO_UNTIL=0 \
scripts/validate-cross-architecture.sh target/release/sex \
  scripts/cross/aarch64-sex.sh target/cross-repro-windows
```

All four new cases require a continuous per-phase amplitude certificate and
sampled complete-image qualification at nine grid points/band. These are
distinct scopes: the image checks are not continuous bounds or PCM guarantees.

| Window case | Ratio | Final T/phase | C / MPFR bits | Continuous stopband upper bound |
| --- | --- | ---: | ---: | ---: |
| Fast Hann | 3/2 | 257 (initial 129 rejected) | 64 / 128 | -88.65439495928853 dB |
| Fast Blackman, gain 2^60 + normalize | 1/3 | 385 | 87 / 160 | -85.61193683816166 dB |
| High Dolph v2, window attenuation 172 dB | 1/2 | 1109 | 96 / 256 | -168.5299471608949 dB |
| Fast Dolph v2, window attenuation 80 dB | 3/2 | 129 | 64 / 128 | -93.04720358366790 dB |

The High target is -160 dB; the other three target -80 dB. The 3/2 Dolph case
exercises actual fractional phase coordinates affected by the signed-DFT-bin
correction. Both architectures independently design caches, exchange them in
both directions, and produce identical complete qualification blocks in
conversion and analysis. The corpus occupies about 41 MiB, mainly coefficient
caches; no large audio payloads are generated.

Full-corpus source fingerprint:
`592ab220e26cf028fdcf6c491e531276d7c9fe9039c5858ed71410eba8f9d0c3`.
Executed full-corpus binary SHA-256:

- x86-64: `86aab42e0b0753d643f17a9baa84467461d449cfa963fc97f6ff33e84369731d`
- AArch64: `38e89b30218e603fb102c7d7afef3fbaa7ffb0bf3eaf183b7d0bdec45a157594`

Durable manifests are [PCM/coefficient identities](cross-repro-windows-2026-09-05.tsv)
and [new window qualification identities](window-qualification-2026-09-05.tsv).
Commands, full reports, cache entries, and build/source/input records remain in
`target/cross-repro-windows`; test logs are in `target/window-qualified`.
This remains AArch64 under QEMU, not physical ARM or production-link qualification.

### Final diagnostic-only revision

After the full corpus, the window preflight received a dedicated typed error
for insufficient C+64 working precision. The old shared error wording named
the native designer's 96-bit baseline even when a window request needed more.
No coefficient generation, planning, arithmetic, cache, or gate behavior changed.

The final revision re-passed all 300 native all-target tests (one opt-in heavy
test ignored), warnings-denied Clippy on both architectures, and AArch64 window
library tests, including the new exact diagnostic. Both rebuilt executables
then independently designed and rendered all four new window cases with cache
disabled: eight additional PCM files and complete qualification blocks match
the full corpus byte-for-byte. Commands and outputs are retained in
`target/window-qualified/diagnostic-recheck`. The 88-render full corpus and this
8-render focused recheck are separate records, not a claim of a second full run.

Final source fingerprint:
`23aab953d005d2dc59b701f302247532f15fd7b321a3b7f5cf10bdb4a9089aa1`.
Final/rechecked production binary SHA-256:

- x86-64: `32695a6ee95ff282e7e4c79ef47b0ae588fe859e1fc1b36a158e4722fd599189`
- AArch64: `e0fcc8e8cfb9f09680175dba7cf47c2e183c29794bccc94331748ae0f5b2901b`

## Harness contract

`scripts/validate-cross-architecture.sh` accepts two executable SeX frontends;
the second can be a launcher wrapper. It refuses matching or missing architecture
identities from `--build-info`, then generates one shared, integer-only PCM
corpus. Each scenario is rendered four ways:

| Execution | Coefficient cache | Input block |
| --- | --- | ---: |
| First architecture | Independently designed first cache | 1 frame |
| Second architecture | Independently designed second cache | 7 frames |
| Second architecture | Reads the first architecture's cache | 4096 frames |
| First architecture | Reads the second architecture's cache | 4096 frames |

All four output files must match byte-for-byte. Reported coefficient SHA-256
values must match, both cross-cache reads must report hits, and the complete
cache directories must be byte-identical. This verifies coefficient generation
as well as execution; sharing one coefficient table alone is insufficient.

The harness preserves input hashes, exact shell-escaped commands and reports,
output/coefficient hashes in `results.tsv`, compiler/build information, and a
source fingerprint. It fails if sources change during the run. `--build-info`
reports architecture, OS, pointer width, endianness, cross-GMP support, and whether
the executable was built explicitly for QEMU-only validation. Build information
is not embedded into audio output.

The corpus contains 16-, 24-, and 32-bit PCM; mono, stereo, and eight channels;
zero samples, positive/negative full scale, single-code perturbations, and deterministic
pseudorandom values. Empty and one-frame files check exact finite duration.
Rate cases include 160/147, 1/3, 160/441, and 1/2. The whole-chain case uses
4096 fractional signal bits through gain, DC, mix, convolution, resampling,
normalization, and ninth-order noise shaping. A large-gain case forces coupled
precision refinement. Optional cases add actual Until-40k and tagged
WAVE/AIFF/FLAC with UTF-8 title/artist metadata.

These are bit-reproducibility tests, not comparisons of filter quality against
other resamplers. Spectral qualification and the rational numerical-error oracle
remain separate requirements.

## Reproducing the isolated AArch64 setup

The host must have Bash, Rust/Cargo with the `aarch64-unknown-linux-gnu` standard
library, QEMU user-mode AArch64, curl, tar/xz, ar, sha256sum, ripgrep, and ordinary
coreutils. The setup does not use containers or install system packages.

```bash
cross_dir=$(mktemp -d /tmp/sex-cross-toolchain.XXXXXX)
bash scripts/cross/bootstrap-aarch64.sh "$cross_dir"
export SEX_CROSS_TOOLCHAIN="$cross_dir"
bash scripts/cross/bootstrap-codecs.sh

cargo build --locked --release --bin sex
export SEX_QEMU_VALIDATION_ONLY=1
scripts/cross/build-aarch64.sh build --release --workspace --bins -j 8

SEX_REPRO_CODECS=1 SEX_REPRO_UNTIL=1 \
  scripts/validate-cross-architecture.sh \
  target/release/sex scripts/cross/aarch64-sex.sh target/cross-repro-full
```

For a second emulated CPU configuration, set `SEX_QEMU_CPU=max` and choose a
different, nonexistent result directory. The default is `cortex-a53`. The
runner accepts an alternate executable via `SEX_QEMU_AARCH64`. Result directories
are intentionally retained and existing ones are rejected, never overwritten.
The scripts do not delete the downloaded toolchain; it is an isolated temporary
directory and can be removed after recording results.

The toolchain pins Zig 0.15.2, with its SHA-256 checked against the
[official release index](https://ziglang.org/download/index.json).
The runtime uses Debian arm64 glibc 2.41 and GCC unwind support; hashes come
from Debian's [libc6](https://packages.debian.org/trixie/arm64/libc6/download) and
[libgcc-s1](https://packages.debian.org/trixie/arm64/libgcc-s1/download) package
records. Optional codec packages are pinned in `scripts/cross/arm64-codecs.sha256`
using Debian's [arm64 package index](https://deb.debian.org/debian/dists/trixie/main/binary-arm64/).
Every download is hash-checked before extraction. If a pinned file disappears
from a mirror, setup fails rather than silently substituting another version.

## Important linker limitation

Rust 1.98.1 supplies `--fix-cortex-a53-843419` for AArch64, but the selected Zig
driver rejects this hardware-erratum workaround. `aarch64-cc.sh` therefore
**refuses** that invocation unless `SEX_QEMU_VALIDATION_ONLY=1` was explicitly
set. Only in that emulator-only mode is the unsupported flag removed.
The CLI records `qemu_only=true` at compilation. These binaries must not be
presented as production ARM artifacts. A hardware/distribution build needs a
cross toolchain that retains the workaround and a fresh hardware qualification.

The upstream GMP binding's `force-cross` feature is exposed as the opt-in
`cross-gmp` Cargo feature. It enables the dependency's cross-build path, not a
different arithmetic algorithm. GMP and MPFR are built from the same locked
sources, with the target's assembly/backend. Their own build-time C test suite
is skipped by the dependency during cross compilation; the SeX tests below
execute the resulting arithmetic under QEMU.

## Additional executed checks

```bash
scripts/cross/build-aarch64.sh test --release --workspace --lib -j 8
scripts/cross/build-aarch64.sh test --release \
  --test arbitrary_signal --test numerical_budget -j 8
scripts/cross/build-aarch64.sh clippy --workspace --all-targets -- -D warnings
scripts/cross/aarch64-run.sh \
  target/aarch64-unknown-linux-gnu/release/sex-fuzz --cases 1000
```

The library run passed 158 tests, and both cross-library integration tests
passed. x86-only SIMD test branches are not executed on ARM. Codec-dependent
tests require the optional runtime; the tagged AIFF/FLAC corpus additionally
fails hard if that runtime is unavailable. The ordinary CLI integration suite
spawns its binary directly and is not claimed as a QEMU-run suite here; the
launcher-aware harness exercises the full CLI instead.

The AArch64 property run produced the same result as x86-64:
1000 cases, seed `2611923443488327891`, checksum `39315e1921c2d028`.
Native and AArch64 Clippy checks both passed with warnings denied. The Zig
linker emits a separate deprecated-optimization notice during linking; no
source warning was suppressed to get the checks through.

## Recorded qualification: 2026-09-05

The complete `cortex-a53` run passed **15 scenarios / 60 renders**, including
all six presets, F=4096 through the complete effect chain, coupled precision
refinement, both directions of cache interchange, and tagged WAVE/AIFF/FLAC.
The Until-40k case independently designed the actual 133733-tap bank on both
architectures; its coefficient identity is the same one qualified spectrally
in [project-status.md](project-status.md#actual-until-40k-qualification).
The corpus does not itself rerun that spectral gate.

The durable [result manifest](cross-repro-2026-09-05.tsv) records each output
file and coefficient SHA-256. Full commands, inputs, logs, caches, and 60 audio
outputs were retained in `target/cross-repro-full` (about 22 MiB). An earlier
`max` CPU-model run passed 12 scenarios / 36 renders, including tagged codecs;
it did not include the later reverse-cache, positive one-frame upsampling,
absurd, or Until-40k additions and is not claimed as the full matrix.

Recorded environment:

- x86-64 Linux host and emulated AArch64 Linux, both 64-bit little-endian;
- Rust 1.98.1 (`48a229cea`, 2026-09-01), LLVM 22.1.8;
- QEMU 10.2.2, Zig 0.15.2, GMP 6.3.0, MPFR 4.2.2;
- libsndfile 1.2.2 and FLAC 1.5.0 on both sides, from Fedora and Debian builds
  respectively; Ogg 1.3.6 on the host and 1.3.5 on the target;
- host build `cross_gmp=false; qemu_only=false`, target build
  `cross_gmp=true; qemu_only=true`.

The harness's source fingerprint (Cargo manifests/lock plus Rust, examples,
and scripts; documentation excluded) was
`3a9dbb25223ea22119211be841a2965699c9eb271ce78e33649019355fd998a6`.
The Cargo.lock SHA-256 was
`fe3d46afca93fcfb62f6c19bb6e18a34ab6f8042c620904c3642de7a089ecbb8`.
The qualified executable SHA-256 values were:

- x86-64: `49ba620fd000a1282828f42fdc325d8a38bf51a398420e93d58d003385437069`;
- AArch64: `181b33163754b94e36b112544f66abfad9c42b88ed7467451fca156dea2cb263`.

These identify this run, not a promise of reproducible executable builds or
identical encoded files across every future codec release.

## Follow-up after the Sane margin correction

The full **15-scenario / 60-render** corpus was executed again with the
corrected Sane planner on the same x86-64/AArch64 setup. All output/cache
comparisons passed. The only changed row relative to
[the first manifest](cross-repro-2026-09-05.tsv) was `sane`:

- output SHA-256:
  `99e08e37635e66759b5d5076808d4654357ff5d5664cbdd424e91d0cbbde5730`;
- coefficient SHA-256:
  `ee56e04e85c075e850b363727d1b7b8c8f93ea954abeb8f4bf9170e45407650e`.

All other 14 rows, including Until-40k, retain their earlier hashes. The new
complete results, commands, cache files, and audio are in
`target/cross-repro-sane-margin`. Its source fingerprint is
`3c85c19df7aab2ef002aa03b551dc8d538559a69bef31050f799c9cd0d9d186f`.
The executed binary hashes are:

- x86-64: `747973b796bb90f1d8fb512a69bf277c717f710264b4603492ed03f1b8d45167`;
- AArch64: `b3ed1ac1c8b8d08d005472ae0cc2734ed7a1ab4e2678b9222d646ecbf081512c`.

The updated AArch64 run passed 159 library tests, including the new 1025-point
Sane regression, both cross-library integration tests, and all-target Clippy.
Later edits to the spectral measurement example do not change these SeX binaries.
The same QEMU-only linking and hardware limitations still apply.

## Follow-up after continuous certification and the High correction

The **14-scenario / 56-render** corpus (codec cases enabled, expensive Until-40k
not repeated) passed after adding continuous certificates and correcting High's
design margin. All complete output and independently generated cache files
agree across x86-64/AArch64 and both cache-interchange directions. The
[durable result manifest](cross-repro-continuous-high-2026-09-05.tsv) matches
`target/cross-repro-continuous-high/results.tsv` exactly.

With the same isolated toolchain environment as above:

```text
SEX_REPRO_CODECS=1 SEX_REPRO_UNTIL=0 scripts/validate-cross-architecture.sh target/release/sex scripts/cross/aarch64-sex.sh target/cross-repro-continuous-high
```

The result directory must not already exist when reproducing the run.

Compared with the preceding Sane-margin run, only `high` and its three
`tagged-*` codec cases changed. The other ten repeated cases retain their
hashes. Until-40k was not rerun here; its previous successful full-run record
above remains historical evidence, not a new qualification.

The new source fingerprint is
`6c7c8e43bcc3d369c1ab3e263102efc7f5fd566caec197fb50a371780e842a68`.
Executed binary SHA-256 values:

- x86-64: `fc57dfffd5b5bbdd702ce52e6a7fa5ecd7c2934188d095d73b39e9c7c16897ee`;
- AArch64: `58c2b880c6006dfd7970487604295e3e78cbc77bb74635c4ed283b2e2fd4807f`.

The fourteen new exact certifier tests also passed on AArch64. Independent
uncached CLI analyses give byte-identical complete reports for Sane 48→16 kHz
and corrected High 48→24 kHz, including certificate verdicts, work/cell counts,
and outward decimal bounds. This is additional analysis reproducibility
evidence; it does not broaden the QEMU-only hardware/linking scope.
The complete AArch64 library suite then passed all 174 tests, and all-target
Clippy passed with source warnings denied. Native all-target tests passed 246
cases with the expensive Until-40k qualification left explicitly opt-in.

## Follow-up: complete sampled harmonic-transfer analysis

The [eight-case harmonic campaign](harmonic-response-analysis.md) passed with
independently designed coefficients and bit-identical complete reports/cache
directories between x86-64 and AArch64. It includes Sane's 479/441 bank with
479 output components per input tone, both High cases, and Until-40k's actual
2/1 bank with two phases of 66867 taps. This qualifies the added analyzer's
reproducibility, not additional PCM renders or continuous image-band bounds.
The eleven new harmonic unit tests and cross all-target Clippy also passed.
Its command, source/binary identities, metrics, and durable manifest are in the
linked report; no signal algorithm or preset was changed in that checkpoint.

## Follow-up: feedback-driven quality refinement

The feedback-enabled **14-scenario / 56-render** corpus passed with codec cases
enabled and Until-40k excluded from this cross run. Its independently generated
banks, complete output files, block sizes 1/7/4096, and both cache-interchange
directions agree on x86-64 and emulated AArch64. F=4096 stateful processing,
amplification-driven coefficient refinement, and tagged WAVE/AIFF/FLAC remain
covered. Results occupy about 5.9 MiB, without a large audio fixture.

```text
SEX_REPRO_CODECS=1 SEX_REPRO_FEEDBACK=1 SEX_REPRO_UNTIL=0 scripts/validate-cross-architecture.sh target/release/sex scripts/cross/aarch64-sex.sh target/cross-repro-feedback
```

The same isolated toolchain environment is required; the result directory must
be new. The [durable manifest](cross-repro-feedback-2026-09-05.tsv) matches
`target/cross-repro-feedback/results.tsv`. The source fingerprint is
`c8f080640792975fce3f3810c2d853dedcc84546a10c7f08570593b6ae77003c`.
Executed binary SHA-256 values:

- x86-64: `11af8e62df8c44c5aa5e18f51d6c8c2f808e368218e31ed60cdcd8a31ef12a49`;
- AArch64: `5f0b8b1f231f19763248dfa881e3e20139092351382e61fe5032343693df1222`.

The ordinary feedback grid exposes real initial Absurd/Pointless response
failures at 1/2. Each requires two assessed shape candidates, with final lengths
4309 and 8473 taps. Those two output/coefficient rows change relative to the
preceding unchecked corpus; the other twelve retain their previous identities.
Targets and preset defaults were not weakened or changed. The
[feedback contract](quality-refinement.md) records initial/accepted measurements,
independent coefficient-versus-shape decisions, cumulative resource limits, and
the explicit sampled scope.

Ten new refiner and two new planner library tests pass on AArch64 under QEMU.
Both all-target Clippy checks pass. Independently generated Sane 3/2 combined
feedback/certificate/harmonic reports and caches also match exactly, retained
in `target/refinement-qualified/*-combined-*`. The recorded full native suite
has 278 passing tests, with the heavy Until-40k test kept separately opt-in.
Test-only additions after the corpus do not change the executed product source
fingerprint above (the harness fingerprints Cargo files, src, crates, examples,
and scripts, not tests or documentation).

## Follow-up: mandatory default quality qualification

The 14-case / 56-render corpus now passes without `--refine-quality` or an
explicit error floor. Both architectures automatically refine Absurd/Pointless
at 1/2, while preserving the target and frequency geometry. The
[new durable manifest](cross-repro-default-quality-2026-09-05.tsv) matches
`target/cross-repro-default-quality/results.tsv` and is identical to the
preceding opt-in feedback manifest in every row. All full cache bytes also
match that prior run. This establishes that the new default selects the same
qualified algorithm as the previous explicit option, not a cheaper substitute.

```text
SEX_REPRO_CODECS=1 SEX_REPRO_FEEDBACK=0 SEX_REPRO_UNTIL=0 scripts/validate-cross-architecture.sh target/release/sex scripts/cross/aarch64-sex.sh target/cross-repro-default-quality
```

The same isolated toolchain environment and a new result directory are
required. The retained corpus is about 5.9 MiB. The source fingerprint is
`80d24349f1bedaea1072227daeba3a5e9f4c44481c5e3378a31591e8a0110d3f`.
Executed binary SHA-256 values:

- x86-64: `81fc8d6fe7e5b04bfda88b94848956aaf66ccb93b513602ae21ab1ddeb00e8aa`;
- AArch64: `c9d9a093b53a054b190c590c5d66afdd075bb45f387635dd861382e5852dbfd2`.

Three additional full-report comparisons also match byte-for-byte between
native and emulated AArch64, retained in `target/default-quality-qualified`:

| Report | SHA-256 |
| --- | --- |
| Sane 3/2, default feedback + certificate + harmonic grid 9 | `82b8f541c90bceb12e8725c4ff297eeb82b7d95a8956b28e3598d410985b7120` |
| Fast 1/3, certificate work=1, explicit inconclusive failure | `c53c85176dc55ed4b0de40d5c8f7efb77d6cceef03953c3d5bb754e4aebd8403` |
| Until-40k 160/147, cheap initial plan with required budgets | `edac8505a4fd229ecc54c0b172a3ad2d9239863af728c800fd863adb8f783c43` |

The combined successful report and independently generated cache also match a
third native run with the redundant `--refine-quality` alias. No default output
or cache identity changes relative to the preceding feedback-qualified corpus.
Compared with the older unchecked corpus, Absurd/Pointless changes are now
mandatory instead of opt-in.

All 280 native all-target tests and both all-target Clippy checks pass. The
separate native 133733-tap Until-40k release test passes with default conversion
feedback, the same C=512 bank, and identical PCM at blocks 1/7/4096. Until-40k
audio was not repeated on AArch64 in this checkpoint. The QEMU-only linking and
finite-corpus limitations still apply; sampled default qualification is not a
continuous long-FIR or complete-image proof.

## Follow-up: shared conversion/analysis gates

The **18-case / 72-render** matrix passes on x86-64 and emulated AArch64 with
shared quality settings. Full output files, independent coefficient caches,
both cache-interchange directions, and bank-bound qualification blocks agree
at blocks 1/7/4096. Four additional checked cases each also run analysis on both
architectures: all eight analysis qualification blocks match those of actual
conversion. The preceding fourteen output rows and every previous cache file
are unchanged.

```text
SEX_REPRO_CODECS=1 SEX_REPRO_SHARED_GATES=1 SEX_REPRO_SHARED_UNTIL=1 SEX_REPRO_UNTIL=0 scripts/validate-cross-architecture.sh target/release/sex scripts/cross/aarch64-sex.sh target/cross-repro-shared-gates
```

Use the same isolated toolchain environment and a new result directory. The
corpus occupies about 40 MiB, predominantly initial/refined Until-40k cache
files; no large audio fixture is generated. Durable manifests record
[complete output/coefficient identities](cross-repro-shared-gates-2026-09-05.tsv)
and [four bank-bound qualification hashes](shared-gates-qualification-2026-09-05.tsv).
The latter has columns case, qualification-block SHA-256, and coefficient
SHA-256. Raw reports and caches remain in `target/cross-repro-shared-gates`.

| Additional case | Bank actually executed | Requested checks |
| --- | --- | --- |
| `certified-native` | Sane 3/2; 3×289 taps, C=62, MPFR=192 | Grid 9, continuous per-phase proof, all-image grid |
| `certified-high` | High 1/2; 1109 taps, C=96, MPFR=256 | Grid 17, proof tap cap 2049, all-image grid |
| `certified-amplified` | Fast 1/3; 385 taps, C=87, MPFR=160 | Gain 2^60 + normalize, grid 9, proof, all-image grid |
| `harmonic-until-up2` | Until-40k 2/1; 2×68197 taps, C=512, MPFR=1024 | Grid 9 and all-image grid; no continuous proof |

Until-40k genuinely refines on both architectures: the initial 66867-tap
candidate violates its unchanged -600 dB per-phase target, then the 68197-tap
candidate passes all requested gates. Its sampled phase stop peak is
-617.025532 dB, main harmonic complex error -623.075995 dB, and strongest
image/image-L2 peak -618.121795 dB at input frequency `3999/8000`. The image
lands at `-4001/16000` in output-rate units. These are measurements on 17 input
frequencies, not continuous upper bounds. The earlier 66867-tap harmonic-only
campaign did not certify the individual phases and is not interchangeable
with this stricter combined qualification.

The source fingerprint is
`48a03e07bd9996a9b9509098bc40950da6a455c35efb8367e69093d9a7fa24d3`.
Executed binary SHA-256 values:

- x86-64: `8a5d8d5a0cb899ff8242eca792f39db82484edc175d263d5a0c3f67c0142a5bd`;
- AArch64: `14b810c44837efc2c4c383dea81780a064b3298105827940538d76920453ead0`.

All 287 native tests and both all-target Clippy checks pass. Eleven refiner
and two planner library tests also pass on AArch64. A separate warm-cache
conversion failure with certificate work=1 produces an identical inconclusive
report on both architectures and preserves each previously rendered output;
the retained files are in `target/shared-gates-qualified/negative-*`.
The QEMU-only linking limitation is unchanged. This remains finite execution
evidence, with exact continuous proofs only for the specified smaller banks;
Until-40k and complete-image response are sampled here.

## Scope still to qualify

Physical ARM machines, further architectures/ABIs, big-endian storage boundaries,
different supported compiler/library versions, and production linking remain
unverified. Passing these finite corpora does not prove every possible input on
every CPU. It is concrete cross-instruction-set execution evidence supporting,
not replacing, the exact arithmetic and serialization contracts.
