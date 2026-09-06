# Shared library entry and standalone frontends

The root `sex` package now builds one Rust library and three thin executables:
`sex`, `sex-rate`, and `sex-analyze`. Parser, planning, DSP, designer/cache routing,
quality gates and transactional output code live once in the library. The
launchers select a frontend enum; they do not spawn `sex`, duplicate its parser,
or implement another resampler. `cargo run` still defaults to `sex`.

## Commands

```sh
sex input.wav -r 48000 output.wav
sex-rate input.wav -r 48000 output.wav
sex-rate input.wav output.wav rate 48000 --preset high
sex-rate plan input.wav -r 48000 --preset until-40k
sex-analyze input.wav -r 48000 --preset high --certify --harmonics
```

`sex-rate` requires an explicit `-r RATE` or positional `rate RATE` for
conversion. It also accepts the existing read-only `plan` command, but rejects
the `analyze` subcommand; use `sex-analyze` or `sex analyze` for that.
Explicit unity-rate conversion retains the existing same-rate bypass and
associated designer/gate restrictions.

`sex-analyze INPUT ...` is equivalent to `sex analyze INPUT ...`, not an
output-writing command. An extra output path is rejected. Like existing
analysis it may materialize coefficient-cache files; use
`SEX_COEFFICIENT_CACHE=off` to disable that. It never publishes an audio output.
Every frontend shares all relevant quality, precision, designer, clipping,
dither and effect options and the same numerical defaults. Conversion-only
options, such as block size, are not newly admitted by analysis.

Each executable has its own usage, version/build-info name and error prefix.
Success is exit status 0 and failure is 1. Informational commands need no audio
paths. CLI arguments are UTF-8; invalid OS arguments now return an explicit
failure instead of an `env::args` panic. This does not add arbitrary-byte CLI
path support. The lower-level I/O APIs still use Rust paths directly.

## Rust facade (`libsex`)

The Cargo package/library name is `sex`, producing `libsex.rlib`; Rust callers
import `sex::...`. It exposes the existing component crates without replacing
their types or arithmetic contracts:

| Root module | Component |
| --- | --- |
| `sex::io` | `sexio`: streaming audio and built-in integer WAVE |
| `sex::io_sndfile` | `sexio-sndfile`: optional runtime codec adapter |
| `sex::q` | `sexq`: exact/fixed arithmetic |
| `sex::fir` | `sexfir`: design, qualification and coefficient cache |
| `sex::rate` | `sexrate`: rational scheduling and polyphase execution |
| `sex::dsp` | `sexdsp`: stages, clipping and dither |
| `sex::plan` | `sexplan`: precision/preset and numerical planning |
| `sex::cli` | Shared command parser and execution entry |

`sex::cli::run(Frontend, arguments)` accepts strings **including argv[0]** and
returns `Result<(), Box<dyn Error>>`; it does not exit the hosting process.
It otherwise has CLI semantics: writes stdout/stderr, reads coefficient-cache
environment settings and performs the requested file operation. Use the
component APIs when explicit data/configuration control is needed instead of
these process-level conventions.

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    sex::cli::run(sex::cli::Frontend::Rate, std::env::args())
}
```

The executable example is `examples/library_rate.rs`. The ordinary launchers
use `cli::entry`, which reads OS arguments, reports errors and returns an
`ExitCode` to `main`. The facade is a v0.1 Rust API, not a stable C ABI,
`libsex.so` export contract or a published registry package. Its aggregate
build requires the existing GMP/MPFR dependencies; component crates remain
independently available with their established feature boundaries.

## Local installation

```sh
cargo build --release --lib --bins
cargo install --offline --locked --path . --root ./local
cargo install --offline --locked --path crates/sexbench --root ./local
cargo install --offline --locked --path crates/sexfuzz --root ./local
```

The three installs provide `sex`, `sex-rate`, `sex-analyze`, `sex-bench`, and
`sex-fuzz` under `./local/bin`. Offline installation requires dependencies
already cached and a working native toolchain. No network is needed for audio
processing. AIFF/FLAC still require runtime libsndfile; built-in integer WAVE
does not. These commands do not modify PATH or install into system directories.
The recorded verification used `target/frontends-qualified/install`, not `./local`.

## Verification scope

Parser tests bind aliases to the same command objects and prohibit wrong modes.
CLI integration checks Kaiser and Remez, matching PCM and complete qualification
between `sex` and `sex-rate`, and matching analysis through both entrypoints.
They also check information commands, missing rate, extra output paths, cheap
Until-40k planning and preservation of existing audio after failure. A separately
built Rust consumer and installed frontends reproduce the prior Remez PCM and
qualification. The installed benchmark and fuzz tools have bounded smoke tests.

Cross-architecture identities and verification commands are in
[cross-architecture.md](cross-architecture.md). This frontend/package milestone
does not establish broad preset/platform qualification, stable binary APIs,
published distribution packages or completion of the full project goal.
