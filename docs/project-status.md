# SeX goal and implementation checkpoint

The active goal is an offline-first Sound Exchange processor with exact rational
rate scheduling, a deterministic fixed-point signal path, measurable quality,
and no silent wrap. Quality takes priority over latency and throughput. The
The original 25-item v0.1 goal is complete; the audited disposition and current
evidence are in [completion-audit.md](completion-audit.md). Later development
may extend the project without retroactively changing the signed-off scope.

The compact [25-requirement evidence map](requirement-evidence-map.md) indexes
the implementation, executed checkpoints and explicit scope limits. It is an
audit index; the separate [completion audit](completion-audit.md) records the
sign-off. The former [open-item audit](goal-audit-open-items.md) retains the
baseline failure and its verified resolution.

## Current checkpoint: Until-40k 160/147 sampled qualification

The ordinary original-ratio command now completes: its 66867-tap/phase bank
fails the unchanged −600 dB target at −596.240 dB, feedback strengthens it to
68197 taps/phase, and that 10,911,520-coefficient bank passes at −617.015 dB.
It renders one 16-bit frame with no clipping using Q65.512 signal arithmetic
and a 1537-bit planned accumulator. The exact numerical bound is 2^-495 FS
against budget 2^-102 FS. This is sampled-grid evidence with the documented
MPFR-design/final-PCM/continuous-band exclusions. A different-block cache
repeat reproduces the complete qualification and WAV, with both caches unchanged.

To make this common ratio practical without changing its math, bigint response
grids are now evaluated independently across phases and extrema are merged in
fixed order. Worker-count regressions are bit-identical; a 160-phase High bank
matches the old binary byte-for-byte and improves 2.10 s to 0.16 s on this
host. The Until search rechecks both cached candidates in 73.60 seconds at
2,023,400 KiB peak RSS. See [deterministic parallel analysis](parallel-response-analysis.md).

All 468 native workspace/all-target release tests pass (one existing heavy
opt-in ignored), plus 134 root/Q/rate tests on each ARM endian profile and
strict Clippy on all three. These checks establish unchanged calculations over
their covered banks, not a universal continuous-frequency result.

## Earlier checkpoint: original −300 dB command qualification

The original `--rate 48000 --error-floor -300dB` command now has a dedicated
executed campaign and permanent regression covering all three rate syntaxes,
blocks 1/7/4096, nonempty stereo input, analysis and warm caches. The real search
rejects C62's coefficient-error budget, raises C to 78 and accepts the unchanged
−300 dB target; whole-chain execution follows at Q65.78. Native and big-endian
AArch64/QEMU output bytes and qualification reports match, including unchanged
foreign cache payloads. See [the evidence and PCM scope](original-cli-command-qualification.md).

All 467 native workspace/all-target release tests pass, with one existing heavy
opt-in test ignored and libsndfile tests required. Strict Clippy and formatting
checks pass (Clippy on native and both ARM profiles). This checkpoint changes
only tests/documentation; processing algorithms and the previously qualified
corpus runtime sources were unchanged at that checkpoint. The later Until-40k
qualification and phase-parallel implementation are recorded above.

## Earlier checkpoint: Until-40k geometry-aware work budgets

The CLI now reserves four initial-bank work equivalents above generic floors
for unset Until-40k coefficient/response-term quotas. Every explicit quota is
preserved independently; retries cannot replenish remaining budgets. Other
presets, library defaults, filter geometry and mandatory gates are unchanged.
See [the precise policy](until-work-budgets.md).

All 466 native workspace/all-target release tests pass, with the existing heavy
opt-in test ignored and libsndfile tests required. Both ARM endian profiles
pass 134 root/Q/rate tests. Strict Clippy passes on all three profiles. The
pre-policy integration expectations needed updating: resource-failure tests
now supply explicit low quotas, while default-plan tests expect automatic
geometry-aware limits. Interrupted/stale-expectation pilot logs are retained
but are not counted as successful qualification.

The complete native/big-endian 14-case corpus also passes: 56 renders and full
processor reports, six analyses, twelve cache payloads and all fourteen identity
rows are byte-identical to the preceding native-wide-proof baseline. This
corpus excludes Until-40k and static-musl codec runtime checks; it does not
stand in for the separate heavy execution below. Evidence is retained in
`target/until-budget-qualified/baseline-comparison.log` and
`target/until-budget-corpus-qualified`. The corpus source fingerprint is
`4fea462e973999d83467381ad6e67324a66297474e9ca0de31f99634eed8df2e`.

At that checkpoint, the actual original-ratio Until-40k command had only been
started with a pinned executable, one-frame 44100 Hz input and normal gates;
mere preflight was explicitly not a success claim. The later completed result
and repeat are recorded in the current checkpoint above.

## Earlier checkpoint: native wide fallback proof and requirement audit

The native-coefficient GMP fallback now uses a separately proved Q65.63 width,
fixing the hard-coded 192-bit assumption left behind by exact native admission.
Some admitted banks need 193 intermediate bits. The regression distinguishes
exact accumulation from a legitimate final destination overflow; a larger
matrix covers both phases and all five rounding modes against unbounded GMP.
All 458 native tests pass (one heavy opt-in ignored); both ARM endian profiles
pass 127 root/Q/rate tests. See [the fallback proof](native-wide-fallback-proof.md).
All 56 ordinary corpus renders, full reports, six analyses and twelve caches
remain byte-identical to the preceding baseline across native/big-endian CPUs.

The [original-goal audit](goal-audit-open-items.md) also verified a functional
gap: ordinary Until-40k 44100→48000 options fail before design because their
10,698,720-coefficient candidate exceeds generic feedback quotas. Heavy-mode
default work policy was the next actionable item at that checkpoint; see the
new policy and completed audit above. The goal was not complete at that checkpoint.

## Earlier checkpoint: exact signed FIR admission

Native, Q1.63/GMP and arbitrary-Q FIR banks use exact signed-interval admission,
including the asymmetric negative endpoint. Extended Q65.63 input has its own
proof, so a tighter baseline estimate cannot cause an intermediate overflow.
The actual minimum and legacy conservative cache/report estimate are explicitly
separated. All 456 native tests pass (one heavy opt-in ignored), and both ARM
endian profiles pass the new regressions and 125 root/Q/rate library tests.
The 56-render corpus, six analyses, full processor reports and twelve caches
remain byte-identical to the preceding baseline across native/big-endian CPUs.
See [exact FIR admission](exact-fir-admission.md) and the detailed remaining-work
section below. This is a correctness checkpoint, not completion of the full goal.

## Earlier checkpoint: shared libsex entry and standalone frontends

The root package now exposes a Rust `sex` library (`libsex.rlib`) and builds
thin `sex`, `sex-rate`, and `sex-analyze` launchers. They share one parser,
numerical planner, quality-gate path, DSP engine, cache and transactional output
implementation. The library re-exports the existing I/O, arithmetic, FIR, rate,
DSP and planner crates. Its CLI runner returns errors without exiting the host
process; the lower component APIs retain their established contracts.

`sex-rate` requires an explicit rate, also accepts cheap `plan`, and rejects
analysis mode. `sex-analyze INPUT ...` is the standalone analysis entry and
cannot publish an audio output. Existing `sex` command behavior and the default
`cargo run` target are retained. Each frontend has its own information commands
and diagnostic prefix. Invalid UTF-8 OS arguments fail explicitly rather than
panicking. See [library-frontends.md](library-frontends.md).

All 342 native release workspace/all-target tests pass (one existing heavy test
ignored), both frontend parser tests pass on AArch64 QEMU, and both warnings-
denied Clippy checks pass. The standalone frontend corpus passes twelve PCM
renders, six analyses and reciprocal cache reads on x86-64/AArch64, retaining
all previous audio, complete qualification and cache identities. A separately
built Rust consumer reproduces the same Remez PCM on both architectures.

Offline local installs provide all five programs: `sex`, `sex-rate`,
`sex-analyze`, `sex-bench`, and `sex-fuzz`. Installed processing/analysis reproduce
the same qualified result, and bounded benchmark/fuzz smoke tests pass. The
8.5 MiB installation is under `target/frontends-qualified/install`; no system
directory or PATH setting was changed. Exact identities and scope are recorded
in [cross-architecture.md](cross-architecture.md).

