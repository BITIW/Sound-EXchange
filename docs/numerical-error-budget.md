# Whole-chain numerical error budget

Status: normative for `sexplan::numerical` and coupled CLI precision planning.
All bounds below are absolute amplitudes, where integer PCM full scale is 1.
They apply before final PCM quantization, dither, or clipping.
Conversions report [measured final PCM error](final-pcm-error.md) separately:
exact full-file squared-error/peak aggregates relative to the immediate pre-PCM
signal, with approximate dB display. It is not included in or certified by the
pre-PCM bound below and is not a spectral noise floor.
After design, the CLI now uses the [actual quantized FIR phase L1](actual-fir-l1-budget.md)
for propagation of earlier errors and peak bounds; cheap `plan` retains the
generic `2*taps` pre-design estimate. This does not change coefficient-error
allowances or the mathematical reference of an uncertified/certified run.

## What is being compared

The reference uses the same input PCM and exact rational time coordinates,
but computes the requested rational effects without rounding. By default its FIR uses the
designer's finite-precision MPFR coefficient values with one coefficient per
phase adjusted to make the reference DC sum **exactly** one. It does not round
those reference coefficients onto the execution Q grid. Normalization uses the
same exact rational scale selected by the actual two-pass execution.

This defines a reproducible computational reference, not an ideal brick-wall
filter or a noiseless analogue recording. The bound excludes:

- approximation of the desired continuous-frequency filter by a finite FIR;
- errors introduced while calculating the MPFR design itself (unless the
  accepted Kaiser bank is certified with `--certify-design`, as below);
- the difference between normalization scales chosen independently for actual
  and hypothetical reference signals;
- final PCM rounding, dither/noise shaping, and output clipping.

The separate MPFR response analyzer measures the quantized FIR. Its sampled
passband/stopband qualification remains necessary; the numerical budget does
not turn that grid into a continuous-band proof.

## Bound arithmetic

The input is exact with `P=1` and `E=0`. `P` bounds the peak of the reference
signal and `E` bounds its difference from execution. Three nonnegative error
components are retained separately: stage rounding, rational effect coefficient
quantization, and FIR coefficient quantization. Their sum is `E`.

The implementation stores bounds as nonnegative GMP integers at binary point
`W = max(signal F, effect C, FIR C) + 64`. Every non-exact bound operation rounds
**up**, including positive rational division and multiplication/rescaling.
Consequently bound-grid rounding can make a plan more conservative, never less.
No floating-point values, logarithms, or least-common-denominator accumulation
are involved. Bound exponent reporting uses integer significant-bit counts.

For a requested rational `n/d`, quantization computes exactly
`q = round_even(n * 2^C / d)`. Its error is enclosed directly from
`abs(q*d - n*2^C) / (d*2^C)`, not assumed equal to a half LSB. Dyadic
coefficients can therefore have exactly zero coefficient error.

## Linear stages

For gain, a matrix row, or convolution taps, let:

```text
L  = sum |requested coefficient|
Lq = sum |quantized coefficient|
D  = sum |quantized - requested coefficient|
epsilon = 2^(-F-1)                  nearest rounding, once after the MAC

P_next <= L * P
E_next <= Lq * E + D * P + epsilon
```

For multiple output rows, the planner uses the maximum row sum of each norm.
Histories and convolution tails obey the same bounds. The `D*P` term is assigned
to effect-coefficient error; `epsilon` to rounding; existing components are
each propagated with `Lq`. A later amplifier therefore amplifies earlier
errors, while a preceding amplifier increases the peak multiplying the local
coefficient error. Attenuation before a stage reduces its incoming reference
peak; the planner does not simply count the presence of a gain option.

## Recursive DC blocker

For `y[n] = x[n] - x[n-1] + R*y[n-1]`, both exact `R` and quantized `Rq` must
lie in `[0,1)`. With zero initial state, this filter's absolute impulse-response
sum is 2. Let `D = |Rq-R|` and `gap = 1-Rq`:

```text
P_next <= 2 * P
E_next <= 2 * E + (2 * P * D + epsilon) / gap
```

Input error passes through the DC blocker's transfer function, giving `2*E`.
Coefficient error and each output rounding enter as recursive forcing, giving
the geometric factor `1/gap`. That factor is divided exactly using the integer
quantized pole gap, with an upward-rounded result. It is not replaced by a
floating-point estimate of `1/(1-R)`. The bound holds for arbitrary file length.

## Designed polyphase FIR

For each phase with `T` Q2.C coefficients, `Lq <= 2*T`. If coefficient `j` is
the one selected for exact DC correction, define the reference values by:

