# Input consistency across normalization passes

`--clip normalize` measures the post-effect/resampler peak in a first streaming
pass, reopens the input, and renders with the resulting exact-rational scale in
a second pass. Both passes start with fresh DSP/resampler state. The scale is
valid for the scanned input, not for arbitrary replacement audio.

Previously only the final output-frame count was compared. That missed changes
to samples at unchanged length, and even some changes to input length: a 1/3
rate ratio rounds both three and four input frames to one output frame.

The shared pass now returns actual input and output frame counts plus, for
normalization only, a streaming SHA-256 of decoded input PCM. Each signed Q1.63
input sample contributes exactly eight little-endian bytes in interleaved order,
before gain, DC removal, mixing, convolution, resampling or signal rescaling.
Chunk boundaries contribute no bytes. Hashing after the effects would be
insufficient: zero gain or a channel mix can erase a source difference.

Before rendering, the reopened input must have the same channels, sample rate,
PCM depth, typed metadata and decoder identity. After rendering, both frame
counts and the complete PCM digests must match before writer finalization can
return success. A mismatch is an error. The surrounding CLI output transaction
then removes its temporary file and leaves any prior destination untouched.
No partial output is published. Direct internal processing to an arbitrary path
is not itself a file transaction; publication belongs to the CLI wrapper.

This adds constant-size hash state, not a complete-track buffer or another
audio pass. Non-normalizing passes do not hash PCM. Coefficients, DSP arithmetic,
normalization scale calculation, dither sequence and output reports are unchanged
for stable input. SHA-256 was already pinned in the workspace for coefficient
caches; the CLI now depends on that same version directly.

## Regression scope

- Empty and nonempty mono/stereo PCM32 input, including signed endpoints, is
  checked against an independently constructed Q1.63 byte-sequence oracle at
  blocks 1, 2, 7 and 4096. Missing digests cannot pass replay validation.
- A real one-tap 1/3 stream shows that different input counts are rejected even
  when their planned output counts are equal. This is a timing fixture, not a
  spectrally qualified resampling filter.
- An injected reopen operation changes input deterministically between scan
  and render, without a timing race. At PCM8/16/24/32, tests change a sample,
  length, channels, sample rate, bit depth or typed metadata. Zero gain ensures
  the sample change cannot be discovered by comparing processed audio instead.
  Each failure preserves an existing destination and removes the temporary file.
  Metadata mutation is injected at the adapter result, not a codec tag-write test.

All 434 native release workspace/all-target tests pass (one existing heavy test
remains opt-in), including the established rate-changing, stateful-effect and
4096-bit normalization integration tests. All three new regressions also execute
on emulated AArch64. Native and cross warnings-denied Clippy and formatting pass.
Native and ARM test logs are retained in `target/normalization-replay-qualified`.

The existing codec-only cross-architecture harness also passes twelve fresh
High 160/147 conversions: tagged WAVE/AIFF/FLAC, PCM16, seeded fifth-order noise
shaping, normalization, blocks 1/7/4096, independent caches and both reciprocal
cache directions. All full output files and qualification blocks match across
x86-64/emulated ARM and the earlier `read-boundary-codecs-v2-qualified` campaign.
The new v2 cache bytes agree across CPUs. Artifacts, exact commands, source
fingerprint and identities are under `target/normalization-replay-codecs-qualified`.

Whole output SHA-256 remains:

| Container | SHA-256 |
| --- | --- |
| WAVE | `41c4405a6c522f3f483c1908078b5019042cbaf558d7d5092be0b8164a82ba53` |
| AIFF | `9b417da16426d69cbc5b40651caf0bc4347349e587cc9c830d711675a485bec6` |
| FLAC | `6f450117e9f4f24030f5ca96bb37127fe6bf1a30fffafd9a6d04d70bcf30574f` |

The complete qualification-block SHA-256 is
`91441e64a9acd65c3e0a92c20c035c4347bb4ac2d619fde1bd6ce1f73bb2074d`.
Both decoders use libsndfile 1.2.2 where needed; ARM is the existing Cortex-A53
QEMU-only validation environment, not physical-hardware/production ABI evidence.

The check establishes consistency of the **decoded streams consumed by the two
passes**, subject to SHA-256 collision resistance. It does not lock the input,
authenticate it, freeze all file bytes, or detect modifications never observed
by either decoder. Unrepresented container metadata is outside the typed
metadata contract. A concurrently modified file can still produce a read/codec
error, which also prevents publication. This is not an OS snapshot facility.
