# Complete sampled harmonic-transfer analysis

`sex analyze --harmonics` examines all output frequency components of the
actual quantized polyphase bank, not just individual phase magnitudes. The
analysis samples the entire input-frequency interval, including transition
and stopband. It reports wanted-component gain/phase error, the strongest
image, aggregate image power, and stopband leakage with exact input/output
frequency coordinates.

```text
sex analyze input-44k1.wav -r 48000 --preset sane --harmonics
sex analyze input-44k1.wav -r 48000 --preset high --harmonics --harmonic-work 500000000
sex analyze input-48k.wav -r 96000 --preset until-40k --harmonics --harmonic-work 500000000
```

The last two examples explicitly permit more analysis work. All measured
harmonic gates must meet the planned preset/error-floor target for a successful
exit. The mandatory [feedback loop](quality-refinement.md) may strengthen a
failed candidate and remeasure it, without weakening the target. Resource
exhaustion or failure to find a qualifying candidate returns nonzero; no partial
harmonic quality pass is published. Per-phase and coefficient-budget checks
also apply to implicit preset targets. `--harmonics --certify` can request both analyses;
their scopes and reports remain separate.
These options now also apply to conversion through the
[shared gate configuration](shared-quality-gates.md). For example,
`sex input.wav -r 48000 output.wav --preset sane --harmonics` requires the
sampled harmonic checks before using the accepted bank. `plan` accepts the
same settings but only reports initial costs/static limits, never response
compliance or a proof.

## Why phase magnitudes are insufficient

Even two phase filters with identical magnitude can have different delays.
Switching between them can produce images, although each individual magnitude
plot looks correct. For example, a 2x zero-order hold has two one-tap phases,
both equal to one. Each phase magnitude is identically one; nevertheless its
wanted and image powers are both 1/2 at input Nyquist.

