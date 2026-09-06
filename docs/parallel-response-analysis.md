# Deterministic phase-parallel response analysis

Sampled response qualification is separable by polyphase bank phase. For each
phase and each requested passband/stopband frequency, SeX still evaluates the
same quantized coefficients with the same MPFR precision and recurrence. It
then merges per-phase extrema in ascending phase-range order. Parallelism does
not change the grid, filter, target, coefficient-error gate, or arithmetic in
one frequency response.

Bigint-bank response analysis uses
`min(available_parallelism, phase_count)` standard-library scoped workers.
One-phase banks retain the sequential path. Each worker owns its temporary
MPFR coefficient vector and response values; the immutable integer bank and
precomputed exact-same grid frequencies are shared. The result contains only
minima/maxima and Boolean comparisons. There is no cross-phase floating-point
sum whose reduction order could depend on scheduling.

The regression `bigint_response_parallelism_is_bit_identical_and_keeps_all_phases`
compares one, two, three, and an over-requested 32 workers, including complete
response strings, compliance decisions, phase count, and invalid-grid failure.
All 140 `sexfir` library tests pass. The complete workspace currently passes
468 release tests, with the existing heavy opt-in test ignored; root/Q/rate
library runs pass 134 tests on each little- and big-endian ARM profile. Strict
Clippy passes on all three build profiles.

An end-to-end 160/147 High-preset benchmark compares the preceding pinned
sequential executable to the new executable using the same cached bank and
one-frame input. Both produce byte-identical WAV and complete FIR qualification
blocks. On the current 32-logical-CPU host, sampled response wall time decreases
from 2.10 s to 0.16 s; user CPU increases from 2.08 s to 2.95 s. These timings
are one observed workload, not a portable speed guarantee.

The actual Until-40k 160/147 run then loads two existing, whole-payload-verified
banks and repeats both mandatory grids. It consumes 1818.37 user seconds in
73.60 wall seconds (2490% reported CPU), with 2,023,400 KiB peak RSS. Candidate
1 remains rejected at −596.2401652407976 dB; candidate 2 is accepted at
−617.0151829745743 dB. The accepted coefficient identity is
`5016336267899ace1c74d98b76e781a2ecd405e132330ee9e63e7354f30de86d`.
This is an actual sampled-grid qualification. It remains explicitly distinct
from a continuous-frequency certificate, all-image proof, MPFR design-error
bound, or final PCM noise floor.

Evidence is retained in `target/parallel-response-qualified` and
`target/until-budget-qualified/until-example-parallel.log`. The interrupted
sequential run is retained separately: it atomically produced both candidate
caches, but no output, and is not counted as a successful execution.

A second conversion at block size 1 and an explicit `sex analyze` run each
repeat both candidate decisions and the accepted qualification exactly. Both
cache files remain unchanged. The current-source native/big-endian corpus then
reproduces all 56 WAVs, 56 complete reports, six analyses, twelve caches and
fourteen identities byte for byte against the preceding baseline.
