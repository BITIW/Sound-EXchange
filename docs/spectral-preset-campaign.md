# Preset-aware reference execution

The [guarded sweep campaign](padded-sweep-campaign.md) now selects the SeX preset
explicitly and sizes fixture guards from its planned FIR. This extends external
execution evidence beyond Sane without forcing Fast to satisfy Sane's bandwidth
or pretending reference filters share High's wider passband.

## Selection and contracts

```bash
# The usual SoX/runtime setup is required; every result directory must be new.
SEX_SPECTRAL_PRESET=fast scripts/validate-spectral-references.sh target/spectral-fast-qualified
SEX_SPECTRAL_PRESET=high scripts/validate-spectral-references.sh target/spectral-high-qualified
```

`SEX_SPECTRAL_PRESET` accepts all six existing preset names. It defaults to
`sane`. Other presets default to the `sweep` suite; requesting the historical
`signals` suite with another preset fails explicitly, because that suite's
bandwidth, cropped intervals and limits are Sane-specific. Sane still defaults
to both suites. The selected preset and optional guard override are recorded
in `matrix.txt`, and each input's channel manifest records its concrete profile.

The chirp formula and one-second measured core are unchanged. Its endpoint is:

- Fast: `floor(79 * min(input_rate, output_rate) / 200)` Hz, inside its 80%
  pass edge.
- Sane and higher: `floor(89 * min(input_rate, output_rate) / 200)` Hz.
  This is the common comparison band, not evidence for High's extra 89..95%
  region or the still wider passbands of heavier presets. Their own CLI bank
  qualification continues to assess their actual specified bands and targets.

Every core/decile SeX waveform SNR must reach
`min(85, preset_stopband_attenuation_db - 6)` dB: 74 dB for Fast, 85 dB for
Sane and higher. The 6 dB allowance defines a bounded chirp regression limit;
it is not a proof relating arbitrary chirp error to stopband attenuation.
Reference engines retain the previously defined 58 dB uncorrected common-band
gate. No existing Sane criterion was weakened, and reference filter settings
remain unchanged. Different Fast/High fixture bandwidths prohibit treating
their SNR columns as a controlled same-input ranking.

## Guard planning and actual-filter verification

The guard on **each** side is `g/4` seconds, where

```text
T = initial planner taps per phase for this preset and ratio
g = max(1, ceil(4 * (floor(T/2) + 1) / input_rate))
input_frames = floor(input_rate * (2*g + 4) / 4) + 7
```

Sample time remains exact-rational before MPFR synthesis:
`t = input_frame/input_rate - g/4`. Integer output bounds include every sample
with `g/4 <= output_time <= g/4 + 1`, still subdivided without gaps into ten
intervals. The extra input-frame allowance is intentional. Both generated
input and rounded expected output are limited to ten million frames (about
40 MB PCM32 per mono file), before creating the fixture. This is a measurement
resource limit, not a processor size limit or proof that expensive FIR execution
will fit the CLI's separate budgets.

Automatic guard quarters at 48000→1000 are 1, 1, 2, 5, 9 and 134 for Fast,
Sane, High, Absurd, Pointless and Until-40k. These follow the planner; accepting
the name does not claim that every corresponding heavy campaign has run.

The CLI's quality feedback can enlarge a FIR after the initial plan. Therefore
the harness now [prequalifies the bank before rendering](spectral-snapshots.md),
then also extracts the **accepted** tap count from the completed conversion
and requires

```text
actual_taps <= 2 * (floor(input_rate * g/4) - 1) + 1
```

`guard-check.tsv` retains both the accepted length and this guard capacity. An
oversized accepted bank fails qualification rather than quietly measuring an
unprotected core. Missing/ambiguous filter metadata also fails. A unity copy
does not use a FIR and is checked with zero active taps.

Set `SEX_SPECTRAL_GUARD_QUARTERS` to explicitly increase the guard. An override
below the planned minimum, integer overflow, or an oversized fixture fails
before audio creation. If feedback outgrows the guard, rerun with a larger
override in a fresh directory; the harness never rewrites old measurements.
This check establishes finite support for SeX's accepted bank, not an analytic
settling bound for each external reference implementation.

## Fast and High results: 2026-09-06

Both presets passed the five-ratio sweep matrix, with all four engines per
ratio: 40 engine/preset/rate combinations, 440 interval checks and ten complete
SeX repeat-file comparisons at block sizes 257/4096. The mandatory CLI actual-
bank quality gates passed at the presets' unchanged -80/-160 dB targets.
Fast used checked-i128 execution with exact GMP overflow fallback; High used
GMP with 96 coefficient fractional bits, not a silently narrowed native bank.

| Input → output Hz | Fast taps | Fast full-core SNR dB | Fast minimum decile dB | High taps | High full-core SNR dB | High minimum decile dB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 48000 → 16000 | 385 | 99.806523 | 93.681907 | 1663 | 164.909301 | 155.004079 |
| 44100 → 48000 | 129 | 98.710399 | 93.131741 | 555 | 183.073304 | 177.417682 |
| 48000 → 44100 | 141 | 97.174832 | 91.675342 | 605 | 182.794166 | 176.024589 |
| 44100 → 47900 | 129 | 98.720313 | 93.150999 | 555 | 183.041318 | 177.337464 |
| 48000 → 1000 | 6145 | 99.891884 | 92.983115 | 26593 | 104.635230 | 94.841201 |

