//! Compile-time Qm.n types over the same checked native arithmetic kernel.
use crate::{
    ArithmeticOutcome, NativeMac, NativeQ, NativeQError, OverflowPolicy, QFormat, RoundingMode,
};

/// Signed QI.F with its binary point in the type, not in every stored sample.
/// I includes the sign bit; I>=1 and I+F<=64. Storage is always one i64;
/// use Q1_31 when a specialized four-byte representation is required.
///
/// ```
/// use sexq::{ConstQ, OverflowPolicy};
/// type Sample = ConstQ<3, 12>;
/// const ONE: Sample = match Sample::from_raw(1 << 12) {
///     Ok(value) => value,
///     Err(_) => panic!("invalid constant"),
/// };
/// assert_eq!(ONE.add(ONE, OverflowPolicy::Error).unwrap().value.raw(), 8192);
/// ```
///
/// Different binary points cannot be mixed implicitly:
/// ```compile_fail
/// use sexq::{ConstQ, OverflowPolicy};
/// let a = ConstQ::<3, 12>::from_raw(1).unwrap();
/// let b = ConstQ::<2, 13>::from_raw(1).unwrap();
/// a.add(b, OverflowPolicy::Error).unwrap();
/// ```
///
/// Invalid type parameters fail when the format is instantiated:
/// ```compile_fail
/// use sexq::ConstQ;
/// const BAD: sexq::QFormat = ConstQ::<0, 63>::FORMAT;
/// ```
/// ```compile_fail
/// use sexq::ConstQ;
/// const BAD: sexq::QFormat = ConstQ::<2, 63>::FORMAT;
/// ```
/// ```compile_fail
/// use sexq::ConstQ;
/// const BAD: sexq::QFormat = ConstQ::<{u32::MAX}, 1>::FORMAT;
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct ConstQ<const I: u32, const F: u32> {
    raw: i64,
}

impl<const I: u32, const F: u32> ConstQ<I, F> {
    pub const FORMAT: QFormat = match QFormat::new(I, F) {
        Ok(format) => format,
        Err(_) => panic!("compile-time Q format requires I>=1 and I+F<=64"),
    };

    pub const fn from_raw(raw: i64) -> Result<Self, NativeQError> {
        match NativeQ::from_raw(raw, Self::FORMAT) {
            Ok(value) => Ok(Self { raw: value.raw() }),
            Err(error) => Err(error),
        }
    }

    pub const fn from_twos_complement_bits(bits: u64) -> Result<Self, NativeQError> {
        match NativeQ::from_twos_complement_bits(bits, Self::FORMAT) {
            Ok(value) => Ok(Self { raw: value.raw() }),
            Err(error) => Err(error),
        }
    }

    pub const fn raw(self) -> i64 {
        self.raw
    }
    pub const fn to_native(self) -> NativeQ {
        NativeQ {
            raw: self.raw,
            format: Self::FORMAT,
        }
    }
    pub const fn to_twos_complement_bits(self) -> u64 {
        self.to_native().to_twos_complement_bits()
    }

    /// Require the runtime format to match exactly; this never rescales.
    pub const fn from_native(value: NativeQ) -> Result<Self, NativeQError> {
        let format = Self::FORMAT;
        if value.format().integer_bits() != I || value.format().fractional_bits() != F {
            return Err(NativeQError::FormatMismatch {
                left: format,
                right: value.format(),
            });
        }
        Ok(Self { raw: value.raw() })
    }

    fn outcome(result: ArithmeticOutcome<NativeQ>) -> ArithmeticOutcome<Self> {
        debug_assert_eq!(result.value.format(), Self::FORMAT);
        ArithmeticOutcome {
            value: Self {
                raw: result.value.raw(),
            },
            saturated: result.saturated,
        }
    }