```text
h_reference[k] = MPFR_value[k]                 for k != j
h_reference[j] = 1 - sum_(k != j) h_reference[k]
```

Every non-corrected coefficient has nearest-rounding error at most `2^(-C-1)`.
The corrected coefficient's error is minus the sum of all those errors. Hence:

```text
D <= (T-1) * 2^-C
L_reference <= Lq + D
P_next <= (Lq+D) * P
E_next <= Lq * E + D * P + epsilon
```

The `D*P` term is assigned to FIR-coefficient error. The maximum bound applies
to every phase without allocating or evaluating the bank during planning.
This conservative analytic bound is distinct from the analyzer's MPFR-measured
coefficient-error sum against the original, not exact-DC-adjusted, MPFR values.
It remains valid after exact phase scheduling, pre-roll zeros, and finite tails.

## Certified mathematical Kaiser reference

With `--certify-design`, the actual accepted bank receives a
[complete-bank enclosure certificate](kaiser-design-enclosures.md). Its joint
maximum phase-L1 coefficient bound `Dcert` compares the final integer bank
against the mathematical, unit-DC-normalized Kaiser FIR. It includes MPFR
design arithmetic, quantization and DC correction together. In
`NumericalStage::CertifiedFir`, this **replaces**, rather than adds to,
`(T-1)*2^-C`. No coefficient error is counted twice. `Dcert` is supplied as
arbitrary-width nonnegative integer numerator and positive denominator, with
explicit width checks, then rounded upward onto the existing bound grid.

The same propagation inequalities apply:

```text
P_next <= (2*T + Dcert) * P
E_next <= 2*T * E + Dcert * P + epsilon.
```

They follow from `q*x_actual - a*x_reference =
q*(x_actual-x_reference) + (q-a)*x_reference`. Thus prior effects/rounding errors
are multiplied by the quantized bank norm, while the joint coefficient error
multiplies the reference input peak, not the full-scale source or an assumed
post-normalization peak. The certified budget still keeps three components,
but its FIR component now includes design error. Generic library callers must
prove the supplied bound and bind it to their actual Q2.C bank; the CLI gets
it only from the recomputed all-phase certificate and verifies the full FIR
specification against its execution plan.

To select the enclosure target, the CLI first evaluates the existing budget
and removes its generic FIR error component from consideration. Let `S=2^W`,
`Praw` be the reference peak just before FIR on that grid, and `Araw` be the
total target minus all propagated rounding/effect-coefficient error, including
the later same-scale normalization rounding. Reserve
`Rraw = ceil(Praw/S) + 1` units for the two outward bound-grid operations. Then:

```text
D_allowed = (Araw - Rraw) / Praw
D_target = min(requested coefficient allowance, D_allowed).
```

Indeed `delta_raw = ceil(Dcert*S)` and
`ceil(delta_raw*Praw/S) <= Dcert*Praw + Praw/S + 1 <= Araw` when
`Dcert <= D_allowed`. All calculations are exact integers/rationals. No peak
division is needed for `Praw=0`; the standalone coefficient allowance still
applies. A nonpositive remaining allowance fails explicitly. The CLI helper
permits only same-scale normalization after FIR; it rejects an unexpected
later stage rather than silently using this simplified target formula.

Enclosure refinement uses that possibly non-dyadic tighter target and retains
its aggregate work/precision limits. A deliberately loose user coefficient
target cannot bypass the whole-chain budget. After certification, the complete
chain is recomputed with `CertifiedFir`, the final sum is checked against the
original numerical target, and integer headroom is revalidated. Only then is
the mathematical-Kaiser reference named in the report and conversion allowed
to proceed. Certification alone does not change sample arithmetic. A proved
lower-bound violation now initiates
[bounded bank redesign](kaiser-design-enclosures.md), increasing coefficient
and automatic designer precision before re-running all response and numerical
checks. Merely inconclusive enclosures or exhausted resources do not trigger
redesign, reset budgets or weaken the target.

This closes the MPFR-design-error exclusion **for successful certified Kaiser
rate conversions**. Ordinary runs retain the original MPFR-value reference.
Unity analysis can certify a hypothetical bank, but its bypassed conversion
chain is not relabeled. Ideal-filter approximation, independently selected
normalization scales and final PCM/dither/clipping remain excluded in both
modes. Persistent certificate storage and non-Kaiser design-error enclosures
remain separate work.

## Normalization and integer headroom

