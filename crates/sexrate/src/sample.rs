use super::{PolyphaseError, PolyphaseFirBigQ63, PolyphaseFirQ63};
use sexq::{Q1_63, RoundingMode, WideQ63};

mod sealed {
    pub trait Sealed {}
    impl Sealed for sexq::Q1_63 {}
    impl Sealed for sexq::WideQ63 {}
}

/// Supported input domains for the shared exact rational scheduler. Sealed so
/// external sample implementations cannot introduce hidden rounding or floats.
pub trait ResamplerSample: sealed::Sealed + Copy + std::fmt::Debug {
    const ZERO: Self;

    #[doc(hidden)]
    fn native_fir(
        bank: &PolyphaseFirQ63,
        phase: u64,
        samples: &[Self],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError>;
    #[doc(hidden)]
    fn big_fir(
        bank: &PolyphaseFirBigQ63,
        phase: u64,
        samples: &[Self],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError>;
}

impl ResamplerSample for Q1_63 {
    const ZERO: Self = Self::ZERO;
    fn native_fir(
        bank: &PolyphaseFirQ63,
        phase: u64,
        samples: &[Self],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        bank.convolve_wide(phase, samples, rounding)
    }
    fn big_fir(
        bank: &PolyphaseFirBigQ63,
        phase: u64,
        samples: &[Self],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        bank.convolve_wide(phase, samples, rounding)
    }
}

impl ResamplerSample for WideQ63 {
    const ZERO: Self = Self::ZERO;
    fn native_fir(
        bank: &PolyphaseFirQ63,
        phase: u64,
        samples: &[Self],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        bank.convolve_wide_input(phase, samples, rounding)
    }
    fn big_fir(
        bank: &PolyphaseFirBigQ63,
        phase: u64,
        samples: &[Self],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        bank.convolve_wide_input(phase, samples, rounding)
    }
}
