# Precision planner and quality presets

Status: normative policy contract for native and arbitrary-width execution.

## Preset definitions

The names are ordered engineering policies rather than marketing aliases. The
transition width is expressed as a fraction of the lower input/output Nyquist;
the tap count shown is for ratios at or above unity. Downsampling scales the
half-length by the exact reduced `down/up` ratio.

| Preset | Transition | Stopband target | Taps/phase | Coefficient fractional bits | Accumulator | MPFR work bits | Final dither policy |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `fast` | 1/5 | -80 dB | 129 | >=40 | >=128 | >=128 | TPDF |
| `sane` | 1/10 | -110 dB | 289 | >=62 | >=128 | >=192 | high-pass TPDF |
| `high` | 1/20 | -160 dB | 555 | >=96 | >=192 | >=256 | fifth-order noise shaping |
| `absurd` | 1/40 | -240 dB | 2049 | >=160 | >=384 | >=384 | ninth-order noise shaping |
| `pointless` | 1/80 | -360 dB | 4097 | >=256 | >=640 | >=512 | ninth-order noise shaping |
| `until-40k` | 1/500 | -600 dB | 66867 | >=512 | >=1024 | >=1024 | ninth-order noise shaping |

These are minimums. A more demanding explicit error floor can only increase
precision and length. Plans fitting Q2.62 and 128 accumulator bits use the
native kernel. Every wider plan is materialized as Q2.n and executed with the
configured GMP accumulator width; it is never silently weakened to Q2.62/i128.
These accumulator entries describe the Q1.63-input filter contract. CLI
processing extends the FIR accumulator plan by `M + F - 64` bits for a Qm.n
signal with `M` integer and `F` fractional bits. At Q65.63 with native
coefficients, execution tries checked i128 and recomputes an overflowing dot
product exactly in GMP. All other signal formats use the arbitrary bank.
Conversion reports both the extended planned width and the minimum width
proved from actual signed coefficient/input intervals. The baseline reserve
is conservative; bank construction checks the exact range separately, including
the Q65.63 fallback domain. Signal format changes do not change
the coefficient cache key; filter-design configuration still controls that key.

## Exact planning rules

The request contains a reduced rational rate, preset, optional integer error
floor such as `-300dB`, and optional MPFR working precision. No configuration
value is parsed through a binary float.

For an attenuation `A`, amplitude bits are calculated conservatively as:

```text
amplitude_bits = ceil(A * 1,000,000,000 / 6,020,599,913)
```

The denominator represents a deliberately low rational approximation of dB per
bit, so this calculation cannot round the required bit count down. The planner
reserves `ceil(log2(taps_per_phase)) + 1` further fractional bits for the sum of
coefficient quantization errors and exact DC correction.

When an explicit target is stricter than the preset, or the preset is
`sane`, `high`, or `until-40k`, the Kaiser design receives 12 dB of additional design attenuation.
Sane's original beta-10 / 257-tap candidate failed its -110 dB floor between
the old 129-point grid samples: a 513-point 48-to-16 kHz analysis measured
-106.34696 dB stopband peak and 4.71621e-6 maximum passband deviation.
The corrected implicit and explicit -110 dB plans use design attenuation
122 dB, beta 12.48566, and 289 taps at unity (865 at ratio 1/3).
A regression preserves the old failure and requires the corrected bank to
pass all three gates on 1025 points/band at ratios 1/3, 3/2, and 1/48.
This changes Sane output/coefficient identities; cache keys already include
the changed design parameters and cannot reuse the old coefficients by mistake.

High's original beta-16 bank also violated its target: at ratio 1/2 the exact
continuous certifier found a passband violation and the response grid measured
-156.15493 dB stopband rejection against the -160 dB requirement. High now uses
design attenuation 172 dB, beta 17.99566, 555 taps at unity and 1109 at 1/2.
The corrected 1/2 bank has a continuous stopband upper bound of -165.13199 dB;
this is an actual quantized-bank certificate, not merely design margin. See
[continuous-response-certificate.md](continuous-response-certificate.md).

Until-40k therefore targets a 612 dB design even with its implicit 600 dB
acceptance floor. Its original 65537-tap/beta-65 policy missed that floor on a
48-to-24 kHz sampled qualification (-599.50 dB); the corrected policy must pass
the same unweakened response gate. The half-length grows in proportion to
`(design_attenuation - 8)`, and Kaiser beta uses the high-attenuation rational
approximation `0.1102 * (A - 8.7)`. This margin is not treated as proof: the
quantized filter is subsequently checked against the requested floor.

An explicit `--precision` is allowed to raise MPFR precision but cannot undercut
the calculated minimum. The selected coefficient width determines whether the
plan uses native i128, the planner's bounded-wide category, or bigint. The
current executor implements both wider categories with the same exact GMP
kernel; a future native 256-bit optimization may replace that implementation
only if it remains bit-identical.