The actual normalizer attenuates only. Against a reference using the same exact
chosen scale, a safe bound is `E_next <= E + epsilon`; the scale can never
amplify an earlier error. This is deliberately conservative: planning has not
scanned the track and does not assume a particular attenuation factor.

Every intermediate execution peak is bounded by `P+E`. The selected signed
integer width reserves its positive endpoint as well as negative full scale.
The existing minimum of 65 integer bits remains; a larger proved requirement
raises it. The signal's total width and bound integer headroom have explicit
1,048,576-bit resource guards.

## Coupled automatic refinement

The previous stage-count/log2-gain policy supplies the initial signal candidate.
For amplitude target bits `B`, the numerical error budget is `2^(-B-2)` FS,
reserving two bits beyond the requested attenuation. The planner evaluates the
complete chain and checks the sum of all three error components against that
budget with integer comparison.

If it fails, signal/effect precision and FIR coefficient precision are increased
separately according to the offending error components. The complete bounds
are then recomputed; a proposed increment alone is never considered proof.
Raising FIR width also raises the necessary MPFR and accumulator widths and
updates backend selection. The final filter plan, not the initial candidate,
is used by design, cache lookup, execution, and reporting.

Explicit `--signal-precision` and `--precision` cannot undercut this refined
plan. Non-convergence after 128 candidates or a resource limit fails explicitly.
Output publication remains transactional on such failures.

For example, `fast` with gain `2^60` before a 160/147 FIR selects Q2.85
coefficients instead of the native Q2.62 bank. If a `2^-60` convolution tap
restores the peak before the FIR, that coefficient-width increase is unnecessary.

```text
sex plan input.wav -r 48000 --preset fast --gain 1152921504606846976 --clip normalize
sex plan input.wav -r 48000 --gain 3/5 --dc-remove 10/11 --mix '1/3,2/7' --convolve '1,-1/5'
```

`plan` and `analyze` accept the same four effects and validate matrix columns
against the source layout. They report per-stage peak/error bounds and separate
final error components in exact powers of two. Response analysis still measures
the resampler FIR, not the frequency response of the whole effect chain.

## Evidence

Unit tests enclose exact rational coefficient errors, exercise downstream
amplification, compare recursive DC state to a rational oracle for 400 samples,
and verify width refinement and explicit-precision rejection. A cross-library
test runs real BigQ gain/DC/mix/convolution/polyphase FIR/normalization against
an independent rational reference at F=16, 63, 127, and 4096, including tails and
block sizes 1, 7, and 4096. Every output difference must lie inside the predicted
bound. CLI regressions require the refined cache identity and byte equality
across block sizes, and preservation of existing output on planning failure.

The certified-reference extension adds eight tests: replacement rather than
double counting, peak/downstream amplification, an independent rational
gain/FIR/normalization sample oracle, invalid supplied bounds, peak-aware target
selection and exact outward-rounding reserve, full-specification mismatch
rejection, and unchanged CLI output through gain ×2^60/DC/mix/convolution/
normalization. The existing transactional-failure test also rejects a loose
coefficient target with insufficient enclosure precision on an amplified chain.
The updated release workspace/all-target suite passes **387 tests**, with one
existing heavy opt-in test ignored. Fourteen numerical-model/planner tests pass
under emulated AArch64. Native and cross strict all-target Clippy pass. Logs:
`target/certified-chain-workspace-tests.log`, `target/certified-chain-arm-tests.log`.

Complete analyze stdout matches x86-64 versus emulated AArch64 with independent
caches for three Fast 48000→72000 cases: plain; gain ×2^60 with DC radius 3/4,
unit mono mix and normalization; and the same chain with a 2^-60 convolution
tap restoring the peak before FIR. All start at 32 enclosure bits; the amplified
case refines to 128 bits and uses C=86, while the others certify at 32 bits
with native C=62. Their conservative complete error bounds are respectively
2^-23, 2^-19 and 2^-22 FS, each inside the unchanged 2^-16 FS numerical budget.
Artifacts: `target/certified-chain-cli-qualified`. Full stdout SHA-256:

- Plain: `20ef51755bed64566e0a28b70901aaf8cb232e44f528aee439c9208ab4bd2293`.
- Amplified: `b4d8a0d191a87ea348272facf5cc31fae0d19cc6c9bc713c4c96943c99b9c1f3`.
- Restored: `b7ae9c18f3c96efa22640b727a9116985c063b2348d9fe6cb757b47beb58f4ab`.

These are numerical-bound and emulated-CPU observations, not physical ARM
qualification or new continuous response/PCM-quality guarantees.
