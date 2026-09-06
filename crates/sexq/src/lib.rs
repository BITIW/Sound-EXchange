//! Deterministic fixed-point arithmetic for SeX.
//!
//! The first backend is [`Q1_63`]: a signed two's-complement value whose
//! numeric value is `raw / 2^63`. Multiplication widens to Q2.126 and
//! [`MacQ126`] keeps that full precision until the caller explicitly finishes
//! the accumulation. FIR coefficients use [`Q2_62`] so unity is exactly
//! representable; [`MacQ125`] accumulates Q1.63 × Q2.62 products. Safe APIs
//! never wrap silently.

use core::fmt;

#[cfg(feature = "bigint")]
mod big;
mod bounds;
mod constant;
pub use bounds::ExactAccumulatorRequirements;
mod simd;
#[cfg(feature = "bigint")]
pub use big::{
    BigAccumulatorRequirements, BigExactAccumulatorRequirements, BigMac, BigQ, BigQError,
    BigQFormat, quantize_big_raw_to_signed_pcm, round_div_integer, round_shift_integer,
    scale_i128_ratio,
};
pub use constant::{ConstMac, ConstQ};
pub use simd::{
    ExactDotBackend, ExactDotError, automatic_exact_dot_backend, exact_dot_backend_available,
    exact_dot_q1_63_q2_62, exact_dot_q1_63_q2_62_with_backend,
};

/// The operation which failed because its exact result was not representable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Add,
    Subtract,
    Multiply,
    Accumulate,
    LeftShift,
    Rescale,
    Quantize,
}

/// An arithmetic contract violation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArithmeticError {
    /// The exact result does not fit the destination or accumulator.
    Overflow(Operation),
    /// A right shift greater than 127 was requested for an `i128` value.
    InvalidRightShift(u32),
    /// PCM widths outside 1..=64 are not meaningful here.
    InvalidPcmWidth(u32),
    /// Q1.31 cannot import a PCM word wider than its 32-bit storage.
    PcmWidthExceedsQ1_31(u32),
    /// A supplied integer is outside the declared signed PCM width.
    PcmValueOutOfRange { value: i64, bits: u32 },
}

impl fmt::Display for ArithmeticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Overflow(operation) => write!(f, "{operation:?} overflow"),
            Self::InvalidRightShift(bits) => {
                write!(f, "cannot right-shift an i128 by {bits} bits")
            }
            Self::InvalidPcmWidth(bits) => {
                write!(f, "PCM width must be in 1..=64, got {bits}")
            }
            Self::PcmWidthExceedsQ1_31(bits) => {
                write!(f, "Q1.31 can import at most 32-bit PCM, got {bits}")
            }
            Self::PcmValueOutOfRange { value, bits } => {
                write!(f, "PCM value {value} is outside signed {bits}-bit range")
            }
        }
    }
}

impl std::error::Error for ArithmeticError {}

/// What to do when a value cannot be represented at an output boundary.
///
/// There is intentionally no wrapping variant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OverflowPolicy {
    /// Return [`ArithmeticError::Overflow`].
    #[default]
    Error,
    /// Clamp to the nearest representable endpoint and report it in the
    /// returned [`ArithmeticOutcome`].
    Saturate,
}

/// Explicit rounding used whenever fractional bits are discarded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RoundingMode {
    TowardZero,
    Floor,
    Ceiling,
    /// Round to nearest; an exact half goes to the even integer.
    #[default]
    NearestTiesToEven,
    /// Round to nearest; an exact half goes away from zero.
    NearestTiesAwayFromZero,
}

/// A value plus an auditable indication that saturation occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArithmeticOutcome<T> {
    pub value: T,
    pub saturated: bool,
}

/// Conservative full-scale accumulator requirement for one FIR phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccumulatorRequirements {
    /// Bound on the absolute raw widened accumulator value.
    pub worst_case_magnitude: u128,
    /// Signed two's-complement width needed to represent both signs of the
    /// conservative bound. This can be 129 even though the bound is `u128`.
    pub signed_bits: u16,
}

impl AccumulatorRequirements {
    pub const fn fits_i128(self) -> bool {
        self.worst_case_magnitude <= i128::MAX as u128
    }
}

impl<T> ArithmeticOutcome<T> {
    const fn exact(value: T) -> Self {
        Self {
            value,
            saturated: false,
        }
    }

    const fn saturated(value: T) -> Self {
        Self {
            value,
            saturated: true,
        }
    }
}

/// Runtime-configurable signed Qm.n format for the native integer backend.
/// `integer_bits` includes the sign bit; total storage width is `m + n` and
/// must fit an `i64`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct QFormat {
    integer_bits: u32,
    fractional_bits: u32,
    total_bits: u32,
}

impl QFormat {
    pub const Q1_31: Self = Self {
        integer_bits: 1,
        fractional_bits: 31,
        total_bits: 32,
    };
    pub const Q1_63: Self = Self {
        integer_bits: 1,
        fractional_bits: 63,
        total_bits: 64,
    };
    pub const Q2_62: Self = Self {
        integer_bits: 2,
        fractional_bits: 62,
        total_bits: 64,
    };

    pub const fn new(integer_bits: u32, fractional_bits: u32) -> Result<Self, NativeQError> {
        if integer_bits == 0 {
            return Err(NativeQError::ZeroIntegerBits);
        }
        let total_bits = match integer_bits.checked_add(fractional_bits) {
            Some(bits) => bits,
            None => {
                return Err(NativeQError::FormatWidthOverflow {
                    integer_bits,
                    fractional_bits,
                });
            }
        };
        if total_bits > 64 {
            return Err(NativeQError::FormatTooWide {
                integer_bits,
                fractional_bits,
            });
        }
        Ok(Self {
            integer_bits,
            fractional_bits,
            total_bits,
        })
    }

    pub const fn integer_bits(self) -> u32 {
        self.integer_bits
    }

    pub const fn fractional_bits(self) -> u32 {
        self.fractional_bits
    }

    pub const fn total_bits(self) -> u32 {
        self.total_bits
    }

    pub const fn minimum_raw(self) -> i64 {
        if self.total_bits == 64 {
            i64::MIN
        } else {
            -(1_i64 << (self.total_bits - 1))
        }
    }

