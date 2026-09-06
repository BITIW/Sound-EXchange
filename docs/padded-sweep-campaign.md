# Complete guarded passband sweep qualification

This extends the [four-engine reference campaign](spectral-reference-campaign.md)
with a separate mono PCM32 fixture. It addresses the original logarithmic
sweep's cropped high-frequency end without changing that historical fixture or
its gates. This is an execution comparison, not a continuous filter certificate
or a measurement of the fixed-point arithmetic's theoretical error floor.

The newer [preset-aware extension](spectral-preset-campaign.md) adds selectable
presets, a Fast-specific common band and guards sized/checked against the FIR.
The fixed quarter-second formulas and numerical results below describe the
original Sane profile, which remains unchanged for the recorded ratios.

## Signal and measured interval

Let `F = floor(89 * min(input_rate, output_rate) / 200)` Hz and
`t = absolute_time - 1/4` seconds. The instantaneous frequency is

```text
f(t) = 1                                      t <= 0
       1 + (F-1) * (1-cos(pi*t))/2            0 < t < 1
       F                                      t >= 1
```

The phase in cycles is the following continuous antiderivative:

```text
c(t) = t                                      t <= 0
       t + (F-1) * (t/2 - sin(pi*t)/(2*pi))   0 < t < 1
       (1+F)/2 + F*(t-1)                     t >= 1
x(t) = sin(2*pi*c(t))/2
```

Both frequency and its first derivative meet the constant-frequency guards
continuously. There is a quarter-second leading guard and at least a
quarter-second trailing guard, with `floor(3*input_rate/2)+7` total input frames.
The fixture generator forms sample time as an exact rational before MPFR-192
evaluation, then rounds once to signed PCM32, ties to even. No synthesized
floating-point value enters the SeX signal API.

The measured core contains **every** output frame in absolute times
`1/4 <= time <= 5/4`, including both endpoints when representable. Integer bounds
are `ceil(output_rate/4)` through `floor(5*output_rate/4)`, inclusive. In addition
to the complete core, ten disjoint consecutive intervals cover those same
frames without omissions or duplication. Each gets a separate uncorrected
waveform SNR against the independently evaluated analytic signal. No amplitude,
phase or time alignment is fitted. The report records actual first/last frame
frequencies, not an assumed frequency range after cropping.

Here “complete” means the entire defined 1..F Hz sweep, through 89% of lower
Nyquist. It does not mean the transition band or Nyquist itself: Sane's declared
pass edge is 90%. This chirp is not globally bandlimited, despite its bounded
instantaneous frequency. Filtering its sidebands and endpoint-join behavior
contributes to waveform error, along with passband shape and PCM quantization.
The chosen guards are qualified for this bounded campaign, not arbitrary
Until-40k filters or other untested rates/presets.

## Gates and output

Every one of the eleven intervals must meet uncorrected waveform SNR:

- SeX Sane: at least 85 dB, the same waveform threshold used for its steady
  passband tones, with no fitted-response correction.
- The reference configurations: at least 58 dB. This **new** full-band
  comparison allows their deliberately different passband shapes: a 0.01 dB
  gain deviation alone corresponds to approximately 58.77 dB waveform SNR.
  It is not a fitted-residual gate and does not alter the original cropped
  logarithmic sweep's 80 dB threshold or any steady-tone gate. For example,
  libsoxr's last interval at 44100→48000 measures 65.90 dB here, although the
  earlier limited-frequency sweep was far above 80 dB.

SeX's duration remains exact nearest/ties-to-even; references may differ by one
frame. Both SeX block sizes (257 and 4096) must produce identical complete
files. Reference engines, bit depth, dither and saturation settings are unchanged
from the original campaign.

`sweep.tsv` retains all eleven interval results, including failures, and their
explicit thresholds. Error spectra are computed for the core and the upper
decile using the shared centered, at-most-4096-sample periodic Hann definition.
Those spectra cover windowed subsegments; only the reported RMS/SNR uses every
sample of its interval. Pairwise SeX/reference RMS, peak and error spectrum use
the same complete core, with no duration correction or sample realignment.

## Reproduction

Use the same SoX/runtime setup described in the parent campaign document:

```bash
# Both legacy multichannel signals and the new sweep, on all five ratios:
scripts/validate-spectral-references.sh target/spectral-v3-qualified

# A new directory is required for every run. To select only the new fixture:
SEX_SPECTRAL_SUITES=sweep scripts/validate-spectral-references.sh target/new-sweep-run
```

`SEX_SPECTRAL_SUITES` accepts `signals`, `sweep`, or both; default is both.
`SEX_SPECTRAL_RATIOS` retains its existing rate-matrix syntax. Whitespace-only
selections fail instead of reporting an empty campaign as passed. Version and
selected matrix, source/executable/package hashes, commands, small audio files,
coefficient cache and all measurements are retained. Legacy case directories
keep their names; new sweep directories have a `-sweep` suffix.

