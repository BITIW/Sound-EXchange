# Coefficient cache contract

Status: normative for cache format `SEXCFIR1`.

High-precision FIR construction is deterministic but deliberately expensive.
SeX stores the complete quantized execution bank after its first design and
loads it on subsequent identical requests. The default directory is
`$XDG_CACHE_HOME/sex/coefficients`, or `$HOME/.cache/sex/coefficients` when XDG
is unset. `SEX_COEFFICIENT_CACHE=/path` overrides it and
`SEX_COEFFICIENT_CACHE=off` disables caching.

The request key is SHA-256 over an algorithm/version domain, reduced rational
rate, taps per phase, exact rational rolloff and Kaiser beta, MPFR working
precision, rounding mode, coefficient fractional width, and accumulator width.
Native and bigint Kaiser entries have separate domains (kinds 1 and 2).
Arbitrary-width non-Kaiser windows use kind 3 and `windowed-big-*.sexfir`:
their identities replace Kaiser beta with the selected algorithm/window and,
for Dolph, its attenuation, and explicitly bind the last-largest DC correction
rule. Native Dolph and bigint Dolph use the corrected v2 signed-bin algorithm.
Existing Kaiser entry bytes and identities are unchanged.

Entries are written to a unique temporary file in the destination directory,
flushed and synchronized, then atomically renamed. A second process winning the
same race is harmless because identical request keys imply identical bytes.

Native coefficients are stored as fixed little-endian signed 64-bit words.
Bigint coefficients are stored as fixed-width little-endian two's-complement
words whose width is derived from Q2.n. Fixed-width storage avoids per-tap text
or allocation metadata even for millions of coefficients.

A cache hit is not trusted blindly. Loading verifies:

- magic/version, backend kind, and embedded request identity;
- expected coefficient count and exact end-of-file;
- every bigint word against its declared Q format;
- the full coefficient SHA-256 recomputed from loaded integers;
- every phase's full-scale L1 accumulator bound;
- the stored and recomputed required accumulator widths.

Windowed bigint loading additionally checks exact per-phase DC sums. Static
designer limits, including the direct Dolph series budget, apply before cache
lookup, and every materialized CLI bank is requalified even on a hit. A cache
is coefficient reuse, not permission to omit response/certificate/image gates.
See [windowed-cli.md](windowed-cli.md).

A malformed or truncated entry fails explicitly rather than running with a
partially loaded filter. Cache status (`hit`, `miss; designed and stored`, or
`disabled`) is printed by conversion and analysis commands.

## Optimized global LS and Remez cache

The library entry points are `cache::design_optimized_big_cached(directory, spec,
limits)` and `cache::refine_optimized_quantization_cached(directory, initial,
target, auto_working_precision, search_limits)`. They use `SEXCFIR1` kind 4 and
`optimized-big-<request SHA-256>.sexfir`, separately from kinds 1/2/3 whose bytes
and keys are unchanged. The [optimized CLI](optimized-cli.md) uses the same
cache for every materialized quality-feedback candidate, with whole-chain
planning and repeated actual-bank spectral gates.

The request domain is `sex/sexfir/optimized-cache-request-v1`. Its hash binds
global LS v1 or Remez v2, reduced L/M, T, rolloff, both exact weights, design
grid or density/iteration cap, working P, coefficient C, accumulator width,
ties-to-even rounding and the last-largest exact-DC correction rule. The input
delay is derived from T. Limits are not part of the identity, but full current
preflight runs before any lookup, including warm hits.

Each entry contains:

| Field | Encoding |
| --- | --- |
| Magic, kind, request key | 8 bytes, 1 byte, 64 ASCII hex bytes |
| Coefficient count | little-endian u64 |
| DC correction, max error and L1 error (decimal/dB), coefficient identity | six length-prefixed UTF-8 strings |
| Required accumulator width | little-endian u32 |
| Solver report | kind 0: LS residual; kind 1: iterations, extrema count/frequencies, weighted prototype error |
| Coefficient word width | little-endian u32, derived from Q2.C |
| Coefficients | exactly L*T fixed-width little-endian two's-complement words |
| Payload checksum | 32 raw SHA-256 bytes over every preceding byte |

Solver strings use the existing bounded report serialization. Loading rejects
strings longer than 128 bytes before allocating them; extrema count must equal
the preflight solver dimension before allocating its vector. Numeric reports
must parse, error/residual values must be finite and nonnegative, max error may
not exceed max phase L1 error, and DC correction must fit the per-phase rounding
bound T. The special dB string `-inf` is allowed; NaN and other infinities are
rejected. Remez iteration count must be in 1..=configured maximum; its recorded
frequencies must strictly increase within [0,1/2], and weighted ripple must be
positive. The solution kind must match the requested method.

Loading also enforces count, exact word width, canonical unused high bits,
coefficient identity, exact per-phase DC, required/full-scale accumulator bound,
whole-payload checksum and exact EOF. Work estimates are recomputed from the
current request, not accepted from stored data. Serialization and hashing are
streamed through buffered I/O; no full-file copy is required for the checksum.
Entries use the existing synchronized temporary-file/atomic-publication path.
Corruption fails explicitly and leaves the offending entry unchanged; there
is no silent regeneration or fallback to another designer.

The checksum covers numerical and solver reports, not only coefficient codes.
It detects accidental corruption but is not cryptographic authentication: a
party controlling the cache can replace content and recompute public hashes.
This is trusted local artifact reuse, not proof of the original MPFR solve.
Report parsing is not continuous minimax certification or an MPFR error enclosure.
Actual-bank quality gates must still run after a hit. In particular, the stored
MPFR-reference L1 error is a measurement, not an authenticated ideal-filter bound.

Cached coefficient search uses the same candidate selection, C/P/accumulator
growth and stopping rules as uncached search. Every candidate is charged its
full structural work and coefficient count, even on a hit: cache availability
cannot bypass logical limits or alter precision selection. The materializer
must return the exact requested spec and recomputed work estimate. The result
remains coefficient-qualified only, not automatically spectrally qualified.

Tests cover both solvers at C=0/8/31/62/63/64/96/4096, cold/warm exact object
equality, truncation, changed header/request/count/report length, unused padding,
coefficient identity and payload checksum, extra EOF, resealed invalid numeric
and solver reports, accumulator and DC mismatches, parameter-key separation,
concurrent publication, cached search parity and limits applied to hits.
Additional failures cover oversized/invalid extrema vectors, failed design
without directory creation and failed publication preserving the prior entry
and removing its temporary file.

The fixed library corpus can be run cold, then warm, with:

```sh
cargo run --release --example optimized_probe -- --cache-directory PATH
```

It stores fifteen entries (thirteen inspected banks plus the two coarse C=8
search candidates), runs 39 integer-stream renders and qualifies five banks
per invocation. It is not a production conversion/analyze cache qualification.
The [2026-09-06 cross-architecture qualification](cross-architecture.md#2026-09-06-optimized-designer-cache-qualification)
compares independently populated caches and reciprocal warm reads, not just
direct designer output.