Scalable very-long global solves, stronger numerical/continuous qualifications
and broader preset/platform evidence remain open. The facade is an initial Rust
API, not a stable C ABI or published distribution. The full goal is incomplete.

## Preceding checkpoint: bit-identical Remez cosine reuse

Remez now reuses the already-rounded MPFR cosine values shared by its grid
evaluations and interpolation matrices. It preserves Gaussian elimination,
arithmetic order, convergence, coefficients and numerical/solver reports.
The arbitrary designer enables the complete table only when the remaining
storage cap fits it; otherwise the direct path remains available. Native
Remez has a fixed 8 MiB optional-table ceiling. Work reports separate core
storage from optional planned scratch bytes, and logical work charges stay
unchanged. See [remez-cosine-cache.md](remez-cosine-cache.md).

Differential tests compare individual cosines, complete error vectors, solvers,
quantization and banks at C=62/96/4096 for unity and 2/3. They check the exact
memory threshold and one byte below it. Persistent-cache tests write/read with
both workspace policies at C=96/4096 and verify identical files and recomputed
resource plans. All 337 native release workspace/all-target tests pass (one
existing heavy test ignored); both new tests pass on AArch64 QEMU, and both
warnings-denied Clippy checks pass.

The focused independent x86-64/AArch64 corpus passes twelve PCM renders, six
analyses and both foreign-cache directions. All prior audio, coefficient and
cache-file identities are unchanged. Qualification reports add only the planned
`cosine_cache_bytes` field; removing that field reproduces every previous
qualification byte. Host benchmarks at T=129 and T=193, C=96, P=160 show about
5.7–5.9x faster solves with identical coefficients and numerical/solver reports.
Exact timings, identities and scope are in [cross-architecture.md](cross-architecture.md).

This removes repeated transcendental work, not cubic solve/selection complexity.
Scalable very-long global solves, stronger numerical/continuous qualifications
and broader platforms remain open. The full goal is incomplete.

## Preceding checkpoint: correct unity designer endpoint geometry

Global LS and Remez now represent the unity-ratio stopband as a single weighted
Nyquist endpoint. Repeated endpoint samples previously changed LS's effective
weight and made Fast unity Remez fail its extrema safety cap. Remez initialization
now includes the isolated endpoint explicitly; native and arbitrary preflight
counts match the actual grid. Unity algorithm/cache identities are versioned
separately, leaving non-unity LS v1/Remez v2 unchanged.

Both optimized unity analyses now qualify their T=129, C=64, P=128 banks with
Fast's unchanged -80 dB target, continuous phase amplitude and sampled harmonic
gates, without numerical retries. Reports explicitly state that the stopband
is only the Nyquist point, not a finite-width rejection band. Analysis remains
hypothetical; same-rate conversion still bypasses the FIR. Exact independent
three-tap minimax and LS solutions check the corrected coefficients.
Details: [unity-designer-grid.md](unity-designer-grid.md).

All 335 native release workspace/all-target tests pass (one existing heavy test
ignored), five unity-filtered library tests pass on AArch64 QEMU (four new), and
both warnings-denied Clippy checks pass. Twelve unity analyses cover independent
cold caches, warm reuse and both foreign-cache directions with identical banks,
complete reports and cache bytes. Six non-unity PCM renders and six analyses
retain the preceding identities. Exact verification scope and hashes are in
[cross-architecture.md](cross-architecture.md).

Scalable very-long global solves, broader numerical recovery, certified MPFR
design-error bounds, continuous complete-image/long-FIR proofs and broader
preset/rate/platform qualification remain open. The full goal is incomplete.

## Preceding checkpoint: bounded numerical solver recovery

Optimized quality refinement now doubles automatic MPFR working precision after
selected typed LS/Remez numerical failures. It retains the filter specification
and target, repeats whole-chain preparation and all preflights, and charges every
failed solve against the shared attempt/work budgets. Explicit numeric precision
is pinned. Corruption, I/O, invalid requests and resource errors never trigger
numerical recovery. Structured failure records and CLI diagnostics distinguish
failed solves from actually assessed banks. See
[solver-precision-recovery.md](solver-precision-recovery.md).

A real LS regression at ratio 2/3, T=129, rolloff 3/5 and C=64 fails at P=128
and recovers at P=256 without changing its -80 dB target. The accepted corrected
bank passes coefficient, phase, continuous amplitude and sampled complete-image
gates; raw coefficients match a P=512 calculation. Cold/warm paths have equal
logical charges and store only the successful bank. Its exact coefficient
identity is locked on both x86-64 and AArch64 QEMU.

All 330 native release workspace/all-target tests pass (one existing heavy test
ignored), all three recovery tests pass on AArch64, and both warnings-denied
Clippy checks pass. Six warm-cache PCM renders and six standalone analyses on
the two architectures retain the preceding optimized-CLI output and complete
qualification identities; cache bytes are unchanged. Exact verification scope
and binary/source hashes are in [cross-architecture.md](cross-architecture.md).

Broader Remez recovery evidence, scalable very-long global solves, certified
MPFR design-error bounds, continuous complete-image/long-FIR proofs and broader
preset/rate/platform qualification remain open. The full goal is incomplete.

## Preceding checkpoint: qualified optimized designers in the CLI

`--designer global-ls|remez` is now shared by conversion, analysis and planning.
Both methods construct arbitrary Q2.C global prototypes, use the kind-4 cache,
and participate in whole-chain numerical planning and actual-bank quality
feedback. Failed coefficient budgets grow C by 16; failed response doubles FIR
half-length without changing method, target, weights or band geometry. Analysis
and conversion report and execute the same qualified bank, with optional exact
continuous phase amplitude and sampled complete-image checks.

Explicit per-candidate/cumulative solver work and storage/grid/precision caps
apply before materialization, equally on cache hits. A failed solve, resource
limit or quality gate never silently selects Kaiser or overwrites existing
audio output. Planning is still cheap and unqualified. Full CLI contracts are
in [optimized-cli.md](optimized-cli.md).

All 327 native release workspace/all-target tests pass (one existing heavy test
ignored), as do warnings-denied Clippy checks on native and AArch64. Both new
real refinement tests pass under AArch64 QEMU, including actual coefficient
and shape growth, unchanged targets, bounded work and wrong-bank rejection.
CLI regressions cover three cases at blocks 1/7/4096, bank/report equality with
analysis, gain 2^60, normalization and preservation of output on warm failures.

The focused x86-64/AArch64-QEMU corpus passes three optimized cases, twelve
PCM renders, six standalone analyses and both cache-interchange directions.
All audio, complete qualification reports and three cache files match. Six
additional paired resource/proof failures preserve output and produce identical
diagnostics. The retained corpus is about 456 KiB; it does not repeat historical
Until-40k/codec cases or certify physical ARM. Exact identities and commands are
in [cross-architecture.md](cross-architecture.md).

At that checkpoint automatic working-precision recovery was next; it is now
described above. Scalable very-long global solves, continuous complete-image/
long-FIR proofs and broader preset/rate/platform qualification remain open.

## Preceding checkpoint: persistent optimized-designer cache

Global LS and Remez now have persistent arbitrary-width bank and solver-report
reuse, including all candidates of bounded coefficient-precision search. A
separate kind-4 entry preserves previous Kaiser/window cache bytes and all
existing coefficient identities. Its streaming SHA-256 covers the complete
header, numerical/solver reports and coefficient payload. Loading checks
bounded strings/counts, finite and consistent reports, canonical Q words,
exact DC, coefficient identity, accumulator bounds, checksum and EOF.

Full preflight and logical search charges apply to hits as well as misses;
warm cache cannot bypass limits or select a different spec. Corruption is an
explicit failure and does not silently overwrite the bad entry. This is
trusted local reuse, not authentication or an MPFR/continuous-quality proof.
Actual-bank spectral gates still run after a hit.

