# Bounded recovery from numerical designer failure

Optimized CLI and `refinement::qualify` now distinguish a numerical solver
failure from coefficient quantization or spectral failure. With automatic
working precision, a recoverable failure proposes `P_next=2*P` and retries
through the same whole-chain preparation and resource preflight.

The retry itself changes only MPFR working P. It does not change C, FIR length,
ratio, target, rolloff, weights, grid policy, iteration cap, rounding or DC
correction. Ordinary coefficient/shape feedback still applies after a bank is
successfully materialized and assessed. Higher P does not imply quality success:
every accepted bank must pass the actual requested coefficient and spectral gates.
MPFR remains confined to design/measurement, not the signal path.

## Typed recovery policy

Only optimized global LS/Remez materialization can trigger this policy. The
recognized numerical causes are:

- `SingularLeastSquaresSystem`;
- `SingularRemezSystem`;
- `RemezExtremaUnavailable`;
- `RemezDidNotConverge`.

The classifier follows typed error causes, including the optimized/cache
wrappers. It does not parse message text. In particular, corruption, I/O,
invalid configuration, out-of-range coefficients, selector allocation limits
and all resource failures remain terminal. A missing-extrema or nonconvergence
failure might not be caused by insufficient precision; bounded retry is an
attempt at recovery, not a diagnosis or promise of convergence.

An explicit numeric `--precision` is pinned and never silently increased.
Automatic doubling must fit both the refinement and designer precision caps
and leave another slot in the shared candidate limit. Otherwise the original
numerical error is returned with the reason retry stopped. The next candidate
must also fit storage, grid, coefficient, ordinary response, cumulative solver
work, and any supplemental gate limits. No limit is raised automatically.

All materialization attempts, including failed solves, consume a candidate
slot and full logical preflight charges. Coefficient/ordinary-response terms
are conservatively reserved even when failure prevents those operations.
Cache hits have identical charges. Failed numerical designs are not cached;
a repeated warm invocation therefore retries the failed lower-P solve before
loading a successful higher-P bank. It does not leap directly to an unrelated
cache entry or silently trust an earlier proof.

`Qualified` and `RefinementError` expose `solver_failures` separately from
assessed `attempts`. Each failure retains its plan, typed cause and proposed
next P (or none). A proposed next P does not assert that its materialization
occurred: subsequent preflight can still fail. CLI diagnostics print these
records and preserve the accepted bank's existing qualification block.

## Real recovery regression

Global LS at L/M=2/3, T=129, r=3/5, weights 1/10, C=64, and the plan-derived
258-point-per-band grid fails at P=128 with
`SingularLeastSquaresSystem { pivot: 110 }`. Keeping those settings fixed and
raising only P to 256 succeeds. The corrected bank passes the unchanged
-80 dB coefficient, sampled phase, continuous per-phase amplitude and sampled
complete-image gates. Its raw coefficients agree with an independent P=512
design. This precision comparison is not a rigorous MPFR error enclosure.

The regression runs cold and warm cache paths, checks identical final banks
and charges, and confirms that only the successful bank is stored. Separate
tests use the same real failure to enforce pinned P, both precision ceilings,
the candidate cap and cumulative designer work. Typed classification tests
cover all four numerical causes and non-retryable lookalikes.

All three tests pass on x86-64 and AArch64 QEMU with a shared locked coefficient
identity. The complete native suite passes 330 tests (one existing heavy test
ignored), and both platforms pass warnings-denied Clippy. Six warm PCM renders
and six analyses retain the preceding CLI identities. Exact hashes and scope
are recorded in [cross-architecture.md](cross-architecture.md).

This is actual LS recovery evidence. It is not evidence that every Remez
failure, preset, rate ratio or very-long global solve can recover. Scalable
solvers, certified design-error enclosures and broader numerical qualification
remain open.
