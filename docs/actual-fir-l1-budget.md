# Numerical budgets from the actual quantized FIR

The non-materializing planner uses `2 * taps_per_phase` as a safe L1 bound for
a Q2.C FIR phase: every signed coefficient has magnitude at most 2. This is a
valid pre-design bound, but retaining it after design can greatly overestimate
the amplification of upstream rounding and effect-coefficient errors.

After the shared response/quantization gates accept a bank, conversion and
analysis now compute

```text
A_raw = max_phase(sum_tap(abs(coefficient_raw[phase, tap])))
A     = A_raw / 2^C
```

All phases and coefficients participate. The sum and maximum are GMP integers,
including for native Q2.62 coefficients; no report string or floating-point
response measurement is used as the gain bound. Cache hits are measured from
the loaded, validated coefficients too.

## Propagation and reference

Let P bound the ideal input peak, E the already-propagated input error, D bound
the FIR coefficient L1 error against the chosen reference, and epsilon bound
the one final FIR rounding. Then:

```text
ideal output peak <= (A + D) * P
output error     <= A * E + D * P + epsilon
```

The term A*E uses the actual quantized operator, so it includes the action of
coefficient perturbation on existing error without another omitted cross term.
Rounding/effect/FIR error components remain separate, with outward integer
rounding on the existing bound grid. This change tightens A only; it does not
reduce D or silently change its reference.

Without a design certificate, D remains the generic `(T-1) * 2^-C` allowance
relative to the exact-DC-adjusted MPFR reference, excluding MPFR design error.
With a bank-bound design certificate, D is its proved joint L1 error relative
to the mathematical FIR. Certificate merging retains the actual A rather than
reverting to `2*T`. The existing final PCM and ideal-filter-approximation
exclusions still apply.

`sexplan::numerical::FirL1Bound` is constructed from a validated native or bigint
Q2.C bank. `evaluate_numerical_budget_with_fir_l1` accepts that optional bound
only for a chain containing exactly one FIR with matching T and C. Its public
caller must bind the object to the bank it actually executes; dimensional
checks are not cryptographic bank authentication. The CLI obtains it directly
from the accepted `PlannedDesign` and shares it across analysis/conversion.

The CLI checks that tightening cannot worsen the generic peak/error bounds or
violate the requested target. It prints the exact A_raw and C alongside the
numerical report. Cheap `plan` still uses a generic, unmaterialized bound.

## Precision and compatibility scope

This checkpoint does **not** reduce previously selected signal/coefficient
widths or accumulator plans. It refines the post-design proof without changing
sample arithmetic. Generic pre-design precision selection remains conservative;
using the stronger post-design proof to reduce execution widths is future work,
not a performance improvement claimed here. The tighter propagated non-FIR
error also reaches the joint design-certificate allowance, so an explicitly
certified automatic search can have less unnecessary refinement; its target,
work limits and certificate checks are not weakened.

The ordinary non-certified control uses Fast 3/2, gain 1/3, normalization and
129 taps/phase at C=62. Exact maximum phase L1 is
`11380094025431245190 * 2^-62` (roughly 2.47), versus the old 258 bound.
Cheap plan versus actual-bank analysis, at unchanged Q65.63:

| Bound | Generic plan | Actual bank |
| --- | --- | --- |
| Ideal peak after FIR | <= 2^7 FS | <= 2^0 FS |
| Propagated rounding | <= 2^-55 FS | <= 2^-61 FS |
| Effect-coefficient error | <= 2^-55 FS | <= 2^-62 FS |
| FIR-coefficient error | <= 2^-56 FS | <= 2^-56 FS |
| Combined numerical error | <= 2^-54 FS | <= 2^-56 FS |

These are conservative bounds, not measured peaks or PCM error values. The
requested budget and selected 192-bit execution accumulator are unchanged.

## Regression scope

Exact two-phase fixtures check maximum versus first/average phase, signed
coefficient cancellation, Q2.62 and bigint C62/96/4096, wrong T/C, absent or
ambiguous FIR stages, outward rounding, and unchanged coefficient-error
allowances for both generic and certified references. A real native design
checks unchanged precision and tighter bounds before and after certificate
merging. The existing five-case CLI analysis/conversion/cache regression now
also requires identical actual-L1 report lines across all three paths.

All 442 native release workspace/all-target tests pass (one existing heavy test
remains opt-in). All three new regressions execute on emulated AArch64. Strict
native/cross Clippy and formatting pass. Four native/ARM cases (native Kaiser,
High bigint Kaiser, Hann and global-LS) have byte-identical complete analysis
reports and rendered WAVs at blocks 1 versus 7. Analysis and conversion numerical
bound/L1 lines also match per case. Logs, generic-versus-actual comparison and
artifacts are under `target/actual-fir-l1-qualified`. This is the established
QEMU-only Cortex-A53 environment, not physical ARM or production ABI evidence.

Two additional High 160/147, PCM16, noise-shaped-5/seed-42 normalized renders
preserve the historical complete WAV identity
`41c4405a6c522f3f483c1908078b5019042cbaf558d7d5092be0b8164a82ba53`
on native and ARM. Their tightened numerical/L1 report lines agree too. This
checks ordinary non-certified output compatibility; it is not a claim that every
previously over-conservative certified automatic search chooses the same bank.
