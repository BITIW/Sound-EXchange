# Completion audit for the original SeX goal

Status: **the original 25-item project goal is implemented and verified for the
documented v0.1 scope**. This sign-off follows the requirement-by-requirement
[evidence map](requirement-evidence-map.md); it does not redefine completion
around a smaller milestone.

## Requirement disposition

| Original items | Disposition | Direct implementation/evidence index |
| --- | --- | --- |
| 1 | Met: SoX C behavior was audited as reference; keep/rewrite and licensing decisions are explicit, with no copied source. | [SoX audit](sox-reference-audit.md) |
| 2–5 | Met: fixed-point canonical signal, native/runtime/compile-time/big Q, explicit overflow/rounding/sign extension/guards, exact widened MAC and one final FIR rounding. | [Fixed-point model](fixed-point-model.md), [exact accumulator intervals](exact-accumulator-intervals.md), [exact FIR admission](exact-fir-admission.md) |
| 6–9 | Met: exact rational polyphase engine, separated Kaiser/window/global designers, quantization feedback and whole-chain automatic precision planning. | [Rational rates](exact-rational-rates.md), [filter design](filter-design.md), [quality refinement](quality-refinement.md), [planner](precision-planner.md) |
| 10–12 | Met: six numerical presets, bigint/MPFR Until-40k behavior and exact rational rate transformations. | [Preset table](precision-planner.md), [Until work policy](until-work-budgets.md), [actual 160/147 result](parallel-response-analysis.md) |
| 13–19 | Met: deterministic dither/noise shaping, four clipping policies, integer libsndfile I/O/metadata, generic streaming DSP, coefficient cache and CPU-independent reproduction. | [Dither](dither.md), [pipeline](dsp-pipeline.md), [I/O](io-formats.md), [cache](coefficient-cache.md), [cross architecture](cross-architecture.md) |
| 20–23 | Met: four-engine reference campaigns, measurable analysis, torture/property/large-stream tests, and exact scalar/AVX2/AVX-512/GMP comparisons. | [Validation](validation.md), [complete input phases](full-input-phase-campaign.md), [testing tools](testing-tools.md) |
| 24–25 | Met: all requested CLI rate forms/extensions, five responsibility libraries, Rust facade, `sex`, `sex-rate`, `sex-analyze`, `sex-bench`, `sex-fuzz`, and mathematical documentation. | [Original command](original-cli-command-qualification.md), [library/frontends](library-frontends.md), [25-item map](requirement-evidence-map.md) |

## Current-code qualification

The audited implementation-and-test fingerprint is
`65e3d30e448447cc3a368113dd2311fffd21254ab479b36e0b84d53246cc4acf`
over `Cargo.toml`, `Cargo.lock`, `src`, `crates`, `examples`, `scripts`, and
`tests`. Documentation and retained result directories are intentionally not
folded into that hash.

- `SEX_REQUIRE_SNDFILE_TESTS=1 cargo test --locked --release --workspace
  --all-targets`: **468 passed, 0 failed**; one deliberately expensive test is
  listed as ignored in that aggregate run.
- That exact ignored test,
  `until_40k_real_filter_meets_floor_and_streams_reproducibly`, was then run
  explicitly: **1 passed, 0 failed, 0 ignored** in 63.99 s.
- Native-only `sexq` without the bigint feature: 27 unit + 2 integration +
  5 documentation tests pass.
- Root/Q/rate library suites: **134 passed** on little-endian AArch64/QEMU and
  **134 passed** on big-endian AArch64/QEMU. Strict Clippy passes on native and
  both ARM profiles; formatting and shell syntax checks pass.
- Current-source native/big-endian corpus: 56 WAV outputs, 56 full processor
  reports, 56 FIR qualification blocks, six analyses, twelve complete cache
  payloads and fourteen identity rows agree byte for byte. They also match the
  preceding baseline. Runtime source fingerprint:
  `13dd332bcaaf9188af4ad4559dc508a2de8138c712e0b820e6c57f82f8f8eee1`.
