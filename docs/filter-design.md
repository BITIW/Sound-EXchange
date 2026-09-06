# Filter-design contract

Status: normative for the Kaiser and general windowed-sinc algorithms in
`sexfir`.

## Numeric environment

Design uses MPFR through Rug 1.30.0 at the precision recorded in the design
specification. Configuration values such as rolloff and Kaiser beta are exact
rationals; binary `f32`/`f64` values are not accepted. MPFR values exist only
inside the designer/analyzer. The execution bank contains signed Q2.62 or Q2.n
integers, never MPFR values.

Dependency versions are locked. Coefficient identity is SHA-256 over the
algorithm version, reduced rate ratio, tap count, rational rolloff/beta,
working precision, causal delay, widths, and every coefficient integer.

The coupled execution planner may raise coefficient precision because an
earlier effect raises the signal peak, even when the original filter-only
coefficient budget passed. Materialization and cache identity use the final
refined widths. Its analytic coefficient-error bound compares with an
exact-DC-corrected version of the MPFR coefficient vector; this differs from
the analyzer's measured error against the original MPFR vector. The distinction
and bound `(T-1)*2^-C` are derived in
[numerical-error-budget.md](numerical-error-budget.md#designed-polyphase-fir).

## Rational polyphase construction

For reduced output/input ratio `L/M`, phase `p`, tap `k`, and odd per-phase tap
count `T`, define:

```text
D = (T - 1) / 2
x = k + p/L - D
fc = 1/2 * min(1, L/M) * rolloff
h[p,k] = 2 fc sinc(2 fc x) Kaiser(x/D, beta)
```

The finite global prototype has support `0...(T-1)L`; consequently the final
tap of every nonzero phase lies outside support and is exactly zero. Each phase
is normalized independently before quantization. Starting the execution clock
at integer input frame `D` compensates the causal delay without approximating
it as a floating-point or fractional number of output frames.

## Window families

`WindowedSincSpec` keeps the same exact-rational cutoff and polyphase geometry,
but replaces the Kaiser factor with one of these symmetric windows for
`u = x/D`:

```text
rectangular:       w(u) = 1
Hann:              w(u) = 0.5 + 0.5 cos(pi u)
Blackman:          w(u) = 0.42 + 0.5 cos(pi u) + 0.08 cos(2 pi u)
```

The implementation evaluates them at the actual fractional phase coordinate;
it does not select a nearby integer window sample. Dolph-Chebyshev construction
also stays in MPFR. For odd length `N`, requested positive sidelobe attenuation
`A_db`, and `j = 0...(N-1)` it constructs:

```text
A       = 10^(A_db/20)
beta    = cosh(acosh(A)/(N-1))
P[j]    = T_(N-1)(beta cos(pi j/N))
k[j]    = j if j <= floor(N/2), else j-N
w(x)    = sum_j P[j] cos(2 pi k[j] x/N) / sum_j P[j]
```

Here `T` is the Chebyshev polynomial, evaluated through `cos` or `cosh` in its
appropriate domain. The last expression is the continuous Fourier extension
needed by fractional polyphase coordinates and is normalized to unity at the
window centre. A seven-point, 100 dB regression is locked against SciPy's
published discrete window values.

Version 2 fixes the signed DFT-bin extension at fractional positions; the
previous positive-index expression agrees only at integer coordinates. A
three-point analytic half-sample regression covers this distinction. Both the
native and arbitrary-width designers share the corrected interpolation.

The direct Dolph series costs `O(L T^2)` for `L` phases and `T` taps per phase,
so it has an explicit term budget and fails before allocation when the request
is too large. This is a current implementation limit, not a mathematical
quality claim; long preset designs continue to use the linear-cost Kaiser
path. All four window identities include the window choice, Dolph attenuation
when present, and the remaining design specification.

The CLI exposes all five windows through `--window`, using arbitrary Q2.C
and shared qualification for non-Kaiser choices. Native Q2.62 APIs remain
available. Exact search, precision, cache, and resource policies are in
[windowed-cli.md](windowed-cli.md); long Dolph requests can fail before design.

## Weighted least-squares designer

The following describes the **legacy native, independent-phase reference**.
It does not enforce combined transition-band image rejection. The new
arbitrary-width [global LS designer](optimized-designers.md) fits one prototype
and has separate identities, bounded precision search and actual-bank checks.

`LeastSquaresSpec` designs each fractional-delay phase directly on an MPFR
frequency grid. For phase `p`, delay `delta_p = D - p/L`, passband `P`, and
stopband `S`, it minimizes:

```text
sum_(f in P) Wp |sum_k h[k] exp(-i 2 pi f k)
                       - exp(-i 2 pi f delta_p)|^2
+ sum_(f in S) Ws |sum_k h[k] exp(-i 2 pi f k)|^2
```

`Wp` and `Ws` are positive exact-rational configuration values. The normal
matrix is real symmetric Toeplitz,
`A[i,j] = sum_f W(f) cos(2 pi f (i-j))`; it is built once and factored with an
MPFR Cholesky decomposition, then reused for every phase. A non-positive pivot
fails explicitly rather than being regularized invisibly.

The passband/stopband convention matches response analysis: for rolloff `r`,
the passband ends at `(2r-1)` times the lower Nyquist and the stopband begins at
the lower Nyquist. The implementation is discrete weighted least squares over
the recorded grid, not a claim of an analytically integrated optimum. Solve
work has an explicit safety budget. Each solved phase is finally normalized,
quantized to Q2.62, exact-DC-corrected, hashed, and accumulator-checked by the
same rules as windowed designs.

## Parks-McClellan equiripple designer

The arbitrary-width [optimized API](optimized-designers.md) reuses this exchange
algorithm and quantizes directly to Q2.C. Its resource preflight additionally
bounds selector tables and rejects excessive rounded extrema. See that document
for the repaired large-exchange regression and remaining qualification limits.

`EquirippleSpec` first designs one odd Type-I global prototype. For `L` phases
and `T` taps per phase its length is:

```text
N = L (T - 1) + 1
```

The global frequency edges are the per-phase passband and stopband edges
divided by `L`. Its desired pass gain is `L`, so decomposition naturally gives
approximately unit DC gain per phase. For centre index `C = (N-1)/2`, the
zero-phase amplitude is represented as:

```text
A(f) = a[0] + sum_(k=1..C) a[k] cos(2 pi f k)
h[C] = a[0]
h[C-k] = h[C+k] = a[k] / 2
```

MPFR Remez exchange solves for `C+1` amplitudes plus the alternating weighted
error. Both endpoints of every disjoint band remain exchange candidates; the
remaining signed local extrema are selected by a maximum-weight alternating
subsequence. Remez v2 distributes the initial references by physical band width,
not by index in the separately sampled pass/stop grids. A one-point sign lobe
remains a candidate even if a neighboring opposite-sign error has larger magnitude.
Convergence requires alternating reference errors, each with magnitude at least
`delta*(1-2^(-P/3))`, and a complete design-grid maximum no greater than
`delta*(1+2^(-P/3))`. An unchanged reference set alone is not success.
The report records iteration count, dense-grid size, extremal
frequencies, and the pre-quantization weighted prototype error. Failure to find
enough extrema, a singular interpolation system, exhausting iterations, and
exceeding the work budget are all distinct errors.

After convergence, phase `p`, tap `k` receives global coefficient `h[Lk+p]`;
the single out-of-support final position is exact zero. Every phase is then
normalized and goes through Q2.62 quantization, exact DC correction, identity
hashing, and native accumulator proof. Consequently the reported prototype
ripple describes the MPFR global exchange result, while `analyze_*` describes
the actual fixed-point polyphase execution bank. Neither number is silently
substituted for the other.

## Quantization-aware invariants

Every coefficient is rounded directly to the planned Q2.n with
nearest/ties-to-even. The designer then computes the integer residual between
the phase sum and exact Q2.n unity
and applies it to the largest-magnitude coefficient. Therefore every stored
phase has an exact raw DC sum of `2^n`.

The report includes both the largest individual coefficient error and the
largest phase L1 error after correction. The latter is a frequency-independent
upper bound on the complex-response change caused by coefficient quantization:

```text
|H_quantized(f) - H_designed(f)| <= sum_k |error[k]|
```

Finally, `sexrate` proves the full-scale coefficient L1 bound for each phase.
A native bank that cannot fit every possible Q1.63 input pattern in signed 128
bits is rejected. A bigint bank must fit its explicitly configured signed
accumulator width. Both checks happen before audio processing.

The historical `native_candidate` constructor is a compatibility façade for the
planner's current `sane` policy, not a promise to retain the original coefficient identity. Wider
plans use the same ideal-design equations and a distinct width-aware identity.

The current Sane planner uses 289 taps when `L >= M` and scales the half-length
by `ceil(144 M/L)` in rational arithmetic for downsampling. This keeps the absolute
transition width from silently shrinking while filter length stays fixed; the
analyzer remains the authority on whether the resulting response is acceptable.
See [`precision-planner.md`](precision-planner.md) for preset numbers and exact
planning rules.

## Response-grid analysis

The [feedback layer](quality-refinement.md) separates coefficient precision
from window shape refinement. It assesses the actual cached/designed bank,
raises `C` on a coefficient-budget failure or window-specific length/shape on a response failure,
then repeats within explicit limits. Requested continuous and harmonic gates
participate in that decision; an inconclusive certificate is never accepted.
The CLI requires this for all materialized FIR banks, with or without an
explicit error floor; `--refine-quality` is now a redundant compatibility alias.
`--window` retains the selected window throughout this loop; see
[windowed-cli.md](windowed-cli.md) for non-Kaiser search and width policies.

The separate [`--certify` mode](continuous-response-certificate.md) can prove
continuous per-phase amplitude bounds using exact rational polynomials and
Bernstein subdivision. Its budget and scope are explicit; it does not turn the
sampled grid below into a proof. Non-Kaiser banks may use the same low-level
certificate API when their coefficients are real and uniformly quantized.

`sex analyze` evaluates every quantized phase with MPFR. It reports the extrema
found on a uniform passband/stopband grid, not an unsampled analytic proof. The
passband ends at `(2 rolloff - 1)` times the lower Nyquist and the stopband starts
at the lower Nyquist, placing the sinc cutoff at the midpoint.

These per-phase transfer functions directly characterize downsampling alias
rejection and fractional-delay ripple. For upsampling, the current stopband
number samples the input Nyquist endpoint. The separate
[`--harmonics` analysis](harmonic-response-analysis.md) now samples every
periodically time-varying output component across the full input band, including
the transition region. Continuous bounds for those complete image responses
remain required before public preset limits are frozen.

## Historical sparse-grid regression cases

The following values are regression contracts for the original beta-10 candidate.
They are measured over 129 points per band for every phase at 192-bit MPFR
precision. They do not qualify the current Sane preset: this grid missed peaks
between samples and overstated suppression for the 1/3 case.

| Conversion | Phases x taps | Passband end | Ripple | Max linear deviation | Stopband start | Stopband peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 44100 -> 48000 (`160/147`) | 160 x 257 | 0.45 | 0.00006265820113691220 dB | 0.000003637947248714758 | 0.5 | -117.0862953781437 dB |
| 48000 -> 16000 (`1/3`) | 1 x 769 | 0.15 | 0.00003411283202005583 dB | 0.000002353316615337044 | 1/6 | -110.5568805632797 dB |

The test suite retains these historical figures and identities using explicit
legacy parameters. The 513-point regression demonstrates both passband and
stopband failure against -110 dB. The corrected Sane policy has a 12 dB design
margin and is separately tested on 1025 points/band at 1/3, 3/2, and 1/48.
For 1/3 the new bank has 865 taps, maximum deviation 5.093866506419039e-7,
ripple 7.557471197964743e-6 dB, and stopband peak -126.2884407695001 dB.
These remain sampled measurements, not continuous-band proofs. See
[precision-planner.md](precision-planner.md) for the compatibility change.