The standalone probe additionally exposes `generate-sweep`, `analyze-sweep`
and `compare-sweep`, with the same arguments as their original counterparts.
The `strict` analysis flag selects both exact SeX duration and its 85 dB gate,
as with the original analyzer's stricter SeX comparison semantics.

## Recorded result: 2026-09-06

All 40 engine/rate/suite combinations passed, as did ten complete-file SeX
block-size repeats. This includes all original tone/impulse gates, 140 separate
impulse responses and 220 sweep interval checks. The new sweep results are:

| Input → output Hz | Endpoint Hz | SeX full-core SNR dB | SeX minimum interval SNR dB | SoX full-core SNR dB | libsoxr full-core SNR dB | libsamplerate full-core SNR dB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 48000 → 16000 | 7120 | 138.873545 | 131.429621 | 158.300071 | 152.236394 | 135.974123 |
| 44100 → 48000 | 19624 | 138.455913 | 131.084297 | 82.054410 | 74.739222 | 140.779041 |
| 48000 → 44100 | 19624 | 138.221106 | 131.208199 | 85.601111 | 77.109084 | 141.521393 |
| 44100 → 47900 | 19624 | 138.443614 | 131.081993 | 82.093914 | 74.769173 | 140.916613 |
| 48000 → 1000 | 445 | 95.628468 | 85.832461 | 98.000886 | 97.857658 | 101.542669 |

The worst SeX interval is the final decile at 1/48. This is not evidence of an
85 dB arithmetic floor. Different filter responses act on the finite-rate
chirp differently; none of these columns establishes a universal ranking.
Libsamplerate emits one fewer frame for the new 44100→48000 and 44100→47900
fixtures; the other new sweep durations agree with SeX.

Final artifacts: `target/spectral-v3-qualified`, approximately 154 MiB.
Source fingerprint:
`2d098dac62708e4d939253a8dbacd1605c0394aa2984893850872e727d040a23`.
SeX binary SHA-256:
`37295b27f4b6855950b25f78915564070351776f07d25710ae5b7c83dcc447f4`.
Probe SHA-256:
`ecd766c19ded5a74e3a8b34100187490d0a35de38ff0dd4f10f1c0f601656987`.
Cargo.lock SHA-256:
`fe3d46afca93fcfb62f6c19bb6e18a34ab6f8042c620904c3642de7a089ecbb8`.
The harness verified source stability throughout the run; the audio processor
binary remains unchanged from the preceding frontend/impulse checkpoints.

347 native release workspace/all-target tests passed; the existing opt-in
heavy test remains ignored in that command. Strict all-target Clippy passed.
All 16 probe tests also passed on emulated AArch64, including MPFR versus
independent analytic phase at both joins, non-divisible-by-four sample rates,
exact core/decile coverage and invalid SNR handling. This uses the existing
QEMU-only linker accommodation, not production ARM qualification. Logs:
`target/spectral-padded-sweep-workspace-tests.log` and
`target/spectral-padded-sweep-arm-tests.log`.

## Controls that exercise the measurement

The pilot artifacts in `target/spectral-padded-sweep-pilot` retain two controls:

1. Mute only absolute times 1.15..1.25 seconds in the 48000→16000 SeX output,
   preserving rate, duration and the first nine deciles. `analyze-sweep strict`
   rejects the last decile at 0 dB SNR and the complete core at 9.93 dB, while
   the other nine deciles retain their original passing values. Files:
   `muted-upper-decile.wav`, `muted-upper-gate.log`, `muted-upper-metrics/`.
2. Add quarter-second +1/4 FS and -1/4 FS constant segments outside the existing
   guards of the 48000→1000 input. Verify the original 72007 input frames are
   unchanged, render SeX with block size 7, then extract the correspondingly
   shifted 1001-frame core. It is byte-identical to the original core, SHA-256
   `3239a38214e54e120f676d0b1c94085617e1adff5082b1acee14ea18098c0cc3`.
   The valid control is `preserved-outer-context.wav` and its `-sex.wav` render;
   extracted files are `original-core.pcm` and `preserved-context-core.pcm`.
   Thus the lower last-decile SNR in this specific SeX case is not caused by
   truncation at the file's outer boundaries.

The first context-construction attempts were invalid: FFmpeg's default channel
layout conversion changed the original PCM32 low bits. They are retained for
diagnosis, not counted as passing controls. The valid construction explicitly
keeps `s32` and the source's `FL` channel layout on all three concat inputs.
Always compare the embedded input before attributing an output difference to
the resampler. Format conversion/mixing inside external reference frontends
can likewise limit measured precision; this campaign measures the configured
end-to-end engines, not an isolated infinite-precision library kernel.

The checks still do not cover other presets, every rational phase/rate,
arbitrarily long guards/FIRs or continuous complete-image bounds.
