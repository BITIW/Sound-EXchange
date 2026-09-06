# Unity-ratio designer grids and Nyquist endpoint semantics

An optimized global prototype has stop edge `min(1,L/M)/(2*L)` and final
frequency 1/2. At reduced ratio L/M=1/1, those edges coincide: the stopband is
a single Nyquist endpoint, not an interval. The designer now gives that endpoint
one grid sample with its requested weight.

Previously the shared global-LS/Remez grid repeated it `points_per_band` times.
For LS this multiplied the endpoint weight by the sampling density. For Remez,
the duplicate-frequency errors generated artificial extrema and tripped the
selector safety cap. The reproduced Fast unity analysis failed before producing
a bank: 137 local extrema exceeded the limit of 136. Raising the cap or MPFR
precision would not correct the underlying grid geometry.

## Grid, work and exchange contracts

For a unity grid with K passband points, total grid size is now K+1, with
exactly one stop point. Other reduced ratios retain their previous two bands
and 2*K grid points. Native Remez and arbitrary LS/Remez preflight use the same
count as actual construction; work/storage estimates are recalculated before
allocation and cache access. Resource limits are not relaxed.

Remez initialization reserves its last reference for the isolated stop point.
The remaining E-1 references span the passband, including both edges, without
duplicate indices. Index interpolation uses widened integer multiplication.
The subsequent signed-extrema selection, alternation checks, solved-ripple
convergence criterion, weights and iteration cap are unchanged.

## Independently known three-tap solutions

For a three-tap Type-I unity prototype, write the amplitude as
`a0 + a1*cos(2*pi*f)`, with raw FIR coefficients `[a1/2, a0, a1/2]`.
Use rolloff 3/4 and equal weights: passband [0,1/4], stop point 1/2.

- Continuous minimax has `a0=3/4`, `a1=1/2`, weighted ripple 1/4. After the
  established exact-DC normalization its FIR is `[1/5, 3/5, 1/5]`.
- Discrete LS on pass points {0,1/4} and one stop point {1/2} has diagonal
  normal matrix diag(3,2), RHS [2,1], and `a0=2/3`, `a1=1/2`. DC normalization
  gives `[3/14, 4/7, 3/14]`. Repeating the stop point changes this solution.

Tests quantize these rational solutions independently, including final DC
correction, and compare actual coefficients. Native and arbitrary C=62 Remez
also match. LS is checked at C=62 and C=96. Other tests check strict grid order,
reference endpoint inclusion, total counts and failure under a smaller grid cap.

## Versioning and user-visible scope

Only unity optimized algorithm domains change:

| Method | Unity identity |
| --- | --- |
| Global LS bigint | `least-squares-global-lowpass-big-unity-v2` |
| Remez bigint | `parks-mcclellan-lowpass-big-unity-v3` |
| Native Remez | `parks-mcclellan-lowpass-unity-v3` |

Non-unity global LS v1 and Remez v2 identities and calculations are retained.
Unity kind-4 cache requests include the new algorithm domain, so old repeated-
endpoint banks cannot be mistaken for corrected banks. Legacy independent-phase
native LS already handled the isolated endpoint and is unchanged.

`sex analyze input.wav -r 44100 --preset fast --designer remez --grid 9
--certify --harmonics` now succeeds for a 44.1 kHz input, as does global LS.
Both accept T=129, C=64, P=128 without numerical retry. Remez has 1057 design
points and converges in 16 iterations; LS has 257 design points. Cold/warm
analysis reports bind the same corrected bank and repeat every requested gate.

Unity analysis remains explicitly hypothetical. Same-rate conversion still
bypasses the resampler and rejects explicit optimized designer selection; this
change does not secretly introduce a low-pass stage into copy operations.
The qualification report explicitly labels the stopband as the Nyquist
endpoint only. Its continuous stopband bound covers that singleton, not a
finite-width anti-aliasing band or PCM noise floor. L=1 also has no separate
image components. See [cross-architecture.md](cross-architecture.md) for recorded
tests and scope; the full project goal remains incomplete.
