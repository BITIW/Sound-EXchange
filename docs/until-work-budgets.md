# Until-40k default quality-work budgets

The CLI resolves cumulative response-refinement limits from the **initial
execution plan**, before designing or loading coefficients. The generic
`sexfir::refinement::Limits` API defaults are unchanged. Ordinary presets also
keep those defaults.

For Until-40k only, each unset work limit is:

- coefficients: `max(4_000_000, 4 × initial coefficient count)`;
- response terms: `max(200_000_000, 4 × initial coefficient count × 2 × grid)`.

These are four **initial-bank work equivalents**, not a guarantee that four
larger refined candidates will fit. They provide a bounded engineering reserve
for mandatory qualification and feedback. The independent defaults of eight
candidate attempts and 16384 MPFR bits are unchanged. The multipliers describe
work, not peak memory or a promised runtime. MPFR precision affects cost even
when the term count is unchanged.

Every explicit `--refinement-*` value wins independently. Supplying just an
attempt limit does not disable automatic coefficient/term limits. Flag ordering
and the redundant `--refine-quality` flag do not affect resolution. Arithmetic
overflow in an automatic budget is an error, never wrapping or saturation.

`plan`, `analyze` and conversion use the same resolver. The response refiner
receives fixed limits for its entire search. Joint-design-certificate retries
receive explicitly frozen **remaining** budgets, so rebuilding the execution
plan cannot refill exhausted quotas. Designer, certificate and harmonic safety
limits remain independent; no spectral gate, coefficient precision, filter
length, transition width or attenuation target is weakened by this policy.

For 44100→48000 at the default Until-40k grid of 65 points per band:

| Work | Initial candidate | Resolved cumulative limit |
| --- | ---: | ---: |
| Coefficients (160 × 66867 taps) | 10,698,720 | 42,794,880 |
| Response terms | 1,390,833,600 | 5,563,334,400 |
| MPFR bits | 1024 | 16384 |

`sex plan` prints the actual estimates and limits without allocating this bank.
Conversion prints resolved Until-40k limits before materialization. Passing
preflight is **not** spectral qualification or a successful render. The common
ratio's actual execution evidence is tracked separately in the
[original-goal audit](goal-audit-open-items.md).

Unit tests exercise the real refiner's preflight with a deliberately failing
materialization hook: automatic defaults reach that hook; explicit low
coefficient, response-term and precision limits reject before it. These tests
do not allocate or claim to assess the ten-million-coefficient bank.
