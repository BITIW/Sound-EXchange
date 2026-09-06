# Exact sample-rate input

`sexrate::SampleRate` stores a positive, reduced frequency in Hz as `u64/u64`.
Parsing never uses a float or approximates a decimal. The public Rust facade
exposes it as `sex::rate::SampleRate`. It is distinct from `RateRatio`, which
represents a dimensionless output/input ratio used by the phase scheduler.

## Syntax and canonical meaning

| Input | Exact Hz | Typical use |
| --- | --- | --- |
| `48000`, `48000.000` | 48000 | Integer container rate |
| `48k`, `48K`, `.048M` | 48000 | Decimal SI suffixes |
| `4.8e4`, `4.8E+4` | 48000 | Exact scientific notation |
| `96000/2` | 48000 | Reduced integer fraction |
| `16537.5`, `33075/2`, `16.5375k` | 33075/2 | Fractional analysis frequency |

Fractions accept positive decimal integer numerator/denominator. Decimal
forms accept an optional decimal point, signed base-ten exponent and `k`/`K`
(10^3) or `M` (10^6) suffix. Lowercase `m` is not a supported rate suffix.
Signs on the mantissa, whitespace, separators, NaN/infinity, hexadecimal,
zero frequencies/denominators and malformed syntax are rejected. All equivalent
forms produce one canonical value and, for a fixed input frequency, one ratio.

The parser accepts at most 256 bytes. It uses bounded GMP integer temporaries
to reduce values before checking the final u64 numerator/denominator range.
This permits large cancelling fraction components and exact decimal expansions
such as 1/2^63, whose unreduced decimal mantissa does not fit u128. Decimal
scaling is capped at powers 10^-256..10^256; greater nonzero magnitudes cannot
reduce into the final range given the text limit. Exponents outside i32 are
rejected. These are explicit representation/resource limits, not rounding.

## Ratio construction

For output `a/b` Hz and input `c/d` Hz, compute `(a*d)/(b*c)`. Before multiplying,
cross-cancel `gcd(a,c)` and `gcd(d,b)`. Only then perform checked u64 products.
A still-unrepresentable ratio fails with `RatioOverflow`; no phase wraps or
floating-point fallback occurs. For example:

```text
16537.5 / 44100 = (33075/2) / 44100 = 3/8
141421.356237 / 44100 = 6734350297/2100000000
```

The ordinary rational clock, FIR phases, frame-count rules, planner and
certificate/cache identity all consume this reduced ratio unchanged.

## CLI and output-container boundary

`-r`, `--rate` and the conversion's positional `rate` effect share the exact
parser. Conversion accepts every spelling that reduces to a positive u32
integer Hz, subject to the existing selected adapter's further limits:

```sh
sex input.wav output.wav --rate 48k
sex input.wav output.wav rate 96000/2
sex analyze input.wav --rate 16537.5 --preset fast --certify-design
sex plan input.wav --rate 141421.356237 --preset fast
```

`plan` and `analyze` retain true fractional frequencies and derive the exact
ratio from the source's frequency. They explicitly label rates that cannot be
represented by the current integer-Hz output adapters. Equivalent spellings
share the same bank identity and coefficient cache. A large denominator or
phase count does not authorize a giant allocation: all existing designer,
response and certificate budgets still apply. Plan can describe an enormous
unqualified candidate without materializing its bank.

The current WAVE and libsndfile adapters expose integer-Hz headers. This is an
adapter contract, not a claim that every audio container in existence can only
represent integers. Conversion to these adapters rejects true fractional Hz
**before** opening a writer or creating a cache; it does not round a header,
mislabel playback speed, or modify an existing destination. Full fractional-Hz
container I/O is not implemented by this change. The resampler library can
process a supported rational ratio independently of file-container metadata.

## Validation: 2026-09-06

Five library tests exercise canonical spellings, exact fractional ratios,
pre-limit cancellation, thousands of decimal/exponent comparisons with integer
fraction oracles, large decimal mantissas, syntax, u64/u32 boundaries and ratio
overflow. Four CLI tests require byte-identical integer-Hz outputs across five
spellings/aliases, shared fractional analysis/cache identity, protected output
on unrepresentable-header rejection, and a materialization-free exotic plan.

The release workspace/all-target suite passes **400 tests**, with one existing
heavy opt-in test ignored. All five new rate tests pass under emulated AArch64;
native/cross strict all-target Clippy passes. Logs:
`target/exact-rate-workspace-tests.log`, `target/exact-rate-arm-tests.log`.

Complete fractional analysis stdout and stderr match x86-64 versus emulated
AArch64 using independent caches: 44100 Hz input, Fast, ratio 3/8,
`--certify-design`, decimal input `16.5375k` on native and fraction `33075/2`
on ARM. Results are in `target/exact-rate-cli-qualified`.
Stdout SHA-256: `3053c5a08d5ed83fa34e40d39c5b884cf3cba354bf636141b9cb97fac5592158`.
Stderr SHA-256: `b4c4da5b72c9d02e3172971e439a02710e06c40c4dd0ec767443b6b334f28fe9`.
This remains emulation with the documented QEMU-only linker accommodation,
not physical ARM or new continuous frequency-response qualification.
