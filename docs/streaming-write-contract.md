# WAVE write failures, accounting and container limits

The separate [libsndfile write contract](sndfile-write-contract.md) records
native status checks, atomic report accounting, returned-code close errors,
and subsequent cross-architecture output-identity verification.

The built-in WAVE adapter now matches the libsndfile adapter's fail-closed
writer policy: **any** error returned from a write call makes that writer
unusable. This includes invalid frame layout, arithmetic overflow, container
limits, count overflow and underlying write errors. Later calls, including an
empty write, return `AudioIoError::WriterFailed`; `finalize` also refuses to
return a successful report. The first call still returns its original error.

`WriteReport` describes fully successful calls only. Frame and saturation
counters are committed together after a complete Q1.63 block; signed-PCM writes
also commit their frame count only after success. Failed partial calls cannot
inflate saturation counts while leaving frame counts unchanged. Successful PCM
quantization, rounding, saturation and serialized file bytes are unchanged.

This is not rollback at the generic `Write + Seek` sink. A failed call may
already have written a prefix, and hound/underlying sink destruction may perform
best-effort header/close/buffer work. Callers must discard partial output and
require successful explicit finalization. The CLI already uses a temporary
output and atomic publication; it removes that temporary file on failure and
preserves an existing destination.

## Header and size checks

Before constructing hound's WAVE writer, the adapter checks that channel count
times bytes per sample fits the u16 block-alignment field, and that block
alignment times sample rate fits the u32 byte-rate field. Invalid combinations
return `WaveLayoutOverflow` before header bytes are written.

The built-in adapter is classic RIFF/WAVE, not RF64. Before each block, it
checks the prospective **total** completed frame count and encoded data size
using checked u64 arithmetic. Pinned hound 3.5.1 writes either:

| Header | Selection | Maximum data bytes |
| --- | --- | ---: |
| 44-byte PCM | At most 2 channels and at most PCM16 | `u32::MAX - 36` |
| 68-byte extensible | More channels or PCM24/32 | `u32::MAX - 60` |

RIFF's size field excludes its initial eight bytes. Data must also end at a
complete frame, so the maximum frame count is the integer floor of this data
limit divided by encoded bytes per frame. Crossing the limit returns
`WaveSizeLimit { data_bytes, maximum }` before writing the new block, and poisons
the writer. This prevents hound's unchecked u32 byte/header counters from
wrapping or panicking. It does not add RF64 or qualify large-file behavior of
the separate libsndfile container writer.

## Evidence

Five new library tests cover:

- errors at every byte of a small PCM8/16/24/32 stereo block, through both Q1.63
  quantization and signed-PCM entry points; unchanged committed reports and no
  further explicit writes after failure;
- partial frames and arithmetic errors, including failure after an earlier
  successfully written block, with unsuccessful finalization;
- final header seek/write failures and final flush failure;
- overflowing header combinations rejected before sink writes;
- exact RIFF size thresholds for mono/stereo/three-channel PCM8/16/24/32,
  checked against actual empty hound headers. Oversized completed-frame state
  is modeled in the test writer; no multi-gigabyte fixture is created.

A new CLI test truncates the final word of otherwise valid PCM16 after two
complete frames. An independent adapter probe confirms those first two frames
can be read and the third cannot. With one-frame CLI blocks, conversion fails,
the previous destination remains byte-identical and no `.sex-output-*` file
is left behind.

The full native release workspace/all-target suite passes 425 tests (the
existing heavy test remains opt-in); the strengthened truncated-input test
also passes independently. All five new writer tests execute on emulated ARM.
Native/cross warnings-denied Clippy and formatting pass. Test logs are under
`target/wave-write-boundary-qualified/{workspace,arm}-tests.log`.
The 10,000-case I/O corpus at root seed 42 also passes on native and ARM with
unchanged checksum `222161181ef0c15b`; full stdout compares byte-identically
(`target/wave-write-boundary-qualified/{native,arm}-io.txt`).

This qualification covers simulated byte failures, real bounded serialization,
and transactional CLI cleanup. It does not simulate power loss, prove filesystem
durability, perform a full 4 GB traversal, or make the generic sink transactional.
