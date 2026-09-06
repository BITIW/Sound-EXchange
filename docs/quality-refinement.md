# Feedback-driven quality refinement

The planner produces an initial candidate, not a quality certificate. The
`sexfir::refinement` layer now designs and assesses that candidate, increases
coefficient precision or the selected window's shape/length when needed, and assesses the new
quantized bank. It never relaxes the requested error floor or moves a band edge.

## CLI contract

```text
sex input.wav -r 48000 output.wav --error-floor -300dB
sex input.wav -r 48000 output.wav --preset high
sex analyze input.wav -r 48000 --preset sane --grid 129
sex analyze input.wav -r 48000 --preset sane --certify --harmonics
sex plan input.wav -r 48000 --preset until-40k
```

Quality feedback is mandatory for every rate-changing conversion and every
analyzed FIR. `--error-floor` can strengthen the target but cannot undercut the
selected preset's minimum attenuation. `--refine-quality` remains a compatible but redundant
alias, and `--refinement-*` options adjust the budgets. `--window` selects
Kaiser or arbitrary-width rectangular/Hann/Blackman/Dolph; see
[windowed-cli.md](windowed-cli.md). `--designer global-ls|remez` selects
[optimized global prototypes](optimized-cli.md) with the same mandatory gates.
There is no CLI switch
to accept an unchecked bank. Same-rate conversion bypasses the resampler and
therefore has no FIR to qualify, even when explicit FIR budgets are tiny.

This deliberately replaces the preceding opt-in policy. Absurd and Pointless
at ratio 1/2 already had observed initial failures; requiring a special flag to
avoid them did not satisfy the preset contract. Their candidate-generation
constants remain unchanged, but default execution now selects the same
qualified banks as the preceding explicitly enabled feedback path. Existing
qualified first candidates keep their coefficient and output identities.

`plan` remains cheap: it neither designs nor accesses a coefficient cache, and
labels its initial candidate unqualified. `analyze` reports the final accepted
plan and actual bank. Conversion finishes qualification before building the
signal pipeline from that final plan. A failed conversion does not publish a
partial file or replace an existing output.

Planning prints first-candidate coefficient, response-term, and MPFR costs,
current limits, and necessary limit increases. For 44.1→48 kHz Until-40k the
first bank needs 10698720 coefficients and 1390833600 ordinary response terms.
The CLI's Until-40k defaults now reserve four initial-bank work equivalents
above generic floors: 42794880 coefficients and 5563334400 terms at this ratio.
Other presets and the library's generic 4000000/200000000 defaults are unchanged.
Explicit user limits remain hard per-field quotas, and retries cannot replenish
them. `plan` succeeds without designing a bank; actual conversion/analysis must
still qualify it. These budgets do not promise that larger refinements or
supplemental proofs will fit. See [the complete policy](until-work-budgets.md).

Each assessed candidate is logged with actual taps, coefficient width, MPFR
width, coefficient-budget verdict, response verdict, and proposed next change.
A proposed change may subsequently be rejected by a resource or explicit-width
limit; it does not mean that the next bank was materialized.
The trace names the requested gates and exclusions. Failed responses retain
sampled passband/stopband measurements, and failed/inconclusive continuous
checks retain exact witnesses or work/cell counts on stderr. Such failure
reports are not printed as a successful final analysis on stdout. To inspect
an initial candidate that fails, use `analyze --refinement-attempts 1`; it still
returns a nonzero status and claims no quality pass. The library's lower-level
designer/analyzers remain available for reference and regression work.

## Separate decisions for coefficient error and filter shape

Each attempt follows this sequence:

```text
whole-chain numerical planning -> resource preflight -> design/cache
    -> assess actual quantized bank
        coefficient budget fails: increase C and repeat
        response fails: strengthen the selected window/length and repeat
        all requested gates pass: retain this exact bank and final plan
        proof inconclusive or resource/error limit: fail explicitly
```

Let `T` be taps per phase, `C` the actual stored coefficient fractional width,
and `B = required_amplitude_bits(target)` the conservative integer dB-to-bits
conversion specified in [precision-planner.md](precision-planner.md).
The coefficient gate requires both the designer's measured quantization gate
and the exact inequality:

```text
(T - 1) * 2^-C <= 2^(-B-3)
```

The left side is a phase-L1 quantization bound relative to the
exact-DC-corrected MPFR reference: each of the other `T-1` nearest-rounded
coefficients contributes at most half an LSB, and the corrected coefficient
absorbs their total error. This bound is frequency independent and does not
include MPFR design-calculation error. Its derivation and reference are shared
with [numerical-error-budget.md](numerical-error-budget.md). The measured
designer report has its separately documented reference; it is an additional
gate, not a replacement for the exact bound.

