# Original requirement → evidence map

This is the evidence index used by the [completed v0.1 audit](completion-audit.md).
Numbers follow the user's original 25-item request. Entries
identify implementation and test evidence to inspect; they do not turn finite
tests, historical snapshots or cached metadata into stronger guarantees.
The original requirements remain in [project-goal.md](project-goal.md).

## Current authoritative checkpoints

- Current native workspace/all-target release run: **467 passed, 0 failed,
  1 explicitly opt-in heavy test ignored**, with libsndfile required. Log:
  `target/original-command-qualified/native-tests.log`. The new original-command
  regression and native/both-ARM Clippy runs are retained in that directory.
- Current runtime sources match the completed 14-case native/big-endian corpus
  fingerprint `4fea462e973999d83467381ad6e67324a66297474e9ca0de31f99634eed8df2e`.
  Since that corpus, only an integration test and documentation changed. Its
  56 WAVs, full processor reports, six analyses, twelve cache files and fourteen
  identity rows match the previous baseline. See
  `target/until-budget-qualified/baseline-comparison.log` and
  `target/until-budget-corpus-qualified`. The fingerprint excludes top-level
  integration tests; it must not be called a hash of the entire worktree.
- The actual Until-40k 44100→48000 command now passes its ordinary sampled
  qualification and renders one frame. Candidate 1 fails unchanged −600 dB
  requirements; candidate 2 passes at −617.015 dB. A different-block repeat
  reproduces qualification/output and leaves both caches unchanged.

## Requirements 1–9: model, arithmetic, resampling and planning

| # | Requirement and implementation evidence | Relevant validation / scope |
| --- | --- | --- |
| 1 | [SoX source audit](sox-reference-audit.md): rate, effects, formats/metadata, dithering, clipping, tests; explicit keep/rewrite decisions. | Reference only, no copied SoX implementation. The audit identifies a source snapshot; upstream links are not themselves current-code proof. |
| 2 | [Fixed-point model](fixed-point-model.md), `sexq`, `sexrate`, `sexdsp`, and CLI processing: canonical samples are signed integers with explicit binary points. | Native/GMP arithmetic and end-to-end tests; MPFR is confined to design/analysis/reporting. PRNG modular operations are not signal wrap. |
| 3 | Q1.31, Q1.63, `NativeQ/QFormat`, `ConstQ/ConstMac`, `BigQ/BigQFormat`; [compile-time formats](compile-time-q.md). | Sign-extension, endpoint, rounding, overflow and format tests; executable and compile-fail API examples. Big values have documented finite safety caps, not unlimited resource promises. |
| 4 | `sexq` add/sub/mul/rescale/shift/round/overflow policies and checked native/GMP MAC types implement the requested arithmetic layer. | [Signed interval proof](exact-accumulator-intervals.md), native-only feature build, native/GMP oracle tests. Public method names need not literally be `q_*`. |
| 5 | Widened products, exact accumulation, one final FIR rounding; [native/Big admission](exact-fir-admission.md) and [wide fallback](native-wide-fallback-proof.md). | Prefix/extrema and 8192-bit endpoint cases, 193-bit native fallback and 225-bit BigQ63 wide counterexamples; independent unbounded-MAC comparisons. |
| 6 | `sexrate` rational phase clock and native/arbitrary streaming polyphase FIR; no literal expanded upsample stream. | Absolute-coordinate integer oracles, all-phase/multichannel/chunk tests, actual 160/147 CLI runs. |
| 7 | `sexfir` Kaiser/windowed sinc, rectangular/Hann/Blackman/Dolph, global least squares and Remez; designers are separate from `sexrate`. | [Window qualification](windowed-cli.md), [optimized designers](optimized-designers.md), [optimized CLI](optimized-cli.md), finite solver limits and explicit nonconvergence. |
| 8 | `sexfir::refinement` assesses quantized coefficients and response; width and filter-shape changes are separate decisions. | Actual [−300 dB command](original-cli-command-qualification.md) rejects C62's coefficient budget and accepts C78; cached candidates are rechecked, not trusted as certificates. |
| 9 | `sexplan` and whole-chain numerical planner choose FIR/precision/guards/reserves; actual bank checks determine execution minimum width. | [Planner policy](precision-planner.md), [numerical budget](numerical-error-budget.md), [actual FIR L1](actual-fir-l1-budget.md), [analysis accumulator](analyze-execution-accumulator.md). Bounds identify their reference and exclusions. |

## Requirements 10–19: heavy modes and complete processing