- A current `sex-fuzz --suite all --cases 1000 --seed 42` run passes all 1000
  replayable generated cases (checksum `d1c79314128056de`). A current
  native/GMP benchmark executes the 160/147 streaming engines; it is an
  engineering measurement, not correctness evidence by itself.

Evidence is under `target/parallel-response-qualified` and
`target/parallel-response-corpus-qualified`. The full workspace is uncommitted;
completion does not imply a Git commit or registry/system installation.

## Original heavy command

The literal positional form from the goal now succeeds without manual work
budgets:

```text
sex input.wav output.wav rate 48000 \
  --preset until-40k --precision auto --error-floor -400dB
```

The preset's stronger −600 dB minimum correctly wins. Candidate 1 (160 ×
66867 taps, C=512, MPFR=1024) is rejected at −596.2401652407976 dB. Candidate
2 (160 × 68197 taps) passes the mandatory sampled response at
−617.0151829745743 dB and has coefficient identity
`5016336267899ace1c74d98b76e781a2ecd405e132330ee9e63e7354f30de86d`.
The output path uses Q65.512 and a 1537-bit planned accumulator, writes one
frame with zero clipping, and reports a 2^-495 FS numerical bound against a
2^-102 FS budget.

Block sizes 1 and 4096 produce the same complete qualification block and WAV
SHA-256 `cfc506654f7c71534dc866523ce027311de0c112924f30617782f1cba52b9d49`.
Both 664/677 MiB coefficient cache files remain byte-identical after replay.
`sex analyze` selects the same bank and reports:

- passband ripple: `1.317420931524651e-30 dB`;
- stopband peak: `-617.0151829745743 dB`;
- maximum coefficient error: `-3037.688421974879 dB`;
- actual accumulator minimum: 1092 bits, with 445 planned headroom bits.

These are actual quantized-bank, 65-point-per-band, all-160-phase measurements.
They are not mislabeled as a continuous-frequency certificate, MPFR designer
error bound, or final-PCM noise floor.

## External and adversarial evidence

Current checksum verification covers all 108 outputs of the complete
five-ratio input-phase campaign and all four outputs of the retained Until
four-engine sweep. The two self-contained campaign snapshots also verify 46
input/tool/source/reference entries. Together with the retained guarded signal,
multitone, near-Nyquist and preset campaigns, this covers SoX, libsoxr and
libsamplerate response/SNR/alias/error/impulse/phase comparisons. The oldest
campaign's output files still pass their 20 SHA entries, but its tool manifest
points at overwritten build paths and deleted temporary packages; it is
historical evidence only and is not used as the self-contained provenance
anchor.

Named and generated tests cover DC, both full-scale endpoints, full-scale sine,
alternating endpoints, positive/negative impulses, silence, 1 Hz, Nyquist edge,
multichannel PCM, one sample, awkward exact-rational rates, chunk partitions,
million-frame downsampling and a virtual 100 GiB frame-count path. No enormous
throwaway file is written.

## Boundaries that do not contradict the goal

- Built-in and libsndfile adapters intentionally accept integer/lossless source
  formats; float/lossy decoded audio is rejected rather than becoming canonical
  signal state.
- Current output containers require an integer-Hz sample rate. Arbitrary
  fractional rates remain exact in planning/analysis; they are never rounded
  silently into a container header.
- RF64 and an actual hundreds-of-gigabytes output are not claimed. The user's
  disk-preservation condition is met with bounded streaming and virtual-size
  counter tests.
- Sampled preset qualification is labeled sampled. Optional continuous and
  all-image modes retain their separate proof scopes and resource limits.
- Cross-CPU evidence covers native x86-64 and emulated AArch64 in both endian
  orders, not every physical CPU ever produced. Determinism follows from the
  checked integer signal algorithms and is additionally tested across worker
  counts and SIMD/GMP backends.
- The delivered `libsex` facade is a Rust v0.1 API. A stable C ABI, package
  registry publication, system-wide installation, and coverage-guided fuzzer
  were not requirements; local frontends and the requested property-fuzz tool
  are present and exercised.

No required functional item remains open in the original goal. Future
performance work, broader codec/container support, additional physical machines
or stronger proof modes may extend v0.1, but are not used to postpone or dilute
this completion decision.
