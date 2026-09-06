# Experimental big-endian qualification

SeX now selects a portable SHA-256 implementation on big-endian targets in
both the FIR library and the root processing library. This selection is part
of their Cargo dependency graphs, not a workspace-only compiler flag. Normal
little-endian builds retain pinned `sha2` 0.11.0; big-endian builds use pinned
`sha2` 0.10.9 with `force-soft`. Neither signal arithmetic nor coefficient
serialization, cache schema or mathematical identities change.

## Defect and regression

The first AArch64 big-endian library run exposed two wrong coefficient
identities while the corresponding numerical tests passed. The inspected
`sha2` 0.11.0 `aarch64_sha2.rs` loads message vectors with an explicit
little-endian assumption; its automatic SHA2 hardware dispatch does not
exclude big-endian AArch64. Under QEMU Cortex-A53, even the empty-message
digest was wrong:

- observed: `b2f2d3a8113e8aa27fe30b53dff301a26c696e60469b75b610cd0e9097ab40f7`;
- SHA-256: `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.

The new `sha256_standard_vectors_are_cpu_independent` test checks the empty
message, `abc`, and the 56-byte `abcdbc…nopq` vector, both one-shot and with
one-byte incremental updates. It fails before the dependency fix and passes
afterward on the big-endian executable. The original common-ratio coefficient
regression is retained; changing the expected hash would conceal the defect.
The same vector test passes on little-endian AArch64.

This is SHA-256 correctness, not a new cryptographic security claim. Hashes
remain integrity/identity checks, not authentication. Previously generated
big-endian caches from the faulty backend should not be treated as portable;
normal existing little-endian cache bytes remain the reference.

## Experimental toolchain

The executable target is `aarch64_be-unknown-linux-musl`, **ELF64 big endian**,
executed with `qemu-aarch64_be-static -cpu cortex-a53`. A separate C byte-order
probe produced `1 2 3 4` for the four bytes of integer `0x01020304`.
`sex --build-info` reports `endian=big`, `cross_gmp=true`, `qemu_only=true`.

This tier-3 target has no prebuilt standard library in the installed toolchain.
The wrapper compiles the installed Rust standard-library source using scoped
`RUSTC_BOOTSTRAP=1` and `-Z build-std=std,panic_unwind,test`. It does not change
application settings or install a target globally. The qualification uses
rustc 1.98.1 (48a229cea, 2026-09-01), Zig 0.15.2, QEMU 10.2.2, and the project's
pinned GMP/MPFR dependencies. This is not a supported production toolchain or
a claim of an independently verified compiler/source release pairing.

All new Cargo, Zig and GMP caches live in a task-specific temporary directory.
The existing pinned Zig installation is reused. `SEX_QEMU_VALIDATION_ONLY=1`
is mandatory: as with the earlier little-endian experiment, the linker wrapper
omits the unsupported Cortex-A53 erratum flag only in this explicitly QEMU-only
profile. Static linking uses `+crt-static` and `link-self-contained=no`.

Example (paths are this run's isolated directories):

```sh
export SEX_CROSS_TOOLCHAIN=/tmp/sex-cross-toolchain.kdS7JT
export SEX_BIG_ENDIAN_WORK=/tmp/sex-big-endian.Ci1i7r
export SEX_QEMU_VALIDATION_ONLY=1
scripts/cross/build-aarch64_be.sh test --locked --release --workspace \
  --exclude sexio-sndfile --features cross-gmp --lib
scripts/cross/build-aarch64_be.sh build --locked --release --features cross-gmp --bins
scripts/cross/build-aarch64_be.sh clippy --locked --workspace --all-targets \
  --features cross-gmp -- -D warnings
