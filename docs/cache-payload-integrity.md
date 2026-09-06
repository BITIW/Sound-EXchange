# Whole-payload integrity for coefficient caches

Native Kaiser, bigint Kaiser and arbitrary-width windowed cache entries now
cover their **entire serialized payload** with a trailing 32-byte SHA-256.
This includes the request header, stored report values, accumulator bound,
coefficient identity and coefficient bytes. Previously the coefficient identity
protected the FIR but did not cover all cached numerical report strings.
An accidental change to a reported error could therefore survive loading.

The optimized LS/Remez cache already had this protection. Its format, request
identity and bytes are unchanged; all four kinds now reuse the same streaming
hash wrapper. Hashing does not buffer the complete cache in memory. Existing
atomic temporary-write, flush, sync and publication behavior is retained.

## Format and compatibility

The common eight-byte envelope magic stays `SEXCFIR1`; the kind byte distinguishes
formats. Full-payload kinds are native **5**, bigint Kaiser **6**, windowed **7**.
Legacy unprotected kinds 1/2/3 are not accepted in those slots. Optimized keeps
its existing kind 4 and footer. Request-key domains for the first three kinds
advance from v1 to v2; coefficient/design identities do not change.

Consequently the first use after this update **redesigns Kaiser/windowed banks
once under new cache keys**, potentially an expensive operation for Until-40k.
Later runs hit the new entries normally. Old files are neither deleted nor
modified. There is no blind migration that merely attaches a checksum to old,
unverified report values. Optimized entries remain reusable without redesign.

All integer fields use explicit little-endian encoding. Big coefficients use
fixed-width two's-complement byte digits in least-significant-byte-first order;
unused high padding bits must be canonical. No native machine-word encoding
is introduced by the digest. The footer hashes preceding bytes, not itself;
the loader requires the full footer and EOF, then independently checks the
existing coefficient identity and accumulator/format constraints.

A checksum is corruption detection, **not authentication** against someone who
can intentionally alter the cache and recompute its hashes. It also does not
turn a cached report into a mathematical certificate of the filter design.
The CLI's spectral/numerical gates remain independently applied on cache hits.

## Evidence

The new integrity regression covers native, big and windowed entries. It
changes the report value while leaving coefficients and their identity intact
and requires a payload-checksum rejection. It then flips one bit at every
serialized byte position, including the footer, and checks rejection of all
mutations, selected truncations and trailing data. Restoring the original file
restores a successful hit. Existing noncanonical-padding tests now address the
coefficient bytes before the footer, rather than accidentally mutating the hash.

A separate migration test reconstructs a legacy native entry with its original
request key and no digest. The next request designs a distinct v2 entry with
identical coefficients/report, leaves the legacy bytes unchanged, and then
gets a normal v2 cache hit. Directly substituting the old kind is rejected.

All 431 native release workspace/all-target tests pass (one existing heavy test
remains opt-in), including the unchanged optimized-cache regressions. Both new
tests execute on emulated AArch64. Strict native/cross Clippy and formatting
pass. Logs: `target/cache-payload-v2-qualified/{workspace,arm}-tests.log`.

Independent x86-64/ARM caches for Fast native, High bigint and Hann contain
byte-identical v2 files (four files total, including Hann feedback candidates).
All three complete analysis reports also match their earlier v1-cache-era
reports byte for byte; the numerical results have not changed. Native reads of
the ARM-produced caches report hits and match the cold reports except for that
hit/miss line. Artifacts and diagnostics are under
`target/cache-payload-v2-qualified`.

This is bounded cache-corruption and cross-architecture evidence, not a new
full Until-40k redesign campaign, malicious-cache authentication, physical ARM
qualification or a guarantee against all process/OS memory exhaustion.
