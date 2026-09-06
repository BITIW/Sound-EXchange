# Compile-time configurable Q formats

The original goal asks for Qm.n widths configurable both at compile time and
at runtime. Previously arbitrary native formats were runtime `QFormat` values;
only the specialized Q1.31/Q1.63/Q2.62 types fixed their layout at compile time.
The `sexq` public API now also exports `ConstQ<I, F>` and
`ConstMac<I, F, CI, CF>` for arbitrary compile-time native formats.

I includes the sign bit, I>=1 and I+F<=64. A raw integer r represents exactly
`r / 2^F`. `ConstQ` stores one i64 with transparent representation; the format
is part of its type and is not stored alongside each sample. Logical width and
physical storage are distinct: `ConstQ<1,31>` occupies eight bytes. The existing
specialized `Q1_31` remains the four-byte backend.

`ConstQ::<I,F>::FORMAT` is a validated associated constant. Invalid sign/width
parameters fail constant evaluation when that format is instantiated. Merely
naming an unused invalid generic type need not force Rust to evaluate it.
`QFormat::new`, native raw/bit-pattern import and typed import support const
evaluation, so applications can declare actual typed sample constants too.

## Arithmetic and conversions

There is no second arithmetic implementation: typed operations delegate to
the existing checked `NativeQ`/`NativeMac` kernels and return the same
`ArithmeticOutcome` (including saturation flags) or arithmetic error.

- `add` and `sub` require operands of the same type; mismatched binary points
  are compile errors, not implicit rescaling.
- `rescale::<OUTPUT_I, OUTPUT_F>` changes format explicitly.
- `mul_to::<OUTPUT_I, OUTPUT_F, RHS_I, RHS_F>` accepts a separately typed
  coefficient/input and rounds the exact product once into the destination.
- `shift_value` keeps the same format and uses the explicit rounding policy.
- `to_native` exports runtime metadata; `from_native` requires an exact layout
  match and never rescales.
- Two's-complement import/export uses exactly the declared logical width,
  including sign extension and rejection of high padding bits.

All five rounding modes and both error/saturate policies remain available.
No wrapping or implicit arithmetic-operator implementation is introduced.

`ConstMac` binds sample and coefficient formats in its type, retains exact i128
products/sums and rounds only in `finish::<OUTPUT_I, OUTPUT_F>`. A wrong operand
type cannot be accumulated. Intermediate overflow poisons the MAC, regardless
of the final saturation policy; `clear` resets it. This remains a checked
native accumulator, not an automatic bigint fallback. The existing runtime
planner/native MAC requirement helpers remain the place to prove its capacity.

## Example

```rust
use sexq::{ConstMac, ConstQ, OverflowPolicy, RoundingMode};

type Sample = ConstQ<3, 12>;
type Coefficient = ConstQ<2, 14>;
let sample = Sample::from_raw(4096)?; // 1
let half = Coefficient::from_raw(8192)?; // 1/2
let mut mac = ConstMac::<3, 12, 2, 14>::new()?;
mac.accumulate(sample, half)?;
mac.accumulate(sample, half)?;
let output = mac.finish::<3, 12>(
    RoundingMode::NearestTiesToEven,
    OverflowPolicy::Error,
)?;
assert_eq!(output.value.raw(), 4096);
```

The public API documentation includes executable versions plus compile-fail
checks for mixed formats, zero sign width, excessive/overflowing width and
incorrect MAC coefficient types. Applications can access the types through
`sexq` directly or the existing `sex::q` facade.

This is a library capability, not a new CLI option or a change to the canonical
CLI signal model. Runtime `NativeQ` and GMP-backed `BigQ` remain available;
the new native types work without `sexq`'s default bigint feature. No float,
SIMD behavior change or speed improvement is claimed by this API addition.

## Qualification

All 447 native release workspace/all-target tests pass (one existing heavy
test remains opt-in). Three new tests cover actual const evaluation, logical
sign extension/ranges and storage, all rounding/overflow modes, mixed-format
products, explicit rescaling, extreme shifts, single-final-rounding MAC and
overflow poison/reset. They compare eight compile-time formats to the established
runtime kernel, including Q1.0, Q3.4, Q1.31, Q16.16, Q32.32, Q1.63, Q2.62 and
Q64.0. All three execute on emulated AArch64 too.

The native-only crate passes its full 25-test suite without GMP, plus two runnable
API examples and five compile-fail doctests. Native/cross warnings-denied Clippy
and formatting pass. Logs are retained under `target/const-q-qualified`.
Cross execution uses the existing QEMU validation environment; compile-fail
doctests were checked natively, not claimed as a separate ARM rustdoc campaign.
