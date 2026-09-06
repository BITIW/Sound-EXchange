# Complete input-phase impulse reference campaign

The opt-in `phases` suite extends the existing Sane reference campaign from at
most eight input impulse positions to **every residue in the reduced input
period** for each requested rate pair. This is separate from the legacy tone/
multitone fixture and guarded sweep; their default selection is unchanged.

For reduced rate L/M, shifting an input impulse by M samples shifts its output
by exactly L frames. The fractional output center of an impulse at input frame
k is `(k*L)/M`. Because L and M are coprime, M adjacent k values visit every
fractional-center numerator modulo M exactly once. This is the input-time
period M, not the engine's L coefficient phases.

## Fixtures and measurements

The harness processes consecutive batches of at most 32 independent channels.
Each contains one exact half-scale PCM32 impulse at `input_rate/2 + offset`.
The input length is `input_rate + M + 7` frames. Extending by M preserves at
least half a second of context before/after every impulse, even for near-unity
coprime rates where M approaches the input rate. The accepted SeX FIR must fit
that context; the harness rejects a larger support instead of claiming a pass.
No whole-track or hundreds-of-gigabytes fixture is required.

Every engine retains the original impulse measurement gates:

- exact rational expected center; no peak-based realignment;
- peak within one output frame and normalized area within 0.0001 of unity;
- 129 DFT frequencies from DC through output Nyquist, with amplitude/phase gates
  inside 85% of lower Nyquist (0.01 dB and 0.001 rad);
- exact SeX nearest/ties-to-even duration; at most one reference frame difference.

All response tables and rational center/residue metadata are retained. The new
suite does not add pairwise error-spectrum tables: existing signals/sweep suites
provide those comparisons. It compares four complete configured engines, not
identical FIR designs, continuous frequencies, every quality preset, or a
continuous periodically time-varying anti-imaging proof.

The DFT measurement now skips exact-zero samples while retaining their original
indices and the order of all nonzero terms. This avoids redundant trig calls
on the long silent contexts, not audio work in SeX. A dense oracle with signed
zeros and fractional centers checks bit-identical real/imaginary values.

## Reproduce

```bash
SEX_SOX_RUNTIME=/path/to/pinned-sox-runtime \
SEX_SOX="$PWD/scripts/sox-reference.sh" \
SEX_SPECTRAL_SUITES=phases \
scripts/validate-spectral-references.sh target/NEW-phase-results
```

The result directory must be new. The default five-rate matrix is retained;
`SEX_SPECTRAL_RATIOS` selects another matrix. `phases`, like legacy `signals`,
is currently Sane-specific and explicitly rejects other presets. Snapshotted
executables/source, tool/reference identities, cache and final snapshot checks
are preserved by the existing harness. Batch WAVs, raw float reference transport
and repeat outputs are retained, so the complete default campaign costs about
a gigabyte of disk rather than only the small report files.

The probe additionally exposes `input-phase-count FROM TO`,
`generate-phases NEW_WAVE FROM TO START COUNT`, and
`analyze-phases WAVE FROM TO START COUNT NEW_REPORT_DIRECTORY [strict]`.
Invalid/empty/overlapping-with-period batches are rejected; COUNT is 1..32 and
START+COUNT must not exceed M. Fixture rates retain the validation-only
1000..192000 Hz limit, not a SeX engine limit.

## Tests and controls

Unit tests prove exact residue coverage, bounded batch size and impulse/context
placement for the five matrix ratios, unity, large upsampling and 1000/1001.
All 444 native release workspace/all-target tests pass (one existing heavy
test remains opt-in); all 20 spectral probe tests execute on emulated ARM.
Strict native/cross Clippy, formatting and shell syntax checks pass.

Reanalysis of all five historical Sane SeX outputs in `spectral-v3-qualified`
produces byte-identical **complete report directories**, including every impulse
response and existing tone/spectrum table. A negative control mutes only the last
channel of a valid three-phase output and fails at `impulse-offset-2`, after
the first two pass. The first attempt used FFmpeg's `3c` shorthand and changed
3.0 to 2.1 layout, unintentionally remixing other channels; that failed at the
first impulse and is not the isolated-mutation evidence. The corrected `3.0`
control preserves the layout. Both attempts remain under
`target/input-phase-tests-qualified`, alongside test logs and legacy reanalysis.

## Completed reference execution

`target/spectral-all-input-phases-qualified` completed successfully with the
snapshotted Sane executable, SoX 14.4.2, libsoxr 0.1.3 and libsamplerate 0.2.2.
All four engines pass every impulse response, duration and common-passband gate:

| Conversion (Hz) | Reduced ratio L/M | Input residues per engine | Batches |
| --- | --- | ---: | ---: |
| 48000 → 16000 | 1/3 | 3 | 1 |
| 44100 → 48000 | 160/147 | 147 | 5 |
| 48000 → 44100 | 147/160 | 160 | 5 |
| 44100 → 47900 | 479/441 | 441 | 14 |
| 48000 → 1000 | 1/48 | 48 | 2 |

That is **799 residues per engine, 3196 impulse responses and 27 batches**.
All 27 whole-file SeX repeats at blocks 257 and 4096 are bit-identical. An
independent scan of the actual per-impulse summaries verifies the expected
denominator, range, uniqueness and complete residue count separately for each
engine/rate pair. Its result is
`target/input-phase-tests-qualified/executed-coverage.tsv`.

The source/tool snapshot and external reference checks all pass. Inputs, output
WAVs, raw reference transport, per-impulse response/center/area tables, commands
and coefficient cache remain in the 1.2 GiB campaign directory. The preserved
summary of the running harness is `/tmp/sex-all-input-phases-campaign.log`;
durable engine reports and results are inside the campaign directory itself.

This closes complete input-residue coverage for these five Sane rate pairs.
It does not promote the sampled 85%-of-lower-Nyquist passband checks into a
continuous proof, broaden this run to other presets, or claim bit-identical
audio between different reference engines.
