# Window selection and arbitrary-width FIR qualification

`sex`, `sex analyze`, and `sex plan` share `--window`:
`kaiser` (default), `rectangular`, `hann`, `blackman`, or `dolph-chebyshev`.
Unknown, repeated, and valueless options are errors. Same-rate conversion
bypasses the FIR and rejects non-Kaiser window selection; unity-rate analysis
explicitly describes its hypothetical bank.

```sh
# For a 44.1 kHz source; identical window/gate configuration in all three modes.
sex plan input.wav -r 66150 --preset fast --window hann --certify --harmonics
sex analyze input.wav -r 66150 --preset fast --window hann --certify --harmonics
sex input.wav output.wav rate 66150 --preset fast --window hann --certify --harmonics
```

Plan is cheap and unqualified. It performs no coefficient materialization,
cache write, frequency evaluation, or proof. Conversion and analysis run
mandatory quantization/response feedback and optional continuous per-phase
amplitude and sampled all-image gates on the actual selected bank. The window
is never substituted with Kaiser after failure. See
[shared-quality-gates.md](shared-quality-gates.md) for the proof scopes.

## Mathematical and execution contracts

All windows use the exact-rational polyphase geometry and sinc cutoff in
[filter-design.md](filter-design.md). `sexfir::windowed::Spec` and `Designed`
expose arbitrary Q2.C coefficients, MPFR design, a BigQ63 bank, provenance,
ordinary response analysis, continuous certification, and harmonic analysis.
The native Q2.62 window APIs remain available independently.

The non-Kaiser CLI route currently uses GMP execution and a C>=64 coefficient
floor. Higher preset, requested error-floor, or whole-chain numerical budgets
can raise C further; High is not reduced to Q2.62. Design precision must be at
least C+64 and respect the configured MPFR limit. At each candidate and after
every numerical precision change, the planner reserves a Q1.63-input MAC width:

```text
each |sample_raw * coefficient_raw| <= 2^(C+64)
MAC bits >= C + 66 + ceil(log2(T))
```

This covers the full Q2.C coefficient range without assuming a Kaiser-like L1
norm. The bank also calculates its tighter exact sum-of-absolute-coefficients
bound. Runtime signal headroom and fractional width enlarge the accumulator
again. MAC arithmetic is exact, with one final nearest-ties-even rescale per
output sample. No intermediate float or per-product rounding is introduced.

Each phase is DC-normalized in MPFR, quantized nearest-ties-even, and corrected
to an exact integer sum of `2^C`. The correction goes to the last coefficient
having the largest absolute raw magnitude, matching the native window API.
Kaiser's established first-largest rule and coefficient/cache identities are
unchanged. The same exact `(T-1)*2^-C` coefficient-error gate and separately
measured MPFR-reference quantization gate apply. MPFR design error itself is
not claimed to have a rigorous bound.

## Feedback and resources

The preset Kaiser length estimate is only the initial candidate for other
windows. For rectangular/Hann/Blackman, a failed response doubles half-length
`D=(T-1)/2`, preserving the window, target, rolloff, and transition edges.
For Dolph, the same doubling also raises the window attenuation parameter by
12 dB. That parameter is not a claim about achieved FIR rejection. Neither
search is optimal-length design or guarantees success under budgets.
A failed coefficient budget raises actual C by 16 before changing filter shape.

All existing refinement limits apply, including cumulative coefficient and
ordinary response work. The direct Dolph series additionally requires
`L*T*T <= 16,777,216` terms per candidate, checked before allocation and before
cache access. A cache hit does not bypass resource limits or quality gates.
Until-40k can therefore be rejected with this direct Dolph implementation;
there is no downshift to a weaker window, preset, target, or coefficient width.
Rectangular can also exhaust its candidate budget without reaching even Fast.

Cache entries use a distinct `windowed-big-*.sexfir` namespace and kind 3 in
the existing versioned binary container. Request/coefficient identities include
window/algorithm version, Dolph attenuation, exact ratio and rolloff, T, C,
working and accumulator widths, rounding, and DC correction rule. Loading
checks the header, request, size, canonical fixed-width two's-complement codes,
EOF, coefficient SHA-256, exact phase DC sums, and accumulator bound. Writes
are atomic; failed processing preserves the previous destination.

## Dolph fractional-phase correction (v2)

The earlier native Dolph implementation used DFT indices `j=0..N-1` as
positive frequencies when extending the window to fractional coordinates.
Indices above N/2 represent negative frequencies. The expressions agree at
integer positions but differ between them, so a discrete-window-only
regression did not detect the error. Correct interpolation uses `k=j` for
`j<=floor(N/2)`, otherwise `k=j-N`, in `cos(2*pi*k*x/N)`.

Native Dolph's algorithm and identity domain are version 2; the new bigint
Dolph algorithm is also labeled v2. Other native windows and Kaiser retain
their identities. A three-point analytic half-sample regression checks the
signed-bin extension, alongside the existing discrete reference test.
Native and bigint C=62 banks agree coefficient-for-coefficient for every
window, with matching measured responses. C=4096 tests check exact phase DC,
deterministic re-design, cache reload, and an 8192-bit configured MAC.

## Recorded verification

The [recorded cross-architecture run](cross-architecture.md) contains 22 cases,
88 PCM renders, and 16 matching analyses, including four checked window cases.
A subsequent precision-diagnostic-only revision repeats all 300 native tests
and eight uncached window renders on x86-64/AArch64; PCM and full reports remain
identical. Source and executable hashes distinguish those two runs. This is
QEMU qualification, not a claim about untested hardware or continuous images.

## Remaining work

Arbitrary-width global least-squares and Parks–McClellan now have separate
[CLI selection and feedback](optimized-cli.md). Faster long-Dolph
construction, window-specific optimal initial length estimates, continuous
complete-image proofs, and practical long-FIR proofs are separate unfinished
work. This milestone does not complete the project goal.