## Signal precision and effect-chain bounds

`plan_signal_precision` accepts the filter plan, rounded-stage count `S`, a
conservative downstream rounding-error amplification bound `2^G`, a minimum
integer width, and an optional explicit fractional width. Its initial floor is:

```text
F = max(63, filter_coefficient_fractional_bits,
        amplitude_bits + ceil(log2(max(1, S))) + G + 2)
```

The CLI bounds FIR magnitude by twice the tap count, gain by a rounded-up
integer ratio, and each mix/convolution row by the sum of rounded-up coefficient
magnitudes. Recursive rounding error for a DC pole `R` includes a conservative
`2/(1-R)` factor; its signal peak uses the DC blocker's L1 bound of 2 instead.
Bounds are propagated using integer bit counts. The minimum integer width is
65, raised when the conservative peak calculation requires more headroom.

Typical automatic signal fractional widths for fast/sane/high/absurd/pointless/
until-40k are 63/63/96/160/256/512. `--signal-precision BITS` may raise but never
lower the automatic minimum; `auto` is the default. Total signal width is capped
at 1,048,576 bits as a resource guard. At F=63, effect coefficients retain the
baseline 62 fractional bits; otherwise they are quantized directly from the
requested rationals at F fractional bits. No pre-rounded Q2.62 coefficient is
promoted and mistaken for a high-precision design.

The CLI now checks this initial candidate with a coupled numerical budget.
Reference peaks and three error components (stage rounding, effect coefficient
quantization, and FIR coefficient quantization) propagate through the actual
effect order using outward-rounded integers. Recursive DC forcing uses the
exact quantized pole gap. Signal and FIR coefficient widths grow independently
until the total bound is at most `2^(-amplitude_bits-2)` FS. Increased FIR width
also updates MPFR precision, accumulator width, backend selection, and the
coefficient cache key. An explicit width cannot undercut the refined plan.

This bounds computation relative to exact rational effects, exact-DC-corrected
MPFR FIR coefficients, and the same chosen normalization scale. It excludes
ideal-filter approximation, MPFR design error, and final PCM/dither. The exact
reference, equations, and qualification are in
[numerical-error-budget.md](numerical-error-budget.md).

## CLI behavior

```text
sex input.wav -r 48000 --preset fast output.wav
sex input.wav -r 48000 --preset sane --error-floor -300dB --precision 256 output.wav
sex input.wav -r 48000 --preset high --signal-precision 4096 output.wav
sex plan input.wav -r 48000 --preset until-40k
sex plan input.wav -r 48000 --gain 4 --dc-remove 10/11 --convolve '1,-1/5'
sex analyze input.wav -r 48000 --preset sane --error-floor -300dB --grid 129
```

[Quality feedback](quality-refinement.md) is mandatory for every materialized
CLI FIR. `--error-floor` selects an explicit target; otherwise the preset target
applies. `--refine-quality` remains a redundant compatibility alias. Conversion checks
every phase on a 65-point-per-band MPFR grid, increases coefficient width or
the selected window's length/shape when needed, and reassesses within explicit cumulative work
limits. The target and band edges never weaken; exhausting a limit fails.
Conversion, `analyze`, and `plan` share `--grid`, `--certify`, `--harmonics`, and
all associated resource controls. Analysis reports the final accepted bank's separate
passband, stopband, and coefficient-quantization compliance, plus recomputed
numerical bounds for the configured effect chain. `plan` and `analyze`
accept `--gain`, `--dc-remove`, `--mix`, and `--convolve`; their response grids
measure the resampler FIR only.
This remains a sampled numerical qualification, not an unsampled analytic proof.
The target qualifies the filter response and coefficient error, not an
end-to-end PCM noise floor. Final PCM and dither have their own quantization
floors, and the absolute coefficient-error bound scales with signal peak.
`sex plan` labels feedback's initial candidate unqualified and stops before
coefficient design, which is especially important for
`until-40k`: planning is immediate even when materializing millions of
high-precision coefficients is intentionally expensive.

`--window` defaults to Kaiser. Other windows start from the same length estimate
but use C>=64, a conservative full-Q2.C-range MAC reserve, and method-preserving
length feedback. All-image and continuous checks remain available for their
actual banks. Direct Dolph also has a static series-work cap; an expensive
request fails before coefficient/cache allocation instead of weakening the
preset. See [windowed-cli.md](windowed-cli.md).
It prints the initial coefficient/selected-grid/MPFR work and explicit limit
increases needed to admit that first candidate. These estimates do not reserve
work for later candidates; supplemental preflight remains explicitly not a
proof. The [shared gate contract](shared-quality-gates.md) binds successful
analysis and conversion reports to the same actual bank. Default guards can
reject an extreme Until-40k request before cache access; increasing guards
never bypasses quality checks. Same-rate conversion bypasses FIR work entirely.