If this coefficient budget fails, the next candidate requests `C_actual + 16`
fractional bits without changing length or beta. Native banks store Q2.62 even
when the plan specifies a lower minimum, so that decision uses 62, not the
smaller requested minimum. MPFR precision, sum guards, backend, and accumulator
requirements are replanned. An explicitly selected MPFR width is never
silently increased; insufficient width fails.

Only after the coefficient budget passes may a failed response strengthen the
filter shape. With the default Kaiser designer, design attenuation `A` and half-length `D`:

```text
A_next = A + 12 dB
D_next = ceil(D * (A_next - 8) / (A - 8))
T_next = 2 * D_next + 1
beta_next = max(beta, 0.1102 * (A_next - 8.7))
```

For rectangular/Hann/Blackman, `D_next=2*D`, without changing the window or its
parameters. Dolph uses that same doubling and raises its window attenuation by
12 dB. Every new candidate recalculates guard/accumulator requirements and
retains the selected method. The initial Kaiser length estimate is only a
heuristic for other windows. Their arbitrary-width CLI route uses C>=64, and
direct Dolph's per-candidate series cap is checked before design or cache read.
Global LS and Remez also double D without changing method, weights, or band
edges. Their structural solver work is bounded both per candidate and across
the search, including cache hits; see [optimized-cli.md](optimized-cli.md).

These operations use checked integer/rational arithmetic. The requested target,
rate ratio, rolloff, transition width, preset identity, and dither policy remain
unchanged. New tap counts also raise coefficient guard requirements where
necessary. This is a deterministic candidate-generation heuristic, not a proof
that the new bank passes, nor a search for the globally shortest FIR or minimum
coefficient width. Only its subsequent assessment may accept it.

## Which response is qualified?

Conversion and `analyze` evaluate every phase with the same `--grid` setting,
defaulting to 65 points per pass/stop band. Passing this grid does not bound unsampled peaks. For
upsampling, the ordinary per-phase stopband is only the input-Nyquist endpoint.
Feedback alone does not turn that scope into full anti-imaging qualification.

Two optional gates participate in the same loop for conversion and analysis:

- `--certify`: an exact continuous per-phase amplitude certificate supersedes
  the corresponding grid verdict. An exact violation can trigger a stronger
  candidate; an inconclusive proof stops the loop, with no grid-only fallback.
- `--harmonics`: all sampled wanted/image components must also meet their
  targets, including aggregate image power. A failure can trigger a stronger
  candidate. It remains sampled, not a continuous all-image certificate.

Supplemental analyses are skipped for candidates already known to fail their
coefficient budget. Their requested resource preflight still happens before
materialization. Final reports reuse the actual accepted assessment rather than
recomputing it. The certificate and harmonic contracts are in
[continuous-response-certificate.md](continuous-response-certificate.md) and
[harmonic-response-analysis.md](harmonic-response-analysis.md).

Different grids or supplemental gates can select different final candidates.
The [shared configuration](shared-quality-gates.md) now lets conversion use
the same `--grid`, `--certify`, and `--harmonics` settings as analysis. The final
bank-bound report is identical in both modes; compare its coefficient identity
and settings before relating results from differently configured runs. A
stricter analysis alone still does not change another invocation's settings.

The generic library hooks separate whole-chain preparation, cache/design, and
assessment. The CLI re-runs its coupled numerical planner before every attempt,
and checks the final plan again before constructing effects, FIR execution,
normalization, and dither. Raising FIR width or length therefore cannot leave
stale signal fractional bits, effect coefficients, or accumulator bounds in
the processing pipeline. FIR spectral response, computational numerical error,
and final PCM/dither/clipping remain separate scopes; `-300dB` does not promise
a 300 dB PCM recording.

## Resource and cache rules

| CLI limit | Default | Scope |
| --- | ---: | --- |
| `--refinement-attempts` | 8 | Assessed candidates; accepted range 1..128 |
| `--refinement-coefficients` | Normally 4000000; Until-40k: max of this and `4*initial_L*initial_T` | Cumulative `sum(L*T)` |
| `--refinement-terms` | Normally 200000000; Until-40k: max of this and `8*grid*initial_L*initial_T` | Cumulative ordinary-grid `sum(2*grid*L*T)` |
| `--refinement-precision-limit` | 16384 | MPFR bits per candidate |

Counts include cached and freshly designed candidates identically. Certificate
and harmonic limits are **per attempt**, additional to these cumulative
budgets. These are deterministic work/width guards, not hard wall-time or RSS
quotas. All counts and shape checks happen before that candidate's cache access
or materialization. In particular, a large Until-40k bank may require explicit
budget increases; the tool fails instead of silently weakening it.

The coefficient cache stores validated banks, not quality certificates.
Different widths or filter shapes have different existing cache identities;
failed-quality candidates may remain useful cached banks. Every hit is assessed
again against the current target and requested gates. A cache hit never bypasses
resource accounting or quality checks.

