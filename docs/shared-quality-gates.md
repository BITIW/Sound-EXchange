# One qualification configuration for analysis and conversion

`--grid`, `--certify`, `--harmonics`, and their resource controls now apply to
conversion, `analyze`, and `plan`. They have one parser, one stored gate
configuration, and one materialization/refinement path. This closes the gap
where stricter analysis could select a bank that the CLI could not actually
use for conversion with the same checks.

```text
sex analyze input.wav -r 48000 --preset sane --grid 129 --certify --harmonics --harmonic-work 500000000
sex input.wav -r 48000 output.wav --preset sane --grid 129 --certify --harmonics --harmonic-work 500000000
sex plan input.wav -r 48000 --preset sane --grid 129 --certify --harmonics --harmonic-work 500000000
```

The higher harmonic work cap admits this denser grid for a typical 44.1→48 kHz
bank. Other ratios can require different limits; use `plan` before a costly run.
The same rate, preset/error-floor, precision, effect chain, clipping policy,
window, and gate settings select the same coefficient bank. `--window` is
shared by all three modes; non-Kaiser arbitrary-width banks use the same
quality gates and include their designer provenance in the qualification
block. See [windowed-cli.md](windowed-cli.md). An input-dependent
normalization scale and final PCM encoding are not part of the FIR response
certificate. Channel layout must still match the requested mixing matrix.
Changing transport block size does not change the qualification or audio.

| Mode | Bank and checks | Result |
| --- | --- | --- |
| `plan` | No bank, no response evaluation, no proof | Initial plan, work estimates and static-limit diagnostics |
| `analyze` | Design/cache, numerical replan, requested qualification/refinement | Final accepted bank and measurements/proof |
| conversion | The same qualification, then execution of that exact accepted bank | PCM plus its bank-bound qualification report |

## Shared controls

`--grid N` requires an integer `N >= 2` and defaults to 65. It controls the
ordinary per-phase grid and the harmonic input-band grid. Duplicate grid
options now fail instead of silently taking the last value. Certificate and
harmonic controls retain their existing bounds, and their limit flags require
`--certify` or `--harmonics`, respectively. Flags may appear before or after
their associated limit options. Invalid and duplicate settings fail identically
in all three modes.

`--refine-quality` remains a redundant compatibility alias: qualification is
mandatory even without supplemental gates. `--refinement-*` limits remain
cumulative across candidates. Certificate and harmonic limits apply to each
candidate; they are additional to the ordinary response/materialization work
budgets. Exhausting any requested check does not authorize a weaker fallback.

For example, the corrected High 1/2 bank exceeds the default certificate tap
guard and needs an explicit increase:

```text
sex input-44k1.wav -r 22050 output.wav --preset high --grid 17 --certify --certificate-taps 2049 --harmonics
```

The proof and image checks measure different things. `--certify` proves
continuous **per-phase amplitude** bounds. `--harmonics` measures all wanted
and image components on the selected input-frequency grid, but does not prove
continuous all-image bounds. Either check can cause a failing candidate to be
strengthened and checked again; an inconclusive certificate stops immediately.
The [feedback contract](quality-refinement.md),
[continuous certificate](continuous-response-certificate.md), and
[harmonic model](harmonic-response-analysis.md) define their mathematics.

## What connects a proof to the written audio?

The CLI passes the common gate configuration into `prepare_design`. The
refiner returns the accepted bank, final plan, and required assessment together.
The converter constructs its signal pipeline from that final numerical plan
and builds its resampling engine from that same owned coefficient bank. It
does not perform a separate design after analyzing a filter.

The refiner rejects assessments made at a different target/grid or missing
phases. The final report additionally checks that every requested supplemental
assessment is present and all required verdicts passed. It includes:

- `qualified FIR sha256`: the actual coefficient-bank identity;
- target, grid size, actual coefficient fractional width, and the exact
  coefficient-L1 quantization bound and its budget;
- sampled phase ripple, deviation, stopband, band endpoints, and compliance;
- selected supplemental resource limits and their complete result reports;
- an explicit final `FIR qualification scope` line.

