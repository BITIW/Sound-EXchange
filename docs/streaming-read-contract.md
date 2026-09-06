# Streaming read allocation and failure contract

Both integer input adapters treat `read_frames(max_frames)` as an upper bound,
not a command to allocate that entire block before looking at the source.
They retain frame-aligned interleaved Q1.63 results and the same successful PCM
values and EOF semantics. The caller still owns the returned vector: requesting
an entire genuinely huge recording can require correspondingly huge memory.
Use finite blocks for bounded-memory processing.

## Allocation

- WAVE/hound grows its result as samples are decoded. Each growth request adds
  at most 8192 samples, capped by the request and the iterator's remaining sample
  count; the allocator may grow capacity geometrically. It does not reserve a
  full declared multi-gigabyte data chunk or the user's entire upper bound.
- libsndfile uses at most 8192 integer scratch samples, or one complete frame
  if the channel count exceeds 8192. It repeats bounded native reads to fill the
  caller's block, growing Q1.63 storage only for returned samples. Scratch and
  result allocations are fallible.
- Count multiplication overflow is rejected before reading and is retryable.
  Allocation errors are `AudioIoError::BufferAllocation`, with the original
  reservation error available as the source; the libsndfile adapter wraps this
  in `SndFileError::Audio`.

This prevents capacity panics and eager allocation driven only by a requested
upper bound or malformed declared length. It is not a process memory sandbox:
allocator/OS overcommit behavior and memory used internally by decoder libraries
are outside this guarantee. Allocation failure after entering decoding is
conservatively terminal, even if it happened before consuming any sample.

## Errors and EOF

WAVE retries `io::ErrorKind::Interrupted` underneath hound's byte parser. This
preserves position even when a multi-byte sample or header field is interrupted.
Other I/O errors, including `WouldBlock`, are terminal: this is a synchronous
offline reader, not a resumable nonblocking decoder. Partial results from the
failed call are discarded. Previously returned complete chunks remain valid.

After a decode/allocation failure, every subsequent read (including zero frames)
returns `ReaderFailed` without touching the source. Such failure is not EOF.
Construction failures return the original header error without a reader object.
Zero-frame requests on healthy readers and reads after successful EOF do not
consume input. Preflight count overflow does not poison the reader.

The libsndfile adapter checks `sf_error` after native reads, including reads
returning a short or zero count, before interpreting them as successful EOF.
The documented no-error value is zero; nonzero values become `SndFileError::Read`.
This follows the [libsndfile error/read API](https://libsndfile.github.io/libsndfile/api.html#error).
Invalid returned frame counts also fail closed. Corruption not diagnosed by
the underlying decoder is not newly guaranteed to be detected.

## Evidence and boundaries

Ten new tests cover:

- interrupted reads at every byte in small PCM8/16/24/32 headers and payloads;
- injected Other/UnexpectedEof/WouldBlock at every byte, preserving original
  errors and preventing further source reads;
- maximal representable requests on one-frame WAVE inputs;
- nearly 4 GB declared WAVE data with an enormous request and a truncated
  physical payload, without allocating a matching buffer;
- retryable count overflow, stable zero/EOF, and bounded multiblock native
  scratch including a 65,535-channel synthetic stream;
- maximal requests on actual tiny WAVE/AIFF/FLAC inputs through libsndfile;
- a real write-only libsndfile handle deliberately passed to the private reader
  test setup, proving the library's read-mode error is not mistaken for EOF.

All 416 native release workspace/all-target tests pass (one existing heavy test
is opt-in). All ten libsndfile crate tests also pass with
`SEX_REQUIRE_SNDFILE_TESTS=1`, which makes an unavailable runtime a failure.
Without that setting, runtime-dependent assertions explicitly print a skip
message; Rust's test harness can still display the containing test as `ok`.
Do not count those skipped assertions as execution evidence.

Eight new WAVE/state-machine tests execute successfully on emulated AArch64.
The two new real-libsndfile cases were **not** ARM runtime-qualified at the
initial checkpoint because that sysroot lacked libsndfile. The subsequent
[strict codec qualification](codec-read-qualification.md) installs the pinned
isolated runtime and executes all ten libsndfile tests on ARM with skips
forbidden, plus twelve bit-identical cross-architecture codec conversions.
Strict native/cross Clippy and formatting pass at the initial checkpoint.
Logs are under `target/read-boundary-{workspace,arm,arm-extra}-tests.log`,
`target/read-boundary-sndfile-required-tests.log`, and
`target/read-boundary-arm-sndfile-availability.log`.

The native 10,000-case I/O property corpus retains checksum
`222161181ef0c15b` (`target/read-boundary-io-10000.txt`). Native CLI WAVE-to-AIFF
and AIFF-to-WAVE conversion accepts `--block-frames 4294967295`; reading that
AIFF at blocks 1 and 4294967295 produces byte-identical WAVE SHA-256
`a8ad65abf0739d30b37aaed6792b9d1d2b5a32fb0bb24a8c29b1cbbfc86924be`.
Artifacts are in `target/read-boundary-cli-qualified`. The earlier request
9223372036854775807 was rejected by the existing u32 CLI parser; its diagnostic
is preserved and is not a successful conversion claim. Library methods accept
usize upper bounds; the CLI's u32 block-size syntax has not changed.

These are bounded fixtures and injection tests, not a full multi-gigabyte
traversal, all-decoder fuzzing, physical ARM qualification, or recovery from an
arbitrary damaged stream.
