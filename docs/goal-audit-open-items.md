# Original-goal audit: Until-40k item resolved

This is a requirement audit, not a completion claim or a reduced
replacement for [the original goal](project-goal.md). Additional proof work,
publication and physical-platform coverage must not be confused with explicit
functional gaps in the user's requested commands.

Use the [25-item evidence map](requirement-evidence-map.md) to audit the complete
original scope; it deliberately does not mark requirements complete merely
because a component or historical test exists.

The separate original `--rate 48000 --error-floor -300dB` example has now been
executed with all three rate syntaxes, full polyphase-period stereo input,
chunk variation, automatic C62→C78 refinement, matching analysis and a
big-endian foreign-cache replay. A permanent regression preserves this path;
see [the scoped evidence](original-cli-command-qualification.md). This does
not resolve the different, much larger Until-40k execution tracked below.

## Until-40k common-ratio defaults: baseline failure and qualification work

The original request explicitly demonstrates Until-40k at 48000 Hz without
requiring separate work-budget overrides. With a one-frame, 44100 Hz PCM input,
the corresponding pre-fix command was:

```sh
SEX_COEFFICIENT_CACHE=off target/release/sex \
  target/native-admission-corpus-qualified/inputs/one16.wav \
  target/native-wide-proof-qualified/until-example.wav rate 48000 \
  --preset until-40k --precision auto --error-floor -400dB
```

It exited 1 before assessing any candidate:

```text
cumulative coefficient limit: requires 10698720, limit 4000000
```

No output file is published. The matching cheap `plan` command confirms:

| Initial candidate | Required | Default cumulative limit |
| --- | ---: | ---: |
| Coefficients: 160 phases × 66867 taps | 10,698,720 | 4,000,000 |
| Response terms at 65 points/band | 1,390,833,600 | 200,000,000 |
| MPFR bits | 1024 | 16384 |

Static designer and supplemental preflight pass. The failure is the generic
quality-feedback quota, not unsupported ratio parsing, fixed-point arithmetic,
missing GMP or an observed spectral failure. Until-40k retains its preset's
600 dB minimum target when the explicit 400 dB request is weaker; that policy
is intentional, not the issue here.

`sexfir::refinement::Limits::default()` defines the two generic quotas; the
pre-fix CLI used those defaults unless the user supplied overrides. The plan reported
the insufficiency accurately. Those diagnostics and safe publication behavior
are good, but they do not make the illustrated heavy command executable with
its ordinary preset options.

### Policy and execution qualification completed

The CLI now implements [geometry-aware Until-40k work budgets](until-work-budgets.md).
At this ratio, automatic cumulative limits become 42,794,880 coefficients and
5,563,334,400 response terms. Explicit overrides remain hard per-field limits;
generic library limits and other presets are unchanged. Real-refiner preflight
unit tests reach a non-allocating materialization hook with the defaults and
reject explicit low limits before that hook. This closes the identified quota
calculation problem.

The actual command completed using the same one-frame input, no quality-budget
overrides and an isolated cache. The first 66867-tap/phase candidate fails the
unchanged −600 dB grid at −596.2401652407976 dB. Feedback retains C=512 and
MPFR=1024 while increasing length to 68197 taps/phase. Candidate 2 passes every
mandatory sampled gate at −617.0151829745743 dB, and the command writes one
48000 Hz PCM frame with zero clips. Its qualified coefficient identity is
`5016336267899ace1c74d98b76e781a2ecd405e132330ee9e63e7354f30de86d`.

The accepted plan uses Q65.512 signal samples, a 1537-bit planned accumulator
(1092-bit actual minimum for the bank), and reports numerical error no greater
than 2^-495 FS against budget 2^-102 FS. Cumulative charged work is 21,610,240
coefficients and 2,809,331,200 response terms, within automatic limits. The
sampled response excludes continuous-frequency proof, MPFR design error and
final PCM; those scopes are not silently promoted.

A different-block warm-cache repeat (1 versus 4096 frames/block) reproduces the
complete qualification block and 46-byte WAV exactly; both WAV SHA-256 values
are `cfc506654f7c71534dc866523ce027311de0c112924f30617782f1cba52b9d49`.
Both complete cache files pass their recorded SHA-256 checks unchanged after
the repeat. Evidence is retained in `target/until-budget-qualified`, including
`until-example-parallel.log`, `until-example-repeat.log`, qualification files,
`output-identities.sha256` and `cache-repeat-check.log`.

### Retained intermediate checkpoint (not response qualification by itself)

The live run completed and atomically stored a first kind-6 bank at
2026-09-06 19:23:09 UTC. A separate bounded-memory, read-only cache inspection
checked the exact serialized length and SHA-256 trailer over the complete
payload: 10,698,720 coefficients, 65 bytes each, total file size 695,417,100
bytes. The stored coefficient identity is
`95b191a795f03e03e5f49c289f34199e61ea7297b36aa4b36a4d6aa3d9be561d`;
the verified payload digest is
`53ab31905c5be5dba43debb7f9e1c3bcad8c4179766f3ce1a91c2ab3bedc7a69`.
Evidence: `target/until-budget-qualified/initial-cache-inspection.json` and its
read-only inspection script. This independently establishes a completed cache
payload, **not** accepted passband/stopband performance. Failed-response banks
are cacheable too; at that intermediate checkpoint no output WAV or completed
search had been observed. The completed result above supersedes that state
without changing what the cache inspection alone proves.

### Resolution

The common-ratio functional item is resolved without a manual quota, skipped
gate, narrowed filter or weakened target. Phase-parallel evaluation preserves
the same per-frequency MPFR calculation and deterministic extrema; its separate
[proof and benchmark](parallel-response-analysis.md) record exact-output checks
and cost. The full 25-item completion audit remains separate.

The baseline failure remains in `target/native-wide-proof-qualified`; it is
deliberately retained as pre-fix evidence and is not confused with the successful
new execution.