| # | Requirement and implementation evidence | Relevant validation / scope |
| --- | --- | --- |
| 10 | Six numerical [preset definitions](precision-planner.md), fixed transition/attenuation/width/dither policies; stricter user targets never weaken preset minima. | Mandatory actual-bank sampled gates and optional continuous/image gates. A sampled acceptance is not a whole-band or complete anti-imaging certificate. |
| 11 | Until-40k uses BigQ signal arithmetic, MPFR coefficient design and automatic bounded refinement; [geometry-aware work budgets](until-work-budgets.md). | The original 160/147 path rejects 66867 taps and accepts 68197 taps at −617.015 dB; [phase-parallel evidence](parallel-response-analysis.md) records cost/scope. Block1/4096 output and full qualification match; both cache files remain unchanged. |
| 12 | `sexrate::SampleRate` exact fractional/decimal/scientific rate parsing and reduced scheduling. | [Exact rational rates](exact-rational-rates.md), overflow/cross-cancellation/property tests. Current output containers require an integer Hz value; fractional analysis/planning remains exact. |
| 13 | Native and BigQ TPDF, high-pass TPDF, noise-shaping orders 1/5/9, target depth and per-channel seeded PRNG. | [Dither contract](dither.md), distribution/state/chunk/native-vs-big tests and exact [final PCM error](final-pcm-error.md) aggregates. |
| 14 | Explicit saturate/error/normalize/allow-headroom policy, extended internal sample range and checked final PCM boundary. | Overshoot, clipping count, protected output and [two-pass input replay](normalization-input-replay.md) tests. Headroom does not authorize an unbounded integer PCM destination. |
| 15 | `sexio` uses hound for integer WAVE; `sexio-sndfile` adapts libsndfile for supported WAVE/AIFF/FLAC and typed metadata. | [I/O format matrix](io-formats.md), required native codec tests and [codec qualification](codec-read-qualification.md). Float/lossy input and big-endian static-musl codec runtime are not supported by this evidence. |
| 16 | Native and arbitrary-Q DSP stage/pipeline APIs: gain, channel mix, DC removal, FIR/convolution, rate, final quantization and format adapters. | [Pipeline contract](dsp-pipeline.md), 4096-fractional-bit stateful chains and frontend/facade integration. Rate remains a separate central engine. |
| 17 | Chunked adapters and FIR overlap/state; finite-duration scheduling, drain and bounded retained history. | Million-frame pathological-ratio test; [read](streaming-read-contract.md)/[write](streaming-write-contract.md) fault injection, partial frames, short reads and transactional output. Coefficient-bank memory is distinct from whole-track storage. |
| 18 | Versioned atomic coefficient cache, request identities, fixed-width integer encoding and complete payload integrity. | [Cache contract](coefficient-cache.md), [integrity tests](cache-payload-integrity.md), cross-endian foreign cache replay; actual 695,417,100-byte Until payload verified without treating it as a response certificate. |
| 19 | Explicit arithmetic/rounding/seed, rational scheduling, immutable bank identity and endian-stable serialization. | [Cross-architecture](cross-architecture.md), [big-endian qualification](big-endian-qualification.md), byte-equal complete outputs/reports/caches. QEMU evidence is not physical hardware coverage or an exhaustive enumeration of every CPU. |

## Requirements 20–25: measurement, validation and ecosystem

| # | Requirement and implementation evidence | Relevant validation / scope |
| --- | --- | --- |
| 20 | Four-engine comparisons with pinned SoX, libsoxr and libsamplerate: SNR, response, alias/error spectra, impulse and phase. | [Spectral campaign](spectral-reference-campaign.md), [preset sweeps](spectral-preset-campaign.md), [guarded sweeps](padded-sweep-campaign.md), [complete input-phase campaign](full-input-phase-campaign.md). Historical snapshots retain commands/tools/results; each states its ratios, bands, PCM limitations and source version. |
| 21 | `sex analyze`/`sex-analyze` report ripple, stopband, coefficient error, actual accumulator requirement/headroom and qualified identity. | Actual [−300 dB analysis](original-cli-command-qualification.md) agrees with conversion. “Headroom” is the declared-input arithmetic reserve, not the measured music peak. |
| 22 | DC, full-scale sine/endpoints, impulse, silence, 1 Hz, near Nyquist, exact awkward ratios, multichannel and one-sample cases. | Named library/CLI torture tests, replayable property driver and finite external fixtures. Million-frame execution plus virtual 100 GiB counter checks avoid enormous disk writes; they do not prove RF64 or full huge-file I/O. |
| 23 | Exact AVX2/AVX-512 backend implementation after scalar/GMP reference correctness. | Scalar/SIMD/native/GMP endpoint and random-oracle comparisons; [benchmark/property tools](testing-tools.md). SIMD is explicit where measured throughput does not justify automatic dispatch; unsupported CPUs retain scalar behavior. |
| 24 | Shared CLI parser/execution for SoX-like `-r`, `--rate`, positional `rate`, preset/precision/error-floor controls. | [Three-form −300 dB campaign](original-cli-command-qualification.md) and regression; the original Until-40k positional command now passes sampled qualification and renders. |
| 25 | Five responsibility-separated component libraries, `libsex` facade, `sex`, `sex-rate`, `sex-analyze`, `sex-bench`, `sex-fuzz`; mathematical contracts linked throughout this index. | [Library/frontends](library-frontends.md), external facade consumer, local install and tool runs. Stable C ABI, registry publication and system-wide installation were not requested completion conditions. `sex-fuzz` is a reproducible property driver, not coverage-guided fuzzing. |

## Completion sign-off

The Until command is resolved with its real exit, all candidate verdicts,
accepted bank, output, resource accounting and cache/chunk replay. Relevant
current sources and retained results have been inspected with the scope
distinctions above. The [completion audit](completion-audit.md) records the
final test matrix, heavy command, external checksums and non-goal boundaries.
The resolved [open-item audit](goal-audit-open-items.md) preserves the original
Until quota failure and successful correction rather than erasing history.