This contiguous report block is identical in analysis stdout and successful
conversion stderr for the same bank/configuration. Stream-specific information
such as extended signal accumulator width, normalization, PCM clipping, and
codec identity remains in the conversion report outside that block. The
qualification binds the FIR, not the entire encoded recording's noise floor.
Its scope excludes signal rounding, effects, MPFR design-calculation error,
and final PCM/dither/clipping.

## Failures, cache hits, and same-rate requests

Initial tap/phase/storage/harmonic-work guards run before cache access or
coefficient design in both analysis and conversion. The refiner repeats its
per-candidate guards after changes. A warm cache contains coefficient banks,
not permission to skip the requested proof. Inconclusive and failed attempts
retain diagnostics but cannot publish a successful final qualification block.

Conversion uses the existing output transaction: failure of qualification or
later PCM processing preserves the previous destination and removes its own
staging file. A filter may have passed before final PCM clipping fails; in
that case no audio is published and no successful conversion qualification
block is printed. The earlier attempt trace remains diagnostic evidence only.

Same-rate conversion bypasses the resampling FIR. Requests for `--certify` or
`--harmonics` therefore fail explicitly unless the rate changes; they are not
silently ignored or used to certify gain/convolution/other stages. Plain
same-rate copying/effects still work, even under tiny FIR limits. `plan` explains
this no-FIR condition. Unity-rate `analyze` remains available for designer work
but labels its FIR hypothetical: a same-rate conversion does not execute it.

## Planning is still cheap

`plan` now uses the selected grid in `2 * grid * L * T` ordinary-response work
instead of assuming 65. It prints selected gates and static supplemental
preflight results. Harmonic work can be estimated without coefficient design;
certificate success/work consumption cannot. A static-limit warning leaves
planning successful and the candidate explicitly unqualified.

An extreme Until-40k bank may need increases in ordinary refinement budgets
as well as supplemental harmonic budgets. Raising one does not disable the
others. Continuous proof for very long FIRs remains resource-limited; selecting
a preset or accepting a plan is not a certificate.

## Regression coverage

Three complete audio cases compare identical analysis/conversion qualification
blocks and PCM at block sizes 1/7/4096:

- Sane 3/2 with a 9-point grid, continuous proof, and all-image measurements;
- High 1/2 with a 17-point grid, bigint coefficients, and an increased tap guard;
- Fast 1/3 with gain `2^60`, normalization, and a coupled numerical plan that
  raises coefficient precision beyond native Q2.62 before proof/execution.

Other tests cover order-independent shared parsing, invalid settings in every
mode, a wrong-grid/partial-bank assessment, selected-grid planning without
cache writes, warm-cache proof/response-budget failure, explicit same-rate
rejection, and a successful FIR proof followed by a final PCM clipping failure.

The cross-architecture harness now compares these bank-bound report blocks for
every resampling case. `SEX_REPRO_SHARED_GATES=1` adds the three configurations
above, each rendered four ways and analyzed on both architectures.
`SEX_REPRO_SHARED_UNTIL=1` additionally starts from an Until-40k 2/1 candidate
with 66867 taps per phase, C=512, MPFR=1024, a 9-point grid, and harmonic checks.
That initial candidate fails the unchanged -600 dB **per-phase** gate
(deviation `1.042622074500858e-30`, stop peak `-596.2401652407976 dB`). The loop
strengthens it to 68197 taps per phase and design attenuation 624 dB, retaining
C=512 and the target. The earlier harmonic-only qualification at this ratio
does not contradict this: individual phase and combined harmonic responses
are different quantities, and both gates are now required.
Tiny fixtures keep the workload in filter arithmetic instead of disk space.
Recorded build/result identities belong in [cross-architecture.md](cross-architecture.md).

The full run passed 18 cases / 72 renders, plus eight analyses on the two
architectures. Every bank-bound report, output file, and independent cache
agrees. All fourteen preceding output/cache identities are preserved. The
accepted Until-40k bank's sampled phase stop peak is -617.025532 dB and its
strongest image/image-L2 peak is -618.121795 dB; the 17-frequency grid does not
establish continuous bounds. All 287 native tests and both Clippy checks pass,
as do thirteen refiner/planner tests on emulated AArch64. The warm-cache
inconclusive conversion also preserves output and matches across architectures.
