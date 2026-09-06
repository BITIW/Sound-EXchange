# Qualified global LS and Remez in the CLI

Conversion, `analyze`, and `plan` share
`--designer windowed-sinc|global-ls|remez`. The default remains windowed sinc
with Kaiser; `--window` selects its window. An optimized designer cannot be
combined with `--window`, including an explicit Kaiser. Same-rate conversion
rejects optimized selection because it has no resampling FIR to execute.
Unity `analyze` instead qualifies a hypothetical bank; its
[single-point stopband](unity-designer-grid.md) is explicitly labeled and does
not imply rejection over a finite-width interval.

For a 44.1 kHz source:

```sh
sex plan input.wav -r 29400 --preset fast --designer remez --grid 9 --certify --harmonics
sex analyze input.wav -r 29400 --preset fast --designer remez --grid 9 --certify --harmonics
sex input.wav output.wav rate 29400 --preset fast --designer remez --grid 9 --certify --harmonics
```

`plan` reports an unqualified initial specification and resource preflight;
it does not solve, read/write the coefficient cache, or claim spectral success.
Analysis and conversion instead share the same whole-chain preparation,
materialization, quality feedback, and accepted bank. Conversion passes that
bank directly to the integer polyphase engine. Their complete bank-bound
qualification blocks include the actual algorithm, spec, solver report,
quantization, requested spectral checks, and coefficient SHA-256.

## Mapping the precision plan to a designer

Both methods use the [global Type-I prototype](optimized-designers.md), not
the legacy independent-phase least-squares reference. For reduced L/M and odd
T taps per phase, `N=L*(T-1)+1` and `A=(N+1)/2`:

| Setting | Global LS | Remez |
| --- | --- | --- |
| Pass/stop weights | 1 / 10 | 1 / 10 |
| Design grid | max(256, 2*A) points per band | density 16 |
| Exchange iteration cap | Not applicable | 64 |
| Coefficients | Arbitrary Q2.C, C>=64 | Arbitrary Q2.C, C>=64 |
| Working precision | Planned MPFR P, at least C+64 | Planned MPFR P, at least C+64 |
| Rounding and DC | Nearest ties-even; last-largest exact phase DC correction | Same |

The preset supplies the exact rolloff, target, and initial T. That initial
Kaiser length estimate is only a heuristic for these methods. Failed actual
coefficient qualification raises C by 16; failed response doubles
`D=(T-1)/2`, so the next T is `4*D+1`. Ratio, target, rolloff, method, weights,
preset, and dither policy remain unchanged. LS design-grid size grows with A;
Remez keeps its density and iteration cap. This is bounded feedback, not an
optimal-length search or a guarantee of convergence.

Every candidate repeats whole-chain numerical planning, including gain,
mixing, DC removal, convolution and signal precision. The Q1.63-input reserve
is at least `C+66+ceil(log2(T))` MAC bits, independent of a Kaiser L1 estimate;
runtime signal range/fractional width enlarges it further. The actual bank
also computes its exact absolute-coefficient-sum bound. Signal execution uses
GMP integer products and sums with one final rescale, never MPFR samples.

The coefficient gate combines the exact corrected-reference budget
`(T-1)*2^-C <= 2^(-B-3)` with measured quantization against the MPFR reference.
Actual-bank phase response is mandatory. `--certify` additionally requires
an exact continuous per-phase amplitude certificate; `--harmonics` requires
the sampled complete-image transfer checks. An inconclusive proof is failure.
Neither the design grid nor an all-image measurement is a continuous image
proof, and MPFR design error does not yet have a certified enclosure.

## Explicit limits and cache behavior

All existing `--refinement-*`, certificate, and harmonic budgets still apply.
The following controls require an optimized designer and positive integers:

| CLI control | Default | Scope |
| --- | ---: | --- |
| `--designer-work` | 4000000000 | Structural work per candidate |
| `--designer-total-work` | 8000000000 | Cumulative candidate work |
| `--designer-storage` | 134217728 | Estimated live solver bytes per candidate |
| `--designer-grid-limit` | 1000000 | Total design-grid points per candidate |
| `--designer-coefficients` | 1000000 | Coefficients per candidate |
| `--designer-precision-limit` | 16384 | MPFR working bits per candidate |

These are structural estimates, not wall-clock or exact process-RSS bounds.
CLI work defaults deliberately exceed the lower library defaults of 67108864
per solve and 200000000 cumulative refinement terms. All applicable caps must
pass; raising one does not bypass the others. Preflight and cumulative charges
precede materialization/cache access, and warm hits incur identical logical
charges. The actual-bank gates run again after a hit.

Remez optionally uses the remaining `--designer-storage` allowance for a
[bit-identical cosine table](remez-cosine-cache.md). The reported core storage
and optional `cosine_cache_bytes` sum to its estimated design workspace. When
the table does not fit, the original calculation remains available with
identical coefficient and solver results.

The [kind-4 persistent cache](coefficient-cache.md#optimized-global-ls-and-remez-cache)
retains the complete solver and numerical reports. Corrupt entries fail
explicitly without silent replacement. A solver, resource, or quality failure
does not select Kaiser, weaken the target, or overwrite the previous audio
destination. [Numerical solver recovery](solver-precision-recovery.md) doubles
automatic working P for selected typed LS/Remez failures within all existing
caps. Explicit P is pinned; coefficient-budget retries are a different operation.

## Verification scope

Real refinement tests force both C growth and FIR growth for both methods,
check full cumulative work, and reject wrong-method banks. CLI tests match
standalone analysis to executed PCM at blocks 1/7/4096, including gain 2^60,
normalization, continuous certificates, image checks and warm-cache failures.
They also verify invalid option combinations, cheap blocked Until-40k plans,
same-rate rejection, and preservation of existing output.

Cross-architecture execution identities and test results are recorded in
[cross-architecture.md](cross-architecture.md). These finite cases do not
qualify every preset/ratio. Scalable very-long global solvers, broader
solver-recovery qualification, continuous complete-image bounds, practical long-FIR
proofs, and physical-platform qualification remain open. The full goal is not
complete.