The relation between polyphase rate conversion, interpolation images, and
periodically time-varying systems is standard multirate theory; see
[Vaidyanathan's tutorial](https://www.systems.caltech.edu/dsp/ppv/papers/ProcIEEEmultirateTUTExtra.pdf)
and the [official `upfirdn` documentation](https://www.mathworks.com/help/signal/ref/upfirdn.html).
The derivation below uses SeX's actual tap order and delay-compensated clock;
it is also checked against its fixed-point streaming implementation.

## The engine's frequency model

Let `L/M` be the reduced output/input ratio, `D` the integer input delay used
by the clock, and `h[p,k]` the stored real coefficients. The timeline supplies
newest-to-oldest input samples:

```text
p_n = n M mod L
q_n = D + floor(n M/L)
y[n] = sum_k h[p_n,k] x[q_n-k].
```

For a unit complex tone `x[j] = exp(2 pi i f j)`, define

```text
H_p(f) = sum_k h[p,k] exp(-2 pi i f k)
B_p(f) = exp(2 pi i f (D-p/L)) H_p(f)
C_r(f) = (1/L) sum_(p=0..L-1) B_p(f) exp(-2 pi i r p/L).
```

Fourier reconstruction of the periodic phase sequence gives

```text
y[n] = sum_(r=0..L-1) C_r(f) exp(2 pi i (f+r) M n/L).
```

`r=0` is the wanted component. The remaining `L-1` components are images,
including those folded back into the output passband after decimation. Each
signed output frequency is computed as an exact rational:

```text
v_r = (f+r) M/L
output_frequency_r = v_r - floor(v_r+1/2), in [-1/2, 1/2).
```

Because `L` and `M` are coprime, the `L` image coordinates are distinct modulo
one for a fixed complex input tone. Labels are image indices, not phase indices
or ascending output-frequency order.

An independent equivalent form uses the interleaved global coefficient vector
`g[Lk+p] = h[p,k]`, including every actual per-phase normalization and DC
correction:

```text
C_r(f) = exp(2 pi i f D) G((f+r)/L) / L.
```

Tests evaluate this sum directly, without the phase-Horner/DFT implementation.
This reconstruction is an analysis identity only: SeX does not insert `L-1`
zeros and perform a literal high-rate convolution in its signal path.

## Measurements and units

Let `v=min(1,L/M)` and `r` now denote the configured rolloff. The wanted
component is tested on `0 <= f <= v(2r-1)/2`. Input stopband is
`v/2 <= f <= 1/2`. Images are examined over **all** `0 <= f <= 1/2`, including
the transition between these bands. Negative input frequencies follow by
conjugate symmetry of the real coefficient bank.

`--grid G` contributes equally spaced exact rational points to passband,
transition, and stopband; duplicate endpoints are removed. With ordinary
nonzero transition/pass widths this is `2G-1` input frequencies for upsampling
and `3G-2` for downsampling. Every one of the `L` components is evaluated at
each input frequency. There is no selection of a few favored images.

Reported quantities are:

- main passband ripple: `20 log10(max |C_0| / min |C_0|)`;
- main complex error: `20 log10(max_pass |C_0-1|)`, including residual phase
  error after the engine's actual delay compensation;
- maximum absolute passband phase error: `max_pass |arg(C_0)|` in radians;
- image peak: `20 log10(max_(f,r>0) |C_r(f)|)`;
- image L2 peak: `20 log10(max_f sqrt(sum_(r>0) |C_r(f)|^2))`;
- stopband harmonic peak: `20 log10(max_(f in stop,r>=0) |C_r(f)|)`.

The complex error, individual image peak, image L2 peak, and stopband component
peak each have their own target gate. In single-phase conversion there are no
separate image components; image metrics are explicitly not applicable, not
invented numerical values. A passband zero makes ripple/phase reporting
undefined and fails the complex-error gate.

Image L2 is the unwanted-component RMS gain for a unit complex input tone
averaged over its demodulated phase cycle. Parseval supplies an independent
check: `sum_r |C_r|^2 = (1/L) sum_p |B_p|^2`. This is not a PCM SNR or a
worst-case arbitrary-input induced norm. The stopband metric is a component
maximum, not an aggregate stopband norm. Real tones combine positive/negative
complex responses; colliding output components must be combined before
interpreting a real waveform's power.

## Numeric and resource contract

Configuration and all frequency coordinates are rational. Coefficient codes
convert exactly from their declared Q format to MPFR; a coefficient that cannot
be represented exactly at the selected working precision is rejected. Native
Q2.62 and arbitrary-width Q2.n banks use the same harmonic algorithm. Tests
include coefficients retaining information below the Q*.63 grid at F=4096.

Each phase response uses complex Horner evaluation. Its exact delay/phase
coordinate supplies the phase alignment. A second complex Horner evaluation
implements every DFT component with precomputed roots; no platform-native
float or SIMD transform substitutes for it. MPFR operations use the recorded
designer precision and nearest rounding. Phase arguments are reduced modulo
one in rational arithmetic before conversion, including very large delays.

Unlike the continuous amplitude certifier, this module does **not** use
directed interval arithmetic. Its decimals are measurements, not outward
enclosures. The coefficient representation is exact, but trigonometry and
response evaluation still have finite analysis precision.

| Analyze-only control | Default |
| --- | ---: |
| `--harmonic-work` | 100000000 |
| `--harmonic-coefficients` | 1000000 |
| `--harmonic-phases` | 1024 |
| `--harmonic-precision-limit` | 16384 bits |
| `--harmonic-storage-bytes` | 134217728 bytes |

Controls require `--harmonics`; conversion and `plan` reject them. Work preflight
uses at most `9 N (L T + L^2 + 4L)` units, where `N` is the input-grid size
bound. These are deterministic weighted complex-operation units, not wall time
or CPU instructions. Operand width also affects runtime. The storage preflight
estimates retained MPFR payloads/objects; it is not a hard process RSS cap and
does not include the already-existing designer bank or arbitrary memory inside
a caller-supplied coefficient callback.

CLI preflight precedes coefficient materialization/cache access. Huge banks
therefore fail cheaply under the default limits. Raising a work or storage
limit is explicit. The algorithm has `O(N (L T + L^2))` analysis work and
`O(L T + L)` retained numeric values, not an exact high-degree polynomial or a
zero-filled high-rate audio buffer.

## Tests and scope

Eleven unit tests cover exact folded frequency coordinates, an independent
global-prototype sum, Parseval, zero-order-hold images, exact per-phase DC
sums, sub-Q*.63 coefficients, complete band grids, undefined zero-bank metrics,
malformed coefficient shapes, and pre-allocation resource/overflow checks.

The execution oracle supplies separate exactly representable cosine/sine
streams to the real Q1.63 resampler at ratios 1/3, 2/1, 3/2, 5/7, and 7/4.
All reconstructed output components match the settled fixed-point outputs at
block sizes 1, 7, and 4096. The independent global-prototype oracle also covers
`u64::MAX` input delay. CLI tests exercise simultaneous harmonic/certificate
reporting and rejection before cache creation.

This is the linear transfer defined by the quantized coefficients. It excludes
per-MAC signal rounding, finite-file edges, generic effects, final PCM/dither/
clipping, and MPFR designer calculation error. These have separate numerical
and execution tests. Sampling more points does not prove unsampled extrema;
continuous certification of complete image bands and practical long-FIR proofs
remain unfinished. Neither this mode nor a per-phase amplitude certificate
alone is an unrestricted end-to-end quality guarantee.

## Reproducible campaign

```text
cargo build --release --bin sex
scripts/validate-harmonic-responses.sh target/release/sex NEW_RESULT_DIRECTORY
SEX_HARMONIC_UNTIL=1 scripts/validate-harmonic-responses.sh target/release/sex NEW_RESULT_DIRECTORY scripts/cross/aarch64-sex.sh
```

The optional second executable may be an independently built AArch64/QEMU
wrapper with the [documented isolated toolchain](cross-architecture.md).
The harness checks five Sane ratios (1/3, 160/147, 147/160, 479/441, 1/48),
High at 160/147 and 1/2, and optionally Until-40k at 2/1. It raises work to
500000000 units, preserves exact commands and full reports, designs independent
caches, and compares complete reports/cache files when a second executable is
provided. It also rejects a changed source tree during the run. The fixture
files are tiny: only their sample-rate metadata is needed for this analysis.

## Historical harmonic-only native measurements

The run in `target/harmonic-qualified/matrix` uses `G=65`: 129 frequencies
for upsampling and 193 for downsampling. Every table entry passed its four
sampled gates. Values below are rounded measurements in dB, not continuous
upper bounds. The full logs retain additional digits and exact peak locations.

| Preset / ratio | Target | Main complex error | Image L2 peak | Stop component peak |
| --- | ---: | ---: | ---: | ---: |
| Sane 1/3 | -110 | -125.859049 | N/A | -126.288441 |
| Sane 160/147 | -110 | -125.943756 | -126.469414 | -126.470593 |
| Sane 147/160 | -110 | -125.970065 | -135.877799 | -126.498563 |
| Sane 479/441 | -110 | -125.945863 | -126.472678 | -126.473851 |
| Sane 1/48 | -110 | -125.936577 | N/A | -126.459355 |
| High 160/147 | -160 | -173.719989 | -173.011477 | -173.011855 |
| High 1/2 | -160 | -173.378396 | N/A | -173.199210 |
| Until-40k 2/1 | -600 | -602.278914 | -602.260765 | -602.260765 |

For Sane 147/160, the aggregate-image peak occurs inside the input band at
`5107/10240`, not at Nyquist. Until-40k really materializes two phases of 66867
taps with 512 fractional coefficient bits and MPFR precision 1024. Its 129-point
analysis charges 155279106 work units. Its coefficient identity is
`587e66e40462871465e131673154f7ce016b717df375d49c1ed72417caaebfd8`.
This new upsampling bank is distinct from the earlier 1/2 Until-40k execution
qualification and does not inherit a continuous certificate from it.

These records predate mandatory implicit per-phase checks. The harmonic-only
Until-40k pass did not establish a -600 dB bound for each individual phase.
The later [shared-gate execution](shared-quality-gates.md) finds the 66867-tap
bank's per-phase stop peak at -596.240165 dB and automatically strengthens it
to 68197 taps at the same target. Its 9-point grid reports -617.025532 dB
per-phase stop and -618.121795 dB image-L2 peak. Those are newer sampled data,
not continuous bounds and not a retroactive change to the historical 65-point
harmonic-only campaign above.

## Cross-architecture result

All eight cases passed on both x86-64 Linux and AArch64 Linux under QEMU.
Complete analysis reports, including peak labels, exact frequency coordinates,
rounding-sensitive decimals, and coefficient identities, match byte-for-byte.
The independently designed complete cache directories also match. The corpus
occupied about 23 MiB, mostly coefficient caches, not generated audio.

The [durable manifest](harmonic-response-2026-09-05.tsv) records case name,
complete first-report SHA-256, and coefficient SHA-256. Its source fingerprint
is `9e6688fc4943a4df35407a83ba4035be5a5431c1f9d2a9e0fb0c90bc4c39d1b3`.
Executed binary SHA-256 values:

- x86-64: `9e173481605be769a3c16a999623549463d02351939c8a6371d2d8bdd4e8ef73`;
- AArch64: `86eb41a3081b741372caccd576a79152063153ce7a18ac3d21782d9ef61cd53e`.

Both all-target Clippy checks passed; 260 native all-target tests and all eleven
new harmonic tests on AArch64 passed. The existing emulated ARM linker/hardware
limitations in [cross-architecture.md](cross-architecture.md) still apply. This
campaign verifies analysis and coefficient generation, not a new set of encoded
audio outputs or physical ARM hardware.