    pub const fn maximum_raw(self) -> i64 {
        if self.total_bits == 64 {
            i64::MAX
        } else {
            (1_i64 << (self.total_bits - 1)) - 1
        }
    }
}

/// A native runtime Qm.n value. The format travels with the raw integer so a
/// mismatched operation is an error instead of an implicit rescale.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeQ {
    raw: i64,
    format: QFormat,
}

impl NativeQ {
    pub const fn from_raw(raw: i64, format: QFormat) -> Result<Self, NativeQError> {
        if raw < format.minimum_raw() || raw > format.maximum_raw() {
            return Err(NativeQError::RawOutOfRange { raw, format });
        }
        Ok(Self { raw, format })
    }

    /// Import an unsigned storage word and sign-extend exactly from the
    /// declared format width. Bits above that width are rejected.
    pub const fn from_twos_complement_bits(
        bits: u64,
        format: QFormat,
    ) -> Result<Self, NativeQError> {
        let raw = if format.total_bits == 64 {
            bits as i64
        } else {
            if bits >> format.total_bits != 0 {
                return Err(NativeQError::BitPatternOutOfRange { bits, format });
            }
            let sign_bit = 1_u64 << (format.total_bits - 1);
            if bits & sign_bit == 0 {
                bits as i64
            } else {
                let mask = (1_u64 << format.total_bits) - 1;
                (bits | !mask) as i64
            }
        };
        Self::from_raw(raw, format)
    }

    pub const fn raw(self) -> i64 {
        self.raw
    }

    pub const fn format(self) -> QFormat {
        self.format
    }

    pub const fn to_twos_complement_bits(self) -> u64 {
        if self.format.total_bits == 64 {
            self.raw as u64
        } else {
            let mask = (1_u64 << self.format.total_bits) - 1;
            self.raw as u64 & mask
        }
    }

