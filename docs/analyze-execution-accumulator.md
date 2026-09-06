# Analysis reports the actual signal accumulator

Previously `sex analyze` reported the designer bank's baseline Q1.63
accumulator requirement and native coefficient-kernel name. Conversion already
promoted that bank for the configured signal format, potentially using a much
wider accumulator and a different execution backend. The old absolute width
and backend therefore did not describe the processing path.

Analysis and conversion now share signal-bank preparation. It applies the
signal-width extension to the execution plan, validates the actual arbitrary-
width bank and obtains its full-input-range L1 requirement. Conversion reuses
that prepared bank; analysis discards it after obtaining the same summary.
Big coefficient arrays remain shared, and the normal native conversion still
moves its coefficient bank rather than cloning it for this report.

The report distinguishes:

- **required bits:** the bank's full-declared-input-range bound (now the
  [exact signed interval](exact-accumulator-intervals.md), rather than the old
  symmetric L1 estimate);
- **planned bits:** the accumulator allocation planned for this signal format;
- **planned headroom:** planned minus required bits, checked for underflow;
- **execution backend:** the actual selected stream, including checked-i128
  with GMP fallback for Q65.63/native coefficients;
- **scope:** the complete declared Q-format range, not a measured audio peak.

The existing `plan` command retains its non-materializing estimates. This
change concerns the bank-bound `analyze` report. For unity rate, analysis still
explicitly describes a hypothetical FIR: normal same-rate conversion bypasses
that bank. Nothing here turns a sampled spectral report into a continuous proof
or measures final PCM/dither noise.

## Verified cases

The new integration test compares analysis against actual conversion for all
four designer-bank variants, with cold and cached analysis. It also verifies
that widening the same native-coefficient signal from F63 to F4096 increases
both widths by exactly 4033 bits.

These five full reports and their diagnostic output are byte-identical across
x86-64 and emulated ARM with independent caches:

| Configuration | Signal | Required | Planned | Spare bits |
| --- | --- | ---: | ---: | ---: |
| Fast Kaiser, 3/2 | Q65.63 | 192 | 192 | 0 |
| Fast Kaiser, 3/2 | Q65.4096 | 4225 | 4225 | 0 |
| High Kaiser, 3/2 | Q65.96 | 259 | 289 | 30 |
| Fast Hann, 3/2 | Q65.96 | 227 | 236 | 9 |
| Fast global-LS, 2/3 | Q65.96 | 227 | 235 | 8 |

Only the first uses checked-i128/GMP fallback; the others use GMP. Corpus input
is the prior tagged PCM24 WAVE at 44100 Hz; the explicit signal precision,
grid 9, no dither and clipping-error settings are shared. Artifacts are under
`target/analyze-execution-qualified`, with native/ARM `.txt` and `.log` pairs.

Full stdout SHA-256, in the table's order:

```text
259b44432904577de60dd7a082d7d46ebcdf55d39dac887ddc307a72d34fcdd6
f81adf85312af5fd320af9205071286b4aa0e8bda9d42deb1f59bc882a402add
9f5f3dbf1b6b44159a8b6cdfb0b30549c339b129cdfde68a11d645b2afdfea1d
4f52911f7fc072685d2267549880333dbcb1f7852c77d731fe02613f3264d337
4e791bdf0cfa40557c5cb9ea67b07b8d8a5d9897ec986d932d43f8ae44966ceb
```

All 429 native release workspace/all-target tests pass (one existing heavy
opt-in ignored), with strict native/cross Clippy and formatting passing.
The full-suite log is `target/analyze-execution-qualified/workspace-tests.log`.

A separate native High 160/147 PCM16 render, using a copy of the historical
coefficient cache, reproduces the earlier tagged WAVE byte for byte:
`41c4405a6c522f3f483c1908078b5019042cbaf558d7d5092be0b8164a82ba53`.
Its `high-baseline.wav`/`.log` are retained in the same directory. This checks
that shared preparation has not changed that actual signal result, while the
reported analysis width/backend are deliberately corrected.
