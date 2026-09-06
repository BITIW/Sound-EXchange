# Stable spectral runs and pre-render FIR checks

Long reference campaigns now preserve the exact project executables they run,
instead of using mutable `target/release` files throughout a long execution.

## Snapshot contract

The harness hashes Cargo.toml, Cargo.lock, src/, crates/, examples/ and scripts/;
builds with Cargo JSON artifact output; copies exactly the reported `sex` and
`spectral_probe` executables into the new result's `tools/`; compiles the
libsamplerate frontend there; and preserves the source under `source/`. Missing
or ambiguous artifacts fail. Configured Cargo target directories are respected
instead of assuming a hardcoded executable location. Parsing requires `jq`.

Before processing, the workspace and copied source must both match the pre-build
fingerprint. All renders/measurements then use the copied project executables.
Completion rechecks those executables, Cargo.lock and the preserved source,
plus external executable/wrapper entrypoints and optional pinned SoX package/
binary hashes. `snapshot-check.log` retains the checks. `tools.sha256` is relative
to the result directory; `reference-entries.sha256` uses external absolute paths.

The whole shell campaign is parsed into a function before it executes. Later
workspace script edits cannot replace unread commands. Source changes after
snapshot preparation are allowed; changes during build/copy still fail.

This is not a hermetic runtime or security attestation: external shared libraries
are not all copied, and compiler/global Cargo configuration is not fully
archived. Reference versions remain recorded. The source copy is the enumerated
build/validation tree, not an archive of documentation, top-level integration
tests or the entire user workspace. Historical artifacts are not overwritten.

## Accepted FIR support is checked before rendering

Until-40k exposed an expensive ordering problem: its initial 1000→2000 plan had
66867 taps/phase, but feedback accepted 68197. Initial 33.5-second guards only
supported 66999 taps. A post-render check would reject a very expensive render.

The sweep harness now runs its copied `sex analyze` first, qualifies/caches the
actual bank, and checks the accepted length against guard capacity. Evidence
is retained in `design-preflight.log` and `guard-preflight.tsv`. Both subsequent
renders must report that same coefficient SHA-256, and complete output files
must match. The post-render guard check remains. A unity copy has no active FIR
and skips preflight. Cached banks still undergo the normal CLI quality gates.

Preflight keeps the existing sampled-bank scope and exclusions: it is not a
continuous-band certificate, an MPFR design-error enclosure or a final PCM floor.

## Verified controls: 2026-09-06

- `target/spectral-snapshot-preflight-smoke`: Fast 48000→16000 passes all four
  engines and the bit-identical repeat. Its SeX audio matches the previous Fast
  campaign. Preflight/render bank identities and snapshot checks agree.
- `target/spectral-snapshot-negative`: replacing only the probe in a separate
  snapshot copy makes its checksum fail. The original copied probe is retained
  as `spectral_probe.original`; the successful snapshot is untouched.
- `target/spectral-preflight-short-guard`: Absurd 8200→4100 initially plans 4097
  taps within capacity 4099. Feedback accepts 4309; preflight rejects this
  before any render log or output audio exists. This uses real FIR refinement.
- `target/spectral-preflight-guard-recovery`: the same case with half-second
  guards passes all four engines and the exact repeat. Accepted coefficients:
  `0a817eb4728f4ccfe322e3ba34269b787ffc5ad8613d49db09e9d398b95540a2`.

Final harness source fingerprint:
`ad1c21f0d626cf7e44e07108bc2dd1e750c40ea41b5bbb5df95d6ca4a9531387`.
Processor/probe binaries are unchanged from the preset checkpoint. Its 349
native tests and 18 emulated-ARM probe tests remain the preceding Rust-code
checkpoint, not a newly expanded test count.

## Until-40k: completed guarded 2/1 sweep

The insufficient-guard pilot, `target/spectral-until-guarded`, was deliberately
interrupted once accepted taps were known to exceed its guard. Its session
ended with status 130, not an observation timeout. Input, cached coefficients
and diagnostics remain available without a successful-campaign claim.

The replacement, `target/spectral-until-guarded-qualified`, uses explicit
137-quarter-second guards (34.25 seconds per side) and the new preflight.
It has 69507 mono input frames at 1000 Hz, exact ratio 2/1 and a 1..445 Hz core
chirp. Guard capacity 68499 covers the accepted 68197 taps/phase, two phases,
C=512 and MPFR-1024. Coefficient identity:
`dd6513d34d55bd899b7b08870ea5e4b3af22ec1a67be29a5bc6548e1fd6ee227`.

Preflight passes the unchanged -600 dB sampled gates: passband deviation
1.42604268899e-31 and stopband peak -617.025532066781 dB. The first render has
now completed: mono PCM32, 2000 Hz, 139014 frames, output SHA-256
`69c869120b99b9dc930acd1ed5b5e654c1dee51c91a34478a714d8926b3b943e`.
Its accepted bank matches preflight and the post-render guard check passed.
The harness completed successfully on 2026-09-06. Its 4096-frame-block repeat
matches the first WAV byte-for-byte with that same hash. All four engines pass
the full core and ten decile checks: 44 interval gates total, 2001 measured
frames per engine, and zero output-duration discrepancy. Frozen project/source
and external reference-entry checks pass in `snapshot-check.log`. Pairwise
waveform differences and spectra are retained; agreement with a reference is
not used as a requirement that different algorithms produce identical audio.

| Engine | Whole-core waveform SNR (dB) | Lowest tested interval SNR (dB) | Whole-core error (dBFS) |
| --- | ---: | ---: | ---: |
| SeX Until-40k | 111.250970577 | 101.925853872 | -120.414723555 |
| SoX | 99.249601898 | 89.390454986 | -108.413354876 |
| libsoxr | 99.076396604 | 89.217270153 | -108.240149581 |
| libsamplerate | 103.928479098 | 94.069710766 | -113.092232075 |

These measurements compare a finite, quantized smooth chirp against its
analytic waveform over 1..445 Hz; they are not estimates of a -600 dB PCM
noise floor or universal algorithm rankings. The mandatory SeX waveform gate
is 85 dB and the three reference gates are 58 dB. The -600 dB target and
-617.0255 dB sampled stopband result concern the coefficient bank separately.
This single ratio, mono fixture and comparison band do not qualify all
Until-40k rates, input phases, its entire passband or continuous images.

The 26 MiB result directory retains inputs, both outputs, all references,
commands, versions, hashes, guard checks and measurement tables. The source
and binary snapshots precede the newer Kaiser design-error certificates, so
the campaign does not claim those later guarantees. No live campaign remains
at this checkpoint; the earlier incorrect-guard attempt remains separately
recorded as failed, not folded into this passing result.
