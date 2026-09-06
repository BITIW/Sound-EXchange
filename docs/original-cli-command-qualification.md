# Original CLI command: automatic −300 dB qualification

The original goal explicitly includes
`sex in.wav out.wav --rate 48000 --error-floor -300dB`, in addition to SoX-like
`-r` and positional `rate` forms. This campaign executes those forms against
the same nonempty 44100 Hz, 24-bit stereo input, not just parser unit tests.
It is independent of the still-running common-ratio Until-40k campaign.

## Executed path and evidence

`target/original-command-qualified/run.sh` uses the pinned executable
`target/until-budget-qualified/sex-qualified`. It reads the 257-frame stereo
fixture from `target/until-budget-corpus-qualified/inputs/stereo24.wav` and
executes all three forms with seed 42 and blocks 1, 7 and 4096 respectively.
All three exit successfully and produce the same 280-frame stereo WAV:

`d5649497c1a52741629da5968b6f81d1b541c3776d1192f127e7d4548b7c0177`.

At ratio 160/147, 280 successive outputs exercise every output polyphase
position. This short, zero-extended signal is a scheduling/chunk/CLI check,
not a settled passband or full-track spectral measurement.

The actual feedback trace is identical across the three forms and analysis:

1. Initial 765-tap/phase, C=62, MPFR=192 candidate passes the response grid
   but fails the exact coefficient-error budget.
2. The engine raises C to 78 without changing the filter length or target.
   The second candidate passes both requirements.
3. Whole-chain planning follows the accepted bank: signal Q65.78 and an actual
   223-bit accumulator requirement. Analysis reports the same declared-input
   range and zero spare bits in the 223-bit execution plan.

The search charges 244800 cumulative coefficients and 31824000 response terms;
no custom work limits or weakened gates are used. The accepted bank identity is
`ddc90dd16fd3b8efe39b559b9d4ea8f0e2efa5a2ac4d95e8b7a850c0b691736e`.
Its exact coefficient-L1 error bound is `764 * 2^-78` against budget `2^-53`.
On the requested 65-point/band grid, reported stopband peak is
−307.3130142337216 dB and ripple is 3.857386928804227e−15 dB. These are sampled
measurements, **not** continuous-frequency proofs or bounds on MPFR design error.

The native cold path stores both candidate banks; warm forms and `analyze`
reassess the same cached candidates. A separate big-endian AArch64/QEMU run
loads these foreign cache files, repeats the full automatic search and yields
identical WAV bytes and the complete FIR qualification block. Both complete
cache payload hashes remain unchanged. See `big-endian.log`,
`foreign-cache-check.log`, `outputs.sha256` and `run.log` in the result directory.
This run uses the experimental static-musl QEMU profile and built-in integer
WAVE, not physical ARM or libsndfile codec runtime.

## PCM scope and regression

The endpoint/random fixture deliberately has full-scale values. The default
saturating path reports 24 clipped components; its measured final PCM error
is consequently about −29.59 dBFS RMS. Identical clipped output does not prove
a −300 dB recording. A separate same-bank run with `--gain 1/2 --clip error`
completes with **zero** clipped components and measured 24-bit high-pass-TPDF
error −144.3313551636940 dBFS RMS. Both reports explicitly separate PCM error
from FIR and pre-PCM numerical budgets.

The permanent regression
`original_error_floor_command_refines_precision_across_rate_forms_and_chunks`
uses a separate low-level 257-frame stereo fixture and `--clip error`, so
saturation cannot hide output differences. It asserts the C62→C78 refinement,
unchanged −300 dB target, zero clipping, complete phase-period duration,
identical output bytes across all three forms/chunks, warm-cache hits and
matching analysis qualification/accumulator scope. No implementation algorithm
or preset was changed by this campaign.