## Regression evidence

The bounded library tests establish both independent refinement directions:

- A deliberately coarse two-phase Fast bank increases from C=8 to C=24 in two
  assessed candidates while keeping its 129 taps and beta unchanged.
- The legacy Sane 1/3 bank fails its unchanged -110 dB continuous target, then
  grows from 769 to 861 taps with design attenuation 122 dB and receives an
  exact continuous certificate. This starts from the actual legacy length; the
  current preset's independently derived 865-tap bank is not changed.
- A currently qualified Sane bank is accepted unchanged. Independent cold/hot
  cache runs retain identical decisions, work counts, and final coefficients.
- Candidate/work caps, inconclusive proof, inadequate explicit precision,
  changed target geometry, and a mismaterialized bank fail explicitly.

A CLI-level preparation test couples the legacy failure with a `2^60` gain and
normalization, requiring a wider-than-native coefficient bank and recomputed
final signal/error plans. File-level tests verify mandatory default and explicit
targets, automatic Absurd/Pointless repair in analysis and actual conversion,
identical audio at blocks 1/7/4096, preservation of existing output on a default
Until-40k limit, same-rate bypass, and planning without design/cache work.

The remaining open work includes certified MPFR design-error enclosures,
practical long-FIR continuous proofs, continuous complete-image bounds,
broader solver-recovery qualification, robust very-large Remez convergence,
faster long-Dolph construction,
and broader preset/rate qualification.
The separate arbitrary-width [optimized library](optimized-designers.md) now
provides both methods and a bounded coefficient-only precision search; that
search does not replace the whole-filter gates described here.
The optimized library's [persistent cache](coefficient-cache.md#optimized-global-ls-and-remez-cache)
now covers every precision-search candidate without bypassing limits or quality checks.

### Actual preset failures found by the feedback corpus

The x86-64/AArch64 corpus exposed two previously unchecked initial candidates
at ratio 1/2. Both fail passband and stopband targets on the ordinary 65-point
grid; coefficient precision is already adequate. Both are accepted after one
shape increase, with their requested floors unchanged:

| Preset | Target dB | Initial taps / sampled stop dB | Accepted taps / sampled stop dB | C |
| --- | ---: | ---: | ---: | ---: |
| Absurd | -240 | 4097 / -237.374690 | 4309 / -260.020264 | 160 |
| Pointless | -360 | 8193 / -357.466097 | 8473 / -377.998232 | 256 |

Accepted maximum passband deviations are `6.418464302069545e-14` and
`1.432556212554106e-19`, respectively. These are sampled measurements, not
continuous certificates. Initial candidate definitions are unchanged; the
CLI regression retains each first-candidate failure and verifies the
accepted second candidate, unchanged target/C, and distinct cache entries.

The feedback-enabled corpus passes 14 scenarios / 56 renders on x86-64 and
AArch64 under QEMU, with independent designs, cold/hot cache interchange,
F=4096, block sizes 1/7/4096, and tagged WAVE/AIFF/FLAC. Only the two strengthened
preset rows change versus the preceding unchecked corpus; the other twelve
output identities agree. The durable manifest and executed-build identities
are recorded in [cross-architecture.md](cross-architecture.md).

The combined feedback + certificate + harmonic Sane 3/2 check also produces
byte-identical complete reports and caches on both architectures. It accepts
one 3×289-tap candidate; the continuous proof covers all three phases with
42 cells / 3756707 work units, and every sampled harmonic gate passes on a
9-point-per-band input grid. This exercises gate integration without implying
continuous bounds for those sampled images.

The heavy native Until-40k release regression was also rerun with feedback in
analysis and each of three conversions. Its 133733-tap C=512 / MPFR=1024 bank
passes in one candidate, retains coefficient identity
`70a22de17e9b06ce9702d0b4441ea72c9f95c749c8ea118793c672dc09967b2a`,
and produces identical Q65.512-to-PCM output at blocks 1/7/4096. The nine-frame
fixture and roughly 9 MB test cache are removed by the test. This is additional
native sampled qualification, not a new cross-architecture Until-40k render
or continuous long-FIR proof. The retained log is
`target/refinement-qualified/until-feedback-test.log`.

The subsequent mandatory-default checkpoint repeats all 14 scenarios / 56
renders without `--refine-quality`: every output and complete cache matches
the preceding opt-in feedback corpus. Native tests now pass 280 cases plus the
separate heavy Until-40k release test, whose conversions also omit the old flag.
Default/alias successful reports and caches, an inconclusive certificate
failure, and the large initial-plan budget report are additionally bit-identical
across x86-64/AArch64. Exact identities are in
[cross-architecture.md](cross-architecture.md#follow-up-mandatory-default-quality-qualification).