```

For a new run, create a fresh isolated `SEX_BIG_ENDIAN_WORK` directory and
provide the installed `rust-src`, Zig installation and big-endian QEMU binary.
Cargo must have access to the locked dependencies (local cache or network).
The wrapper places `-Z` after the subcommand so Clippy forwards the source-built
standard-library setting. An earlier Clippy invocation failed because it did
not forward that setting; its log is retained, not counted as a passed check.

## Scope and retained evidence

The completed default corpus passes all eleven cases (44 renders): exact copy,
empty/one-frame input, Fast, Sane eight-channel PCM, High, Absurd, Pointless,
a 4096-fractional-bit stateful gain/DC/mix/convolution/rate/dither/normalization
chain, and amplified quality refinement. A separate optimized corpus passes
three LS/Remez/amplified-LS cases (12 renders and six analyses), including
continuous phase certificates and sampled harmonic gates. Both harnesses exit
successfully and verify the unchanged source fingerprint at completion.
Every pair of independently generated cache directories compares equal;
together the two corpora contain twelve distinct cache files per CPU.

All 56 renders match across CPU, chunk size and reciprocal cache use, as do
the bank-bound qualification reports. The six optimized analysis reports
match their corresponding conversion qualification blocks. Exact PCM and
coefficient identities are retained in
[the identity table](big-endian-identities-2026-09-06.tsv). The two complete
corpora occupy about 6.5 MiB under `target/big-endian-corpus-qualified` and
`target/big-endian-optimized-qualified`.

Both corpus source manifests are
`33047c981522aa3407da03d49ee10c061a1d26c969478e1f985ea1c9cdd02220`
(Cargo files plus sorted source/crate/example/script files; excludes docs and
top-level integration tests). `target/big-endian-qualified/binaries.sha256`
records and rechecks all six frontend executables. The executed main CLI hashes
are native `7b3139dfd6dd71629aec5ca79df7cd3e714efd07c5c55fbef7deaf0b306f6545`
and big-endian `48b2a4d2217b459de5dae04ac941aee04df4b00b3fc68db7fc6e789d2c19cf6d`.

All 448 native release workspace/all-target tests pass (one heavy opt-in test
ignored). The final big-endian library invocation exits zero with all 330 tests
passing and none ignored: root 35, DSP 33, FIR 138, built-in I/O 17, planner 24,
Q arithmetic 36, rate 47. This includes both formerly failing coefficient
identities, cache mutation checks and actual 8192-bit arithmetic tests.
Warnings-denied workspace/all-target Clippy passes for native,
little-endian AArch64 and big-endian AArch64. Formatting and shell syntax checks
pass. Target-specific dependency-tree reports confirm the expected SHA-256
version is selected even without using the build wrapper.

The built-in integer WAVE adapter is usable in this static-musl executable.
Runtime-loaded libsndfile is **not** available in this profile. Big-endian
AIFF/FLAC or metadata/codec-runtime qualification is therefore not claimed;
the library run explicitly excludes `sexio-sndfile` instead of counting
unavailable-runtime skips as successful codec execution. Native codec tests
are separately run with `SEX_REQUIRE_SNDFILE_TESTS=1`.

Logs, including the original failures, are retained under
`target/big-endian-qualified`. The build caches and target output occupy about
one GiB together; no large audio track is generated.
The intermediate `library-tests-after-fix.log` contains 330 passing tests but
its wrapper exits 127: the wrapper file had been edited while its Cargo command
was still running, disturbing the shell's next read after Cargo returned.
That intermediate invocation is not counted as a clean qualification command;
`library-tests-final.log` is the repeat with an unchanged wrapper.

The cross-architecture harness compares independently designed coefficient
banks, complete cache files, qualification reports, and output WAVE bytes. It
uses native/cross cold caches plus both foreign-cache directions and block
sizes 1, 7 and 4096. Its default corpus excludes codecs and Until-40k:

```sh
SEX_QEMU_AARCH64=qemu-aarch64_be-static SEX_REPRO_CODECS=0 SEX_REPRO_UNTIL=0 \
scripts/validate-cross-architecture.sh target/release/sex \
  scripts/cross/aarch64_be-sex.sh target/big-endian-corpus-qualified
```

Use a new result directory for each run. The emulator override above records
the actual big-endian executable in the harness manifest; the second-program
wrapper itself already selects the big-endian runner. This bounded QEMU
qualification does not prove every CPU, physical ARM, every ABI, every preset
and ratio, or production linking. The original project goal remains open.

The optimized-only repeat uses the same command with
`SEX_REPRO_OPTIMIZED_ONLY=1` and a different new result directory.
