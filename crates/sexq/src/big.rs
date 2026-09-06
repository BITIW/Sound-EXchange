use crate::{ArithmeticError, ArithmeticOutcome, Operation, OverflowPolicy, RoundingMode};
use core::fmt;
use rug::Integer;

const MAX_BIG_WIDTH_BITS: u32 = 1_048_576;

/// Arbitrary-precision Qm.n format. `m` includes the sign bit. The one-million
/// bit construction limit is a resource safety boundary, not a numeric backend
/// limitation, and comfortably includes Until-40k plans.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BigQFormat {
    integer_bits: u32,
    fractional_bits: u32,
    total_bits: u32,
}

impl BigQFormat {
    pub fn new(integer_bits: u32, fractional_bits: u32) -> Result<Self, BigQError> {
        if integer_bits == 0 {
            return Err(BigQError::ZeroIntegerBits);
        }
        let total_bits =
            integer_bits
                .checked_add(fractional_bits)
                .ok_or(BigQError::WidthOverflow {
                    integer_bits,
                    fractional_bits,
                })?;
        if total_bits > MAX_BIG_WIDTH_BITS {
            return Err(BigQError::WidthSafetyLimit {
                requested: total_bits,
                maximum: MAX_BIG_WIDTH_BITS,
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

    pub fn minimum_raw(self) -> Integer {
        -(Integer::from(1) << (self.total_bits - 1))
    }

    pub fn maximum_raw(self) -> Integer {
        (Integer::from(1) << (self.total_bits - 1)) - 1
    }
}

/// Signed arbitrary-precision fixed-point value with an explicit binary point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BigQ {
    raw: Integer,
    format: BigQFormat,
}

impl BigQ {
    pub fn zero(format: BigQFormat) -> Self {
        Self {
            raw: Integer::new(),
            format,
        }
    }

    pub fn one(format: BigQFormat) -> Result<Self, BigQError> {
        Self::from_raw(Integer::from(1) << format.fractional_bits, format)
    }

    pub fn from_i64(raw: i64, format: BigQFormat) -> Result<Self, BigQError> {
        Self::from_raw(Integer::from(raw), format)
    }

    pub fn from_i128(raw: i128, format: BigQFormat) -> Result<Self, BigQError> {
        Self::from_raw(Integer::from(raw), format)
    }

    pub fn from_raw(raw: Integer, format: BigQFormat) -> Result<Self, BigQError> {
        if raw < format.minimum_raw() || raw > format.maximum_raw() {
            return Err(BigQError::RawOutOfRange { raw, format });
        }
        Ok(Self { raw, format })
    }

    /// Import signed PCM with its exact binary point. A lower-precision
    /// destination requires the explicitly selected rounding/overflow policy.
    pub fn from_signed_pcm(
        value: i64,
        bits: u32,
        destination: BigQFormat,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, BigQError> {
        let (minimum, maximum) = crate::pcm_bounds(bits).map_err(BigQError::Pcm)?;
        if value < minimum || value > maximum {
            return Err(BigQError::Pcm(ArithmeticError::PcmValueOutOfRange {
                value,
                bits,
            }));
        }
        Self::from_i64(value, BigQFormat::new(1, bits - 1)?)?.rescale(destination, rounding, policy)
    }

    /// Quantize directly from this binary point to PCM, without a Q*.63
    /// staging value or a second rounding at the final format boundary.
    pub fn to_signed_pcm(
        &self,
        bits: u32,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<i64>, BigQError> {
        quantize_big_raw_to_signed_pcm(
            &self.raw,
            self.format.fractional_bits,
            bits,
            rounding,
            policy,
        )
    }

    /// Exact rational scaling at the same binary point. The numerator is
    /// signed; the denominator must be positive. Only the final quotient rounds.
    pub fn scale_ratio(
        &self,
        numerator: &Integer,
        denominator: &Integer,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, BigQError> {
        if denominator <= &0 {
            return Err(BigQError::NonPositiveDivisor);
        }
        let product = Integer::from(&self.raw * numerator);
        let raw = round_div_integer(&product, denominator, rounding)?;
        narrow_big_q(raw, self.format, policy, Operation::Multiply)
    }

    /// Interpret a non-negative storage word at exactly the declared two's-
    /// complement width and sign-extend it into GMP's signed representation.
    pub fn from_twos_complement_bits(bits: Integer, format: BigQFormat) -> Result<Self, BigQError> {
        if bits < 0 || bits.significant_bits() > format.total_bits {
            return Err(BigQError::BitPatternOutOfRange { bits, format });
        }
        let sign_bit = Integer::from(1) << (format.total_bits - 1);
        let raw = if bits >= sign_bit {
            bits - (Integer::from(1) << format.total_bits)
        } else {
            bits
        };
        Self::from_raw(raw, format)
    }

    pub const fn format(&self) -> BigQFormat {
        self.format
    }

    pub const fn raw(&self) -> &Integer {
        &self.raw
    }

    pub fn raw_i64(&self) -> Option<i64> {
        self.raw.to_i64()
    }

    pub fn raw_i128(&self) -> Option<i128> {
        self.raw.to_i128()
    }

    pub fn to_twos_complement_bits(&self) -> Integer {
        if self.raw < 0 {
            &self.raw + (Integer::from(1) << self.format.total_bits)
        } else {
            self.raw.clone()
        }
    }

    pub fn add(
        &self,
        rhs: &Self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, BigQError> {
        self.require_same_format(rhs)?;
        let raw = Integer::from(&self.raw + &rhs.raw);
        narrow_big_q(raw, self.format, policy, Operation::Add)
    }

    pub fn sub(
        &self,
        rhs: &Self,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, BigQError> {
        self.require_same_format(rhs)?;
        let raw = Integer::from(&self.raw - &rhs.raw);
        narrow_big_q(raw, self.format, policy, Operation::Subtract)
    }

    pub fn mul_to(
        &self,
        rhs: &Self,
        destination: BigQFormat,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, BigQError> {
        let product_fractional_bits = self
            .format
            .fractional_bits
            .checked_add(rhs.format.fractional_bits)
            .ok_or(BigQError::FractionalWidthOverflow)?;
        let product = Integer::from(&self.raw * &rhs.raw);
        let rescaled = rescale_integer(
            &product,
            product_fractional_bits,
            destination.fractional_bits,
            rounding,
        );
        narrow_big_q(rescaled, destination, policy, Operation::Multiply)
    }

    pub fn rescale(
        &self,
        destination: BigQFormat,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, BigQError> {
        let raw = rescale_integer(
            &self.raw,
            self.format.fractional_bits,
            destination.fractional_bits,
            rounding,
        );
        narrow_big_q(raw, destination, policy, Operation::Rescale)
    }

    pub fn shift_value(
        &self,
        exponent: i32,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, BigQError> {
        let raw = if exponent >= 0 {
            Integer::from(&self.raw << exponent as u32)
        } else {
            round_shift_integer(&self.raw, exponent.unsigned_abs(), rounding)
        };
        narrow_big_q(raw, self.format, policy, Operation::Rescale)
    }

    fn require_same_format(&self, rhs: &Self) -> Result<(), BigQError> {
        if self.format == rhs.format {
            Ok(())
        } else {
            Err(BigQError::FormatMismatch {
                left: self.format,
                right: rhs.format,
            })
        }
    }
}

/// Quantize an extended integer at an explicit binary point directly to PCM.
/// Dither/feedback may exceed the declared sample range before this boundary.
pub fn quantize_big_raw_to_signed_pcm(
    raw: &Integer,
    fractional_bits: u32,
    bits: u32,
    rounding: RoundingMode,
    policy: OverflowPolicy,
) -> Result<ArithmeticOutcome<i64>, BigQError> {
    let (minimum, maximum) = crate::pcm_bounds(bits).map_err(BigQError::Pcm)?;
    let rounded = rescale_integer(raw, fractional_bits, bits - 1, rounding);
    if rounded >= minimum && rounded <= maximum {
        return Ok(ArithmeticOutcome::exact(
            rounded.to_i64().expect("PCM bounds fit i64"),
        ));
    }
    match policy {
        OverflowPolicy::Error => Err(BigQError::Overflow(Operation::Quantize)),
        OverflowPolicy::Saturate => Ok(ArithmeticOutcome::saturated(if rounded < 0 {
            minimum
        } else {
            maximum
        })),
    }
}

fn narrow_big_q(
    raw: Integer,
    format: BigQFormat,
    policy: OverflowPolicy,
    operation: Operation,
) -> Result<ArithmeticOutcome<BigQ>, BigQError> {
    let minimum = format.minimum_raw();
    let maximum = format.maximum_raw();
    if raw >= minimum && raw <= maximum {
        return Ok(ArithmeticOutcome::exact(BigQ { raw, format }));
    }
    match policy {
        OverflowPolicy::Error => Err(BigQError::Overflow(operation)),
        OverflowPolicy::Saturate => Ok(ArithmeticOutcome::saturated(BigQ {
            raw: if raw < 0 { minimum } else { maximum },
            format,
        })),
    }
}

/// Round `value / 2^shift` using magnitude/remainder arithmetic. No behavior
/// depends on GMP's signed right-shift convention.
pub fn round_shift_integer(value: &Integer, shift: u32, mode: RoundingMode) -> Integer {
    if shift == 0 {
        return value.clone();
    }
    let negative = value < &0;
    let mut magnitude = value.clone();
    magnitude.abs_mut();
    let quotient = Integer::from(&magnitude >> shift);
    let reconstructed = Integer::from(&quotient << shift);
    let remainder = magnitude - reconstructed;
    let half = Integer::from(1) << (shift - 1);
    let increment = match mode {
        RoundingMode::TowardZero => false,
        RoundingMode::Floor => negative && remainder != 0,
        RoundingMode::Ceiling => !negative && remainder != 0,
        RoundingMode::NearestTiesAwayFromZero => remainder >= half,
        RoundingMode::NearestTiesToEven => {
            remainder > half || (remainder == half && quotient.is_odd())
        }
    };
    let mut rounded = quotient;
    if increment {
        rounded += 1;
    }
    if negative { -rounded } else { rounded }
}

/// Round `value / positive_divisor` without relying on a signed division
/// convention. This is the non-power-of-two counterpart of
/// [`round_shift_integer`].
pub fn round_div_integer(
    value: &Integer,
    positive_divisor: &Integer,
    mode: RoundingMode,
) -> Result<Integer, BigQError> {
    if positive_divisor <= &0 {
        return Err(BigQError::NonPositiveDivisor);
    }
    let negative = value < &0;
    let mut magnitude = value.clone();
    magnitude.abs_mut();
    let quotient = Integer::from(&magnitude / positive_divisor);
    let remainder = magnitude - Integer::from(&quotient * positive_divisor);
    let mut twice_remainder = remainder.clone();
    twice_remainder <<= 1;
    let increment = match mode {
        RoundingMode::TowardZero => false,
        RoundingMode::Floor => negative && remainder != 0,
        RoundingMode::Ceiling => !negative && remainder != 0,
        RoundingMode::NearestTiesAwayFromZero => twice_remainder >= *positive_divisor,
        RoundingMode::NearestTiesToEven => {
            twice_remainder > *positive_divisor
                || (twice_remainder == *positive_divisor && quotient.is_odd())
        }
    };
    let mut rounded = quotient;
    if increment {
        rounded += 1;
    }
    Ok(if negative { -rounded } else { rounded })
}

/// Scale a native extended integer by an exact rational through GMP, then
/// return it only when the rounded mathematical result fits `i128`.
pub fn scale_i128_ratio(
    value: i128,
    numerator: u128,
    denominator: u128,
    mode: RoundingMode,
) -> Result<i128, BigQError> {
    if denominator == 0 {
        return Err(BigQError::NonPositiveDivisor);
    }
    let product = Integer::from(value) * Integer::from(numerator);
    let rounded = round_div_integer(&product, &Integer::from(denominator), mode)?;
    rounded.to_i128().ok_or(BigQError::I128ResultOutOfRange)
}

fn rescale_integer(
    value: &Integer,
    from_fractional_bits: u32,
    to_fractional_bits: u32,
    rounding: RoundingMode,
) -> Integer {
    match to_fractional_bits.cmp(&from_fractional_bits) {
        core::cmp::Ordering::Equal => value.clone(),
        core::cmp::Ordering::Less => {
            round_shift_integer(value, from_fractional_bits - to_fractional_bits, rounding)
        }
        core::cmp::Ordering::Greater => {
            Integer::from(value << (to_fractional_bits - from_fractional_bits))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BigAccumulatorRequirements {
    pub worst_case_magnitude: Integer,
    pub signed_bits: u32,
}

impl BigAccumulatorRequirements {
    pub fn fits_signed_bits(&self, bits: u32) -> bool {
        bits >= self.signed_bits
    }
}

/// Attainable full-input-range MAC interval and its minimum signed width.
/// Each partial sum is inside this interval, since zero is a valid sample.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BigExactAccumulatorRequirements {
    pub minimum_raw: Integer,
    pub maximum_raw: Integer,
    pub signed_bits: u32,
}

impl BigExactAccumulatorRequirements {
    pub const fn fits_signed_bits(&self, bits: u32) -> bool {
        bits >= self.signed_bits
    }
}

/// Exact arbitrary-precision MAC. `accumulator_bits = None` is mathematically
/// unbounded; a concrete width enforces the planner's signed range after every
/// addition without rounding or saturation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BigMac {
    sample_format: BigQFormat,
    coefficient_format: BigQFormat,
    fractional_bits: u32,
    accumulator_bits: Option<u32>,
    accumulator_range: Option<(Integer, Integer)>,
    raw: Integer,
    poisoned: bool,
}

impl BigMac {
    /// Compute exact signed extrema, preserving the asymmetric sample range.
    /// The legacy `requirements_for` remains the conservative designer/cache API.
    pub fn exact_requirements_for(
        sample_format: BigQFormat,
        coefficient_format: BigQFormat,
        coefficients: &[BigQ],
    ) -> Result<BigExactAccumulatorRequirements, BigQError> {
        let mut positive = Integer::new();
        let mut negative = Integer::new();
        for coefficient in coefficients {
            if coefficient.format != coefficient_format {
                return Err(BigQError::FormatMismatch {
                    left: coefficient_format,
                    right: coefficient.format,
                });
            }
            if coefficient.raw < 0 {
                negative -= &coefficient.raw;
            } else {
                positive += &coefficient.raw;
            }
        }
        let magnitude = (positive.clone() + &negative) << (sample_format.total_bits - 1);
        let negative_magnitude: Integer = magnitude.clone() - negative;
        let maximum_raw = magnitude - positive;
        let negative_bits = if negative_magnitude == 0 {
            0
        } else {
            (negative_magnitude.clone() - 1_u32).significant_bits()
        };
        let signed_bits = 1 + negative_bits.max(maximum_raw.significant_bits());
        Ok(BigExactAccumulatorRequirements {
            minimum_raw: -negative_magnitude,
            maximum_raw,
            signed_bits,
        })
    }

    pub fn new(
        sample_format: BigQFormat,
        coefficient_format: BigQFormat,
        accumulator_bits: Option<u32>,
    ) -> Result<Self, BigQError> {
        if let Some(bits) = accumulator_bits
            && (bits == 0 || bits > MAX_BIG_WIDTH_BITS)
        {
            return Err(BigQError::AccumulatorWidthSafetyLimit {
                requested: bits,
                maximum: MAX_BIG_WIDTH_BITS,
            });
        }
        let fractional_bits = sample_format
            .fractional_bits
            .checked_add(coefficient_format.fractional_bits)
            .ok_or(BigQError::FractionalWidthOverflow)?;
        let accumulator_range = accumulator_bits.map(|bits| {
            let limit = Integer::from(1) << (bits - 1);
            (-limit.clone(), limit - 1)
        });
        Ok(Self {
            sample_format,
            coefficient_format,
            fractional_bits,
            accumulator_bits,
            accumulator_range,
            raw: Integer::new(),
            poisoned: false,
        })
    }

    pub const fn fractional_bits(&self) -> u32 {
        self.fractional_bits
    }

    pub const fn accumulator_bits(&self) -> Option<u32> {
        self.accumulator_bits
    }

    pub const fn raw(&self) -> &Integer {
        &self.raw
    }

    pub fn requirements_for(
        sample_format: BigQFormat,
        coefficient_format: BigQFormat,
        coefficients: &[BigQ],
    ) -> Result<BigAccumulatorRequirements, BigQError> {
        let sample_peak = Integer::from(1) << (sample_format.total_bits - 1);
        let mut coefficient_l1 = Integer::new();
        for coefficient in coefficients {
            if coefficient.format != coefficient_format {
                return Err(BigQError::FormatMismatch {
                    left: coefficient_format,
                    right: coefficient.format,
                });
            }
            let mut magnitude = coefficient.raw.clone();
            magnitude.abs_mut();
            coefficient_l1 += magnitude;
        }
        let worst_case_magnitude = sample_peak * coefficient_l1;
        let signed_bits = if worst_case_magnitude == 0 {
            1
        } else {
            worst_case_magnitude.significant_bits().saturating_add(1)
        };
        Ok(BigAccumulatorRequirements {
            worst_case_magnitude,
            signed_bits,
        })
    }

    pub fn accumulate(&mut self, sample: &BigQ, coefficient: &BigQ) -> Result<(), BigQError> {
        if self.poisoned {
            return Err(BigQError::PoisonedAccumulator);
        }
        let result = self.accumulate_inner(sample, coefficient);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Losslessly import a Q65.63 sample before the exact extended product.
    pub fn accumulate_wide(
        &mut self,
        sample: crate::WideQ63,
        coefficient: &BigQ,
    ) -> Result<(), BigQError> {
        let sample = BigQ::from_i128(sample.raw(), BigQFormat::new(65, 63)?)?;
        self.accumulate(&sample, coefficient)
    }

    /// Round once to the extended signal format. Never clip to Q1.63 here.
    pub fn finish_wide(self, rounding: RoundingMode) -> Result<crate::WideQ63, BigQError> {
        let value = self.finish(BigQFormat::new(65, 63)?, rounding, OverflowPolicy::Error)?;
        Ok(crate::WideQ63::from_raw(
            value
                .value
                .raw_i128()
                .ok_or(BigQError::I128ResultOutOfRange)?,
        ))
    }

    fn accumulate_inner(&mut self, sample: &BigQ, coefficient: &BigQ) -> Result<(), BigQError> {
        if sample.format != self.sample_format {
            return Err(BigQError::FormatMismatch {
                left: self.sample_format,
                right: sample.format,
            });
        }
        if coefficient.format != self.coefficient_format {
            return Err(BigQError::FormatMismatch {
                left: self.coefficient_format,
                right: coefficient.format,
            });
        }
        self.raw += Integer::from(&sample.raw * &coefficient.raw);
        if let Some(bits) = self.accumulator_bits {
            // The same inclusive signed endpoints are constant for this MAC.
            // Construct them once, not twice per FIR tap at arbitrary width.
            let (minimum, maximum) = self
                .accumulator_range
                .as_ref()
                .expect("bounded MAC has a range");
            if &self.raw < minimum || &self.raw > maximum {
                return Err(BigQError::AccumulatorOverflow { bits });
            }
        }
        Ok(())
    }

    pub fn finish(
        self,
        destination: BigQFormat,
        rounding: RoundingMode,
        policy: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<BigQ>, BigQError> {
        if self.poisoned {
            return Err(BigQError::PoisonedAccumulator);
        }
        let raw = rescale_integer(
            &self.raw,
            self.fractional_bits,
            destination.fractional_bits,
            rounding,
        );
        narrow_big_q(raw, destination, policy, Operation::Accumulate)
    }

    pub fn clear(&mut self) {
        self.raw = Integer::new();
        self.poisoned = false;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BigQError {
    ZeroIntegerBits,
    WidthOverflow {
        integer_bits: u32,
        fractional_bits: u32,
    },
    WidthSafetyLimit {
        requested: u32,
        maximum: u32,
    },
    FractionalWidthOverflow,
    NonPositiveDivisor,
    I128ResultOutOfRange,
    RawOutOfRange {
        raw: Integer,
        format: BigQFormat,
    },
    BitPatternOutOfRange {
        bits: Integer,
        format: BigQFormat,
    },
    FormatMismatch {
        left: BigQFormat,
        right: BigQFormat,
    },
    AccumulatorWidthSafetyLimit {
        requested: u32,
        maximum: u32,
    },
    AccumulatorOverflow {
        bits: u32,
    },
    Overflow(Operation),
    Pcm(ArithmeticError),
    PoisonedAccumulator,
}

impl fmt::Display for BigQError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroIntegerBits => f.write_str("big Q format needs a sign/integer bit"),
            Self::WidthOverflow {
                integer_bits,
                fractional_bits,
            } => write!(f, "Q{integer_bits}.{fractional_bits} width overflowed u32"),
            Self::WidthSafetyLimit { requested, maximum } => write!(
                f,
                "big Q width {requested} exceeds resource safety limit {maximum}"
            ),
            Self::FractionalWidthOverflow => {
                f.write_str("combined big Q fractional width overflowed u32")
            }
            Self::NonPositiveDivisor => f.write_str("integer divisor must be positive"),
            Self::I128ResultOutOfRange => {
                f.write_str("rounded arbitrary-precision result does not fit i128")
            }
            Self::RawOutOfRange { raw, format } => write!(
                f,
                "raw value {raw} is outside Q{}.{}",
                format.integer_bits, format.fractional_bits
            ),
            Self::BitPatternOutOfRange { bits, format } => write!(
                f,
                "storage word {bits} exceeds Q{}.{} width",
                format.integer_bits, format.fractional_bits
            ),
            Self::FormatMismatch { left, right } => write!(
                f,
                "big Q format mismatch: Q{}.{} versus Q{}.{}",
                left.integer_bits, left.fractional_bits, right.integer_bits, right.fractional_bits
            ),
            Self::AccumulatorWidthSafetyLimit { requested, maximum } => write!(
                f,
                "accumulator width {requested} is outside 1..={maximum} bits"
            ),
            Self::AccumulatorOverflow { bits } => {
                write!(
                    f,
                    "exact MAC sum exceeds the configured signed {bits}-bit accumulator"
                )
            }
            Self::Overflow(operation) => write!(f, "{operation:?} overflowed destination Q format"),
            Self::Pcm(error) => error.fmt(f),
            Self::PoisonedAccumulator => {
                f.write_str("big Q accumulator is poisoned by an earlier error")
            }
        }
    }
}

impl std::error::Error for BigQError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_pcm_quantization_keeps_sub_q63_tie_breaking_information() {
        let format = BigQFormat::new(5, 256).unwrap();
        let raw: Integer = (Integer::from(65_533) << 240_u32) + 1; // 32766.5 PCM units plus 2^-256
        for sign in [1, -1] {
            let value = BigQ::from_raw(raw.clone() * sign, format).unwrap();
            assert_eq!(
                value
                    .to_signed_pcm(16, RoundingMode::NearestTiesToEven, OverflowPolicy::Error)
                    .unwrap()
                    .value,
                sign * 32767
            );
            let narrowed = value
                .rescale(
                    BigQFormat::new(65, 63).unwrap(),
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error,
                )
                .unwrap()
                .value;
            assert_eq!(
                narrowed
                    .to_signed_pcm(16, RoundingMode::NearestTiesToEven, OverflowPolicy::Error)
                    .unwrap()
                    .value,
                sign * 32766
            );
        }
    }

    #[test]
    fn arbitrary_pcm_import_export_validates_endpoints_and_never_wraps() {
        let format = BigQFormat::new(65, 4096).unwrap();
        for bits in [1, 8, 16, 24, 32, 64] {
            let (minimum, maximum) = crate::pcm_bounds(bits).unwrap();
            for raw in [minimum, maximum, 0, -1] {
                let sample = BigQ::from_signed_pcm(
                    raw,
                    bits,
                    format,
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error,
                )
                .unwrap();
                assert!(!sample.saturated);
                assert_eq!(
                    sample
                        .value
                        .to_signed_pcm(bits, RoundingMode::NearestTiesToEven, OverflowPolicy::Error)
                        .unwrap()
                        .value,
                    raw
                );
            }
        }
        let one = BigQ::one(format).unwrap();
        assert!(
            one.to_signed_pcm(24, RoundingMode::NearestTiesToEven, OverflowPolicy::Error)
                .is_err()
        );
        let clipped = one
            .to_signed_pcm(
                24,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Saturate,
            )
            .unwrap();
        assert_eq!((clipped.value, clipped.saturated), (8_388_607, true));
        assert!(
            BigQ::from_signed_pcm(
                128,
                8,
                format,
                RoundingMode::TowardZero,
                OverflowPolicy::Saturate
            )
            .is_err()
        );
        assert!(
            one.to_signed_pcm(0, RoundingMode::TowardZero, OverflowPolicy::Error)
                .is_err()
        );
        assert!(
            one.to_signed_pcm(65, RoundingMode::TowardZero, OverflowPolicy::Error)
                .is_err()
        );
        let integer_format = BigQFormat::new(2, 0).unwrap();
        assert_eq!(
            BigQ::one(integer_format)
                .unwrap()
                .to_signed_pcm(16, RoundingMode::TowardZero, OverflowPolicy::Saturate)
                .unwrap()
                .value,
            32767
        );
    }

    #[test]
    fn arbitrary_ratio_scaling_retains_the_declared_precision() {
        let format = BigQFormat::new(65, 4096).unwrap();
        let sample = BigQ::from_i64(5, format).unwrap();
        let two = Integer::from(2);
        let half = sample
            .scale_ratio(
                &Integer::from(-1),
                &two,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
        assert_eq!(half.value.raw_i64(), Some(-2));
        assert_eq!(half.value.format(), format);
        let huge = (Integer::from(1) << 8192_u32) + 123;
        let maximum = BigQ::from_raw(format.maximum_raw(), format).unwrap();
        assert_eq!(
            maximum
                .scale_ratio(
                    &huge,
                    &huge,
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error
                )
                .unwrap()
                .value,
            maximum
        );
        assert!(
            sample
                .scale_ratio(
                    &two,
                    &Integer::from(0),
                    RoundingMode::TowardZero,
                    OverflowPolicy::Error
                )
                .is_err()
        );
    }

    #[test]
    fn arbitrary_width_sign_extension_round_trips() {
        let format = BigQFormat::new(2, 510).unwrap();
        let mut minus_one_bits = Integer::from(1);
        minus_one_bits <<= 512;
        minus_one_bits -= 1;
        let minus_one = BigQ::from_twos_complement_bits(minus_one_bits.clone(), format).unwrap();
        assert_eq!(minus_one.raw(), &Integer::from(-1));
        assert_eq!(minus_one.to_twos_complement_bits(), minus_one_bits);
        assert!(BigQFormat::new(1, MAX_BIG_WIDTH_BITS).is_err());
    }

    #[test]
    fn arbitrary_rounding_modes_match_native_contract() {
        let half: Integer = Integer::from(1) << 199;
        let negative_half = -half.clone();
        assert_eq!(
            round_shift_integer(&half, 200, RoundingMode::NearestTiesToEven),
            0
        );
        assert_eq!(
            round_shift_integer(&negative_half, 200, RoundingMode::NearestTiesAwayFromZero),
            -1
        );
        assert_eq!(
            round_shift_integer(&Integer::from(-1), 200, RoundingMode::Floor),
            -1
        );
        assert_eq!(
            round_shift_integer(&Integer::from(-1), 200, RoundingMode::Ceiling),
            0
        );
    }

    #[test]
    fn rational_scaling_is_exact_and_uses_explicit_ties() {
        assert_eq!(
            scale_i128_ratio(5, 1, 2, RoundingMode::NearestTiesToEven),
            Ok(2)
        );
        assert_eq!(
            scale_i128_ratio(-5, 1, 2, RoundingMode::NearestTiesAwayFromZero),
            Ok(-3)
        );
        assert_eq!(
            scale_i128_ratio(i128::MAX, u128::MAX, u128::MAX, RoundingMode::TowardZero),
            Ok(i128::MAX)
        );
        assert_eq!(
            scale_i128_ratio(1, 1, 0, RoundingMode::TowardZero),
            Err(BigQError::NonPositiveDivisor)
        );
    }

    #[test]
    fn big_q_operations_are_explicit_and_saturating_only_by_policy() {
        let format = BigQFormat::new(2, 126).unwrap();
        let half = BigQ::from_raw(Integer::from(1) << 125, format).unwrap();
        let one_and_half = BigQ::from_raw(Integer::from(3) << 125, format).unwrap();
        let product = one_and_half
            .mul_to(
                &half,
                format,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
        assert_eq!(product.value.raw(), &(Integer::from(3) << 124));

        let maximum = BigQ::from_raw(format.maximum_raw(), format).unwrap();
        assert!(maximum.add(&half, OverflowPolicy::Error).is_err());
        let saturated = maximum.add(&half, OverflowPolicy::Saturate).unwrap();
        assert!(saturated.saturated);
        assert_eq!(saturated.value.raw(), &format.maximum_raw());
    }

    #[test]
    fn bigint_mac_supports_planned_8192_bit_accumulator() {
        let sample_format = BigQFormat::new(1, 511).unwrap();
        let coefficient_format = BigQFormat::new(2, 510).unwrap();
        let sample = BigQ::from_raw(Integer::from(1) << 510, sample_format).unwrap();
        let coefficient = BigQ::from_raw(Integer::from(1) << 510, coefficient_format).unwrap();
        let coefficients = vec![coefficient.clone(); 4_097];
        let requirements =
            BigMac::requirements_for(sample_format, coefficient_format, &coefficients).unwrap();
        assert_eq!(requirements.signed_bits, 1_035);
        assert!(requirements.fits_signed_bits(8_192));

        let mut mac = BigMac::new(sample_format, coefficient_format, Some(8_192)).unwrap();
        for coefficient in &coefficients {
            mac.accumulate(&sample, coefficient).unwrap();
        }
        let destination = BigQFormat::new(16, 511).unwrap();
        let output = mac
            .finish(
                destination,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
        assert_eq!(output.value.raw(), &(Integer::from(4_097) << 510));
    }

    #[test]
    fn bounded_big_mac_poisoning_is_visible() {
        let format = BigQFormat::new(2, 2).unwrap();
        let maximum = BigQ::from_raw(format.maximum_raw(), format).unwrap();
        let mut mac = BigMac::new(format, format, Some(4)).unwrap();
        assert!(mac.accumulate(&maximum, &maximum).is_err());
        assert_eq!(
            mac.accumulate(&maximum, &maximum),
            Err(BigQError::PoisonedAccumulator)
        );
        mac.clear();
        assert_eq!(mac.raw(), &Integer::from(0));
    }

    #[test]
    fn cached_mac_ranges_preserve_every_signed_endpoint_and_reset() {
        for bits in [1, 2, 31, 32, 63, 64, 127, 128, 8192] {
            let format = BigQFormat::new(bits + 1, 0).unwrap();
            let cf = BigQFormat::new(2, 0).unwrap();
            let one = BigQ::one(cf).unwrap();
            let minimum = -(Integer::from(1) << (bits - 1));
            let maximum: Integer = (Integer::from(1) << (bits - 1)) - 1;
            for raw in [Integer::new(), minimum.clone(), maximum.clone()] {
                let sample = BigQ::from_raw(raw.clone(), format).unwrap();
                let mut mac = BigMac::new(format, cf, Some(bits)).unwrap();
                mac.accumulate(&sample, &one).unwrap();
                assert_eq!(mac.raw(), &raw);
                assert_eq!(
                    mac.clone()
                        .finish(format, RoundingMode::Floor, OverflowPolicy::Error)
                        .unwrap()
                        .value
                        .raw(),
                    &raw
                );
                mac.clear();
                assert_eq!(mac.accumulator_bits(), Some(bits));
                assert_eq!(mac.raw(), &0);
                mac.accumulate(&sample, &one).unwrap();
                assert_eq!(mac.raw(), &raw);
            }
            for raw in [minimum - 1, maximum + 1] {
                let sample = BigQ::from_raw(raw, format).unwrap();
                let mut mac = BigMac::new(format, cf, Some(bits)).unwrap();
                assert_eq!(
                    mac.accumulate(&sample, &one),
                    Err(BigQError::AccumulatorOverflow { bits })
                );
                assert_eq!(
                    mac.accumulate(&sample, &one),
                    Err(BigQError::PoisonedAccumulator)
                );
                assert_eq!(
                    mac.clone()
                        .finish(format, RoundingMode::Floor, OverflowPolicy::Saturate),
                    Err(BigQError::PoisonedAccumulator)
                );
                mac.clear();
                mac.accumulate(&BigQ::zero(format), &one).unwrap();
                assert_eq!(mac.raw(), &0);
            }
        }
    }

    #[test]
    fn actual_8192_bit_mac_cancels_without_losing_sub_output_grid_terms() {
        let format = BigQFormat::new(3, 4096).unwrap();
        let cf = BigQFormat::new(2, 4096).unwrap();
        let large = BigQ::from_raw((Integer::from(1) << 4097) - 1, format).unwrap();
        let coefficient = BigQ::from_raw((Integer::from(1) << 4094) - 1, cf).unwrap();
        let negative = BigQ::from_raw(-coefficient.raw().clone(), cf).unwrap();
        let half = BigQ::from_raw(Integer::from(1) << 4095, cf).unwrap();
        let tiny = BigQ::from_i64(1, format).unwrap();
        for reverse in [false, true] {
            let (first, second) = if reverse {
                (&negative, &coefficient)
            } else {
                (&coefficient, &negative)
            };
            let mut mac = BigMac::new(format, cf, Some(8192)).unwrap();
            mac.accumulate(&large, first).unwrap();
            // This product genuinely requires 8192 signed bits, not merely a
            // small value stored in an accumulator configured to that width.
            assert_eq!(mac.raw().significant_bits(), 8191);
            let mut too_narrow = BigMac::new(format, cf, Some(8191)).unwrap();
            assert_eq!(
                too_narrow.accumulate(&large, first),
                Err(BigQError::AccumulatorOverflow { bits: 8191 })
            );
            mac.accumulate(&large, second).unwrap();
            assert_eq!(mac.raw(), &0);
            mac.accumulate(&tiny, &half).unwrap();
            mac.accumulate(&tiny, &half).unwrap();
            assert_eq!(mac.raw(), &(Integer::from(1) << 4096));
            assert_eq!(
                mac.finish(
                    format,
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error
                )
                .unwrap()
                .value
                .raw(),
                &1
            );
            // Rounding the two half-output-LSB products independently would
            // produce 0 + 0. Keeping their exact sum produces the required 1.
        }
    }
}