    pub fn add(
        self,
        rhs: Self,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, NativeQError> {
        self.to_native()
            .add(rhs.to_native(), overflow)
            .map(Self::outcome)
    }
    pub fn sub(
        self,
        rhs: Self,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, NativeQError> {
        self.to_native()
            .sub(rhs.to_native(), overflow)
            .map(Self::outcome)
    }
    /// Destination I/F first, then the right operand's I/F. The exact i128
    /// product is rounded only at the explicit destination boundary.
    pub fn mul_to<const OI: u32, const OF: u32, const RI: u32, const RF: u32>(
        self,
        rhs: ConstQ<RI, RF>,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<ConstQ<OI, OF>>, NativeQError> {
        self.to_native()
            .mul_to(
                rhs.to_native(),
                ConstQ::<OI, OF>::FORMAT,
                rounding,
                overflow,
            )
            .map(ConstQ::<OI, OF>::outcome)
    }
    pub fn rescale<const OI: u32, const OF: u32>(
        self,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<ConstQ<OI, OF>>, NativeQError> {
        self.to_native()
            .rescale(ConstQ::<OI, OF>::FORMAT, rounding, overflow)
            .map(ConstQ::<OI, OF>::outcome)
    }
    pub fn shift_value(
        self,
        shift: i32,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Self>, NativeQError> {
        self.to_native()
            .shift_value(shift, rounding, overflow)
            .map(Self::outcome)
    }
}

/// Typed sample/coefficient formats with the existing checked i128 MAC.
/// Products are never rounded individually. Overflow poisons the accumulator;
/// clear resets it, and finish performs the only rounding to its typed output.
/// ```
/// use sexq::{ConstMac, ConstQ, OverflowPolicy, RoundingMode};
/// let sample = ConstQ::<3, 12>::from_raw(4096).unwrap(); // exactly one
/// let half = ConstQ::<2, 14>::from_raw(8192).unwrap();
/// let mut mac = ConstMac::<3, 12, 2, 14>::new().unwrap();
/// mac.accumulate(sample, half).unwrap();
/// mac.accumulate(sample, half).unwrap();
/// let output = mac.finish::<3, 12>(RoundingMode::NearestTiesToEven,
///     OverflowPolicy::Error).unwrap();
/// assert_eq!(output.value.raw(), 4096);
/// ```
/// ```compile_fail
/// use sexq::{ConstMac, ConstQ};
/// let mut mac = ConstMac::<1, 63, 2, 62>::new().unwrap();
/// let sample = ConstQ::<1, 63>::from_raw(1).unwrap();
/// let wrong_coefficient = ConstQ::<1, 63>::from_raw(1).unwrap();
/// mac.accumulate(sample, wrong_coefficient).unwrap();
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstMac<const I: u32, const F: u32, const CI: u32, const CF: u32> {
    inner: NativeMac,
}

impl<const I: u32, const F: u32, const CI: u32, const CF: u32> ConstMac<I, F, CI, CF> {
    pub fn new() -> Result<Self, NativeQError> {
        Ok(Self {
            inner: NativeMac::new(ConstQ::<I, F>::FORMAT, ConstQ::<CI, CF>::FORMAT)?,
        })
    }
    pub const fn raw(&self) -> i128 {
        self.inner.raw()
    }
    pub const fn fractional_bits(&self) -> u32 {
        self.inner.fractional_bits()
    }
    pub fn clear(&mut self) {
        self.inner.clear();
    }
    pub fn accumulate(
        &mut self,
        sample: ConstQ<I, F>,
        coefficient: ConstQ<CI, CF>,
    ) -> Result<(), NativeQError> {
        self.inner
            .accumulate(sample.to_native(), coefficient.to_native())
    }
    pub fn finish<const OI: u32, const OF: u32>(
        self,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<ConstQ<OI, OF>>, NativeQError> {
        self.inner
            .finish(ConstQ::<OI, OF>::FORMAT, rounding, overflow)
            .map(ConstQ::<OI, OF>::outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: QFormat = match QFormat::new(3, 4) {
        Ok(format) => format,
        Err(_) => panic!("format"),
    };
    const NEGATIVE: ConstQ<3, 4> = match ConstQ::from_twos_complement_bits(0x7f) {
        Ok(value) => value,
        Err(_) => panic!("sample"),
    };
    const RUNTIME: NativeQ = match NativeQ::from_raw(-1, FORMAT) {
        Ok(value) => value,
        Err(_) => panic!("runtime sample"),
    };
    const RESTORED: ConstQ<3, 4> = match ConstQ::from_native(RUNTIME) {
        Ok(value) => value,
        Err(_) => panic!("import"),
    };

    #[test]
    fn const_q_constants_storage_and_explicit_runtime_conversion() {
        assert_eq!(
            std::mem::size_of::<ConstQ<3, 4>>(),
            std::mem::size_of::<i64>()
        );
        assert_eq!(
            std::mem::align_of::<ConstQ<3, 4>>(),
            std::mem::align_of::<i64>()
        );
        assert_eq!(NEGATIVE.raw(), -1);
        assert_eq!(NEGATIVE, RESTORED);
        assert_eq!(NEGATIVE.to_native(), RUNTIME);
        assert_eq!(NEGATIVE.to_twos_complement_bits(), 0x7f);
        assert!(ConstQ::<3, 4>::from_twos_complement_bits(0x80).is_err());
        assert!(ConstQ::<3, 4>::from_raw(64).is_err());
        assert!(ConstQ::<3, 4>::from_raw(-65).is_err());
        assert!(ConstQ::<2, 5>::from_native(RUNTIME).is_err());
        assert_eq!(
            ConstQ::<1, 63>::from_twos_complement_bits(1_u64 << 63)
                .unwrap()
                .raw(),
            i64::MIN
        );
    }

    fn native(
        result: Result<ArithmeticOutcome<ConstQ<3, 8>>, NativeQError>,
    ) -> Result<ArithmeticOutcome<NativeQ>, NativeQError> {
        result.map(|outcome| ArithmeticOutcome {
            value: outcome.value.to_native(),
            saturated: outcome.saturated,
        })
    }

    fn differential<const I: u32, const F: u32>() {
        let format = ConstQ::<I, F>::FORMAT;
        let values = [
            format.minimum_raw(),
            format.minimum_raw() + 1,
            -1,
            0,
            format.maximum_raw().min(1),
            format.maximum_raw().min(2),
            format.maximum_raw(),
        ];
        for raw in values {
            let a = ConstQ::<I, F>::from_raw(raw).unwrap();
            let runtime = a.to_native();
            for overflow in [OverflowPolicy::Error, OverflowPolicy::Saturate] {
                for rhs in values {
                    let b = ConstQ::<I, F>::from_raw(rhs).unwrap();
                    for (typed, dynamic) in [
                        (a.add(b, overflow), runtime.add(b.to_native(), overflow)),
                        (a.sub(b, overflow), runtime.sub(b.to_native(), overflow)),
                    ] {
                        assert_eq!(
                            typed.map(|o| ArithmeticOutcome {
                                value: o.value.to_native(),
                                saturated: o.saturated
                            }),
                            dynamic
                        );
                    }
                }
                for rounding in [
                    RoundingMode::TowardZero,
                    RoundingMode::Floor,
                    RoundingMode::Ceiling,
                    RoundingMode::NearestTiesToEven,
                    RoundingMode::NearestTiesAwayFromZero,
                ] {
                    assert_eq!(
                        native(a.rescale::<3, 8>(rounding, overflow)),
                        runtime.rescale(ConstQ::<3, 8>::FORMAT, rounding, overflow)
                    );
                    for shift in [i32::MIN, -129, -64, -1, 0, 1, 63, 128, i32::MAX] {
                        assert_eq!(
                            a.shift_value(shift, rounding, overflow)
                                .map(|o| ArithmeticOutcome {
                                    value: o.value.to_native(),
                                    saturated: o.saturated
                                }),
                            runtime.shift_value(shift, rounding, overflow)
                        );
                    }
                    for rhs in [-16, -1, 0, 1, 15] {
                        let b = ConstQ::<2, 3>::from_raw(rhs).unwrap();
                        assert_eq!(
                            native(a.mul_to::<3, 8, 2, 3>(b, rounding, overflow)),
                            runtime.mul_to(
                                b.to_native(),
                                ConstQ::<3, 8>::FORMAT,
                                rounding,
                                overflow
                            )
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn const_q_arithmetic_matches_runtime_all_policies_and_boundaries() {
        differential::<1, 0>();
        differential::<3, 4>();
        differential::<1, 31>();
        differential::<16, 16>();
        differential::<32, 32>();
        differential::<1, 63>();
        differential::<2, 62>();
        differential::<64, 0>();
    }

    #[test]
    fn const_mac_rounds_once_and_preserves_overflow_poison_and_reset() {
        let sample = ConstQ::<1, 63>::from_raw(1).unwrap();
        let half = ConstQ::<2, 62>::from_raw(1_i64 << 61).unwrap();
        let mut mac = ConstMac::<1, 63, 2, 62>::new().unwrap();
        assert_eq!(mac.fractional_bits(), 125);
        mac.accumulate(sample, half).unwrap();
        assert_eq!(
            mac.clone()
                .finish::<1, 63>(RoundingMode::NearestTiesToEven, OverflowPolicy::Error)
                .unwrap()
                .value
                .raw(),
            0
        );
        mac.accumulate(sample, half).unwrap();
        assert_eq!(
            mac.finish::<1, 63>(RoundingMode::NearestTiesToEven, OverflowPolicy::Error)
                .unwrap()
                .value
                .raw(),
            1
        );

        let min = ConstQ::<1, 63>::from_raw(i64::MIN).unwrap();
        let zero = ConstQ::<1, 63>::from_raw(0).unwrap();
        let mut mac = ConstMac::<1, 63, 1, 63>::new().unwrap();
        mac.accumulate(min, min).unwrap();
        assert_eq!(mac.raw(), 1_i128 << 126);
        assert!(mac.accumulate(min, min).is_err());
        assert_eq!(
            mac.accumulate(zero, zero),
            Err(NativeQError::PoisonedAccumulator)
        );
        assert_eq!(
            mac.clone()
                .finish::<1, 63>(RoundingMode::NearestTiesToEven, OverflowPolicy::Saturate),
            Err(NativeQError::PoisonedAccumulator)
        );
        mac.clear();
        assert_eq!(mac.raw(), 0);
        mac.accumulate(zero, zero).unwrap();
        assert_eq!(
            mac.finish::<3, 4>(RoundingMode::NearestTiesToEven, OverflowPolicy::Error)
                .unwrap()
                .value
                .raw(),
            0
        );
    }
}
