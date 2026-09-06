# Rigorous Kaiser FIR design enclosures

`sexfir::enclosure` supplies finite, directed-rounding bounds for the
**mathematical normalized Kaiser FIR phase** at exact rational coordinates.
It encloses the window, sinc, signed summation and normalization, then bounds
the difference from actual final integer coefficients, including DC correction.
Existing designers, coefficient identities and the fixed-point signal algorithm
are not replaced. CLI certification now covers accepted Kaiser banks using the
planner's coefficient allowance by default, tightened as needed by the
available whole-chain budget. Certified Kaiser runs propagate the joint error
through effects and signal rounding; ordinary runs retain the design-error
exclusion. Certificate persistence remains outstanding.

## Positive series with a bounded remainder

For an exact rational squared argument `s >= 0`, the API encloses

```text
I0(sqrt(s)) = sum(k=0..infinity) (s/4)^k / (k!)^2.
```

This specializes the modified-Bessel series in
[NIST DLMF 10.25.2](https://dlmf.nist.gov/10.25.E2) to order zero. Taking the
squared argument avoids introducing a rounded square root. The remainder bound
below follows directly from the positive terms and their decreasing ratios.

Set `a = s/4`, `t0 = 1`, and `tk = t(k-1)*a/k²`. After summing through index `n`,
the next term is `t(n+1)`. Every later successive ratio is at most
`r = a/(n+2)²`. Once `r < 1`,

```text
0 <= omitted_tail <= t(n+1) / (1-r).
```

The implementation encloses `a`, each term and each partial sum with explicit
MPFR downward/upward rounding. Its computed `r_upper` bounds `r`; when
`r_upper < 1`, downward rounding of `1-r_upper` gives a positive denominator
lower bound. Upward division of the next-term upper bound gives `tail_upper`.
The returned endpoints are `partial_sum_lower` and an upward-rounded
`partial_sum_upper + tail_upper`. Thus arithmetic rounding and truncation of
the infinite series are both included, not confused with one another.

The loop stops when `tail_upper <= partial_sum_lower / 2^precision_bits`, using
downward rounding for that comparison threshold. This controls the omitted
tail only: endpoint precision is **not** a promise that the entire interval is
one ulp wide. Accumulated directed rounding can make it wider. Call `width()`
for the exact rational width of the finite dyadic endpoints.

There is no success based merely on the sum ceasing to change. Exhausting the
term budget, exceeding the input/precision limits or encountering nonfinite
MPFR arithmetic returns a typed error, not an incomplete bound. `I0(0)` is
exactly `[1,1]` with zero terms and zero tail.

## Window enclosure and approximation error

For radius `d > 0`, exact rational beta `b >= 0`, and exact rational distance x:

```text
w(x) = I0(sqrt(b² * (1 - x²/d²))) / I0(b),   |x| <= d
       0,                                    otherwise.
```

Both squared arguments are formed with exact rational arithmetic. A
`KaiserWindow` instance computes the denominator enclosure once and reuses it
across coordinates. Divide numerator-lower by denominator-upper downward, and
numerator-upper by denominator-lower upward. Since the series is increasing on
nonnegative squared arguments, the exact window lies in `[0,1]`; intersecting
the upper endpoint with 1 is valid. Symmetry follows from exact `x²`.

At x=0 the numerator and denominator are identical, so return `[1,1]` rather
than losing this dependence in interval division. Beta zero also gives one
inside the support. The endpoint `|x|=d` is included; just outside the radius
the window is exactly zero. The API uses exact coordinates. For the existing
polyphase designer, that coordinate is `tap + phase/up - input_delay`.

`absolute_error_bound(approximation)` returns, with exact rational arithmetic,
the larger distance from a finite MPFR approximation to either interval
endpoint. This bounds its error relative to the mathematical window value,
including the uncertainty of the enclosure. NaN and infinity are rejected.
It does not label that error as FIR quantization error.

## Exact sinc geometry and normalized coefficients

`kaiser_phase(spec, phase, limits)` constructs one phase independently of the
existing coefficient designer. For ratio `L/M`, phase `j`, tap `k`, and
`d = floor(taps_per_phase/2)`, it uses these exact rationals:

```text
x_k = k + j/L - d
2c  = rolloff * min(1, L/M)
h_k = sin(pi * 2c * |x_k|) / (pi * |x_k|) * w(x_k), x_k != 0
      2c,                                        x_k == 0
g   = sum h_k
a_k = h_k / g
```

Support exclusion is evaluated exactly before transcendental work. Negative
sinc lobes remain signed. Pi has correctly rounded MPFR downward/upward
endpoints. Signed interval products and quotients use all four endpoint
combinations, each rounded outward. The gain is a directed sum, not a
nearest-rounded approximation. A gain interval containing zero fails explicitly;
the caller must refine precision or reject the design. Normalization loses
dependencies between numerator and gain, making bounds conservative but valid.

`sin_pi_rational` first reduces the magnitude's integer part with exact integer
division, retaining its parity. For fractional part `f`, set `t = |f-1/2|`.
On `[0,1/2]`, `cos(pi*t)` is decreasing, so an upward-rounded t followed by
downward-rounded MPFR `cos_pi` gives the lower bound, and conversely for the
upper bound. Integer parity and the original sign select the final sign.
This avoids converting huge integer offsets to MPFR or multiplying a rounded
coordinate by a rounded pi for quadrant selection. Integer inputs return
exact zero. Tiny fractions at insufficient endpoint precision can have wide
enclosures; precision is not silently increased or mistaken for accuracy.

`PhaseEnclosure::compare_quantized(raw, fractional_bits)` consumes exactly one
phase of **final** integer coefficients. Let `q_k = raw_k / 2^F`, exactly, and
let `[l_k,u_k]` enclose the mathematical normalized coefficient. It returns:

```text
e_k         = max(|q_k-l_k|, |q_k-u_k|)
max_absolute = max e_k
phase_l1     = sum e_k
```

These differences and reductions use exact rational arithmetic. Consequently
the bound includes the original designer's MPFR error, quantization, any DC
correction and the enclosure's uncertainty **jointly**. It does not assign
separate certified figures to these contributions. Comparing the final bank
avoids needing to prove which coefficient the existing DC-correction selection
branch will choose. Merely increasing F cannot hide low-precision design error.

For any frequency, the complex response difference of this phase is bounded
by `phase_l1`, by the triangle inequality and unit magnitude of each Fourier
basis factor. This is not a bound on the ideal filter's approximation error,
an entire rate transform's images, output PCM/dither, or a complete stopband
certificate. Callers must cover every phase and propagate the applicable error
budget before using this in a whole-chain claim.

## Resource and API scope

The window API comprises `Limits`, `i0_squared`, `BesselEnclosure`, `Enclosure`
and `KaiserWindow`. Endpoints are private and readable through accessors.
Default limits are 192 endpoint bits, 16384 nonconstant series terms and 65536
bits per rational numerator/denominator. Limits are explicit and bounded; the
derived squared arguments passed to I0 are also checked against the rational
input budget. Endpoint precision, series count and input-bit settings cannot
exceed 1048576. A caller can retry with more precision/resources after an
explicit failure; the module never silently lowers precision.

The phase API adds `PhaseLimits`, `kaiser_phase`, `sin_pi_rational`,
`PhaseEnclosure`, and `CoefficientErrorBounds`. Default phase limits are 4097
taps and one million total nonconstant I0 terms, including the shared
denominator and all numerators. The total work allowance clips each series
budget before evaluation; it never returns a partial coefficient vector on
exhaustion. Maximum configurable limits are 1048576 taps and one billion terms.
Exact support/center/zero-beta shortcuts consume zero numerator terms. Phase
range, mathematical specification, input bit budgets and coefficient count are
checked; coefficient fractional width and integer magnitude are bounded too.
Endpoint precision is independent of the original designer's working precision.

MPFR remains in the design/measurement layer. This module introduces no float
canonical samples, FIR intermediate rounding or changes to the signal path.
The enclosure module does not alter supplied banks. The CLI can now redesign
a provably insufficient bank as described below; certificate persistence is
still separate unfinished work.

## Validation: 2026-09-06

Five new tests cover:

- Independent exact-rational partial sums plus geometric tail brackets against
  MPFR outward endpoints at 16, 32, 64, 192, 512, 1024 and 4096 bits, including
  zero, non-dyadic rationals, beta-scale arguments and a squared argument 2^-2000.
- Independent exact numerator/denominator series brackets for the window,
  symmetry, center, support endpoint, just-outside support and beta zero.
- Exact error bounds around the existing MPFR window implementation for all
  six presets, including near-boundary rational coordinates.
- Invalid limits/domains, insufficient term budgets, oversized rational inputs
  and nonfinite approximations failing closed.

These numerical tests exercise the directed arithmetic independently; the
general enclosure claim additionally relies on the mathematical inequalities
above and MPFR's directed-rounding contract, not on sampled agreement alone.

At the window-only checkpoint the release workspace/all-target suite passed
354 tests, with the existing
heavy opt-in test ignored. Strict native and AArch64 all-target Clippy pass.
All five enclosure tests also pass under emulated AArch64. The existing
QEMU-only linker accommodation remains: this is not physical ARM or production
linking qualification. Logs: `target/kaiser-enclosure-workspace-tests.log` and
`target/kaiser-enclosure-arm-tests.log`.

The diagnostic `cargo run --release --example kaiser_window_enclosure` reports
30 window-width observations: six preset betas, each at 64/192/512/1024/4096
endpoint bits and x=d/2. Native and emulated-ARM TSVs match exactly, SHA-256
`dc9607b89c8d94df02a33cb8d7392a0f10e1c6dce7c686bffb7ebd627c46e127`.
The files are under `target/kaiser-enclosure-qualified`. For example, the
Until-40k beta at 4096 bits uses 683 denominator-series terms and gives an
absolute window interval width bounded above by 2.5784180027905909302e-1234.
That is one window coordinate, not a full-window maximum or a FIR stopband.

Sane and High 48000→16000 renders from the new build also match the preceding
frozen processor byte-for-byte (blocks 257 versus 4096). Their SHA-256 values
are `a8ad65abf0739d30b37aaed6792b9d1d2b5a32fb0bb24a8c29b1cbbfc86924be`
and `61c9580807b2a526fb0f8e697de16939e0d42bc91d71d79cc0031cc81d6c68b0`,
respectively; audio and logs are in the same result directory. These checks
complement the existing workspace regressions; the new enclosure module is
not on the conversion path at that checkpoint. The long Until-40k run used its
own earlier executable/source snapshot, unaffected by rebuilding the workspace;
its [completed guarded sweep](spectral-snapshots.md) is now recorded separately.

## Normalized-phase validation: 2026-09-06

Ten additional tests cover exact integer/half/sixth phases and huge integer
offsets, irrational sine values via exact squares, tiny 2^-1000 coordinates,
signed interval corners, phase support, limits, actual native/GMP coefficients
and arbitrary DC adjustments. Independent exact-rational Machin-series bounds
for pi and positive-series bounds for I0 check analytic three-tap normalized
filters at both beta zero and beta eight. A deliberately 96-bit-designed bank
with 256 fractional coefficient bits demonstrates that the new bound captures
design error, while a 512-bit design at the same coefficient width is much
more accurate. As above, tests complement the arithmetic proof, not replace it.

The updated release workspace/all-target suite passes **364 tests**, with one
existing heavy opt-in test ignored. All 15 window/phase enclosure tests pass
under emulated AArch64. Native and AArch64 strict all-target Clippy pass. Logs
are `target/kaiser-phase-workspace-tests.log` and
`target/kaiser-phase-arm-tests.log`; this remains emulation with the documented
QEMU-only linker accommodation, not physical ARM qualification.

`cargo run --release --example kaiser_phase_enclosure` reports 44 phase/width
combinations: all phases of 1/3, 3/2 and 7/11, using 17 taps, beta eight,
rolloff 1/2 and coefficient fractional widths 62/96/256/512. These deliberately
small banks qualify arithmetic, not preset frequency response. Native and
emulated-ARM results match byte-for-byte, SHA-256
`7f76771735da828532bd95ebfcbfdab8e8eca02228908bbbbc3cf059b7da40cb`:
`target/kaiser-phase-native.tsv` and `target/kaiser-phase-arm.tsv`.
For example, the 1/3 bank with 512 fractional bits, 1024 design bits and 1152
enclosure bits has joint phase-L1 coefficient error bounded above by
2.8506762943375343938e-154. This is neither its stopband attenuation nor a
whole-chain audio error floor. No conversion-path algorithm was changed.

## Complete-bank API and automatic enclosure refinement

`certify_kaiser(&DesignedFilter, BankLimits)` and
`certify_kaiser_big(&DesignedBigFilter, BankLimits)` certify **every** phase of
an immutable actual Kaiser bank. They validate layout and resource limits, then
recompute the existing coefficient SHA-256 from the actual table and require
agreement with its design report. Hash encoding is unchanged: the hash helpers
now accept iterators so verification does not copy the full bank. Only one
phase's coefficients and intervals are materialized at a time, together with
the accumulated per-phase error summaries. A caller cannot construct or mutate
the private fields of the returned `BankCertificate`.

The certificate retains the exact mathematical specification, actual bank
identity and coefficient fractional width, an ordered error bound for every
phase, maximum coefficient error, maximum phase-L1 error, final endpoint
precision, pass count, total I0 work and requested limits. No phase subsampling
is permitted. Let `E = max_j sum_k e_jk`. For any input sequence with
`|x_n| <= X`, comparison of the two **unrounded** FIR sums gives:

```text
|sum_k q_jk*x_nk - sum_k a_jk*x_nk| <= X * E.
```

`signal_error_bound(X)` returns this product as an exact rational, checks the
input-size budget and rejects negative X. Zero-padding at finite-stream edges
preserves the bound. This extends the coefficient-error result to the actual
phase schedule without pretending it covers final rounding, effects, dither,
clipping or ideal-filter approximation. It still does not prove a whole
continuous-band image or stopband specification.

The optional exact `target_max_phase_l1` requests `E <= target`. If a complete
pass returns a wider bound, endpoint precision doubles, capped at exactly
`max_precision_bits` (including a non-power-of-two cap). Every phase is then
rechecked. The bank and its original designer precision never change. At the
cap, an unresolved target returns `TargetNotMet` with the final upper bound;
no success certificate is returned. A proved violation instead returns
`TargetViolated` as soon as a complete pass establishes it (see below).
Enclosure errors such as unresolved normalization
gain and exhausted series budgets fail explicitly instead of triggering
unbudgeted retries. Without a target, one complete pass suffices.

The default bank limits are 4096 phases, 1048576 total coefficients, ten million
aggregate I0 terms and a 4096-bit endpoint ceiling, alongside the existing
per-phase limits. Work accounting includes repeated denominator calculations,
all phases and **all refinement passes**. Each phase's work cap is clipped to
the remaining total before evaluation. Configurable hard caps are 1048576
phases, 1073741824 coefficients, one billion I0 terms and 1048576 endpoint bits;
all are explicit ceilings, not preallocations or performance promises. Phase
and total-coefficient caps are checked before hashing or interval construction.

Eight bank tests cover complete phase coverage, native/GMP identity, unchanged
input banks, successful refinement with an exact 100-bit ceiling, work totals
across every pass, exact-budget success versus one-term-short failure, target
failure, rejected partial coverage, altered identities/layouts, invalid limits
and exact signal-peak scaling. The refinement target is deliberately separate
from filter redesign: reducing enclosure uncertainty cannot repair a bad bank.

At this complete-bank checkpoint, the release workspace/all-target suite passes
**372 tests**, with one existing heavy opt-in test ignored. All 23 enclosure
tests pass under emulated AArch64, and strict native/cross all-target Clippy
passes. Logs: `target/kaiser-bank-workspace-tests.log` and
`target/kaiser-bank-arm-tests.log`.

`cargo run --release --example kaiser_bank_certificate` produces 13 complete
bank reports: twelve small arithmetic banks (three ratios, four coefficient
widths, automatic enclosure refinement) and the native Sane candidate for
160/147, covering all 160 phases and 289 taps per phase. For that candidate,
192-bit endpoints and 2143506 I0 terms yield a maximum joint phase-L1 bound
of 1.8357464732384180819e-17. Its bank identity is
`90515b7fc9fc8be293bc360dc65803fb19629a3d8bea9069679ced22198c8ac2`.
This certifies the coefficient error of that actual candidate, not its preset
response gates. All 13 native/emulated-ARM reports match byte-for-byte:
`target/kaiser-bank-native.tsv`, `target/kaiser-bank-arm.tsv`, SHA-256
`4d6b6e128b5ec44d05d20c8143237bae17105078fee36a2df8c2fc2bc30767bb`.
The existing emulation/linker qualification caveat still applies.

## CLI use and failure behavior

```sh
sex analyze input.wav -r 48000 --certify-design
sex input.wav output.wav -r 48000 --preset high --certify-design
sex plan input.wav -r 48000 --certify-design
```

`--certify-design` is separate from `--certify`, which proves per-phase
continuous-band amplitude constraints. It runs after the mandatory response
feedback has selected its actual bank. Initial phase/tap/coefficient limits are
checked before design or cache access, then checked again against the accepted
bank, whose length may have grown. Only Kaiser windowed-sinc is currently
supported; other designers fail explicitly. Native and GMP banks, including
cache hits, use the same verification. Certificates themselves are **not**
persisted: each requested run recomputes the bank identity and all-phase bounds.

The default joint phase-L1 target is exactly `2^-(plan.amplitude_error_bits+3)`,
the existing planner's coefficient allowance. An explicit
`--design-certificate-error-bits N` requires `E <= 2^-N`. This changes the
certification target, not the bank, preset, or original design precision. The
enclosure precision starts at 192 bits and can refine through 4096 by default.
Failure does not silently weaken the target or return a partial-phase report.

Controls (all require `--certify-design`, with duplicates rejected):

| Option suffix after `--design-certificate-` | Default | Meaning |
| --- | ---: | --- |
| `bits` | 192 | Initial directed endpoint precision, at least 16 |
| `max-bits` | 4096 | Endpoint refinement ceiling, at least initial precision |
| `error-bits` | planner amplitude bits + 3 | Exact joint phase-L1 target exponent |
| `work` | 10000000 | Aggregate I0 terms across phases and passes |
| `phase-work` | 1000000 | Per-phase I0 terms |
| `series` | 16384 | Terms per individual I0 evaluation |
| `input-bits` | 65536 | Rational numerator/denominator and raw-integer bit limit |
| `taps` | 4097 | Maximum taps per phase |
| `phases` | 4096 | Maximum phase count; no subsampling |
| `coefficients` | 1048576 | Maximum total bank coefficients |

Numeric ranges follow the library caps; target exponent must be smaller than
the rational input-bit limit. The report includes actual bank identity,
coverage, coefficient width, endpoint precision, pass/work counts, target,
outward-rounded decimal bounds and the **exact rational maximum phase-L1
bound**. Design error, quantization and DC correction are deliberately reported
jointly. The input-peak-scaled coefficient bound is now propagated through the
whole-chain numerical report, replacing its generic FIR quantization allowance.
The [certified numerical model](numerical-error-budget.md) describes the exact
target calculation, upward rounding, same-scale normalization and final gate.
Its combined bound includes effects and signal rounding but still excludes
final PCM/dither/clipping and ideal-filter approximation. The effective
coefficient target, which may be tighter than `2^-N`, is printed exactly.

Analysis writes its certificate to stdout. Conversion requires certification
before creating/publishing the output and writes the certificate to stderr.
Resource, unsupported-designer and target failures return nonzero without
overwriting an existing destination. Same-rate conversion bypasses FIR and
rejects `--certify-design`; unity-rate analysis already labels its bank as
hypothetical. Plan mode reports the request and initial resource checks but
does not build a bank, create a coefficient cache, or claim certification.

Remaining integration: persist and validate certificates with the cache
contract. Non-Kaiser designers need their own enclosures, and broader
mathematical/physical-platform qualification remains outstanding. The broader
goal is not complete.

### Proved violation versus inconclusive enclosure

For each final rational coefficient q and mathematical enclosure `[l,u]`, the
minimum possible absolute error is also an exact rational:

```text
e_lower = l-q, if q<l
          q-u, if q>u
          0,   otherwise.
```

`CoefficientErrorBounds::phase_l1_lower` sums those lower bounds within a
phase. The maximum of these sums over all phases lower-bounds the bank's true
maximum phase-L1 error. If this value is strictly greater than the requested
target, merely narrowing the enclosures cannot make the bank comply.
`TargetViolated` returns the lower/upper bounds, target, endpoint precision
and aggregate I0 terms spent so far. This is checked after a complete pass,
before deciding whether to refine further. Equality is not treated as a
violation. If the upper bound misses the target but the lower bound does not
prove a violation, only enclosure refinement is justified; at its ceiling
the result is explicitly inconclusive and does not initiate CLI redesign.

### Automatic certified bank redesign

The CLI responds only to `TargetViolated` by proposing a wider bank. For
upper error bound U and effective target D, it uses exact integer arithmetic
to choose `increment = ceil(log2(U/D)) + 8`. It increases actual coefficient
fractional width by this increment and, in automatic design-precision mode,
also increases MPFR working precision, respecting the planner's minimum and
32-bit width granularity. The eight extra bits are a conservative proposal,
**not a proof** of the next bank's error. Explicit MPFR precision is preserved;
if it cannot support the new coefficient width the request fails.

The proposal preserves ratio, taps, Kaiser beta, rolloff, preset, response
bands and requested targets. The candidate re-enters numerical planning,
coefficient design/cache lookup and all requested response gates. Only then
is its actual bank certified again against the recomputed whole-chain
allowance. Mandatory response feedback may still adjust a candidate if its
new actual response demands that; no response target is relaxed. Ordinary
conversions without `--certify-design` keep their existing behavior.

Resource accounting spans the whole search. `--design-certificate-work`
includes every I0 term in rejected and successful banks, across all endpoint
passes; remaining work clips the next certification before evaluation.
`--refinement-attempts`, `--refinement-coefficients` and `--refinement-terms`
are also shared across all response searches, with cache hits charged the
same as fresh banks. Precision caps remain enforced. Counters use checked
subtraction. Failure of any budget, inconclusive proof, unsupported designer
or explicit-precision constraint is terminal, not an excuse to reset a budget.
Output publication remains after all checks. The report records rejected
coefficient identities, width proposals, total redesign count and aggregate
certificate work.

At the redesign checkpoint, **391 workspace/all-target tests pass**, with the
existing heavy opt-in test ignored. New tests distinguish wide-but-inconclusive
bounds from proved violations, verify exact brackets and early work counts,
preserved shape/explicit precision, and a complete CLI redesign with exact
aggregate-work success versus one-term-short failure. Response attempt/count/
term caps and inconclusive-proof non-redesign are exercised too. Twenty-five
enclosure tests and four CLI certificate tests pass under emulated AArch64;
strict native/cross all-target Clippy passes. Logs are
`target/certified-redesign-workspace-tests.log`,
`target/certified-redesign-arm-enclosure-tests.log` and
`target/certified-redesign-arm-cli-tests.log`.

For Fast 48000→72000 with `--design-certificate-error-bits 200`, the actual
bank is automatically redesigned from C=62/MPFR=128 to C=213/MPFR=288,
keeping three phases of 129 taps. Both banks pass response checks; the final
384-bit enclosure proves joint phase-L1 error <= 2.5660886763705937782e-63.
Total certificate work is 54242 I0 terms across both banks (39183 for the
successful bank). The accepted identity is
`8de60e133dbc1ba8f95f8bbb4b783f15ed0d66d778cccf14f60c3c3b6ae0c02a`.
Complete stdout and stderr match x86-64 versus emulated ARM with independent
caches under `target/certified-redesign-cli-qualified`:
stdout SHA-256 `391e52c886253e4743b9d04f03560b686abe35c93cbfb2faa917a0405c62f3fc`,
stderr SHA-256 `0c880532bbc551ba5c2a1d08351c430444376a54e4f7d9230f4e26784c4ee24e`.
This is numerical/CLI evidence, not a new continuous response or physical ARM
qualification claim.

### CLI validation: 2026-09-06

Seven new tests cover parser controls and dependencies, the planner-derived
target, native/GMP coefficient-cache miss and hit, identical reports for
analysis and conversion, unchanged rendered bytes, denied resources/targets
preserving existing output, plan mode without cache creation, and the real
Absurd 4097→4309 tap refinement. The last case rejects the default 4097-tap
certificate cap after feedback and then certifies the accepted cached bank
when the cap is raised to 5000. No initial-bank certificate is reused.

At the initial CLI integration checkpoint the release workspace/all-target
suite passed **379 tests**, with one existing
heavy opt-in test ignored. Native/cross strict Clippy passes; the three new
CLI parser/target unit tests pass on emulated AArch64. Logs:
`target/design-certificate-cli-workspace-tests.log` and
`target/design-certificate-cli-arm-tests.log`.

Full analyze stdout matches byte-for-byte between x86-64 and emulated AArch64
with independently generated caches, for Fast/High at 48000→72000 and Absurd
at 48000→24000. All use initial enclosure precision 32 and tap cap 5000;
High/Absurd refine to 64 endpoint bits. Results are under
`target/design-certificate-cli-qualified`, with stdout SHA-256 values:

- Fast: `c799438fb538c3ec34050c0e7058e14afab9524a996fde5f61f75db1f77fc7d5`.
- High: `4896a061e22012e6be1515dd0b62d13c20a275e5acf25c65cd739e3d61301180`.
- Absurd: `4f651939c18ea29741f983a2f4736e870bd9fb88848f2ee2a9c94bb1ad41330a`.

These are coefficient-error/CLI reproducibility observations, not extra
stopband proofs or physical ARM qualification. The completed Until-40k campaign
used its earlier frozen processor and did not execute this newer feature.