High's 26593-tap 1/48 case needs half-second guards instead of the previous
quarter-second fixture. The entire chirp, including its last decile, is still
measured. These SNRs include chirp sidebands, filter shape, final PCM and
measurement precision; they are not intrinsic arithmetic-floor certificates.

Artifacts: `target/spectral-fast-qualified` (about 14 MiB) and
`target/spectral-high-qualified` (about 19 MiB). Both retained campaigns verified
the same stable source fingerprint:
`76c3f9849cf7189c1f464924819aaf0b2cf162f15236e0575bdc44df83b5f34c`.
SeX binary SHA-256 remains
`37295b27f4b6855950b25f78915564070351776f07d25710ae5b7c83dcc447f4`;
the extended probe is
`b27bd4c8e90f58f8c6e55f542f578db47811dcfb05cd43b50a4ecaf57483be91`.
Versions, actual coefficient identities, complete commands, source/tool/input/
output hashes, caches, guard checks, all four engine measurements and pairwise
differences are retained in those directories.

## Heavy 1/48 qualification

At 48000→1000, the initial Absurd bank missed its unchanged -240 dB target:
98305 taps gave a sampled stopband peak of -237.238261 dB and passband
deviation 1.39953365e-12. Bounded quality feedback accepted 103391 taps with
the same 160 fractional coefficient bits and MPFR-384 design. This fits the
planned 1.25-second guards (119999-tap capacity). All four engine comparisons
and the complete-file SeX repeat passed. SeX core SNR was 108.188292 dB;
minimum decile SNR was 98.477558 dB.

The corresponding Pointless first candidate also missed its -360 dB target:
196609 taps gave a sampled stopband peak of -357.538435 dB and passband
deviation 1.31449923e-18. Feedback accepted 203313 taps without reducing the
target or the 256 coefficient fractional bits / MPFR-512 work precision.
The accepted bank fits its 2.25-second guards (215999-tap capacity). SeX passes
every interval: 110.448870 dB full-core SNR and 100.735724 dB minimum decile SNR.
The repeat at block size 4096 is byte-identical to the original 257-frame-block
render, and all three external references pass the same common-band campaign.

Together these two heavy cases add eight engine/preset/rate combinations,
88 interval checks and two complete-file repeat checks, for 48 combinations,
528 intervals and 12 SeX repeats across this checkpoint's four presets.
At this profile checkpoint Until-40k had unit coverage but had not run through
the long-sweep execution campaign. A subsequent
[snapshot-protected guarded 2/1 campaign](spectral-snapshots.md) now passes all
four engines, 44 interval checks and the complete-file repeat. It has its own
frozen source/build identity and does not retroactively broaden this checkpoint's
matrix or qualify every Until-40k ratio.

These are actual feedback executions, not merely hypothetical profile tests.
The initial and accepted bank results, work accounting and accepted guard
capacities are retained in `target/spectral-absurd-long-guard` (about 5.7 MiB)
and `target/spectral-pointless-long-guard` (about 16 MiB). They have the same
source/executable identities as the Fast/High runs above. Accepted coefficient
SHA-256 identities are
`b7ea39c43774e28c84cf2bbe19ad996766135d9db1d44dc59a7c353dface28ce`
(Absurd) and
`583b9069b44a17283f93ec17eef5b2d019df99a4d1d4ce4100d51a59fe1756fe`
(Pointless). The unchanged sampled CLI response
gates are not continuous-band certificates. The SNR figures continue to
include finite chirp/filter behavior and final PCM, not just arithmetic error.

## Regression and cross-platform checks

349 native release workspace/all-target tests passed (one existing heavy
opt-in test ignored), together with strict native and AArch64 all-target
Clippy. All 18 probe tests passed on emulated AArch64. This retains the existing
QEMU-only linking limitation; it is not physical ARM or production linking
qualification.

The five Sane input WAVE fixtures remain byte-identical to the preceding v3
campaign. Reanalyzing the existing SeX outputs reproduces every `sweep.tsv`
byte-for-byte. New profile tests cover all six presets' planned guards and
passband inclusion, shifted MPFR PCM equality, threshold boundaries, forbidden
short guards, checked size limits and actual-tap capacity.

The High 48000→1000 fixture generated on emulated ARM is identical to the
native fixture, SHA-256
`a439782894c37eb05d0efd680b4666d7b5ab2156101e536bfdc06f463f161b65`.
This is generator reproducibility evidence, separate from processor cross-CPU
qualification. Regression fixtures/reports and rejected short-guard, excessive-
tap and unsupported-suite controls are retained under
`target/spectral-presets-regressions`. Test logs are
`target/spectral-presets-workspace-tests.log` and
`target/spectral-presets-arm-tests.log`.

Remaining scope includes the wider preset-specific upper passbands, broader
heavy-preset execution, more ratios/input phases and continuous complete-image
proofs. The selected common-band matrix does not establish those claims.
