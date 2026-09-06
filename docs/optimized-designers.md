# Arbitrary-width global LS and Remez designers

`sexfir::optimized` separates MPFR filter construction from integer execution.
`Method::GlobalLeastSquares` and `Method::Equiripple` produce Q2.C coefficients
directly, not by promoting an already rounded Q2.62 bank. The
[CLI integration](optimized-cli.md) now combines these APIs with whole-chain
precision planning and bounded coefficient/shape feedback. Broad preset/rate
qualification and scalable very-long global solves remain open.
Persistent bank/solver-report reuse and cached coefficient-precision search are
available through `cache::design_optimized_big_cached` and
`cache::refine_optimized_quantization_cached`. The
[cache contract](coefficient-cache.md#optimized-global-ls-and-remez-cache) specifies
checksums, validation, limits and trust assumptions.

## Global prototype and quantization

For exact reduced ratio L/M and odd T taps per phase, both methods construct
one Type-I prototype of length N=L(T-1)+1. Let A=(N+1)/2. Its zero-phase
amplitude is `a[0] + sum_(j=1..A-1) a[j] cos(2*pi*j*f)`. The middle coefficient
is a[0]; symmetric coefficients at offsets +/-j are a[j]/2. Phase p gets
`h[L*k+p]`, with out-of-support positions exactly zero. Input delay is (T-1)/2.

With `nyq=min(1,L/M)/2`, the prototype pass edge is `(2*r-1)*nyq/L`, stop
edge `nyq/L`, and final stop frequency 1/2. Desired prototype pass gain is L.
The configuration uses rational band edges and positive rational weights;
MPFR approximates trigonometric values only in the designer/measurements.
At unity ratio the stopband collapses to the Nyquist endpoint. The corrected
[unity grid](unity-designer-grid.md) includes that point once, with its requested
weight, and uses unity-specific versioned identities; non-unity grids are unchanged.

Each decomposed phase is normalized by its MPFR DC sum, quantized with nearest
ties-to-even, then corrected to exact integer DC sum 2^C. The residual goes
to the last largest-magnitude raw coefficient. The actual corrected bank is
range-checked, accumulator-checked and hashed. Phase normalization and integer
correction can change the original global optimum: all post-design analyses
therefore operate on the corrected bank, not its prototype.

The identity includes the algorithm/domain version, rational ratio and bands,
weights, grid/iteration settings, working and coefficient precision, accumulator
width, rounding/DC policy, delay and length-prefixed signed integer coefficients.
Resource limits affect acceptance but not coefficient identities.

## Global least squares versus the legacy reference

Global LS minimizes `sum_f w(f)*(A(f)-desired(f))^2` on the recorded discrete
grid. Define cosine moments `C[k]=sum_f w(f)*cos(2*pi*k*f)`. Then

```text
R[j,k] = (C[abs(j-k)] + C[j+k])/2
b[j]   = sum_pass w(f)*L*cos(2*pi*j*f)
R*a = b
```

An MPFR Cholesky solve rejects non-positive pivots. The reported maximum
`|R*a-b|` is a finite-precision residual before phase normalization, not a
condition-number estimate, continuous LS optimum, or certified MPFR error.
The algorithm identity is `least-squares-global-lowpass-big-v1`.

The older native `design_least_squares` independently fits fractional-delay
phases and remains explicitly a reference API. Those independent phases do
not constrain their combined transition-band images. The regression at L/M=2/3,
T=31, r=3/4, weights (Wp,Ws)=(1,10), 256 points per band and P=160 passes a continuous
per-phase amplitude certificate at 40 dB, yet its sampled image near input
frequency 47/192 reaches approximately -27.77377 dB. Global LS at the same
geometry passes the complete-image grid. This is a changed mathematical method,
not an alleged bit-identical replacement of legacy LS.

## Remez v2 and the repaired exchange regression

Remez uses the shared MPFR interpolation/exchange implementation, with
algorithm identity `parks-mcclellan-lowpass-big-v2` (native Q2.62 uses
`parks-mcclellan-lowpass-v2`). Its report records iterations,
extremal frequencies and pre-normalization weighted prototype error. Small C=62
fixtures match the native implementation in raw coefficients, exchange reports,
quantization measurements and actual-bank responses.

The previous version failed at L/M=2/3, T=129, r=9/10, weights (Wp,Ws)=(1,10),
density 16 and 64 allowed iterations. At P=160 it exhausted iterations;
P=512 instead reported only 129 of 130 required extrema. Remez v2 solves the
same geometry at both precisions in 12 iterations. The resulting C=96 raw
coefficients agree between precisions and pass the unchanged 80 dB target:
sampled phase response, continuous phase amplitude certificate, coefficient
budget and sampled complete-image transfer. The continuous stopband upper
bound is -106.8464251509297 dB; sampled image peak is -140.0847775290125 dB.

Two changes repair the exchange. First, initial reference spacing uses the
combined physical width of the two bands, excluding the transition gap. The
fixture starts with 37 references in the passband and 93 in the stopband,
instead of 65 in each merely because both grids have the same point count.
Second, signed extrema preserve single-point sign lobes. For example, errors
`[1,-2,3,-4,5]` contain five alternating extrema, although an absolute-magnitude
neighbor comparison discards the middle three. The diagnostic run reached
129 alternating sign changes across its 130 reference errors, yet the old
absolute-peak selector returned only 129 candidates. The selector, not the
requested attenuation, had lost necessary reference points.

Convergence is also stricter: reference errors must alternate and all lie
within relative tolerance `2^(-P/3)` of the positive solved ripple delta,
while the maximum across the complete design grid must not exceed the upper
tolerance. An unchanged set alone is not success. The native and arbitrary
coefficient identities are versioned because initialization, candidates and
stopping behavior changed. Global LS, window and Kaiser identities are untouched.

These are discrete-grid convergence checks, not a continuous minimax-optimality
proof. The underlying solve remains MPFR Gaussian elimination. Very large or
ill-conditioned designs and more scalable interpolation/exchange remain open.
The [whole-quality refiner](solver-precision-recovery.md) now provides bounded
automatic P retries for selected typed numerical errors; direct `design` stays
a single exact-spec attempt. Background on the alternation
condition and initialization difficulties is in Silviu Filip's
[primary seminar notes](https://www.lip6.fr/public/2015-03-02_Filip.pdf),
slides 7 and 11; the barycentric and reference-scaling methods discussed there
are not claimed as implemented here.

## Resource contracts

`Spec::preflight` validates configuration and bounds coefficient count, grid,
structural work, estimated MPFR/GMP storage and precision before allocating a
grid, solver matrix or coefficient bank. Defaults are 1,000,000 coefficients,
1,000,000 grid points, 67,108,864 work units, 128 MiB estimated storage and
16,384 working bits. Working precision P must be at least C+64; this is a
minimum design margin, not a bound on an ill-conditioned solve's error.

LS reserves its global A-by-A solve, cosine moments, RHS and residual work,
not T-by-T independent phase solves. Remez reserves its E=A+1 interpolation
solve for every allowed iteration and both error evaluation and extrema
selection. Its selector has an explicit cap K=2E+4 local candidates. Rounded
plateaus can exceed this cap; they cause a typed error before dynamic-programming
table allocation, never truncation of excess candidates. The estimate includes
E*K^2 predecessor examinations, (E+1)*K MPFR score slots and predecessor storage.
V2 also charges initialization and convergence scans. The native and arbitrary
APIs share the same exchange-work estimator. For E references, G design-grid
points and I allowed iterations, its work is
`I*(E^3+E^2+2GE+8G+8E+E*(2E+4)^2) + 4G+12E`, plus `4LT` for quantization.
Dynamic-programming score addition and tie ordering remain unchanged; signed
candidate extraction does change. Corrected native budgeting can reject banks
that previously fit only because selector work was omitted. For example, ratio
2/3 with T=65 requires 115,772,704 structural units, above the native fixed cap;
the arbitrary-width API permits an explicit larger limit.

These are conservative structural estimates, not timeouts, OS memory limits,
or precision-independent bounds on transcendental cost. Large global solves
can require explicitly raised limits and may still fail numerically.
Remez can now reuse a [bit-identical in-memory cosine table](remez-cosine-cache.md)
when the remaining storage allowance fits it. `Work.storage_bytes` is the
uncached core and `Work.cosine_cache_bytes` is optional planned workspace;
their sum stays within the cap. Failure to fit the optional table selects the
unchanged direct path, not a weaker filter. Logical work and persistent
coefficient identities remain unchanged.

## Precision search and qualification

`refine_quantization` re-solves the actual MPFR problem after each failed
coefficient-error gate. It grows C and accumulator width by 16 bits, preserving
method, length, bands, weights, grid and iteration cap. Automatic working
precision raises P to at least C+64 in 32-bit steps; explicit P is not silently
changed. A solver failure stops immediately, rather than triggering a retry.
Attempts and aggregate estimated work/coefficient counts are bounded (defaults:
8, 200 million work units, 4 million coefficients).

For the target's conservative amplitude bit requirement B, the coefficient gate
requires `(T-1)*2^-C <= 2^(-B-3)` and checks the measured per-phase L1 error
against its MPFR reference. The algebraic budget is evaluated exactly for a
unit-DC reference; the MPFR-reference error is a measurement, not an
outward-rounded enclosure. Neither certifies reference-design error or
ideal-filter approximation. The returned search
result is coefficient-qualified only: tests deliberately use short filters that
pass this gate after C=8 -> 24 but still fail the 80 dB spectral target.

Separate actual-bank APIs provide sampled per-phase response, exact continuous
per-phase amplitude certificates, and sampled complete-image transfer analysis.
An inconclusive certificate is not success. Complete-image grids are not
continuous image proofs. CLI/whole-chain integration requires its chosen
spectral gates; coefficient-only search success is not treated as filter quality.

## Execution and verification

The resulting bank uses the existing GMP one-round MAC and rational polyphase
streaming engine. Tests cover real C=4096 coefficients with an 8192-bit configured
accumulator, exact DC, silence, signed full-scale/alternating samples, impulses,
tiny raw codes, independent integer-MAC rounding/saturation oracles and Q65.160
stereo streams at chunks 1/3/4096. The extra coefficient and signal low bits are
checked to ensure this is not Q2.62/Q1.63 promotion.

`cargo run --release --example optimized_probe` is a fixed, reproducible library
corpus: thirteen returned banks, thirty-nine chunked integer-stream renders and
five qualified banks. Two T=31 banks use the 40 dB target; global LS and the
repaired Remez at P=160/P=512 use T=129, Fast's r=9/10 geometry and an 80 dB
target, with a 257-point-per-band phase grid, continuous amplitude certificate and
65-point-per-band complete-image grid (193 distinct input frequencies). It prints full coefficient identities, reports,
raw output integers. It is not a new CLI mode or PCM/cache test.

With `-- --cache-directory PATH`, the same example materializes all banks and
every coefficient-search candidate through the persistent cache. A second run
uses hits but still repeats the actual-bank response, certificate and image
checks. Cache mode preserves every numerical report and raw stream; it does
not qualify production CLI selection or PCM export for these methods.

The 2026-09-06 v2 corpus completes with byte-identical full output on x86-64 and
AArch64 QEMU. All 317 native all-target tests and 17 optimized-library tests on
AArch64 pass, along with warnings-denied Clippy on both architectures. Build,
source and output hashes are recorded in
[the v2 qualification](cross-architecture.md#2026-09-06-remez-v2-regression-repair).

The historical 2026-09-05 v1 native/AArch64-QEMU run completed with byte-identical full output
and the same explicit failure. All 313 native release all-target tests and
13 new AArch64 library tests pass, as do warnings-denied Clippy checks on both
architectures. Exact builds, output hashes and scope are recorded in
[cross-architecture.md](cross-architecture.md#2026-09-05-arbitrary-width-global-designer-library-qualification).