    pub fn add(
        self,
        rhs: Self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, NativeQError> {
        self.require_same_format(rhs)?;
        narrow_native_q(
            i128::from(self.raw) + i128::from(rhs.raw),
            self.format,
            policy,
            Operation::Add,
        )
    }

    pub fn sub(
        self,
        rhs: Self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, NativeQError> {
        self.require_same_format(rhs)?;
        narrow_native_q(
            i128::from(self.raw) - i128::from(rhs.raw),
            self.format,
            policy,
            Operation::Subtract,
        )
    }

    /// Multiply into an explicit destination format. The `i64 * i64` product
    /// is exact in `i128`; only the final rescale is rounded.
    pub fn mul_to(
        self,
        rhs: Self,
        destination: QFormat,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, NativeQError> {
        let product = i128::from(self.raw) * i128::from(rhs.raw);
        let product_fractional_bits = self
            .format
            .fractional_bits
            .checked_add(rhs.format.fractional_bits)
            .ok_or(NativeQError::FractionalWidthOverflow)?;
        let rescaled = rescale_i128(
            product,
            product_fractional_bits,
            destination.fractional_bits,
            rounding,
        )?;
        narrow_native_q(rescaled, destination, policy, Operation::Multiply)
    }

    pub fn rescale(
        self,
        destination: QFormat,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, NativeQError> {
        let rescaled = rescale_i128(
            i128::from(self.raw),
            self.format.fractional_bits,
            destination.fractional_bits,
            rounding,
        )?;
        narrow_native_q(rescaled, destination, policy, Operation::Rescale)
    }

    /// Multiply by an exact power of two while keeping the same Q format.
    pub fn shift_value(
        self,
        exponent: i32,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, NativeQError> {
        let shifted = if exponent >= 0 {
            checked_left_shift_i128(i128::from(self.raw), exponent as u32, Operation::LeftShift)?
        } else {
            round_shift_i128(i128::from(self.raw), exponent.unsigned_abs(), rounding)?
        };
        narrow_native_q(shifted, self.format, policy, Operation::Rescale)
    }

    fn require_same_format(self, rhs: Self) -> Result<(), NativeQError> {
        if self.format == rhs.format {
            Ok(())
        } else {
            Err(NativeQError::FormatMismatch {
                left: self.format,
                right: rhs.format,
            })
        }
    }
}

fn narrow_native_q(
    value: i128,
    format: QFormat,
    policy: OverflowPolicy,
    operation: Operation,
) -> Result<ArithmeticOutcome<NativeQ>, NativeQError> {
    let minimum = i128::from(format.minimum_raw());
    let maximum = i128::from(format.maximum_raw());
    if value >= minimum && value <= maximum {
        return Ok(ArithmeticOutcome::exact(NativeQ {
            raw: value as i64,
            format,
        }));
    }
    match policy {
        OverflowPolicy::Error => Err(NativeQError::Arithmetic(ArithmeticError::Overflow(
            operation,
        ))),
        OverflowPolicy::Saturate => Ok(ArithmeticOutcome::saturated(NativeQ {
            raw: if value.is_negative() {
                format.minimum_raw()
            } else {
                format.maximum_raw()
            },
            format,
        })),
    }
}

/// Runtime native MAC for any pair of Q formats whose raw products and sum fit
/// `i128`. It never rounds an individual product.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeMac {
    sample_format: QFormat,
    coefficient_format: QFormat,
    fractional_bits: u32,
    raw: i128,
    poisoned: bool,
}

impl NativeMac {
    /// Exact full-input-range width, unlike the legacy symmetric L1 bound.
    pub fn exact_requirements_for(
        sample_format: QFormat,
        coefficient_format: QFormat,
        coefficients: &[NativeQ],
    ) -> Result<ExactAccumulatorRequirements, NativeQError> {
        for coefficient in coefficients {
            if coefficient.format != coefficient_format {
                return Err(NativeQError::FormatMismatch {
                    left: coefficient_format,
                    right: coefficient.format,
                });
            }
        }
        Ok(bounds::exact_native_requirements(
            sample_format.total_bits(),
            coefficients.iter().map(|c| c.raw()),
        )?)
    }

    pub fn new(sample_format: QFormat, coefficient_format: QFormat) -> Result<Self, NativeQError> {
        let fractional_bits = sample_format
            .fractional_bits
            .checked_add(coefficient_format.fractional_bits)
            .ok_or(NativeQError::FractionalWidthOverflow)?;
        Ok(Self {
            sample_format,
            coefficient_format,
            fractional_bits,
            raw: 0,
            poisoned: false,
        })
    }

    pub const fn raw(&self) -> i128 {
        self.raw
    }

    pub const fn fractional_bits(&self) -> u32 {
        self.fractional_bits
    }

    /// Conservative full-scale bound for the declared sample format and an
    /// actual coefficient vector. This is the runtime-Q analogue of the
    /// specialized FIR proof.
    pub fn requirements_for(
        sample_format: QFormat,
        coefficient_format: QFormat,
        coefficients: &[NativeQ],
    ) -> Result<AccumulatorRequirements, NativeQError> {
        let sample_peak = sample_format.minimum_raw().unsigned_abs() as u128;
        let mut coefficient_l1_raw = 0_u128;
        for coefficient in coefficients {
            if coefficient.format != coefficient_format {
                return Err(NativeQError::FormatMismatch {
                    left: coefficient_format,
                    right: coefficient.format,
                });
            }
            coefficient_l1_raw = coefficient_l1_raw
                .checked_add(coefficient.raw.unsigned_abs() as u128)
                .ok_or(NativeQError::Arithmetic(ArithmeticError::Overflow(
                    Operation::Accumulate,
                )))?;
        }
        let worst_case_magnitude =
            sample_peak
                .checked_mul(coefficient_l1_raw)
                .ok_or(NativeQError::Arithmetic(ArithmeticError::Overflow(
                    Operation::Accumulate,
                )))?;
        let magnitude_bits = if worst_case_magnitude == 0 {
            0
        } else {
            u128::BITS - worst_case_magnitude.leading_zeros()
        };
        Ok(AccumulatorRequirements {
            worst_case_magnitude,
            signed_bits: (magnitude_bits + 1) as u16,
        })
    }

    pub fn clear(&mut self) {
        self.raw = 0;
        self.poisoned = false;
    }

    pub fn accumulate(
        &mut self,
        sample: NativeQ,
        coefficient: NativeQ,
    ) -> Result<(), NativeQError> {
        if self.poisoned {
            return Err(NativeQError::PoisonedAccumulator);
        }
        let result = self.accumulate_inner(sample, coefficient);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn accumulate_inner(
        &mut self,
        sample: NativeQ,
        coefficient: NativeQ,
    ) -> Result<(), NativeQError> {
        if sample.format != self.sample_format {
            return Err(NativeQError::FormatMismatch {
                left: self.sample_format,
                right: sample.format,
            });
        }
        if coefficient.format != self.coefficient_format {
            return Err(NativeQError::FormatMismatch {
                left: self.coefficient_format,
                right: coefficient.format,
            });
        }
        let product = i128::from(sample.raw) * i128::from(coefficient.raw);
        self.raw = self
            .raw
            .checked_add(product)
            .ok_or(NativeQError::Arithmetic(ArithmeticError::Overflow(
                Operation::Accumulate,
            )))?;
        Ok(())
    }

    pub fn finish(
        self,
        destination: QFormat,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<NativeQ>, NativeQError> {
        if self.poisoned {
            return Err(NativeQError::PoisonedAccumulator);
        }
        let rescaled = rescale_i128(
            self.raw,
            self.fractional_bits,
            destination.fractional_bits,
            rounding,
        )?;
        narrow_native_q(rescaled, destination, policy, Operation::Accumulate)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeQError {
    ZeroIntegerBits,
    FormatWidthOverflow {
        integer_bits: u32,
        fractional_bits: u32,
    },
    FormatTooWide {
        integer_bits: u32,
        fractional_bits: u32,
    },
    FractionalWidthOverflow,
    RawOutOfRange {
        raw: i64,
        format: QFormat,
    },
    BitPatternOutOfRange {
        bits: u64,
        format: QFormat,
    },
    FormatMismatch {
        left: QFormat,
        right: QFormat,
    },
    PoisonedAccumulator,
    Arithmetic(ArithmeticError),
}

impl fmt::Display for NativeQError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroIntegerBits => f.write_str("Q format needs at least one integer/sign bit"),
            Self::FormatWidthOverflow {
                integer_bits,
                fractional_bits,
            }
            | Self::FormatTooWide {
                integer_bits,
                fractional_bits,
            } => write!(
                f,
                "native Q{integer_bits}.{fractional_bits} exceeds the 64-bit backend"
            ),
            Self::FractionalWidthOverflow => {
                f.write_str("combined Q fractional width overflowed u32")
            }
            Self::RawOutOfRange { raw, format } => write!(
                f,
                "raw value {raw} is outside signed Q{}.{}",
                format.integer_bits, format.fractional_bits
            ),
            Self::BitPatternOutOfRange { bits, format } => write!(
                f,
                "bit pattern {bits:#x} exceeds the {}-bit Q{}.{} storage width",
                format.total_bits, format.integer_bits, format.fractional_bits
            ),
            Self::FormatMismatch { left, right } => write!(
                f,
                "Q format mismatch: Q{}.{} versus Q{}.{}",
                left.integer_bits, left.fractional_bits, right.integer_bits, right.fractional_bits
            ),
            Self::PoisonedAccumulator => {
                f.write_str("native Q accumulator is poisoned by an earlier error")
            }
            Self::Arithmetic(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for NativeQError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Arithmetic(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ArithmeticError> for NativeQError {
    fn from(value: ArithmeticError) -> Self {
        Self::Arithmetic(value)
    }
}

/// Round `value / 2^shift` without relying on implementation-specific signed
/// shift behaviour.
pub fn round_shift_i128(
    value: i128,
    shift: u32,
    mode: RoundingMode,
) -> Result<i128, ArithmeticError> {
    if shift == 0 {
        return Ok(value);
    }
    if shift > 127 {
        return Err(ArithmeticError::InvalidRightShift(shift));
    }

    let negative = value.is_negative();
    let magnitude = value.unsigned_abs();
    let divisor = 1_u128 << shift;
    let quotient = magnitude >> shift;
    let remainder = magnitude & (divisor - 1);

    let increment = match mode {
        RoundingMode::TowardZero => false,
        RoundingMode::Floor => negative && remainder != 0,
        RoundingMode::Ceiling => !negative && remainder != 0,
        RoundingMode::NearestTiesAwayFromZero => remainder >= divisor / 2,
        RoundingMode::NearestTiesToEven => {
            remainder > divisor / 2 || (remainder == divisor / 2 && quotient & 1 == 1)
        }
    };

    signed_from_magnitude(
        quotient + u128::from(increment),
        negative,
        Operation::Rescale,
    )
}

/// Change the number of fractional bits while preserving the represented
/// value. Increasing precision is exact; decreasing it uses `rounding`.
pub fn rescale_i128(
    value: i128,
    from_fractional_bits: u32,
    to_fractional_bits: u32,
    rounding: RoundingMode,
) -> Result<i128, ArithmeticError> {
    match to_fractional_bits.cmp(&from_fractional_bits) {
        core::cmp::Ordering::Equal => Ok(value),
        core::cmp::Ordering::Less => {
            round_shift_i128(value, from_fractional_bits - to_fractional_bits, rounding)
        }
        core::cmp::Ordering::Greater => checked_left_shift_i128(
            value,
            to_fractional_bits - from_fractional_bits,
            Operation::Rescale,
        ),
    }
}

fn checked_left_shift_i128(
    value: i128,
    shift: u32,
    operation: Operation,
) -> Result<i128, ArithmeticError> {
    match shift {
        0 => Ok(value),
        1..=126 => value
            .checked_mul(1_i128 << shift)
            .ok_or(ArithmeticError::Overflow(operation)),
        127 => match value {
            0 => Ok(0),
            -1 => Ok(i128::MIN),
            _ => Err(ArithmeticError::Overflow(operation)),
        },
        _ => Err(ArithmeticError::Overflow(operation)),
    }
}

fn signed_from_magnitude(
    magnitude: u128,
    negative: bool,
    operation: Operation,
) -> Result<i128, ArithmeticError> {
    if negative {
        if magnitude == 1_u128 << 127 {
            Ok(i128::MIN)
        } else {
            i128::try_from(magnitude)
                .map(|value| -value)
                .map_err(|_| ArithmeticError::Overflow(operation))
        }
    } else {
        i128::try_from(magnitude).map_err(|_| ArithmeticError::Overflow(operation))
    }
}

fn narrow_i64(
    value: i128,
    policy: OverflowPolicy,
    operation: Operation,
) -> Result<ArithmeticOutcome<i64>, ArithmeticError> {
    if let Ok(value) = i64::try_from(value) {
        return Ok(ArithmeticOutcome::exact(value));
    }
    match policy {
        OverflowPolicy::Error => Err(ArithmeticError::Overflow(operation)),
        OverflowPolicy::Saturate => Ok(ArithmeticOutcome::saturated(if value.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        })),
    }
}

fn narrow_i32(
    value: i128,
    policy: OverflowPolicy,
    operation: Operation,
) -> Result<ArithmeticOutcome<i32>, ArithmeticError> {
    if let Ok(value) = i32::try_from(value) {
        return Ok(ArithmeticOutcome::exact(value));
    }
    match policy {
        OverflowPolicy::Error => Err(ArithmeticError::Overflow(operation)),
        OverflowPolicy::Saturate => Ok(ArithmeticOutcome::saturated(if value.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        })),
    }
}

/// A native signed Q1.31 sample in the interval `[-1, 1)`.
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Q1_31(i32);

impl Q1_31 {
    pub const FRACTIONAL_BITS: u32 = 31;
    pub const MIN: Self = Self(i32::MIN);
    pub const MAX: Self = Self(i32::MAX);
    pub const ZERO: Self = Self(0);

    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> i32 {
        self.0
    }

    pub fn from_signed_pcm(value: i32, bits: u32) -> Result<Self, ArithmeticError> {
        if bits > 32 {
            return Err(ArithmeticError::PcmWidthExceedsQ1_31(bits));
        }
        let (minimum, maximum) = pcm_bounds(bits)?;
        if i64::from(value) < minimum || i64::from(value) > maximum {
            return Err(ArithmeticError::PcmValueOutOfRange {
                value: i64::from(value),
                bits,
            });
        }
        let shifted = i64::from(value) << (32 - bits);
        Ok(Self(shifted as i32))
    }

    pub const fn to_q1_63(self) -> Q1_63 {
        Q1_63::from_raw((self.0 as i64) << 32)
    }

    pub fn from_q1_63(
        value: Q1_63,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, ArithmeticError> {
        let rounded = round_shift_i128(i128::from(value.raw()), 32, rounding)?;
        narrow_i32(rounded, policy, Operation::Rescale).map(|outcome| ArithmeticOutcome {
            value: Self(outcome.value),
            saturated: outcome.saturated,
        })
    }

    pub fn add(
        self,
        rhs: Self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, ArithmeticError> {
        narrow_i32(
            i128::from(self.0) + i128::from(rhs.0),
            policy,
            Operation::Add,
        )
        .map(|outcome| ArithmeticOutcome {
            value: Self(outcome.value),
            saturated: outcome.saturated,
        })
    }

    pub fn sub(
        self,
        rhs: Self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, ArithmeticError> {
        narrow_i32(
            i128::from(self.0) - i128::from(rhs.0),
            policy,
            Operation::Subtract,
        )
        .map(|outcome| ArithmeticOutcome {
            value: Self(outcome.value),
            saturated: outcome.saturated,
        })
    }

    /// Return the exact signed Q2.62 product.
    pub const fn widening_mul(self, rhs: Self) -> i64 {
        self.0 as i64 * rhs.0 as i64
    }

    pub fn mul(
        self,
        rhs: Self,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, ArithmeticError> {
        let rounded = round_shift_i128(
            i128::from(self.widening_mul(rhs)),
            Self::FRACTIONAL_BITS,
            rounding,
        )?;
        narrow_i32(rounded, policy, Operation::Multiply).map(|outcome| ArithmeticOutcome {
            value: Self(outcome.value),
            saturated: outcome.saturated,
        })
    }
}

/// A canonical signed Q1.63 sample in the interval `[-1, 1)`.
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Q1_63(i64);

impl Q1_63 {
    pub const FRACTIONAL_BITS: u32 = 63;
    pub const MIN: Self = Self(i64::MIN);
    pub const MAX: Self = Self(i64::MAX);
    pub const ZERO: Self = Self(0);

    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Import signed integer PCM exactly. No rounding is performed.
    pub fn from_signed_pcm(value: i64, bits: u32) -> Result<Self, ArithmeticError> {
        let (minimum, maximum) = pcm_bounds(bits)?;
        if value < minimum || value > maximum {
            return Err(ArithmeticError::PcmValueOutOfRange { value, bits });
        }
        let shifted = checked_left_shift_i128(i128::from(value), 64 - bits, Operation::LeftShift)?;
        Ok(Self(
            i64::try_from(shifted).expect("validated PCM always maps to Q1.63"),
        ))
    }

    /// Quantize to signed integer PCM. Saturation is visible in the outcome.
    pub fn to_signed_pcm(
        self,
        bits: u32,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<i64>, ArithmeticError> {
        quantize_q1_63_raw_to_signed_pcm(i128::from(self.0), bits, rounding, policy)
    }

    pub fn add(
        self,
        rhs: Self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, ArithmeticError> {
        narrow_i64(
            i128::from(self.0) + i128::from(rhs.0),
            policy,
            Operation::Add,
        )
        .map(|outcome| ArithmeticOutcome {
            value: Self(outcome.value),
            saturated: outcome.saturated,
        })
    }

    pub fn sub(
        self,
        rhs: Self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, ArithmeticError> {
        narrow_i64(
            i128::from(self.0) - i128::from(rhs.0),
            policy,
            Operation::Subtract,
        )
        .map(|outcome| ArithmeticOutcome {
            value: Self(outcome.value),
            saturated: outcome.saturated,
        })
    }

    /// Return the exact Q2.126 product without rounding or narrowing.
    pub const fn widening_mul(self, rhs: Self) -> i128 {
        self.0 as i128 * rhs.0 as i128
    }

    pub fn mul(
        self,
        rhs: Self,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, ArithmeticError> {
        let rounded = round_shift_i128(self.widening_mul(rhs), Self::FRACTIONAL_BITS, rounding)?;
        narrow_i64(rounded, policy, Operation::Multiply).map(|outcome| ArithmeticOutcome {
            value: Self(outcome.value),
            saturated: outcome.saturated,
        })
    }
}

/// Extended Q65.63 sample used to preserve signal headroom between DSP stages.
/// It has the same binary point as [`Q1_63`] but uses the full signed `i128`
/// range, so conversion from Q1.63 is exact and conversion back is explicit.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct WideQ63(i128);

impl WideQ63 {
    pub const FRACTIONAL_BITS: u32 = 63;
    pub const MIN: Self = Self(i128::MIN);
    pub const MAX: Self = Self(i128::MAX);
    pub const ZERO: Self = Self(0);

    pub const fn from_raw(raw: i128) -> Self {
        Self(raw)
    }

    pub const fn from_q1_63(value: Q1_63) -> Self {
        Self(value.raw() as i128)
    }

    pub const fn raw(self) -> i128 {
        self.0
    }

    pub const fn unsigned_abs(self) -> u128 {
        self.0.unsigned_abs()
    }

    pub fn to_q1_63(
        self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Q1_63>, ArithmeticError> {
        narrow_i64(self.0, policy, Operation::Rescale).map(|outcome| ArithmeticOutcome {
            value: Q1_63::from_raw(outcome.value),
            saturated: outcome.saturated,
        })
    }

    /// Multiply by an exact non-negative rational and round once. GMP is used
    /// so even a Q65.63 endpoint times a 128-bit ratio cannot overflow an
    /// intermediate product.
    #[cfg(feature = "bigint")]
    pub fn scale_ratio(
        self,
        numerator: u128,
        denominator: u128,
        rounding: RoundingMode,
    ) -> Result<Self, BigQError> {
        scale_i128_ratio(self.0, numerator, denominator, rounding).map(Self)
    }
}

/// Quantize an extended Q1.63 value to signed PCM. Unlike [`Q1_63`], the raw
/// input may temporarily contain dither, noise-shaping feedback, or headroom
/// outside `[-1, 1)`. Overflow remains explicit at the PCM boundary.
pub fn quantize_q1_63_raw_to_signed_pcm(
    raw: i128,
    bits: u32,
    rounding: RoundingMode,
    policy: OverflowPolicy,
) -> Result<ArithmeticOutcome<i64>, ArithmeticError> {
    let (minimum, maximum) = pcm_bounds(bits)?;
    let rounded = round_shift_i128(raw, 64 - bits, rounding)?;
    if rounded >= i128::from(minimum) && rounded <= i128::from(maximum) {
        return Ok(ArithmeticOutcome::exact(rounded as i64));
    }
    match policy {
        OverflowPolicy::Error => Err(ArithmeticError::Overflow(Operation::Quantize)),
        OverflowPolicy::Saturate => Ok(ArithmeticOutcome::saturated(if rounded < 0 {
            minimum
        } else {
            maximum
        })),
    }
}

/// A signed Q2.62 coefficient in the interval `[-2, 2 - 2^-62]`.
///
/// Unlike Q1.63, this format represents `+1` exactly and leaves an additional
/// integer/guard bit in Q1.63 FIR multiplication.
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Q2_62(i64);

impl Q2_62 {
    pub const FRACTIONAL_BITS: u32 = 62;
    pub const MIN: Self = Self(i64::MIN);
    pub const MAX: Self = Self(i64::MAX);
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1_i64 << Self::FRACTIONAL_BITS);

    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Return the exact Q3.125 product of a Q1.63 sample and this coefficient.
    pub const fn widening_mul_sample(self, sample: Q1_63) -> i128 {
        self.0 as i128 * sample.raw() as i128
    }
}

fn pcm_bounds(bits: u32) -> Result<(i64, i64), ArithmeticError> {
    match bits {
        1..=63 => {
            let half_range = 1_i64 << (bits - 1);
            Ok((-half_range, half_range - 1))
        }
        64 => Ok((i64::MIN, i64::MAX)),
        _ => Err(ArithmeticError::InvalidPcmWidth(bits)),
    }
}

/// An exact native accumulator for Q1.63 × Q1.63 products.
///
/// The stored value has 126 fractional bits. Accumulator overflow is always
/// an error: saturating an intermediate sum would violate the FIR contract.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacQ126 {
    raw: i128,
}

/// An exact native accumulator for Q1.63 × Q2.62 FIR products.
///
/// The stored value has 125 fractional bits. Accumulator overflow is always
/// an error and output conversion performs exactly one rounding step.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacQ125 {
    raw: i128,
}

impl MacQ125 {
    /// Minimum signed width for independently full-range Q1.63 samples.
    pub fn exact_requirements_for(
        coefficients: &[Q2_62],
    ) -> Result<ExactAccumulatorRequirements, ArithmeticError> {
        bounds::exact_native_requirements(64, coefficients.iter().map(|c| c.raw()))
    }

    pub const FRACTIONAL_BITS: u32 = 125;

    pub const fn new() -> Self {
        Self { raw: 0 }
    }

    pub const fn raw(self) -> i128 {
        self.raw
    }

    pub fn requirements_for(
        coefficients: &[Q2_62],
    ) -> Result<AccumulatorRequirements, ArithmeticError> {
        let sample_peak = 1_u128 << Q1_63::FRACTIONAL_BITS;
        let mut worst_case_magnitude = 0_u128;
        for coefficient in coefficients {
            let term = sample_peak
                .checked_mul(coefficient.raw().unsigned_abs().into())
                .ok_or(ArithmeticError::Overflow(Operation::Accumulate))?;
            worst_case_magnitude = worst_case_magnitude
                .checked_add(term)
                .ok_or(ArithmeticError::Overflow(Operation::Accumulate))?;
        }
        let magnitude_bits = if worst_case_magnitude == 0 {
            0
        } else {
            u128::BITS - worst_case_magnitude.leading_zeros()
        };
        Ok(AccumulatorRequirements {
            worst_case_magnitude,
            signed_bits: (magnitude_bits + 1) as u16,
        })
    }

    pub fn clear(&mut self) {
        self.raw = 0;
    }

    pub fn accumulate(&mut self, sample: Q1_63, coefficient: Q2_62) -> Result<(), ArithmeticError> {
        self.accumulate_raw_product(coefficient.widening_mul_sample(sample))
    }

    pub fn accumulate_raw_product(&mut self, product: i128) -> Result<(), ArithmeticError> {
        self.raw = self
            .raw
            .checked_add(product)
            .ok_or(ArithmeticError::Overflow(Operation::Accumulate))?;
        Ok(())
    }

    pub fn finish(
        self,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Q1_63>, ArithmeticError> {
        let wide = self.finish_wide(rounding)?;
        narrow_i64(wide.raw(), policy, Operation::Accumulate).map(|outcome| ArithmeticOutcome {
            value: Q1_63(outcome.value),
            saturated: outcome.saturated,
        })
    }

    /// Finish the exact MAC at Q65.63 without narrowing away signal headroom.
    pub fn finish_wide(self, rounding: RoundingMode) -> Result<WideQ63, ArithmeticError> {
        let rounded = round_shift_i128(
            self.raw,
            Self::FRACTIONAL_BITS - Q1_63::FRACTIONAL_BITS,
            rounding,
        )?;
        Ok(WideQ63::from_raw(rounded))
    }
}

impl MacQ126 {
    /// Minimum signed width for independently full-range Q1.63 samples.
    pub fn exact_requirements_for(
        coefficients: &[Q1_63],
    ) -> Result<ExactAccumulatorRequirements, ArithmeticError> {
        bounds::exact_native_requirements(64, coefficients.iter().map(|c| c.raw()))
    }

    pub const FRACTIONAL_BITS: u32 = 126;

    pub const fn new() -> Self {
        Self { raw: 0 }
    }

    pub const fn raw(self) -> i128 {
        self.raw
    }

    /// Bound accumulator width for arbitrary full-scale Q1.63 inputs.
    ///
    /// This uses `2^63 * sum(abs(coefficient_raw))`, deliberately assuming
    /// the worst possible sign for every input sample. A designer may prove a
    /// tighter bound separately, but the execution engine must not guess one.
    pub fn requirements_for(
        coefficients: &[Q1_63],
    ) -> Result<AccumulatorRequirements, ArithmeticError> {
        let sample_peak = 1_u128 << Q1_63::FRACTIONAL_BITS;
        let mut worst_case_magnitude = 0_u128;
        for coefficient in coefficients {
            let term = sample_peak
                .checked_mul(coefficient.raw().unsigned_abs().into())
                .ok_or(ArithmeticError::Overflow(Operation::Accumulate))?;
            worst_case_magnitude = worst_case_magnitude
                .checked_add(term)
                .ok_or(ArithmeticError::Overflow(Operation::Accumulate))?;
        }
        let magnitude_bits = if worst_case_magnitude == 0 {
            0
        } else {
            u128::BITS - worst_case_magnitude.leading_zeros()
        };
        Ok(AccumulatorRequirements {
            worst_case_magnitude,
            signed_bits: (magnitude_bits + 1) as u16,
        })
    }

    pub fn clear(&mut self) {
        self.raw = 0;
    }

    pub fn accumulate(&mut self, sample: Q1_63, coefficient: Q1_63) -> Result<(), ArithmeticError> {
        self.accumulate_raw_product(sample.widening_mul(coefficient))
    }

    /// Add an already widened Q2.126 product without changing its scale.
    pub fn accumulate_raw_product(&mut self, product: i128) -> Result<(), ArithmeticError> {
        self.raw = self
            .raw
            .checked_add(product)
            .ok_or(ArithmeticError::Overflow(Operation::Accumulate))?;
        Ok(())
    }

    /// Perform the single permitted FIR output rounding and narrowing step.
    pub fn finish(
        self,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Q1_63>, ArithmeticError> {
        let rounded = round_shift_i128(self.raw, Q1_63::FRACTIONAL_BITS, rounding)?;
        narrow_i64(rounded, policy, Operation::Accumulate).map(|outcome| ArithmeticOutcome {
            value: Q1_63(outcome.value),
            saturated: outcome.saturated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q_formats_define_exact_ranges_and_sign_extension() {
        assert_eq!(QFormat::Q1_31.minimum_raw(), i64::from(i32::MIN));
        assert_eq!(QFormat::Q1_31.maximum_raw(), i64::from(i32::MAX));
        assert_eq!(QFormat::Q2_62.minimum_raw(), i64::MIN);
        assert_eq!(QFormat::new(3, 4).unwrap().total_bits(), 7);
        assert!(QFormat::new(0, 32).is_err());
        assert!(QFormat::new(2, 63).is_err());

        let format = QFormat::new(3, 4).unwrap();
        assert_eq!(
            NativeQ::from_twos_complement_bits(0b100_0000, format)
                .unwrap()
                .raw(),
            -64
        );
        assert_eq!(
            NativeQ::from_twos_complement_bits(0b111_1111, format)
                .unwrap()
                .raw(),
            -1
        );
        assert_eq!(
            NativeQ::from_raw(-1, format)
                .unwrap()
                .to_twos_complement_bits(),
            0b111_1111
        );
        assert!(NativeQ::from_twos_complement_bits(0b1000_0000, format).is_err());
    }

    #[test]
    fn native_runtime_q_never_wraps_or_implicitly_rescales() {
        let format = QFormat::new(2, 6).unwrap();
        let maximum = NativeQ::from_raw(format.maximum_raw(), format).unwrap();
        let one_lsb = NativeQ::from_raw(1, format).unwrap();
        assert!(maximum.add(one_lsb, OverflowPolicy::Error).is_err());
        let saturated = maximum.add(one_lsb, OverflowPolicy::Saturate).unwrap();
        assert!(saturated.saturated);
        assert_eq!(saturated.value, maximum);
        assert!(
            maximum
                .add(
                    NativeQ::from_raw(1, QFormat::new(3, 5).unwrap()).unwrap(),
                    OverflowPolicy::Error,
                )
                .is_err()
        );
    }

    #[test]
    fn native_runtime_q_rescale_multiply_and_shift_are_explicit() {
        let half_q1_31 = NativeQ::from_raw(1_i64 << 30, QFormat::Q1_31).unwrap();
        let half_q1_63 = half_q1_31
            .rescale(
                QFormat::Q1_63,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
        assert_eq!(half_q1_63.value.raw(), 1_i64 << 62);
        assert!(!half_q1_63.saturated);

        let source = QFormat::new(2, 6).unwrap();
        let one_and_half = NativeQ::from_raw(96, source).unwrap();
        let half = NativeQ::from_raw(32, source).unwrap();
        let product = one_and_half
            .mul_to(
                half,
                source,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
        assert_eq!(product.value.raw(), 48);
        assert_eq!(
            product
                .value
                .shift_value(1, RoundingMode::NearestTiesToEven, OverflowPolicy::Error,)
                .unwrap()
                .value
                .raw(),
            96
        );
    }

    #[test]
    fn runtime_mac_rounds_only_after_the_last_product() {
        let format = QFormat::new(1, 3).unwrap();
        let sample = NativeQ::from_raw(1, format).unwrap();
        let coefficient = NativeQ::from_raw(2, format).unwrap();
        let mut mac = NativeMac::new(format, format).unwrap();
        for _ in 0..4 {
            mac.accumulate(sample, coefficient).unwrap();
        }
        assert_eq!(mac.raw(), 8);
        assert_eq!(mac.fractional_bits(), 6);
        assert_eq!(
            mac.finish(
                format,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap()
            .value
            .raw(),
            1
        );

        let requirement = NativeMac::requirements_for(
            format,
            format,
            &[coefficient, coefficient, coefficient, coefficient],
        )
        .unwrap();
        assert_eq!(requirement.worst_case_magnitude, 64);
        assert_eq!(requirement.signed_bits, 8);
    }

    #[test]
    fn q1_31_is_a_real_32_bit_backend() {
        assert_eq!(core::mem::size_of::<Q1_31>(), 4);
        assert_eq!(
            Q1_31::from_signed_pcm(i16::MIN.into(), 16).unwrap(),
            Q1_31::MIN
        );
        assert_eq!(Q1_31::MIN.to_q1_63(), Q1_63::MIN);
        assert_eq!(
            Q1_31::from_q1_63(
                Q1_63::from_raw(1_i64 << 62),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap()
            .value
            .raw(),
            1_i32 << 30
        );
        let half = Q1_31::from_raw(1_i32 << 30);
        assert_eq!(
            half.mul(half, RoundingMode::NearestTiesToEven, OverflowPolicy::Error,)
                .unwrap()
                .value
                .raw(),
            1_i32 << 29
        );
        assert!(
            Q1_31::MIN
                .mul(
                    Q1_31::MIN,
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error,
                )
                .is_err()
        );
    }

    #[test]
    fn signed_pcm_import_is_exact() {
        assert_eq!(
            Q1_63::from_signed_pcm(i16::MIN.into(), 16).unwrap(),
            Q1_63::MIN
        );
        assert_eq!(
            Q1_63::from_signed_pcm(i16::MAX.into(), 16).unwrap().raw(),
            i64::from(i16::MAX) << 48
        );
        assert_eq!(Q1_63::from_signed_pcm(-1, 24).unwrap().raw(), -1_i64 << 40);
    }

    #[test]
    fn signed_pcm_import_rejects_false_width_claims() {
        assert_eq!(
            Q1_63::from_signed_pcm(128, 8),
            Err(ArithmeticError::PcmValueOutOfRange {
                value: 128,
                bits: 8
            })
        );
        assert_eq!(
            Q1_63::from_signed_pcm(0, 0),
            Err(ArithmeticError::InvalidPcmWidth(0))
        );
    }

    #[test]
    fn rounding_modes_are_symmetric_and_explicit() {
        let half = 1_i128 << 62;
        assert_eq!(
            round_shift_i128(half, 63, RoundingMode::NearestTiesToEven),
            Ok(0)
        );
        assert_eq!(
            round_shift_i128(-half, 63, RoundingMode::NearestTiesToEven),
            Ok(0)
        );
        assert_eq!(
            round_shift_i128(3 * half, 63, RoundingMode::NearestTiesToEven),
            Ok(2)
        );
        assert_eq!(
            round_shift_i128(-3 * half, 63, RoundingMode::NearestTiesToEven),
            Ok(-2)
        );
        assert_eq!(
            round_shift_i128(-half, 63, RoundingMode::NearestTiesAwayFromZero),
            Ok(-1)
        );
        assert_eq!(round_shift_i128(-1, 1, RoundingMode::Floor), Ok(-1));
        assert_eq!(round_shift_i128(-1, 1, RoundingMode::Ceiling), Ok(0));
    }

    #[test]
    fn rounding_handles_i128_min_without_abs_overflow() {
        assert_eq!(
            round_shift_i128(i128::MIN, 127, RoundingMode::NearestTiesToEven),
            Ok(-1)
        );
    }

    #[test]
    fn add_and_sub_never_wrap() {
        assert_eq!(
            Q1_63::MAX.add(Q1_63::from_raw(1), OverflowPolicy::Error),
            Err(ArithmeticError::Overflow(Operation::Add))
        );
        assert_eq!(
            Q1_63::MAX.add(Q1_63::from_raw(1), OverflowPolicy::Saturate),
            Ok(ArithmeticOutcome::saturated(Q1_63::MAX))
        );
        assert_eq!(
            Q1_63::MIN.sub(Q1_63::from_raw(1), OverflowPolicy::Saturate),
            Ok(ArithmeticOutcome::saturated(Q1_63::MIN))
        );
    }

    #[test]
    fn multiplication_widens_before_one_rounding_step() {
        let half = Q1_63::from_raw(1_i64 << 62);
        let quarter = half
            .mul(half, RoundingMode::NearestTiesToEven, OverflowPolicy::Error)
            .unwrap();
        assert_eq!(
            quarter,
            ArithmeticOutcome::exact(Q1_63::from_raw(1_i64 << 61))
        );
        assert_eq!(half.widening_mul(half), 1_i128 << 124);
    }

    #[test]
    fn positive_one_product_obeys_boundary_policy() {
        assert_eq!(
            Q1_63::MIN.mul(
                Q1_63::MIN,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error
            ),
            Err(ArithmeticError::Overflow(Operation::Multiply))
        );
        assert_eq!(
            Q1_63::MIN.mul(
                Q1_63::MIN,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Saturate
            ),
            Ok(ArithmeticOutcome::saturated(Q1_63::MAX))
        );
    }

    #[test]
    fn mac_rounds_once_after_all_products() {
        let mut mac = MacQ126::new();
        let half_lsb = 1_i128 << 62;
        mac.accumulate_raw_product(half_lsb).unwrap();
        mac.accumulate_raw_product(half_lsb).unwrap();
        assert_eq!(
            mac.finish(RoundingMode::NearestTiesToEven, OverflowPolicy::Error),
            Ok(ArithmeticOutcome::exact(Q1_63::from_raw(1)))
        );
    }

    #[test]
    fn accumulator_overflow_is_an_error_even_when_outputs_may_saturate() {
        let mut mac = MacQ126 { raw: i128::MAX };
        assert_eq!(
            mac.accumulate_raw_product(1),
            Err(ArithmeticError::Overflow(Operation::Accumulate))
        );
    }

    #[test]
    fn accumulator_planner_uses_coefficient_l1_bound() {
        let half = Q1_63::from_raw(1_i64 << 62);
        let requirement = MacQ126::requirements_for(&[half, half]).unwrap();
        assert_eq!(requirement.worst_case_magnitude, 1_u128 << 126);
        assert_eq!(requirement.signed_bits, 128);
        assert!(requirement.fits_i128());

        let too_wide = MacQ126::requirements_for(&[Q1_63::MIN, Q1_63::MIN]).unwrap();
        assert_eq!(too_wide.worst_case_magnitude, 1_u128 << 127);
        assert_eq!(too_wide.signed_bits, 129);
        assert!(!too_wide.fits_i128());
    }

    #[test]
    fn q2_62_represents_unity_exactly() {
        assert_eq!(Q2_62::ONE.raw(), 1_i64 << 62);
        let sample = Q1_63::from_raw(0x0123_4567_89ab_cdef);
        let mut mac = MacQ125::new();
        mac.accumulate(sample, Q2_62::ONE).unwrap();
        assert_eq!(
            mac.finish(RoundingMode::NearestTiesToEven, OverflowPolicy::Error),
            Ok(ArithmeticOutcome::exact(sample))
        );
    }

    #[test]
    fn q2_62_fir_bound_exposes_native_guard_bit() {
        let unity = MacQ125::requirements_for(&[Q2_62::ONE]).unwrap();
        assert_eq!(unity.worst_case_magnitude, 1_u128 << 125);
        assert_eq!(unity.signed_bits, 127);
        assert!(unity.fits_i128());

        let too_wide = MacQ125::requirements_for(&[Q2_62::MIN, Q2_62::MIN]).unwrap();
        assert_eq!(too_wide.worst_case_magnitude, 1_u128 << 127);
        assert_eq!(too_wide.signed_bits, 129);
        assert!(!too_wide.fits_i128());
    }

    #[test]
    fn pcm_quantization_reports_endpoint_saturation() {
        assert_eq!(
            Q1_63::MAX.to_signed_pcm(
                16,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Saturate
            ),
            Ok(ArithmeticOutcome::saturated(i16::MAX.into()))
        );
        assert_eq!(
            Q1_63::MIN.to_signed_pcm(16, RoundingMode::NearestTiesToEven, OverflowPolicy::Error),
            Ok(ArithmeticOutcome::exact(i16::MIN.into()))
        );
        assert_eq!(
            quantize_q1_63_raw_to_signed_pcm(
                i128::from(i64::MAX) + (1_i128 << 48),
                16,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Saturate,
            ),
            Ok(ArithmeticOutcome::saturated(i16::MAX.into()))
        );
    }

    #[test]
    fn rescale_left_shift_detects_mathematical_overflow() {
        assert_eq!(
            rescale_i128(i128::MAX, 0, 1, RoundingMode::NearestTiesToEven),
            Err(ArithmeticError::Overflow(Operation::Rescale))
        );
        assert_eq!(
            rescale_i128(-1, 0, 127, RoundingMode::NearestTiesToEven),
            Ok(i128::MIN)
        );
    }
}
