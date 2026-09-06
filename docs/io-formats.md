# Audio I/O and metadata contract

Status: normative for the current built-in WAVE and runtime-loaded libsndfile
adapters.

## Separation from signal processing

Container parsing, codec calls, metadata, and PCM alignment live outside the Q
arithmetic and DSP crates. An input adapter must deliver complete interleaved
frames as Q1.63. An output adapter receives either Q1.63 plus an explicit
rounding/overflow policy or already quantized signed PCM. No adapter may expose
a float as the canonical sample representation.

The built-in `hound` adapter handles integer PCM WAVE and retains support for
declared widths from 1 through 32 bits. `sexio-sndfile` loads
the platform libsndfile shared library at runtime and currently exposes:

| Container | Input | Output |
| --- | --- | --- |
| WAVE | signed/unsigned 8-bit, PCM 16/24/32 | unsigned 8-bit, PCM 16/24/32 |
| AIFF | signed 8-bit, PCM 16/24/32 | signed 8-bit, PCM 16/24/32 |
| FLAC | integer subtypes reported by libsndfile | signed 8-bit, PCM 16/24 |

The CLI keeps the built-in writer for metadata-free WAVE, preserving the
project's locked WAVE byte regressions. AIFF/FLAC and tagged WAVE output use the
libsndfile adapter. Non-WAVE input currently uses libsndfile. WAVE audio samples
still use the built-in integer adapter, while an optional read-only libsndfile
probe imports known INFO tags. If the runtime library or a required symbol is
absent, metadata-free WAVE processing remains available and the probe returns
an empty metadata set.

Float WAVE, double, Vorbis, and other lossy or float-decoded libsndfile subtypes
are rejected by subtype before any sample enters the fixed-point path. This is
intentional: adding a decoder later requires a separately documented
reproducibility boundary rather than pretending its internal float operations
are part of the exact Q model.

## Integer alignment

The built-in WAVE adapter converts an exact signed B-bit PCM integer `s` to:

```text
Q1.63 raw = s << (64 - B)
```

libsndfile's 32-bit integer API presents integer PCM left-aligned in a signed
32-bit word. The adapter therefore performs:

```text
Q1.63 raw = libsndfile_i32 << 32
libsndfile_i32 = signed_B_bit_PCM << (32 - B)
```

Both directions are shifts, not floating-point scaling. Tests round-trip WAVE,
AIFF, and FLAC endpoints and interior values through the real installed shared
library.

## Typed metadata

Metadata is an ordered collection of unique typed UTF-8 entries. Current keys
are title, copyright, software, artist, comment, date, album, license, track
number, and genre. Duplicate keys, embedded NUL bytes, invalid UTF-8 returned by
the library, or a container refusing a requested tag are visible errors; tags
are not silently rewritten or discarded by the libsndfile writer.

The CLI copies metadata obtained from its input adapter to its output adapter.
FLAC title/artist and WAVE INFO title/artist preservation are covered by
end-to-end tests, including exact PCM equality and repeated-output byte
identity with the same runtime. The type is kept in `sexio`, not in the
libsndfile backend, so future adapters can share the same policy without
depending on a codec library.

## Transactional output

The CLI reserves a uniquely named file beside the requested output, writes and
finalizes the complete container there, synchronizes the file, and only then
renames it to the destination. Any earlier processing, clipping, allocation, or
codec error removes the temporary file on unwind. Existing destination bytes
are therefore not truncated by a failed conversion. Keeping the temporary file
in the destination directory also keeps publication on one filesystem, where
the host's rename replacement is atomic.

## Runtime and reproducibility boundary

Dynamic loading keeps builds independent of system headers and linker-time
development packages. Missing library or required symbols produce explicit
diagnostics. All unsafe C calls are isolated in one private module; public
handles close at most once, buffers are sized in frames times channels, and a
writer is poisoned after a failed block.

Both `sexio` adapters depend on the native-only `sexq` feature surface. They do
not pull GMP/MPFR into container parsing, and can therefore be cross-checked for
a target even when that target has no configured arbitrary-precision toolchain.

The loader tries `libsndfile.so.1`/`libsndfile.so` on ELF-style systems,
`libsndfile.1.dylib`/`libsndfile.dylib` on macOS, and
`sndfile.dll`/`libsndfile-1.dll` on Windows. Windows paths use libsndfile's
`sf_wchar_open` entry point with NUL-checked UTF-16 rather than narrowing an
`OsStr` through the byte-path API.

PCM results, Q arithmetic, scheduling, dither, and metadata policy remain
deterministic. Encoded AIFF/FLAC container bytes are regression-tested for
repeatability with the same libsndfile runtime. Cross-version byte identity of
an external FLAC encoder is not claimed. Every conversion report identifies
both I/O backends; libsndfile-backed endpoints include the exact string returned
by `sf_version_string`, while built-in WAVE endpoints identify the internal
integer adapter explicitly.
