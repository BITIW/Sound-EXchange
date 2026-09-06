# SoX reference audit

Reference snapshot: upstream SoX `master`, commit `f30947` (2024-05-30),
inspected from the [official SourceForge repository](https://sourceforge.net/p/sox/code/ci/master/tree/).
This document records ideas, not copied implementation.

## Findings and decisions

| Area | What SoX does | SeX decision |
|---|---|---|
| Canonical sample | `sox_sample_t` is signed 32-bit. Conversion macros round halves toward positive infinity and count clips. | Replace with explicit Q backends. Rounding and overflow are typed policies; wrap is absent from safe APIs. |
| Rate conversion | [`rate.c`](https://sourceforge.net/p/sox/code/ci/master/tree/src/rate.c) uses staged half-band/DFT filters and a polyphase FIR. Samples and coefficients are `double`; scheduling uses a 32.32 clock or `long double`. | Keep staged/polyphase concepts as reference. Rewrite sample/coefficient arithmetic and scheduling using fixed-point and exact rational state. |
| Effects | [`effects.c`](https://sourceforge.net/p/sox/code/ci/master/tree/src/effects.c) provides `start`, bounded `flow`, `drain`, and `stop`; buffers are streamed and channels are normally interleaved. | Keep lifecycle, bounded-buffer, and explicit drain ideas. Use Rust state machines and typed stream contracts, not handler cloning or C callbacks. |
| Coefficients | Rate coefficients are generated in floating point, arranged by phase, and shared between channels. | Keep immutable cross-channel coefficient sharing and phase-local layout. Designer produces quantized Q coefficients plus an error report and cache identity. |
| Dither | [`dither.c`](https://sourceforge.net/p/sox/code/ci/master/tree/src/dither.c) implements TPDF, sloped TPDF, FIR/IIR noise shaping, target precision, and clip counting. | Retain these modes as behavioural references. Specify a portable PRNG, explicit seed, exact fixed-point feedback, and selectable clip policy. |
| Formats/metadata | [`formats.c`](https://sourceforge.net/p/sox/code/ci/master/tree/src/formats.c) separates handlers, detection, encoding information, and comments from effects. | Preserve the boundary, initially behind libsndfile or an equivalent adapter. Keep metadata typed and independent from DSP. |
| Tests | SoX has sample-conversion edge tests plus sweep/tone tools that estimate response and error by least-squares fitting. | Retain endpoint, sweep, and fitted-error approaches; add bit-exact vectors, impulses, multitone, pathological ratios, chunk-boundary invariance, and cross-backend equivalence. |

## Explicitly not inherited

- `double` samples or coefficients in the signal path.
- Approximate rate ratios or floating-point phase clocks.
- Implicit, macro-specific rounding and clipping behaviour.
- Accumulator wrap or saturation hidden inside a FIR.
- Format plugins coupled directly to DSP internals.

The SoX source is LGPL/GPL depending on component. Until a deliberate licensing
review is complete, SeX uses it only to understand behaviour and test strategy;
no source is copied into this repository.