All 323 native release workspace/all-target tests pass (one existing heavy
test ignored), along with warnings-denied Clippy on native and AArch64. Six new cache tests pass
under AArch64 QEMU, including C=4096, resealed malformed reports, corrupt
payloads, parallel writers, parameter identity and cached-search budget parity.
The library probe now accepts `--cache-directory`, exercising fifteen cache
entries, thirteen inspected banks, 39 integer renders and five qualified banks.
Native and AArch64 cold, warm and foreign-cache full outputs match: six complete
runs and 234 integer-stream renders. Both architectures independently produce
the same fifteen cache files; all thirty files remain unchanged after reciprocal
reads. All preceding Remez-v2 numerical reports/streams are unchanged.
Exact source/binary/cache/output identities are recorded in [cross-architecture.md](cross-architecture.md).

At that checkpoint, production CLI selection and whole-quality-planner
integration were next; they are now described above. Scalable global solves,
full image proofs and broader platform qualification remain open. See
[coefficient-cache.md](coefficient-cache.md#optimized-global-ls-and-remez-cache).

## Preceding checkpoint: Remez v2 signed extrema and verified convergence

The retained 129-tap-per-phase Remez regression now succeeds at both P=160 and
P=512 with identical C=96 raw coefficients, in 12 iterations. Ratio 2/3, r=9/10,
weights (1,10), design density 16, iteration cap 64 and target 80 dB are unchanged.
The actual corrected bank passes coefficient and sampled phase gates, an exact
continuous phase amplitude certificate and the 65-point-per-band complete-image
grid. Continuous stopband upper bound is -106.8464251509297 dB; sampled image
peak is -140.0847775290125 dB. These are not interchangeable scopes.

The root selector bug discarded single-point sign lobes when an opposite-sign
neighbor had a larger absolute error. Signed extrema preserve those lobes.
Frequency-width-based seeding also avoids allocating half of the initial
references to a much narrower band merely because its grid has the same size.
Convergence now requires actual alternating reference errors and matching
solved ripple across the design grid; a frozen reference set alone is insufficient.
Native and arbitrary Remez coefficient identities are v2. Other designers retain
their identities. Both Remez backends now charge the same bounded exchange work.

All 317 native release workspace/all-target tests pass with one existing heavy
test ignored; warnings-denied Clippy passes on native and AArch64. All 17
optimized-library tests pass under AArch64 QEMU. The executable library corpus
independently designs thirteen banks, runs 39 chunked integer streams and
qualifies five banks on each architecture; the full 51,472-byte outputs match.
Exact identities and rebuild commands are in [cross-architecture.md](cross-architecture.md).
CLI/cache integration, full shape/precision planning, scalable very-long global
solvers, continuous complete-image bounds and broader platform qualification
remained open at that checkpoint. Later required CLI/cache work is recorded
above; optional extensions remain scoped in [optimized-designers.md](optimized-designers.md).

## Preceding checkpoint: arbitrary-width global LS and Remez library APIs

`sexfir::optimized` now solves directly into arbitrary Q2.C, including real
C=4096 coefficients executed with an 8192-bit configured integer MAC. Its
bounded coefficient-only search grows C and accumulator width together while
preserving method, bands, length, weights and design grid. Separate response,
continuous amplitude and all-image APIs inspect the actual corrected bank.
At that checkpoint these methods were not yet selectable through the CLI or
persistent cache; later checkpoints above record both additions.

The work exposed a mathematical issue in the legacy independent-phase LS:
continuous per-phase amplitude success can coexist with a transition-band image
near -27.77 dB. The new `GlobalLeastSquares` fits one Type-I prototype and passes
that regression's complete-image grid. A separate T=129, ratio 2/3, r=9/10
bank passes an 80 dB target with a continuous stopband upper bound of
-82.54646954872555 dB and sampled image peak -150.7939938126297 dB. These are
different measurements; the latter is not a continuous complete-image bound.

Large Remez exchange is not solved: the retained T=129/r=9/10 fixture fails
at both 160 and 512 working bits. It returns a typed error, never an
unconverged bank or a silently changed target. Its selector now rejects excess
rounded extrema before allocating tables, and arbitrary-width preflight
accounts for the selection work and storage. Robust exchange, solver-precision
retries, CLI/cache integration and full shape/precision planning remain open.

All 313 native release workspace/all-target tests pass (one existing opt-in
heavy test remains ignored), and warnings-denied Clippy passes on native and
AArch64. All 13 new library tests also pass under AArch64 QEMU. The fixed
cross-architecture probe independently constructs eleven banks and executes
33 integer-stream renders on each architecture; complete outputs (coefficient
identities, reports, raw streams, and the final solver failure) are byte-identical.
This is a library corpus, not a rerun of the preceding 88-render PCM corpus.
The 13 added regressions include the known explicit Remez failure;
passing that regression is not a claim of successful filter design.
See [optimized-designers.md](optimized-designers.md) for the mathematical model,
resource and precision contracts, and verification scope. The full project
goal remains incomplete.

## Preceding checkpoint: arbitrary-width CLI window selection

Conversion, `analyze`, and `plan` now share `--window` with Kaiser (unchanged
default), rectangular, Hann, Blackman, and Dolph–Chebyshev. Non-Kaiser banks
use arbitrary Q2.C with C>=64, GMP execution, their own versioned cache entries,
and the same mandatory quantization/response refinement and optional continuous
and all-image checks. Whole-chain planning is repeated after every candidate
and precision change, with a full-coefficient-range accumulator reserve.
The selected window and target are never silently replaced after failure.

This work exposed and fixed Dolph's fractional-coordinate DFT interpolation:
negative bins must not be treated as positive aliases away from integer sample
positions. The corrected Dolph algorithm/identity is v2; other native windows
and Kaiser retain their identities. A separate planner fix reserves enough
MAC width for long rectangular banks without assuming Kaiser's L1 norm.

All 300 native workspace/all-target tests pass (one opt-in heavy test remains
ignored in the default suite), as do warnings-denied Clippy checks on native
and AArch64. Window-library tests also pass under AArch64 QEMU. Coverage includes
all-window native-versus-bigint C=62 coefficient/measurement identity, C=4096
and 8192-bit configured MAC, cache corruption/canonical padding, coefficient
versus shape feedback, window mismatch rejection, and three full CLI cases
matching analysis to PCM at blocks 1/7/4096. Warm-cache inconclusive proofs
preserve previous output, and large Dolph requests fail before cache access.

The 22-scenario x86-64/AArch64-QEMU corpus passed 88 PCM renders and 16
standalone analyses, including four new window cases. All 18 preceding
PCM/coefficient rows, qualification blocks, and Kaiser cache bytes are unchanged.
Hann 3/2 refines from 129 to 257 taps; High Dolph 1/2 certifies a continuous
stopband upper bound of -168.5299471608949 dB against -160 dB. A final
diagnostic-only precision-error correction re-passed the complete native suite
and eight additional uncached window renders across both architectures, with
identical outputs/reports. The approximately 41 MiB corpus, exact identities,
and distinction between the full and focused runs are documented in
[cross-architecture.md](cross-architecture.md).

See [windowed-cli.md](windowed-cli.md) for the mathematical, resource, and cache
contracts. At that checkpoint LS/equiripple were native-only library APIs;
later checkpoints added the required arbitrary-width CLI path. Faster
long-Dolph construction and stronger optional proofs remain future extensions.

## Preceding checkpoint: shared conversion/analysis quality gates

`--grid`, `--certify`, `--harmonics`, and all their resource controls now have
one configuration/parser across conversion, `analyze`, and `plan`. The same
materialization/refinement path returns the qualified bank and assessment;
conversion constructs its resampler from that exact bank. Both modes produce
an identical bank-bound qualification block with the coefficient SHA-256,
target, actual C, quantization budget, grid measurements, and requested
certificate/harmonic reports. The refiner rejects a wrong-grid or partial-bank
assessment. No second, differently configured design follows analysis.

Planning uses the selected grid for its work estimate, accepts all gate settings,
and reports static supplemental limits without designing or claiming proof.
Same-rate conversion explicitly rejects supplemental FIR checks because it
bypasses the resampler. Unity-rate analysis labels its bank hypothetical.
Failures preserve existing output, including a qualified FIR followed by a
later PCM clipping failure; no successful conversion qualification is printed
for that failed output.

All 287 native all-target tests and both native/AArch64 all-target Clippy
checks pass. Three full CLI cases match analysis reports to actual PCM at
blocks 1/7/4096: native Sane 3/2, bigint High 1/2, and Fast 1/3 with gain 2^60
and normalization-driven coupled precision. Additional tests cover shared
argument validation, warm-cache proof failure, resource preflight, and same-rate
rejection. The eleven refiner and two planner tests also pass on AArch64 under
QEMU. The contract is in [shared-quality-gates.md](shared-quality-gates.md).
Continuous complete-image and practical long-FIR proofs remain open; the full
goal is still active.

The expanded 18-case / 72-render x86-64/AArch64 matrix passes, including all
four new checked cases and eight matching standalone analysis reports. The
preceding fourteen output rows and cache files are unchanged. Until-40k 2/1
actually refines from 66867 to 68197 taps per phase because the first bank
fails its -600 dB per-phase target; both architectures select the same new
bank and produce identical PCM/reports. Its all-image qualification uses
17 input frequencies, not continuous bounds. The corpus is about 40 MiB,
mostly coefficient caches. A separate warm-cache proof-budget failure also
matches across architectures and preserves the destination. Exact identities
and commands are in [cross-architecture.md](cross-architecture.md).

## Preceding checkpoint: mandatory preset qualification

Every rate-changing conversion and analyzed FIR now uses the feedback loop,
even without an explicit error floor or `--refine-quality`. The latter remains
a redundant compatibility alias; there is no unchecked CLI conversion switch.
The accepted design carries a required assessment, and the old optional
unqualified materialization/analyzer fallback has been removed. Same-rate
conversion still bypasses FIR work entirely.

This closes the observed default-policy gap: Absurd and Pointless at ratio 1/2
now select their strengthened 4309/8473-tap banks automatically, with unchanged
targets, fractional widths, dither policy, and band edges. Ordinary analysis
and conversion used the same 65-point grid. At that checkpoint stronger gates
were still analyze-only; the shared configuration described above now allows
conversion to select those same checks and bank. Differently configured runs
must still not be conflated.

`plan` labels its candidate unqualified and reports initial coefficient, grid,
and MPFR work with any needed limit increases. The huge Until-40k 160/147
candidate is rejected before cache access under the default guards; tests
verify that an existing output survives. A failed response retains sampled
measurements, and an inconclusive proof retains its work/cell diagnostics on
stderr without printing a successful analysis on stdout.

The 280 native all-target tests and native/cross Clippy pass. The separate
native Until-40k release test passes with default feedback in conversion:
133733 taps, C=512, identical coefficient identity and PCM at blocks 1/7/4096.
The new regression also executes both strengthened presets through real PCM
conversion at all three block sizes. Gate scope and migration are documented
in [quality-refinement.md](quality-refinement.md). The full goal remains active.

The full 14-case / 56-render x86-64/AArch64 corpus also passes without the
former opt-in flag. Every output and cache identity matches the preceding
feedback-enabled corpus. Independent success, inconclusive-proof, and large
initial-plan reports match exactly between both architectures; the explicit
compatibility alias also produces the same successful report/cache. Build and
result identities are recorded in [cross-architecture.md](cross-architecture.md).

## Preceding checkpoint: feedback-driven quality refinement

An explicit `--error-floor` now automatically qualifies the actual materialized
bank in a bounded feedback loop; `--refine-quality` enables the same behavior
for an implicit preset target. Coefficient-budget failure raises fractional
width independently of shape. Response failure raises Kaiser design attenuation
and length without changing the target or band edges. Requested continuous and
all-image gates participate; an inconclusive certificate stops explicitly.
Whole-chain signal/error planning is rerun before design and before constructing
the final pipeline, so no stale precision or accumulator plan survives a change.
`plan` remains a cheap, explicitly unqualified initial candidate.

The coarse-bank regression grows C=8 to C=24 without changing its FIR. The
legacy Sane 1/3 regression grows from 769 to 861 taps and receives an exact
continuous certificate at its unchanged -110 dB target. Candidate/work caps,
explicit precision, cache reuse, incorrect materialization, and preservation of
existing output are tested. The contract and limits are in
[quality-refinement.md](quality-refinement.md).

All 278 native workspace/all-target tests and both native/AArch64 all-target
Clippy checks pass. The twelve new refiner/planner library tests also pass on
AArch64 under QEMU. With feedback enabled, the 14-case / 56-render corpus passes
including F=4096 and tagged WAVE/AIFF/FLAC. It detects actual initial failures in
Absurd and Pointless at 1/2 and independently selects matching strengthened
banks on both architectures: 4097→4309 and 8193→8473 taps. Their file/cache
identities change intentionally; the other twelve scenarios retain their
previous output identities. At that checkpoint the initial preset definitions
were unchanged, and the no-feedback CLI path still used those candidates;
the current mandatory policy above removes that path.
Independent combined feedback/certificate/harmonic reports and caches also
match exactly for Sane 3/2. The separate native Until-40k release test also
passes with feedback enabled for analysis and all three PCM renders: its
133733-tap C=512 bank is accepted unchanged, with identical output at blocks
1/7/4096. No new Until-40k AArch64 audio render was performed here.
Full continuous anti-imaging and long-FIR proofs
remain open; feedback acceptance is only as strong as the requested gates.

## Preceding checkpoint: complete sampled harmonic-transfer analysis

`sex analyze --harmonics` now evaluates all `L` output components of the actual
quantized bank at each exact rational input-frequency grid point. It includes
the transition band, wanted-component complex/phase error, image amplitudes,
aggregate image power, and exact signed output-frequency locations. This
closes the previous analyzer's single-Nyquist-point scope gap for sampled
anti-imaging measurements, without calling sampled extrema continuous bounds.

The implementation uses coefficient-exact MPFR conversion, complex Horner
evaluation and a complete phase DFT. It agrees with independent global-prototype
sums, Parseval, and real Q1.63 cosine/sine execution at five ratios and block
sizes 1/7/4096. Tests include DC image cancellation, zero-order-hold images,
4096-fractional-bit coefficients, and early work/storage/shape rejection.
All 260 native workspace tests and Clippy pass; the eleven new harmonic tests
also pass on AArch64 under QEMU. The mathematical model, CLI controls, and
qualification harness are documented in
[harmonic-response-analysis.md](harmonic-response-analysis.md).

The eight-case campaign passed on x86-64 and AArch64 with byte-identical complete
reports and independently generated coefficient caches. It covers Sane at
1/3, 160/147, 147/160, 479/441, and 1/48; High at 160/147 and 1/2; and an actual
Until-40k upsampling bank at 2/1 (two phases × 66867 taps, C=512, MPFR=1024).
Until's measured strongest image and image-L2 peak are -602.260765 dB against
the -600 dB target. This is 129 input-grid points across the complete band, not
a continuous proof. Both all-target Clippy checks pass. The retained corpus is
about 23 MiB; no large audio fixture was generated.

The model is the linear transfer defined by quantized coefficients; per-MAC
rounding, finite-stream edges, effects, final PCM, and MPFR designer error have
separate scopes. Continuous complete-image bounds and practical long-FIR
certification remain open. The full goal is still active.

## Preceding checkpoint: exact continuous-frequency certificates

`sex analyze --certify` now bounds the actual quantized per-phase FIR amplitude
over entire requested bands. Exact GMP autocorrelation/Chebyshev polynomials
and Bernstein subdivision provide interval bounds; MPFR uses directed rounding
only for endpoints, dB targets, and outward reporting. Resource exhaustion is
explicitly inconclusive, never a quality pass. Default tap limits reject
Until-40k before coefficient materialization/cache access.

Sane is certified at 1/3 and across all 160 phases of 160/147. High's original
1/2 bank produced an exact -160 dB violation witness; the corrected preset
keeps that target, adds 12 dB design margin, and grows from 513 to 555 taps at
unity (1025 to 1109 at 1/2). The corrected bank has continuous passband deviation
at most 5.02632e-9 and stopband at most -165.13199 dB. High identities change
intentionally; old failure/new proof are regression-tested. Mathematical
derivation, exact result scope, limits, and full enclosures are in
[continuous-response-certificate.md](continuous-response-certificate.md).

Fourteen certifier tests pass on x86-64 and AArch64 under QEMU, including exact
complex/rational oracles, F=4096, hidden peaks, partial-bank failure, and
outward reporting. The complete native workspace has 246 passing tests.
All 174 AArch64 library tests and both native/cross all-target Clippy checks
also pass. The expensive Until-40k test was not repeated in that checkpoint,
which did not certify its long FIR, full anti-imaging, or MPFR design-calculation
error. The current checkpoint supplies the requested sampled Until qualification;
the stronger proof scopes remain explicitly separate.

## Preceding checkpoint: four-engine spectral campaign and Sane correction

The bounded reference campaign now runs real SoX, libsoxr, and libsamplerate
against SeX Sane at five ratios: 1/3, 160/147, 147/160, 479/441, and 1/48.
All 20 engine/rate combinations passed their explicitly scoped gates, along
with five byte-identical SeX repeats at different block sizes. MPFR-synthesized
32-bit PCM includes 16/18-channel silence, DC, impulse, full scale, 1 Hz,
pass/stop/near-Nyquist tones, multitone, and a logarithmic sweep. Reports retain
gain/phase fits, waveform and fitted residuals, alias peaks, impulse response,
and sample-aligned pairwise error spectra. References are not assumed to have
the same passband shape. Results and limitations are in
[spectral-reference-campaign.md](spectral-reference-campaign.md).

This exposed a real preset defect: the original Sane 129-point grid missed
peaks. At 48→16 kHz a 513-point grid measured -106.34696 dB rejection, failing
the -110 dB target. Sane now reserves 12 dB design margin, keeping that target
unchanged; the FIR grows from 769 to 865 taps and measures -126.28844 dB on
1025 points. The old failure is retained as a regression, and the corrected
plan passes all three quantized-bank gates at 1/3, 3/2, and 1/48. This remains
sampled qualification, not a continuous-band proof. Sane coefficient/output
identities intentionally change; the other five presets do not.
An additional CLI check passed 1025 points/band across all 160 phases of the
actual 44.1→48 kHz bank; its passband deviation was 6.0844e-7. The upsampling
stopband endpoint measurement is not a full anti-imaging certificate.

## Cross-architecture execution qualification

The full integer corpus now passes byte-for-byte between x86-64 Linux and
AArch64 Linux under QEMU: 15 scenarios, each rendered four ways with independent
coefficient generation, block sizes 1/7/4096, and cache interchange in both
directions. All six presets are covered, including an actual 133733-tap
Until-40k bank, 4096-fractional-bit stateful DSP, coupled precision refinement,
and tagged WAVE/AIFF/FLAC. Entire output files and cache directories agree.
The full matrix was rerun after the Sane correction; only its expected Sane
hashes changed relative to the previous record.
After the subsequent High correction, 14 scenarios / 56 renders were repeated
successfully (all except the expensive Until-40k case). Only High and its three
tagged-format cases changed; the other ten repeated cases retain their hashes.
Independent uncached Sane/High continuous-analysis reports also match exactly
between x86-64 and AArch64.

The isolated setup pins and verifies the cross compiler and codec runtimes,
without installing system packages. `sex --build-info` identifies the executed
architecture and ABI. The selected Zig linker cannot preserve a Cortex-A53
hardware-erratum workaround: its removal is explicitly opt-in for emulator-only
validation and recorded in the binary. This is therefore not a production ARM
build or physical-hardware qualification. Commands, exact hashes, limitations,
and the durable result manifest are in
[cross-architecture.md](cross-architecture.md).

## Whole-chain numerical precision planning

`PolyphaseFirBig`, `CausalResamplerBig`, and `InterleavedResamplerBig` now
carry runtime-format BigQ samples, not only bigint coefficients. They share
the exact finite/causal timeline with the native and Q65.63 APIs. Signal,
coefficient, and output formats are explicit; full-range L1 bounds determine
the accumulator width. Existing designer/cache coefficients are shared through
an immutable Arc rather than copied for each signal format or channel.

`sexdsp::arbitrary::{BigStage, BigPipeline}` similarly retains runtime Qm.n
through gain, DC removal, mixing, convolution, history, and tails. Q65.63 stages
are now exact adapters over that implementation. `BigQ` supports direct signed
PCM import/export and exact rational scaling at its own binary point, avoiding
an intermediate Q*.63 rounding. Tests cover 4096 fractional bits and declared
signal formats with thousands of integer bits, as well as an 8192-bit FIR MAC.

The CLI now selects the signal format automatically and retains it through
effects, FIR, normalization, dither, noise-shaping history, and direct PCM
export. Typical fractional widths are 63/63/96/160/256/512 for fast through
Until-40k. `--signal-precision auto|BITS` can raise the computed minimum,
independently of MPFR `--precision`. Exact input effect rationals are quantized
at the selected precision, never promoted from pre-rounded Q2.62 values.

The signal planner's stage-count/gain policy is now an initial candidate.
`sexplan::numerical` propagates reference peaks and three error components:
stage rounding, effect coefficient quantization, and FIR coefficient
quantization. Bound arithmetic uses outward-rounded GMP integers. DC forcing
uses the exact quantized pole gap; effect coefficient errors come directly
from the requested rational and actual nearest-quantized value.

The CLI increases signal and FIR coefficient precision independently and
re-evaluates until the combined error is at most `2^(-amplitude_bits-2)` FS.
The refined filter plan controls MPFR width, accumulator planning, backend,
cache identity, and actual execution. `plan` and `analyze` accept all four
current effects and report per-stage peak/error bounds. A gain of 2^60 before
the fast 160/147 FIR raises its coefficients from native Q2.62 to Q2.85; an
inverse attenuation before that FIR removes the need for that increase.

The comparison reference uses exact rational effects, exact-DC-corrected MPFR
FIR values, and the same selected normalization scale. It excludes ideal-filter
approximation, MPFR design-calculation error, and final PCM/dither/clipping.
The recurrence proofs and exact scope are in
[numerical-error-budget.md](numerical-error-budget.md).

## Existing CLI contract

The CLI retains its selected Qm.n across gain, DC removal, channel mix,
convolution, and resampling. Generic stages use exact GMP products and proven accumulator
widths. Native-coefficient resampling attempts checked i128 and recomputes an
overflowing dot product exactly in GMP. Every MAC rounds only once; final PCM
quantization owns clipping. `--gain 4 --clip normalize` is supported without
an intermediate Q1.63 clamp. Normalization targets the actual selected PCM
positive endpoint, accounting for dither and shaping guard.

`BigDitherQuantizer` generates integer noise at the actual signal binary point.
F=63 agrees with the native implementation across every PCM width 1..64,
all five rounding modes, and all six dither/shaping modes. Wider formats draw
explicit little-endian PRNG limbs and retain arbitrary-precision feedback.
Interleaved quantization commits a complete block or rolls it back and poisons
the instance. Normalize scan/render use one shared streaming-pass implementation.

Validation at this checkpoint:

- 287 workspace/all-target tests passed; Clippy with warnings denied passed. One expensive
  Until-40k qualification is opt-in and was run separately in release mode.
- 49 native DSP/arithmetic/I/O tests passed without default bigint features.
- Native DSP and libsndfile adapter passed Windows MSVC cross-target checking;
  this is not a Windows execution test or a full GMP CLI cross-build.
- 174 library tests passed on AArch64 under QEMU after the continuous-certifier
  and High changes, along with all-target Clippy. The preceding checkpoint also
  passed both cross-library integration tests and 1000 deterministic property
  cases with the same checksum as x86-64. x86-only SIMD branches are not
  executed on ARM.
- 1000 deterministic property cases passed with root seed
  `2611923443488327891`, checksum `39315e1921c2d028`. Wide products, whole/chunked
  execution, native/GMP equivalence, narrow SIMD equivalence, PCM round trips,
  and arbitrary-fractional signals against an independent rational-coordinate
  raw-integer oracle are covered.
- A cross-library test imports real 24-bit PCM, attenuates by 2^-200,
  resamples 160/147 with MPFR-designed coefficients, and amplifies by 2^200.
  Direct PCM output equals the unattenuated reference for block sizes 1, 7,
  and 4096. The attenuated samples would all vanish at a Q*.63 boundary.
- Full amplified effect-chain CLI outputs match byte-for-byte at block sizes
  1, 7, and 4096 for both fast and high presets.
- The CLI preserves a sub-Q*.63 bit that decides a final 16-bit PCM tie.
  A 4096-fractional-bit gain/DC/mix/convolution/rate/normalize chain produces
  byte-identical output at block sizes 1, 7, and 4096 with TPDF, high-pass TPDF,
  and ninth-order shaping. Near-unity DC radii receive enough precision, and
  tiny rational mix coefficients no longer vanish through Q2.62 staging.
- An independent exact-rational oracle covers complete gain/DC/mix/convolution/
  polyphase-FIR/normalization execution at F=16, 63, 127, and 4096. Every output
  error is inside the predicted whole-chain bound at blocks 1, 7, and 4096.
  Additional tests cover 400-frame recursive DC state, exact coefficient-error
  enclosures, independently refined widths, distinct coefficient-cache keys,
  and preserving existing output when explicit MPFR precision is insufficient.
- The bounded 48 kHz to 16 kHz libsoxr/libsamplerate reference smoke test passed,
  including exact duration and repeat-file equality. Its measured stop-tone
  residual is affected by fixture quantization; it is not an intrinsic filter
  stopband measurement or the complete requested reference campaign.

## Actual Until-40k qualification

The original beta-65 preset missed its -600 dB target at ratio 1/2: a 17-point
grid measured -599.5041 dB and a passband deviation of about 1.054e-30. The gate
failed explicitly. Until-40k now receives a 12 dB design margin, like stricter
custom targets, without weakening the -600 dB acceptance floor. At unity or
upsampling this gives 66867 taps/phase; at ratio 1/2 it gives 133733.

The opt-in regression test uses nine mono PCM frames at 44100 Hz, converts to
22050 Hz, and qualifies the quantized bank on 65 points per band at MPFR 1024.
Measured results:

- passband ripple: `6.771616404292742e-30 dB`;
- maximum passband deviation: `7.692266178683119e-31`;
- stopband peak: `-602.2607651540772 dB`;
- maximum coefficient quantization error: `-3038.959257253248 dB`;
- quantization response-error bound: `-2991.990760321494 dB`;
- coefficient SHA-256:
  `70a22de17e9b06ce9702d0b4441ea72c9f95c749c8ea118793c672dc09967b2a`.

The unchanged passband, stopband, and coefficient gates all passed. Conversion
uses Q65.512 samples with a 1537-bit accumulator plan, reuses the coefficient
cache, emits exactly four frames, and is byte-identical for blocks 1, 7, and
4096. This is a sampled single-ratio qualification, not a continuous-band,
all-ratio, or final PCM noise-floor guarantee. The test removes its small audio
fixture and roughly 9 MB cache; no large-track fixture is generated.
The command is in [testing-tools.md](testing-tools.md#explicit-until-40k-qualification).
It was rerun after coupled planning was integrated: the coefficient identity
and response measurements remained unchanged, with a computational numerical
error bound of `2^-494` FS against the defined reference (budget `2^-102` FS).

## Remaining work toward the original goal

Native and Q1.63/GMP FIR constructors now use exact signed admission too,
closing the preceding constructor limitation. The legacy symmetric estimate
remains available for cache/report compatibility; new minimum-width accessors
expose the actual requirement. Q65.63 gets its own all-phase proof rather than
assuming the exact Q1.63 width plus 64 is sufficient. A real cancellation case
needs 225 intermediate bits where that shortcut allows only 224, despite a
representable final result. Native i128 endpoint tests execute scalar, AVX2
and full-vector AVX-512, with an unsafe second phase still rejected. Real
unity Kaiser/cache tests cover an exactly admitted 160-bit bank. All 456 native
tests pass (one heavy opt-in ignored); the new cases pass on both ARM endian
profiles, with strict Clippy across all three. See
[exact FIR admission](exact-fir-admission.md) for proof, API and qualification scope.
All 56 corpus renders, full processor reports, six analyses and twelve caches
also preserve the preceding baseline byte for byte.

The arbitrary-Q resampler's automatic accumulator width and CLI execution
requirement now use attainable signed intervals, not symmetric absolute-L1
estimates. This removes an unnecessary bit at negative power-of-two endpoints
while still proving every intermediate sum safe. Additive native/GMP APIs,
exhaustive small-format oracles and endpoint tests through 8192 bits cover the
same rule. The native execution summary checks every phase with phase-bounded
scratch. All 453 native tests pass (one heavy opt-in ignored); all five new
tests execute on both ARM endian profiles, with strict Clippy on all three
platforms. Complete root/Q/rate library runs pass 123 tests per ARM profile.
All 56 native/big-endian corpus renders, full processor reports, six analyses
and twelve complete caches remain byte-identical to the preceding baseline.
Legacy conservative designer/cache APIs and explicit reserves remain
unchanged, as documented in [exact accumulator intervals](exact-accumulator-intervals.md).
This tightens the actual execution requirement without reducing signal precision
or changing the FIR algorithm; it does not close the complete project audit.

CPU-independent qualification now includes experimental **big-endian** AArch64
under QEMU, not just two little-endian CPUs. It exposed and fixed an upstream
SHA-256 hardware-backend endian assumption: the root and FIR libraries now
select a pinned portable backend automatically on big-endian targets. Standard
vectors reproduce the original failure and pass with the fix; existing native
coefficient/cache identities and signal algorithms are retained. All 56
native/big-endian corpus renders, six optimized analyses, bank qualification
blocks and reciprocal complete caches match. All 448 native tests pass (one
heavy opt-in ignored), plus 330 big-endian library tests in a clean completed
invocation, with warnings-denied Clippy on all three target profiles.
See [big-endian qualification](big-endian-qualification.md) for retained failures,
commands, exact identities and static-musl/codec limitations. This materially
extends reproducibility evidence, not a proof for every CPU or the full goal.

A requirements audit found and closed the missing general compile-time Qm.n
surface: `ConstQ<I,F>` and typed `ConstMac<I,F,CI,CF>` now complement runtime
formats, sharing the same native arithmetic and checked one-rounding MAC.
Invalid widths and mismatched operand types are compile-fail tested; const
sample/format construction, all policies and endpoint/poison/reset behavior
are checked on native and ARM. All 447 native workspace tests pass (one heavy
opt-in ignored), plus the native-only 25-test suite and seven doctests. Strict
native/cross Clippy passes. See [compile-time Q formats](compile-time-q.md).
This closes an explicit sample-model requirement, not the full goal audit.

The Sane reference campaign now covers every input-time residue on all five
ratios: 3/147/160/441/48 positions, 799 per engine and 3196 impulse responses
across SeX/SoX/libsoxr/libsamplerate. All gates and 27 SeX block-size repeats pass;
an independent audit of actual summaries verifies unique, complete coverage for
each engine/rate pair. The snapshotted 1.2 GiB result is retained. All 444 native
tests pass (one heavy opt-in ignored); all 20 spectral probe tests execute on ARM,
with strict native/cross Clippy passing. Sparse DFT optimization retains every
historical five-ratio report byte, and a muted-last-channel control fails on the
intended phase. See [complete input-phase evidence](full-input-phase-campaign.md)
for batch geometry, reference versions and finite-frequency/Sane-only scope.

Post-design numerical budgets now use exact maximum phase L1 from the actual
native/bigint coefficient bank, replacing the coarse `2*taps` propagation gain.
The same bank-bound value survives design-certificate merging; coefficient-error
allowances and references remain unchanged. Chosen precision is not reduced in
this checkpoint. All 442 native tests pass (one heavy opt-in ignored), the three
new regressions execute on ARM, and strict native/cross Clippy passes. Four bank
kinds have matching complete analysis reports, conversion bounds and audio across
CPUs. See [actual FIR L1 budgets](actual-fir-l1-budget.md) for exact formulas,
measured bound tightening and the still-conservative pre-design precision step.

Final PCM error is now measured separately from the pre-PCM numerical target.
The bigint streaming meter keeps exact squared-error sums and peaks, including
actual dither/shaping/saturation relative to the immediate pre-PCM signal after
normalization; MPFR is confined to the approximate dB display. It distinguishes
empty input and exact PCM bypass. Five new tests bring native coverage to 439
passes (one heavy opt-in ignored); four new library/display regressions execute
on ARM. Twenty-four F63/F4096 all-dither renders match audio and metric reports
across CPUs, and two High 160/147 renders preserve historical WAV bytes. Strict
native/cross Clippy and native-only `sexdsp` builds pass. See
[final PCM error](final-pcm-error.md) for exact formulas and measurement scope.

Two-pass normalization now verifies the actual input frame count and a streaming
digest of decoded PCM before effects, plus unchanged format, typed metadata and
decoder on reopen. Changed input fails before output publication even when a
zero gain erases the changed samples or a rate ratio hides the changed length.
Three new regressions execute on native and emulated ARM; 434 native workspace
tests pass (one heavy opt-in ignored), with strict native/cross Clippy passing.
Twelve High 160/147 WAVE/AIFF/FLAC renders retain the prior complete audio and
qualification identities across x86-64/ARM, blocks 1/7/4096 and reciprocal caches.
See [normalization input replay](normalization-input-replay.md) for transactional
failure evidence and the distinction from locking/snapshotting the source file.

Native/big Kaiser and windowed caches now protect report values as well as
coefficients with a whole-payload SHA-256 footer. New versioned request keys
and kinds preserve but do not trust/reseal older unprotected files; first use
redesigns those banks once, while optimized LS/Remez cache bytes stay unchanged.
Every-byte mutation/report-corruption and legacy-preservation tests pass on
native/ARM. Four fresh v2 cache files match across CPUs, foreign-cache hits work,
and complete analysis reports match historical values. All 431 native tests
pass (one heavy opt-in ignored), with strict native/cross Clippy passing.
See [cache payload integrity](cache-payload-integrity.md) for migration cost,
format details and the distinction between integrity and authentication.

`sex analyze` now reports the actual signal-format accumulator and execution
backend instead of the designer's baseline Q1.63 values. Analysis and conversion
share bank preparation and full-range checks; the explicit scope distinguishes
planned spare bits from an observed audio peak. A five-case integration test
covers native, extended-native-coefficient, High, windowed and optimized banks,
including cache hits and exact F63→F4096 width growth. All five complete reports
match on x86-64/emulated ARM, and a historical High PCM render remains identical.
Native coverage reaches 429 passing tests (one heavy opt-in ignored), with
strict native/cross Clippy and formatting passing. See
[analysis execution bounds](analyze-execution-accumulator.md) for exact cases.

The libsndfile writer now checks native error status even when the returned
frame count appears complete, preflights both report counters before writing,
and commits them together after success. Close failures report the actual
returned error code rather than stale handle text. Three new regressions cover
controlled status/close failures and real-file counter-overflow preflight.
All 428 native tests pass (one heavy opt-in ignored); ARM executes 13 codec and
30 root tests with runtime skips forbidden. Six no-rate WAVE/AIFF/FLAC copies
are identical across CPUs and to the historical tagged inputs. See
[libsndfile write completion](sndfile-write-contract.md) for exact scope,
including the distinction between callback injection and disk failure.

WAVE writing now fails closed after any write error and cannot subsequently
finalize with a successful report. Reports commit only complete successful
blocks. Header field products and the full classic-RIFF data/header size are
checked before entering hound's narrow counters; this prevents silent wrapping
or panic at the container boundary without creating a giant fixture. Five
fault/limit tests pass on native and emulated ARM; a new CLI truncated-PCM test
preserves the prior destination and removes its temporary output. Native
coverage reaches 425 passing tests (one heavy opt-in ignored), with strict
native/cross Clippy and formatting passing. See the
[streaming write contract](streaming-write-contract.md) for exact limits and
the distinction between failed generic sinks and transactional CLI publication.

Bigint FIR boundary coverage now includes an actual product requiring 8192
signed bits (8191 fails), two-sign cancellation retaining a single output LSB,
and an automatic full-range FIR accumulator of 8196 bits (8195 fails). Signed
endpoint/reset/poison checks cover widths from 1 through 8192. `BigMac` reuses
its immutable signed endpoints instead of reconstructing them per tap; all
exact checks and the single final rounding remain unchanged. The full native
suite passes 419 tests (one heavy opt-in ignored), and all three new tests
execute on emulated ARM. Strict native/cross Clippy passes. A bounded GMP-192
benchmark improves from 2.520 to 1.258 seconds with the same checksum; see
[BigMac boundaries](big-mac-boundaries.md) for the oracles and measurement scope.

The missing ARM libsndfile runtime has now been provisioned in the isolated
validation sysroot from eight hash-checked pinned packages, without system
installation. All ten libsndfile tests execute on ARM with skips forbidden
(plus 30 root-library tests). A focused High 160/147 corpus passes twelve
WAVE/AIFF/FLAC conversions: output bytes, qualification blocks, coefficient
identities and complete reciprocal caches agree on x86-64 and emulated ARM.
Actual multi-block AIFF reads at blocks 1 and 4294967295 also retain the native
WAVE identity. The [codec read qualification](codec-read-qualification.md)
records manifests, failed pilots, exact scope and reproduction commands.
Production Rust is unchanged; this closes the preceding runtime-availability
gap but does not qualify physical ARM or production linking.

Both input adapters now grow output storage from decoded samples instead of
eagerly allocating the requested upper bound. WAVE retries interrupted byte
reads; other decoding/allocation errors poison the reader and cannot silently
resume at a lost sample boundary. libsndfile uses bounded native scratch,
fallible result growth and `sf_error` checks before treating a short read as EOF.
Ten new tests bring native release workspace/all-target coverage to 416 passing
tests (one heavy opt-in ignored). Eight new tests execute on emulated ARM;
two require missing ARM libsndfile and are explicitly not runtime-qualified
there. All native libsndfile tests pass with runtime availability required.
Native CLI output is identical at blocks 1 and 4294967295; the existing
10,000-case I/O corpus retains its checksum. See the
[streaming read contract](streaming-read-contract.md) for allocation/error
semantics, exact evidence, skip detection and remaining scope.

The original malformed-input/streaming requirement now has a bounded
`sex-fuzz --suite io` property suite: independent WAVE fixtures, exact PCM and
writer-byte oracles, header/payload mutations, truncation, short reads,
zero-frame/EOF checks and explicit driver budgets. `--case-seed` now actually
replays the reported failing case instead of advancing a newly seeded root
generator. A 52-byte fixture checks prefix-only decoding of a declared nearly
4 GB stream without allocating or writing that stream. The 10,000-case seed-42
batch matches on native x86-64 and emulated AArch64; three new I/O unit tests
pass on ARM. The full native suite now passes 406 tests (one heavy opt-in
ignored), with strict native/cross Clippy and formatting checks passing.
See [testing-tools.md](testing-tools.md) for identities and scope: full
multi-gigabyte traversal, other malformed containers and coverage-guided
instrumentation remain unqualified. Production DSP and I/O code are unchanged.

The original exact-rational user-rate requirement now reaches the CLI:
`SampleRate` parses decimal/scientific Hz, integer fractions and k/M suffixes
with exact integer reduction and checked cross-cancellation. `--rate` aliases
`-r`; equivalent integer-Hz spellings produce identical files and share caches.
Plan/analyze retain fractional frequencies (16537.5/44100 becomes exactly 3/8).
The current integer-Hz adapters explicitly refuse nonintegral output headers;
no playback rate is silently rounded. Nine new tests bring the suite to 400
passing tests (one heavy opt-in ignored), with five rate tests on emulated ARM,
strict native/cross Clippy, and byte-identical fractional analysis stdout/stderr
across CPUs using different exact spellings and independent caches.
The [rate contract and limits](exact-rational-rates.md) record the remaining
fractional-container boundary and the non-materializing exotic-rate plan.

The [Kaiser design enclosures](kaiser-design-enclosures.md) now propagate
directed MPFR rounding and the infinite I0 remainder through exact-rational
sinc geometry, signed summation and per-phase normalization. Comparing final
integer coefficients gives a joint design/quantization/DC-correction bound,
including an all-frequency phase-L1 error bound. Complete-bank certification
now recomputes actual coefficient identity, checks every phase, refines endpoint
precision against an optional exact joint-error target, and accounts I0 work
across all phases and retries. It also provides an exact input-peak-scaled
pre-rounding signal-error bound. All 23 enclosure tests pass on native and
emulated ARM. The complete workspace suite passes 372 tests (one existing
heavy test opt-in), strict native/cross Clippy passes, and 13 complete-bank
reports match across CPUs, including the 160-phase/289-tap Sane 160/147
candidate. Earlier 44 phase/width and 30 window-width reports also match.
CLI `--certify-design` now certifies the actual feedback-accepted Kaiser bank,
including cache hits, with the planner's coefficient allowance as its default
joint-error target. Conversion certifies before output publication; plan only
reports the request. Seven new tests bring the suite to 379 passing tests
(one heavy opt-in ignored), including unchanged native/GMP audio and the real
Absurd 4097→4309 tap refinement. Complete Fast/High/Absurd analyze reports
match on x86-64 and emulated ARM with independent caches; strict Clippy passes.
Successful certified Kaiser runs now replace the generic FIR allowance with
their joint design/quantization/DC-correction bound in the complete numerical
budget. The certificate target accounts for the peak immediately before FIR
and reserves exact outward-grid rounding costs; the final chain is rechecked
before execution. Ordinary runs retain the old MPFR-value reference, while
certified runs compare against the mathematical normalized Kaiser. Final PCM,
ideal-filter approximation and independently chosen normalization scales remain
excluded. Proved lower-bound violations now trigger automatic bank redesign;
inconclusive enclosures do not. Coefficient/automatic MPFR widths increase,
all response/numerical gates run again, and response/certificate budgets span
the entire search. A real Fast 2^-200 coefficient-target request upgrades
C=62/MPFR=128 to C=213/MPFR=288 without weakening shape or targets. Full stdout
and stderr match x86-64/emulated ARM; 391 workspace tests pass (one heavy
opt-in ignored), with new exact-budget/one-term-short and non-redesign checks.
Persistent certificates and non-Kaiser enclosures remain unfinished.
The updated workspace suite passes 387 tests (one heavy opt-in ignored),
fourteen numerical tests pass on emulated ARM, and strict native/cross Clippy
passes. Complete plain/amplified/restored Fast chain reports match across CPUs;
the amplified chain automatically refines its enclosure to 128 bits, while
the others need only 32. The [numerical budget evidence](numerical-error-budget.md)
records exact scope, formulas, logs and hashes.

The [spectral harness now snapshots its binaries and source](spectral-snapshots.md)
before long runs and checks the accepted FIR against fixture guards before
rendering. A real Absurd feedback case demonstrates early rejection and safe
recovery with longer guards. Both recovered outputs and the existing Fast
control pass all four engines and exact repeats. The Until-40k 2/1 full-sweep
run has now completed with 34.25-second guards and 68197 taps per phase at the
unchanged -600 dB target. Both 139014-frame PCM32 renders match bit-for-bit;
all four engines pass the full core plus ten decile checks (44 total).
SeX's whole-core waveform SNR is 111.250970577 dB, the weakest decile
101.925853872 dB; these finite PCM sweep metrics are separate from the bank's
sampled -617.025532066781 dB stopband. Snapshot/reference-entry checks pass
and the 26 MiB artifact set is retained. This is one mono 2/1 case, not general
Until-40k qualification. Its earlier short-guard attempt was
explicitly stopped for a verified guard mismatch, not restarted on a timeout.

The [preset-aware reference extension](spectral-preset-campaign.md) now passes
the complete five-ratio sweep matrix for Fast and High against all three
external references: 40 engine/preset/rate combinations, 440 interval checks
and ten bit-identical SeX repeats. Guard lengths follow the initial FIR plan
and are then checked against the accepted bank, so High's 26593-tap 1/48 bank
gets half-second guards. Fast's common comparison band ends at 79% of lower
Nyquist; High shares Sane's 89% comparison edge, not its full 95% passband.
The mandatory bank gates still assess each preset's own target and bands.
349 workspace tests pass (one existing heavy test opt-in), all 18 probe tests
pass on emulated ARM, and strict native/cross Clippy passes. Sane fixtures and
measurement tables remain byte-identical to the preceding checkpoint.
Absurd and Pointless additionally pass the full four-engine sweep comparison
at 1/48, with bit-identical block-size repeats. Their first FIR candidates miss
the unchanged -240/-360 dB targets; feedback accepts 103391 and 203313 taps,
respectively, inside their 1.25/2.25-second guards. Across these four presets
the checkpoint covers 48 engine/preset/rate combinations and 528 intervals.
Until-40k has guard-planning tests here, not a new full-sweep execution claim.

The [guarded sweep extension](padded-sweep-campaign.md) now measures every
output sample across a complete 1 Hz to 89%-of-lower-Nyquist sweep, plus ten
separate subintervals. All 40 engine/rate/suite combinations pass on the five
existing ratios, including 220 new interval checks and ten whole-file SeX
chunk-size repeats. The earlier multichannel gates are unchanged. A deliberately
muted upper decile fails the new gate, and a byte-preserving external-context
control leaves SeX's complete 1/48 core unchanged. The workspace suite passes
347 tests (one existing heavy opt-in test ignored), strict Clippy passes,
and all 16 probe tests pass on emulated AArch64. This still measures complete
configured engines and finite chirps, not an intrinsic arithmetic noise floor.

The [spectral reference campaign](spectral-reference-campaign.md#multi-input-phase-extension-2026-09-06)
now additionally checks 35 input-impulse positions per engine across the same
five ratios: all three phases at 1/3, and eight positions each at 160/147,
147/160, 479/441 and 1/48. All 140 impulse responses and the original signal
gates passed, along with five whole-file SeX chunk-size repeats. A deliberately
two-frame-delayed, duration-preserving output failed the impulse timing gate.
At that preceding checkpoint, the native workspace suite passed 344 tests
(one existing heavy test opt-in), strict Clippy passed, and all 13 then-existing
probe tests passed on emulated AArch64.
Audio arithmetic and the SeX executable are unchanged by this validation-only
extension. This is bounded phase coverage, not a continuous all-image proof.

1. Extend the proved computational bound to certified MPFR design-error
   enclosures and stronger filter-approximation qualifications. Keep final
   PCM/dither measurements separately from the numerical signal-path target
   (the new full-band PCM error meter does not certify spectral noise).
   Tighten conservative FIR L1/peak estimates where actual
   designed coefficients permit it.
2. Broaden preset/rate qualification beyond the now-shared bounded Kaiser,
   window, global-LS and Remez CLI feedback. Add practical long-FIR and
   continuous complete-image proofs, stronger numerical recovery evidence and
   scalable very-long global solvers. Preserve the established exact targets,
   coefficient identities and cache contracts while extending coverage.
3. Extend the five-ratio Sane/Fast/High and single-ratio Absurd/Pointless
   reference evidence to wider preset-specific upper bands and heavier modes,
   wider guarded-sweep coverage and comprehensive
   continuous periodically time-varying anti-imaging qualifications beyond the
   new all-component grid analyzer. Extend the emulated AArch64
   evidence to physical hardware, production linking, and additional ABIs.
4. Extend large-stream, malformed-input, and coverage-guided fuzz testing
   without generating gratuitous hundred-gigabyte fixtures. Qualify any SIMD
   optimization against the same exact algorithm and an actual speed gate.

The public Rust facade, rate/analyze frontends and offline local installation
are now verified above. Keep their mathematical/API documentation aligned with
future execution changes; external publication has not been performed.

The mathematical contract is in [fixed-point-model.md](fixed-point-model.md),
stage semantics in [dsp-pipeline.md](dsp-pipeline.md), and upstream reuse/rewrite
decisions in [sox-reference-audit.md](sox-reference-audit.md).
