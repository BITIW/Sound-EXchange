# Bit-identical Remez cosine reuse

Remez previously recomputed the same design-grid cosine values in every
interpolation matrix and every full-grid error evaluation. The optional
in-memory cosine table computes each value once using the same MPFR operations
and working precision, then clones that value when needed. It introduces no
trigonometric recurrence, different rounding, SIMD or replacement solver.
Gaussian elimination, sum/product order, signed extrema and convergence are
unchanged. The signal path remains integer-only.

For G grid points and A prototype amplitudes, the table has G*A MPFR values.
It reduces repeated transcendental evaluation across exchange iterations,
but does not remove the cubic global solve or selector complexity. It is not
a scalable barycentric Remez implementation or a claim that Until-40k global
designs now fit practical resources.

## Resource policy

The arbitrary designer first preflights its existing uncached core, work,
coefficient, grid and precision limits. It enables the complete table only
when the remaining storage allowance fits its conservative estimate:

```text
float_bytes = 64 + 8*ceil(P/64)
cosine_cache_bytes = G*A*float_bytes + 32*G
storage_bytes + cosine_cache_bytes <= max_storage_bytes
```

Here `Work.storage_bytes` remains the required uncached-core estimate.
The new `Work.cosine_cache_bytes` reports optional planned scratch storage,
zero when the table will not fit. Their sum is the estimated design peak;
these estimates are not an exact process-RSS guarantee. Overflow of the
optional estimate disables the table. An insufficient core budget remains a
typed preflight failure. There is no partial-table allocation, hidden increase
of a user's storage limit or rejection solely because the optional cache will
not fit. Native Q2.62 Remez uses a separate fixed 8 MiB optional-table ceiling.

The logical structural-work charge stays at the conservative uncached value,
so warm coefficient-cache hits and cosine-cached solves cannot bypass work
limits. The `Work` fields describe a planned design workspace even when a
persistent coefficient-cache hit means no solve is executed.

## Identity and persistent-cache contract

The table is process-local scratch, not a new persistent cache artifact. It
does not change any coefficient algorithm/domain, raw coefficient, iteration
count, extremal frequency, quantization measurement or solver report.
The existing persistent kind-4 payload does not store `Work`; loading recomputes
the plan under current limits. A bank written with the table enabled can be
loaded with a smaller, valid core-only allowance and vice versa. Actual-bank
quality gates still run after loading.

Qualification reports add the planned `cosine_cache_bytes` field. That is a
diagnostic extension, not a changed filter identity or proof scope. Earlier
reports should be compared after removing only this new field, rather than
mistaking a different whole-report hash for different audio or coefficients.

## Verification and benchmark

The differential test compares every cached and independently recomputed cosine,
complete error vectors, solver results, quantization reports and integer banks
at C=62, C=96 and C=4096 for unity and 2/3 ratios. It tests the exact storage
threshold and one byte below it, where the original path must remain usable.
Native and arbitrary Remez regression tests continue to compare coefficients
and measured results. A separate bounded benchmark runs the real global solve
through both paths and asserts identical banks and numerical/solver reports:

```sh
cargo run --release --example remez_cache_bench -- 129
cargo run --release --example remez_cache_bench -- 193
```

It uses ratio 2/3, C=96 and P=160. Timings are host-specific wall-clock
measurements, not a test assertion or an algorithmic complexity guarantee.
Recorded timings, source/binary identities and cross-architecture validation
are in [cross-architecture.md](cross-architecture.md).
