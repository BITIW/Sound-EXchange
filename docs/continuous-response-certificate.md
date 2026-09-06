# Continuous-frequency FIR certificates

`sex analyze --certify` bounds the amplitude response of the actual quantized
FIR bank over complete frequency intervals. This is separate from the existing
MPFR response grid and from the whole-chain computational-error bound.

```text
sex analyze input-48k.wav -r 16000 --preset sane --certify
sex analyze input-44k1.wav -r 48000 --preset sane --certify
sex analyze input-48k.wav -r 24000 --preset high --certify --certificate-taps 2049
```

The target is the planned preset/error-floor attenuation, including when
`--error-floor` is omitted. `Certified` means every stored phase passed both
specified bands. `Violated` includes an exact rational frequency coordinate
inside a band where the actual response fails. `Inconclusive` means a resource
limit or interval uncertainty prevented a decision. Neither non-certified
outcome publishes partial whole-bank bounds.

These are per-bank outcomes. The mandatory CLI [feedback loop](quality-refinement.md)
can strengthen a `Violated` candidate and certify the new bank against the same
target. It succeeds only after all requested gates pass for that final bank;
resource exhaustion or inability to qualify a candidate returns nonzero.
`Inconclusive` stops immediately, without a grid-only fallback. Rejected proof
diagnostics retain work/cell counts or exact witnesses on stderr. Different
gate settings can select different banks, so compare coefficient identities
before applying an analysis result to an ordinary conversion.
The [shared gate configuration](shared-quality-gates.md) now lets conversion
request these same checks directly, for example
`sex input-48k.wav -r 16000 output.wav --preset sane --certify`. Its successful
qualification report names the actual bank executed for that audio; a failed
or inconclusive required proof preserves the previous output file.

## Exact polynomial

For a phase with signed coefficient codes `r_i` and fractional width `F`, let
`h_i = r_i / 2^F`, `H(f) = sum_i h_i exp(-2 pi j f i)`, and `x = cos(2 pi f)`.
Its squared magnitude is

```text
a_k  = sum_i r_i r_(i+k)
P(x) = a_0 + 2 sum_(k=1..T-1) a_k T_k(x)
|H(f)|^2 = P(x) / 2^(2F)
T_0(x) = 1; T_1(x) = x; T_(k+1)(x) = 2x T_k(x) - T_(k-1)(x).
```

This follows by pairing conjugate terms in `H * conjugate(H)`. It applies to
asymmetric fractional-delay phases as well as symmetric phase zero. Integer
autocorrelation and the Chebyshev recurrence produce an exact GMP power-basis
polynomial; there is no intermediate floating-point coefficient conversion.
The low-level `certify_bank` API accepts arbitrary real uniformly quantized
banks, including coefficients with thousands of fractional bits.

## Interval bounds and subdivision

On `x = a(1-t) + bt`, `0 <= t <= 1`, express a degree-`n` polynomial as
`sum_i b_i B_(n,i)(t)`, where

```text
B_(n,i)(t) = binomial(n,i) t^i (1-t)^(n-i).
```

