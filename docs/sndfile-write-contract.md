# libsndfile write completion and accounting

The libsndfile adapter now checks both the returned frame count and the native
error status before accepting a write. A full frame count does not override a
nonzero `sf_error`; an explicit zero-length call also cannot hide an error
reported by that call. A failed write poisons the writer, as before, and prevents
further writes or successful finalization.

Prospective frame and saturation counters are checked **before** passing the
block to libsndfile. The two counters are assigned together only after the
complete native write succeeds. Count overflow cannot write another block or
leave a partly updated report. Scratch allocation failures now preserve
`AudioIoError::BufferAllocation` rather than being mislabeled count overflow.

On close, the handle has been consumed and must not be queried. A failed close
now resolves its actual returned code with `sf_error_number`, rather than using
the handle's potentially unrelated pre-close text. Successful-close and
successful-write behavior remain unchanged. The runtime loader therefore also
requires the standard `sf_error_number` symbol.

## Verification

Three new regressions cover:

- a controlled native callback returning all requested frames while reporting
  an error: requests for 0, 1 and 7 frames must fail; the same count with zero
  status succeeds;
- a controlled failing close callback with deliberately stale handle-error
  text: the returned-code message wins. Message lookup uses the real loaded
  library, without querying a consumed handle;
- actual WAVE writing with modeled counter exhaustion: both frame overflow and
  saturation-count overflow leave the entire report unchanged, poison the
  writer and fail finalization. Reopening the small file proves only the prior
  successful frame was written; no huge file or sample array is allocated.

The first two are explicitly callback-injection tests, not induced physical
disk failures. Their inert handles are used only with non-dereferencing test
callbacks; no fake pointer is passed to a real handle-taking library function.

All **428 native release workspace/all-target tests** pass, with the existing
heavy Until-40k test still opt-in. On emulated ARM, all 13 libsndfile tests and
30 root-library tests pass. Both runs require installed libsndfile via
`SEX_REQUIRE_SNDFILE_TESTS=1`; no runtime assertions are skipped. Native/cross
warnings-denied Clippy and formatting pass. Logs are
`target/sndfile-write-boundary-qualified/{workspace,arm}-tests.log`.

## Successful output identity

The previously qualified tagged WAVE/AIFF/FLAC PCM16 files were copied through
the current CLI with no dither and clipping errors enabled, at blocks 1 on
x86-64 and 7 on ARM. All six files are byte-identical both across architectures
and to their historical input files. This preserves PCM, metadata and complete
serialization, not just a sample checksum.

The output hashes remain the ones in
[codec-read-identities-2026-09-06.tsv](codec-read-identities-2026-09-06.tsv).
Commands, backend versions and actual files are in
`target/sndfile-write-boundary-qualified/copies`. This is a no-rate conversion
check of the changed I/O path; it does not repeat the historical High filter
design/qualification campaign.

## Limits

The generic codec writer is not transactional: a failed native write can leave
a prefix at its destination. The CLI's separate temporary-output/atomic-publish
contract still protects the published destination. These changes do not add
RF64, establish libsndfile's large-container size policy, exercise filesystem
power-loss durability, or qualify arbitrary codec versions/physical ARM.
The separate [built-in WAVE contract](streaming-write-contract.md) documents
its explicit classic-RIFF limit and byte-level fault tests.