These weights are nonnegative and sum to one by the binomial theorem, so the
polynomial lies between the smallest and largest control value `b_i` throughout
the interval. This standard Bernstein convex-hull property and midpoint
subdivision are also described in the
[Cornell spline lecture](https://www.cs.cornell.edu/courses/cs4620/2011fa/lectures/27splinesWeb.pdf).
SeX implements its own exact-integer conversion and subdivision.

Band endpoints are dyadic rationals. Write `a=A/S`, `b=B/S` with a common
power-of-two denominator. Horner conversion keeps a common integer denominator
`D_d = d! S^d`. Multiplying degree `d` by `x` and adding power coefficient `p`
gives, with out-of-range controls interpreted as zero:

```text
D_(d+1) = D_d S (d+1)
C'_i = A (d+1-i) C_i + B i C_(i-1) + p D_(d+1).
```

After conversion the denominator includes `2^(2F)`. All controls remain exact.
An interval passes only if its entire control hull lies inside the allowed
squared-amplitude range. Otherwise exact midpoint de Casteljau subdivision
produces two covering children. Its averaging is represented by addition and
a common extra denominator factor `2^n`, not by rounding controls. Depth-first
traversal retains a bounded stack, not an exponentially growing queue.

The first and last controls are actual polynomial values at interval endpoints.
They can witness a violation; interior controls cannot. A negative stopband
control alone is not evidence of a negative response: `P(x) >= 0` follows from
the squared-magnitude identity, so certified lower bounds are clamped to zero.

## Directed endpoints, thresholds, and reports

Frequency uses input-rate units. With reduced ratio `L/M`, rolloff `r`, and
`v = min(1, L/M)`, the bands match the existing analyzer:

```text
passband: 0 <= f <= v (2r-1) / 2
stopband: v/2 <= f <= 1/2.
```

MPFR `cos_pi` with directed rounding encloses the cosine of each exact rational
twice-frequency. Cosine decreases on this domain, so argument and result
rounding directions are opposite. Directed conversion to a dyadic grid gives
outer intervals covering the requested bands and inner intervals contained in
them. Certification covers the outer intervals; violation witnesses must lie
in the inner intervals. Thus a rounded endpoint cannot invent a violation in
a narrow sliver outside the real band.

For positive target attenuation `A`, let `epsilon = 10^(-A/20)`. MPFR brackets
this value. Acceptance uses the smaller epsilon; violation detection uses the
larger epsilon. The exact acceptance comparisons are
`(1-epsilon)^2 <= P/2^(2F) <= (1+epsilon)^2` in passband and
`P/2^(2F) <= epsilon^2` in stopband. For `A` divisible by 20, epsilon is instead
constructed as an exact rational power of ten. For `A` divisible by 10, the
stopband threshold is exact even if epsilon is irrational.

Reported amplitude-deviation and stopband-dB bounds use directed conversion,
square root, subtraction, logarithm, multiplication, and decimal formatting.
They round outward, not to a nearest display value. The
[MPFR manual](https://www.mpfr.org/mpfr-current/mpfr.html)
specifies the directed-rounding behavior on which these enclosures rely.
No faithful-rounding mode is used. These MPFR operations belong to analysis,
not the signal path.

## Resource contract

| Analyze-only option | Default | Meaning |
| --- | ---: | --- |
| `--certificate-taps` | 1025 | Maximum taps per phase |
| `--certificate-work` | 1000000000 | Deterministic charged work units |
| `--certificate-depth` | 32 | Maximum midpoint depth, permitted range 0..64 |
| `--certificate-endpoint-bits` | 32 | Dyadic endpoint width, permitted range 8..128 |
| `--certificate-integer-bits` | 262144 | Conservative exact-integer width preflight |

These controls require `--certify`; conversion and `plan` reject them. Work is
charged for polynomial construction, Bernstein conversion, control inspection,
and subdivision. It is an algorithmic budget, not elapsed seconds or a count of
CPU instructions. Operand width strongly affects cost. Width preflight includes
coefficient codes, dyadic scale, factorials, maximum subdivision depth, target
precision, and rational-comparison cross products. It is not a hard RSS cap.

The CLI checks planned taps before coefficient design/cache access. Until-40k
therefore returns `Inconclusive(Taps)` immediately with default limits instead
of first materializing its large bank. The library checks each returned phase
before polynomial work. Autocorrelation/conversion/subdivision are quadratic
in tap count; exact integers also grow with tap count and depth. This algorithm
is useful for bounded banks, not yet practical certification of 133733-tap
Until-40k designs. Raising limits is an explicit resource decision, not an
automatic way to obtain a proof.

## Recorded qualification

All values below are outward upper enclosures, not measured maxima. The default
endpoint/depth/work/width controls were used; corrected High raises only the
tap limit to 2049. Reports are retained in `target/continuous-qualified`.

| Bank | Phases / taps | Passband amplitude deviation <= | Stopband <= | Cells / work |
| --- | --- | ---: | ---: | --- |
| Sane 48→16 kHz | 1 / 865 | 1.247087993893623e-6 | -118.3614082418053 dB | 26 / 15735215 |
| Sane 44.1→48 kHz | 160 / 289 | 3.117945932922535e-6 | -120.2500519484514 dB **endpoint only** | 2124 / 195150458 |
| Corrected High 48→24 kHz | 1 / 1109 | 5.026313725757771e-9 | -165.1319921609433 dB | 30 / 28320533 |

Coefficient identities, in table order:

```text
ee56e04e85c075e850b363727d1b7b8c8f93ea954abeb8f4bf9170e45407650e
90515b7fc9fc8be293bc360dc65803fb19629a3d8bea9069679ced22198c8ac2
da8c515195d1feca0a25a62f2beda70ae139221949c89c44dfa8876032e85b10
```

The original High bank, 1025 taps and beta 16, violated the -160 dB passband
target at the exact coordinate `x = 12762330023/137438953472`. Its 65-point
report also found -156.15493 dB stopband rejection. High now reserves 12 dB
design margin without changing the target: design attenuation 172 dB, beta
17.99566, 555 unity-rate taps and 1109 at ratio 1/2. This intentionally changes
High coefficient/output identities; cache keys already include these design
parameters. Regression tests retain both the old failure and corrected proof.

Fourteen certifier unit tests cover independent rational complex summation,
scalar Chebyshev witness evaluation, Bernstein hulls and endpoints, a hidden
interior peak, resource/endpoint uncertainty, partial-bank failure, outward
decimal reporting, and identical exact certificates at F=63 and F=4096. They
passed natively and on AArch64 under QEMU. Sane and corrected High downsampling
CLI reports also match byte-for-byte across these architectures, with each
architecture independently designing coefficients and cache disabled.

## What this does not prove

This is an algorithmic certificate supported by exact arithmetic and tests, not
a formally verified implementation or an exported, independently checkable
proof transcript. Trust includes GMP/MPFR and the implementation above.

It proves amplitude constraints for the actual quantized phases only. It does
not prove MPFR designer calculation error, optimum FIR length, phase/delay
response, complete effect-chain response, finite-stream edge behavior, final
PCM/dither noise, or every preset/ratio. In particular, **upsampling's stopband
here is a single input-Nyquist endpoint**, not the complete periodically
time-varying anti-imaging response. A failed resource budget is not a failed
filter; an exact violation is. The two outcomes must remain distinct.
The separate [`--harmonics` mode](harmonic-response-analysis.md) now measures
every image on an input-frequency grid. It supplements this certificate's
scope without claiming a continuous image-band proof.
