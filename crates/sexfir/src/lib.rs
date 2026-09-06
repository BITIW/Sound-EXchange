//! High-precision FIR design for SeX.
//!
//! This crate owns coefficient generation and quantization. The resampling
//! engine consumes only the resulting fixed-point bank.

pub mod cache;
pub mod certificate;
pub mod enclosure;
pub mod harmonics;
pub mod optimized;
pub mod refinement;
pub mod windowed;

use rug::float::{Constant, Round};
use rug::{Float, Integer};
use sexplan::{BackendRequirement, ErrorFloor, PrecisionPlan};
use sexq::{BigQ, BigQError, BigQFormat, Q2_62, RoundingMode};
use sexrate::{PolyphaseError, PolyphaseFirBigQ63, PolyphaseFirQ63, RateRatio};
#[cfg(target_endian = "little")]
use sha2::{Digest, Sha256};
#[cfg(target_endian = "big")]
use sha2_software::{Digest, Sha256};
use std::fmt;

const MAX_NATIVE_COEFFICIENTS: usize = 16_777_216;
const MAX_BIG_COEFFICIENTS: usize = 16_777_216;
const MAX_WORKING_PRECISION_BITS: u32 = 1_048_576;
const MAX_DOLPH_SERIES_TERMS: usize = 16_777_216;
const MAX_LEAST_SQUARES_WORK: usize = 67_108_864;
const MAX_REMEZ_WORK: usize = 67_108_864;

/// An exact non-negative rational configuration value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Fraction {
    numerator: u64,
    denominator: u64,
}

impl Fraction {
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, DesignError> {
        if denominator == 0 {
            return Err(DesignError::ZeroDenominator);
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    pub const fn numerator(self) -> u64 {
        self.numerator
    }

    pub const fn denominator(self) -> u64 {
        self.denominator
    }
}

const fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

/// Fully explicit Kaiser-windowed sinc design request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KaiserSpec {
    pub ratio: RateRatio,
    pub taps_per_phase: usize,
    /// Fraction of the lower input/output Nyquist used as the sinc cutoff.
    pub rolloff: Fraction,
    /// Kaiser beta, represented without a binary float configuration value.
    pub beta: Fraction,
    /// MPFR working precision. It is part of the coefficient identity.
    pub working_precision_bits: u32,
    pub quantization_rounding: RoundingMode,
}

impl KaiserSpec {
    /// Native materialization of the current sane policy. The compatibility
    /// name does not imply continuous-band qualification of every rate ratio.
    pub fn native_candidate(ratio: RateRatio) -> Self {
        let plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio,
            preset: sexplan::QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .expect("the built-in sane planning policy is valid");
        Self::from_precision_plan(&plan).expect("the sane plan fits the native backend")
    }

    /// Materialize a planner result for the current Q2.62/i128 execution
    /// backend. Wider plans fail explicitly instead of losing precision.
    pub fn from_precision_plan(plan: &PrecisionPlan) -> Result<Self, DesignError> {
        if plan.coefficient_fractional_bits > Q2_62::FRACTIONAL_BITS {
            return Err(DesignError::PlannedCoefficientPrecisionUnavailable {
                required: plan.coefficient_fractional_bits,
                available: Q2_62::FRACTIONAL_BITS,
            });
        }
        if plan.backend != BackendRequirement::NativeI128 {
            return Err(DesignError::PlannedAccumulatorBackendUnavailable(
                plan.backend,
            ));
        }
        let taps_per_phase = usize::try_from(plan.taps_per_phase)
            .map_err(|_| DesignError::PlannedTapCountTooLarge(plan.taps_per_phase))?;
        Ok(Self {
            ratio: plan.ratio,
            taps_per_phase,
            rolloff: Fraction::new(plan.rolloff.numerator(), plan.rolloff.denominator())?,
            beta: Fraction::new(plan.kaiser_beta.numerator(), plan.kaiser_beta.denominator())?,
            working_precision_bits: plan.working_precision_bits,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        })
    }

    fn validate(&self) -> Result<(), DesignError> {
        if self.taps_per_phase < 3 {
            return Err(DesignError::TooFewTaps(self.taps_per_phase));
        }
        if self.taps_per_phase.is_multiple_of(2) {
            return Err(DesignError::EvenTapCount(self.taps_per_phase));
        }
        if self.rolloff.numerator == 0 || self.rolloff.numerator > self.rolloff.denominator {
            return Err(DesignError::InvalidRolloff(self.rolloff));
        }
        if self.working_precision_bits < 96 {
            return Err(DesignError::WorkingPrecisionTooLow(
                self.working_precision_bits,
            ));
        }
        if self.working_precision_bits > MAX_WORKING_PRECISION_BITS {
            return Err(DesignError::WorkingPrecisionTooHigh(
                self.working_precision_bits,
            ));
        }
        if self.quantization_rounding != RoundingMode::NearestTiesToEven {
            return Err(DesignError::UnsupportedQuantizationRounding(
                self.quantization_rounding,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WindowFunction {
    Rectangular,
    Hann,
    Blackman,
    DolphChebyshev { attenuation_db: u32 },
}

impl WindowFunction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rectangular => "rectangular",
            Self::Hann => "hann",
            Self::Blackman => "blackman",
            Self::DolphChebyshev { .. } => "dolph-chebyshev",
        }
    }

    const fn algorithm(self) -> &'static str {
        match self {
            Self::Rectangular => "windowed-sinc-rectangular-v1",
            Self::Hann => "windowed-sinc-hann-v1",
            Self::Blackman => "windowed-sinc-blackman-v1",
            Self::DolphChebyshev { .. } => "windowed-sinc-dolph-chebyshev-v2",
        }
    }
}

/// Explicit non-parametric windowed-sinc request. These designers share the
/// same rational cutoff and polyphase geometry as Kaiser, while keeping window
/// choice in the coefficient identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowedSincSpec {
    pub ratio: RateRatio,
    pub taps_per_phase: usize,
    pub rolloff: Fraction,
    pub window: WindowFunction,
    pub working_precision_bits: u32,
    pub quantization_rounding: RoundingMode,
}

impl WindowedSincSpec {
    fn validate(&self) -> Result<(), DesignError> {
        KaiserSpec {
            ratio: self.ratio,
            taps_per_phase: self.taps_per_phase,
            rolloff: self.rolloff,
            beta: Fraction::new(1, 1)?,
            working_precision_bits: self.working_precision_bits,
            quantization_rounding: self.quantization_rounding,
        }
        .validate()?;
        if matches!(
            self.window,
            WindowFunction::DolphChebyshev { attenuation_db: 0 }
        ) {
            return Err(DesignError::InvalidDolphAttenuation(0));
        }
        Ok(())
    }
}

/// Discrete weighted least-squares low-pass request. The transition geometry
/// matches the existing Kaiser contract: the passband ends at `(2r - 1)` times
/// the lower Nyquist, while the stopband starts at the lower Nyquist.
/// The legacy `design_least_squares` fits phases independently. The same
/// geometric parameters can select a global Type-I prototype via
/// `optimized::Method::GlobalLeastSquares`, including full anti-imaging bands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeastSquaresSpec {
    pub ratio: RateRatio,
    pub taps_per_phase: usize,
    pub rolloff: Fraction,
    pub passband_weight: Fraction,
    pub stopband_weight: Fraction,
    pub grid_points_per_band: u32,
    pub working_precision_bits: u32,
    pub quantization_rounding: RoundingMode,
}

impl LeastSquaresSpec {
    fn validate(&self) -> Result<(), DesignError> {
        KaiserSpec {
            ratio: self.ratio,
            taps_per_phase: self.taps_per_phase,
            rolloff: self.rolloff,
            beta: Fraction::new(1, 1)?,
            working_precision_bits: self.working_precision_bits,
            quantization_rounding: self.quantization_rounding,
        }
        .validate()?;
        if u128::from(self.rolloff.numerator) * 2 <= u128::from(self.rolloff.denominator) {
            return Err(DesignError::InvalidLeastSquaresRolloff(self.rolloff));
        }
        if self.passband_weight.numerator == 0 {
            return Err(DesignError::ZeroLeastSquaresWeight("passband"));
        }
        if self.stopband_weight.numerator == 0 {
            return Err(DesignError::ZeroLeastSquaresWeight("stopband"));
        }
        if self.grid_points_per_band < 2 {
            return Err(DesignError::LeastSquaresGridTooSmall(
                self.grid_points_per_band,
            ));
        }
        Ok(())
    }
}

/// Parks-McClellan low-pass request for a Type-I global prototype which is
/// decomposed into the exact rational polyphase layout after exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquirippleSpec {
    pub ratio: RateRatio,
    pub taps_per_phase: usize,
    pub rolloff: Fraction,
    pub passband_weight: Fraction,
    pub stopband_weight: Fraction,
    pub grid_density: u32,
    pub max_iterations: u32,
    pub working_precision_bits: u32,
    pub quantization_rounding: RoundingMode,
}

impl EquirippleSpec {
    fn validate(&self) -> Result<(), DesignError> {
        KaiserSpec {
            ratio: self.ratio,
            taps_per_phase: self.taps_per_phase,
            rolloff: self.rolloff,
            beta: Fraction::new(1, 1)?,
            working_precision_bits: self.working_precision_bits,
            quantization_rounding: self.quantization_rounding,
        }
        .validate()?;
        if u128::from(self.rolloff.numerator) * 2 <= u128::from(self.rolloff.denominator) {
            return Err(DesignError::InvalidEquirippleRolloff(self.rolloff));
        }
        if self.passband_weight.numerator == 0 {
            return Err(DesignError::ZeroEquirippleWeight("passband"));
        }
        if self.stopband_weight.numerator == 0 {
            return Err(DesignError::ZeroEquirippleWeight("stopband"));
        }
        if self.grid_density < 4 {
            return Err(DesignError::RemezGridDensityTooSmall(self.grid_density));
        }
        if self.max_iterations == 0 {
            return Err(DesignError::ZeroRemezIterations);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuantizationReport {
    pub algorithm: &'static str,
    pub working_precision_bits: u32,
    pub coefficient_fractional_bits: u32,
    pub coefficient_count: usize,
    /// Largest per-phase correction used to make the quantized DC sum exact.
    pub max_dc_correction_raw: u64,
    /// MPFR measurement after DC correction, expressed in coefficient units.
    pub max_abs_coefficient_error_decimal: String,
    pub max_abs_coefficient_error_db: String,
    /// Worst phase sum of absolute coefficient errors. This bounds the
    /// quantization-induced complex response error at every frequency.
    pub max_phase_l1_error_decimal: String,
    pub max_phase_l1_error_db: String,
    /// Signed width proven sufficient for every full-scale phase MAC.
    /// Legacy symmetric L1 estimate, preserved in cache records. The bank's
    /// `minimum_accumulator_bits()` reports its exact signed requirement.
    pub required_accumulator_bits: u16,
    /// SHA-256 over the complete design specification and coefficient bytes.
    pub coefficient_sha256: String,
}

/// A designer result containing only fixed-point execution data plus its
/// auditable provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesignedFilter {
    spec: KaiserSpec,
    ratio: RateRatio,
    input_delay_frames: u64,
    bank: PolyphaseFirQ63,
    report: QuantizationReport,
}

/// Kaiser request for a coefficient table wider than native Q2.62. The common
/// mathematical specification is deliberately shared with the native designer;
/// only quantization and execution widths differ.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BigKaiserSpec {
    pub core: KaiserSpec,
    pub coefficient_fractional_bits: u32,
    pub accumulator_bits: u32,
}

impl BigKaiserSpec {
    pub fn from_precision_plan(plan: &PrecisionPlan) -> Result<Self, DesignError> {
        let taps_per_phase = usize::try_from(plan.taps_per_phase)
            .map_err(|_| DesignError::PlannedTapCountTooLarge(plan.taps_per_phase))?;
        Ok(Self {
            core: KaiserSpec {
                ratio: plan.ratio,
                taps_per_phase,
                rolloff: Fraction::new(plan.rolloff.numerator(), plan.rolloff.denominator())?,
                beta: Fraction::new(plan.kaiser_beta.numerator(), plan.kaiser_beta.denominator())?,
                working_precision_bits: plan.working_precision_bits,
                quantization_rounding: RoundingMode::NearestTiesToEven,
            },
            coefficient_fractional_bits: plan.coefficient_fractional_bits,
            accumulator_bits: plan.planned_accumulator_bits,
        })
    }

    fn validate(&self) -> Result<(), DesignError> {
        self.core.validate()?;
        BigQFormat::new(2, self.coefficient_fractional_bits)?;
        if self.accumulator_bits == 0 {
            return Err(DesignError::ZeroAccumulatorWidth);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BigQuantizationReport {
    pub algorithm: &'static str,
    pub working_precision_bits: u32,
    pub coefficient_fractional_bits: u32,
    pub coefficient_count: usize,
    pub max_dc_correction_raw: String,
    pub max_abs_coefficient_error_decimal: String,
    pub max_abs_coefficient_error_db: String,
    pub max_phase_l1_error_decimal: String,
    pub max_phase_l1_error_db: String,
    pub configured_accumulator_bits: u32,
    /// Legacy symmetric L1 estimate; an exactly admitted bank can use one bit
    /// less. Consult its `minimum_accumulator_bits()` for the exact requirement.
    pub required_accumulator_bits: u32,
    pub coefficient_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesignedBigFilter {
    spec: BigKaiserSpec,
    input_delay_frames: u64,
    bank: PolyphaseFirBigQ63,
    report: BigQuantizationReport,
}

impl DesignedBigFilter {
    pub const fn spec(&self) -> &BigKaiserSpec {
        &self.spec
    }

    pub const fn ratio(&self) -> RateRatio {
        self.spec.core.ratio
    }

    pub const fn input_delay_frames(&self) -> u64 {
        self.input_delay_frames
    }

    pub const fn bank(&self) -> &PolyphaseFirBigQ63 {
        &self.bank
    }

    pub const fn report(&self) -> &BigQuantizationReport {
        &self.report
    }

    pub fn into_bank(self) -> PolyphaseFirBigQ63 {
        self.bank
    }
}

impl DesignedFilter {
    pub const fn spec(&self) -> &KaiserSpec {
        &self.spec
    }

    pub const fn ratio(&self) -> RateRatio {
        self.ratio
    }

    /// Causal input delay. Starting the phase clock at this input index makes
    /// output frame zero correspond to input time zero.
    pub const fn input_delay_frames(&self) -> u64 {
        self.input_delay_frames
    }

    pub const fn bank(&self) -> &PolyphaseFirQ63 {
        &self.bank
    }

    pub const fn report(&self) -> &QuantizationReport {
        &self.report
    }

    pub fn into_bank(self) -> PolyphaseFirQ63 {
        self.bank
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesignedWindowedFilter {
    spec: WindowedSincSpec,
    input_delay_frames: u64,
    bank: PolyphaseFirQ63,
    report: QuantizationReport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesignedLeastSquaresFilter {
    spec: LeastSquaresSpec,
    input_delay_frames: u64,
    bank: PolyphaseFirQ63,
    report: QuantizationReport,
}

impl DesignedLeastSquaresFilter {
    pub const fn spec(&self) -> &LeastSquaresSpec {
        &self.spec
    }

    pub const fn ratio(&self) -> RateRatio {
        self.spec.ratio
    }

    pub const fn input_delay_frames(&self) -> u64 {
        self.input_delay_frames
    }

    pub const fn bank(&self) -> &PolyphaseFirQ63 {
        &self.bank
    }

    pub const fn report(&self) -> &QuantizationReport {
        &self.report
    }

    pub fn into_bank(self) -> PolyphaseFirQ63 {
        self.bank
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquirippleReport {
    pub quantization: QuantizationReport,
    pub exchange_iterations: u32,
    pub dense_grid_points: usize,
    pub extremal_frequencies_decimal: Vec<String>,
    /// Weighted minimax error of the converged MPFR global prototype before
    /// exact per-phase DC correction and Q2.62 quantization.
    pub prototype_weighted_error_decimal: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesignedEquirippleFilter {
    spec: EquirippleSpec,
    input_delay_frames: u64,
    bank: PolyphaseFirQ63,
    report: EquirippleReport,
}

impl DesignedEquirippleFilter {
    pub const fn spec(&self) -> &EquirippleSpec {
        &self.spec
    }

    pub const fn ratio(&self) -> RateRatio {
        self.spec.ratio
    }

    pub const fn input_delay_frames(&self) -> u64 {
        self.input_delay_frames
    }

    pub const fn bank(&self) -> &PolyphaseFirQ63 {
        &self.bank
    }

    pub const fn report(&self) -> &EquirippleReport {
        &self.report
    }

    pub fn into_bank(self) -> PolyphaseFirQ63 {
        self.bank
    }
}

impl DesignedWindowedFilter {
    pub const fn spec(&self) -> &WindowedSincSpec {
        &self.spec
    }

    pub const fn ratio(&self) -> RateRatio {
        self.spec.ratio
    }

    pub const fn input_delay_frames(&self) -> u64 {
        self.input_delay_frames
    }

    pub const fn bank(&self) -> &PolyphaseFirQ63 {
        &self.bank
    }

    pub const fn report(&self) -> &QuantizationReport {
        &self.report
    }

    pub fn into_bank(self) -> PolyphaseFirQ63 {
        self.bank
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DesignError {
    ZeroDenominator,
    TooFewTaps(usize),
    EvenTapCount(usize),
    InvalidRolloff(Fraction),
    WorkingPrecisionTooLow(u32),
    CoefficientWorkingPrecisionTooLow { requested: u32, minimum: u32 },
    WorkingPrecisionTooHigh(u32),
    UnsupportedQuantizationRounding(RoundingMode),
    PlannedCoefficientPrecisionUnavailable { required: u32, available: u32 },
    PlannedAccumulatorBackendUnavailable(BackendRequirement),
    PlannedTapCountTooLarge(u64),
    ZeroAccumulatorWidth,
    PhaseCountTooLarge(u64),
    CoefficientCountOverflow,
    CoefficientBudgetExceeded { requested: usize, maximum: usize },
    BigCoefficientBudgetExceeded { requested: usize, maximum: usize },
    InvalidDolphAttenuation(u32),
    DolphSeriesBudgetExceeded { requested: usize, maximum: usize },
    ZeroDolphNormalization,
    InvalidLeastSquaresRolloff(Fraction),
    ZeroLeastSquaresWeight(&'static str),
    LeastSquaresGridTooSmall(u32),
    LeastSquaresWorkBudgetExceeded { requested: usize, maximum: usize },
    SingularLeastSquaresSystem { pivot: usize },
    InvalidEquirippleRolloff(Fraction),
    ZeroEquirippleWeight(&'static str),
    RemezGridDensityTooSmall(u32),
    ZeroRemezIterations,
    RemezWorkBudgetExceeded { requested: usize, maximum: usize },
    SingularRemezSystem { pivot: usize },
    RemezExtremaUnavailable { required: usize, found: usize },
    RemezExtremaBudgetExceeded { found: usize, maximum: usize },
    RemezDidNotConverge { iterations: u32 },
    BesselDidNotConverge,
    ZeroPhaseGain(usize),
    NonFiniteCoefficient { phase: usize, tap: usize },
    CoefficientOutOfRange { phase: usize, tap: usize },
    DcCorrectionOverflow { phase: usize },
    AnalysisGridTooSmall(u32),
    InvalidInternalMeasurement,
    BigArithmetic(BigQError),
    Polyphase(PolyphaseError),
}

impl fmt::Display for DesignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ZeroDenominator => f.write_str("fraction denominator must be non-zero"),
            Self::TooFewTaps(taps) => {
                write!(f, "at least 3 taps per phase are required, got {taps}")
            }
            Self::EvenTapCount(taps) => write!(f, "tap count must be odd, got {taps}"),
            Self::InvalidRolloff(value) => write!(
                f,
                "rolloff must be in (0, 1], got {}/{}",
                value.numerator, value.denominator
            ),
            Self::WorkingPrecisionTooLow(bits) => {
                write!(
                    f,
                    "MPFR working precision must be at least 96 bits, got {bits}"
                )
            }
            Self::WorkingPrecisionTooHigh(bits) => write!(
                f,
                "MPFR working precision {bits} exceeds safety limit {MAX_WORKING_PRECISION_BITS}"
            ),
            Self::CoefficientWorkingPrecisionTooLow { requested, minimum } => write!(
                f,
                "MPFR coefficient design requires at least {minimum} working bits (C+64), got {requested}"
            ),
            Self::UnsupportedQuantizationRounding(mode) => {
                write!(f, "coefficient quantization does not yet support {mode:?}")
            }
            Self::PlannedCoefficientPrecisionUnavailable {
                required,
                available,
            } => write!(
                f,
                "plan requires {required} coefficient fractional bits; native Q2.62 provides {available}"
            ),
            Self::PlannedAccumulatorBackendUnavailable(backend) => {
                write!(
                    f,
                    "planned accumulator backend {backend:?} is unavailable through native KaiserSpec; use BigKaiserSpec"
                )
            }
            Self::PlannedTapCountTooLarge(taps) => {
                write!(
                    f,
                    "planned tap count {taps} is not addressable on this platform"
                )
            }
            Self::ZeroAccumulatorWidth => f.write_str("bigint accumulator width must be non-zero"),
            Self::PhaseCountTooLarge(phases) => {
                write!(
                    f,
                    "phase count {phases} is not addressable on this platform"
                )
            }
            Self::CoefficientCountOverflow => f.write_str("coefficient count overflowed usize"),
            Self::CoefficientBudgetExceeded { requested, maximum } => write!(
                f,
                "native design needs {requested} coefficients; safety limit is {maximum}"
            ),
            Self::BigCoefficientBudgetExceeded { requested, maximum } => write!(
                f,
                "bigint design needs {requested} coefficients; safety limit is {maximum}"
            ),
            Self::InvalidDolphAttenuation(attenuation) => write!(
                f,
                "Dolph-Chebyshev attenuation must be positive, got {attenuation} dB"
            ),
            Self::DolphSeriesBudgetExceeded { requested, maximum } => write!(
                f,
                "direct MPFR Dolph-Chebyshev series needs {requested} terms; safety limit is {maximum}"
            ),
            Self::ZeroDolphNormalization => {
                f.write_str("Dolph-Chebyshev inverse DFT has zero center normalization")
            }
            Self::InvalidLeastSquaresRolloff(value) => write!(
                f,
                "least-squares rolloff must be greater than 1/2, got {}/{}",
                value.numerator, value.denominator
            ),
            Self::ZeroLeastSquaresWeight(band) => {
                write!(f, "least-squares {band} weight must be positive")
            }
            Self::LeastSquaresGridTooSmall(points) => write!(
                f,
                "least-squares design needs at least 2 grid points per band, got {points}"
            ),
            Self::LeastSquaresWorkBudgetExceeded { requested, maximum } => write!(
                f,
                "least-squares solve needs approximately {requested} MPFR operations; safety limit is {maximum}"
            ),
            Self::SingularLeastSquaresSystem { pivot } => write!(
                f,
                "least-squares normal matrix is not positive definite at pivot {pivot}"
            ),
            Self::InvalidEquirippleRolloff(value) => write!(
                f,
                "equiripple rolloff must be greater than 1/2, got {}/{}",
                value.numerator, value.denominator
            ),
            Self::ZeroEquirippleWeight(band) => {
                write!(f, "equiripple {band} weight must be positive")
            }
            Self::RemezGridDensityTooSmall(density) => {
                write!(f, "Remez grid density must be at least 4, got {density}")
            }
            Self::ZeroRemezIterations => {
                f.write_str("Remez exchange requires at least one iteration")
            }
            Self::RemezWorkBudgetExceeded { requested, maximum } => write!(
                f,
                "Remez exchange needs approximately {requested} MPFR operations; safety limit is {maximum}"
            ),
            Self::SingularRemezSystem { pivot } => {
                write!(f, "Remez interpolation system is singular at pivot {pivot}")
            }
            Self::RemezExtremaUnavailable { required, found } => write!(
                f,
                "Remez exchange needs {required} alternating extrema, found {found}"
            ),
            Self::RemezExtremaBudgetExceeded { found, maximum } => write!(
                f,
                "Remez exchange found {found} local extrema; selector safety limit is {maximum}"
            ),
            Self::RemezDidNotConverge { iterations } => {
                write!(
                    f,
                    "Remez exchange did not converge in {iterations} iterations"
                )
            }
            Self::BesselDidNotConverge => f.write_str("Kaiser I0 series did not converge"),
            Self::ZeroPhaseGain(phase) => write!(f, "phase {phase} has zero unnormalized DC gain"),
            Self::NonFiniteCoefficient { phase, tap } => {
                write!(
                    f,
                    "phase {phase}, tap {tap} produced a non-finite coefficient"
                )
            }
            Self::CoefficientOutOfRange { phase, tap } => {
                write!(f, "phase {phase}, tap {tap} does not fit Q2.62")
            }
            Self::DcCorrectionOverflow { phase } => {
                write!(f, "exact DC correction overflowed Q2.62 in phase {phase}")
            }
            Self::AnalysisGridTooSmall(points) => {
                write!(
                    f,
                    "response analysis needs at least 2 grid points, got {points}"
                )
            }
            Self::InvalidInternalMeasurement => {
                f.write_str("an internal MPFR measurement could not be reconstructed")
            }
            Self::BigArithmetic(ref error) => error.fmt(f),
            Self::Polyphase(ref error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DesignError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BigArithmetic(error) => Some(error),
            Self::Polyphase(error) => Some(error),
            _ => None,
        }
    }
}

impl From<PolyphaseError> for DesignError {
    fn from(value: PolyphaseError) -> Self {
        Self::Polyphase(value)
    }
}

impl From<BigQError> for DesignError {
    fn from(value: BigQError) -> Self {
        Self::BigArithmetic(value)
    }
}

/// Design, normalize, and quantize a causal rational polyphase bank.
pub fn design_kaiser(spec: &KaiserSpec) -> Result<DesignedFilter, DesignError> {
    spec.validate()?;
    let phase_count = usize::try_from(spec.ratio.up())
        .map_err(|_| DesignError::PhaseCountTooLarge(spec.ratio.up()))?;
    let coefficient_count = phase_count
        .checked_mul(spec.taps_per_phase)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    if coefficient_count > MAX_NATIVE_COEFFICIENTS {
        return Err(DesignError::CoefficientBudgetExceeded {
            requested: coefficient_count,
            maximum: MAX_NATIVE_COEFFICIENTS,
        });
    }
    let precision = spec.working_precision_bits;
    let input_delay_frames = u64::try_from((spec.taps_per_phase - 1) / 2)
        .map_err(|_| DesignError::CoefficientCountOverflow)?;

    let cutoff = cutoff_for(spec, precision);
    let beta = float_from_fraction(spec.beta, precision);
    let i0_beta = bessel_i0(&beta, precision)?;
    let pi = Float::with_val(precision, Constant::Pi);
    let mut coefficients = Vec::with_capacity(coefficient_count);
    let mut max_dc_correction_raw = 0_u64;
    let mut max_abs_error = Float::with_val(precision, 0);
    let mut max_phase_l1_error = Float::with_val(precision, 0);

    for phase in 0..phase_count {
        let mut ideal = Vec::with_capacity(spec.taps_per_phase);
        for tap in 0..spec.taps_per_phase {
            ideal.push(design_tap(
                spec,
                phase,
                tap,
                input_delay_frames,
                &cutoff,
                &beta,
                &i0_beta,
                &pi,
            )?);
        }

        let mut sum = Float::with_val(precision, 0);
        for coefficient in &ideal {
            sum += coefficient;
        }
        if sum.is_zero() {
            return Err(DesignError::ZeroPhaseGain(phase));
        }
        for coefficient in &mut ideal {
            *coefficient /= &sum;
        }

        let phase_start = coefficients.len();
        let mut raw_sum = 0_i128;
        for (tap, coefficient) in ideal.iter().enumerate() {
            let raw = quantize_q2_62(coefficient, phase, tap)?;
            raw_sum = raw_sum
                .checked_add(i128::from(raw))
                .ok_or(DesignError::DcCorrectionOverflow { phase })?;
            coefficients.push(Q2_62::from_raw(raw));
        }

        let residual = i128::from(Q2_62::ONE.raw()) - raw_sum;
        let correction_index = coefficients[phase_start..]
            .iter()
            .enumerate()
            .max_by_key(|(_, coefficient)| coefficient.raw().unsigned_abs())
            .map(|(index, _)| phase_start + index)
            .expect("validated non-empty phase");
        let corrected = i128::from(coefficients[correction_index].raw()) + residual;
        let corrected =
            i64::try_from(corrected).map_err(|_| DesignError::DcCorrectionOverflow { phase })?;
        coefficients[correction_index] = Q2_62::from_raw(corrected);
        let correction_magnitude = u64::try_from(residual.unsigned_abs())
            .map_err(|_| DesignError::DcCorrectionOverflow { phase })?;
        max_dc_correction_raw = max_dc_correction_raw.max(correction_magnitude);

        let mut phase_l1_error = Float::with_val(precision, 0);
        for (tap, coefficient) in ideal.iter().enumerate() {
            let raw = coefficients[phase_start + tap].raw();
            let mut quantized = Float::with_val(precision, raw);
            quantized >>= Q2_62::FRACTIONAL_BITS;
            quantized -= coefficient;
            quantized.abs_mut();
            if quantized > max_abs_error {
                max_abs_error = quantized.clone();
            }
            phase_l1_error += quantized;
        }
        if phase_l1_error > max_phase_l1_error {
            max_phase_l1_error = phase_l1_error;
        }
    }

    let bank = PolyphaseFirQ63::for_ratio(spec.ratio, spec.taps_per_phase, coefficients.clone())?;
    let report = QuantizationReport {
        algorithm: "kaiser-windowed-sinc-v1",
        working_precision_bits: precision,
        coefficient_fractional_bits: Q2_62::FRACTIONAL_BITS,
        coefficient_count,
        max_dc_correction_raw,
        max_abs_coefficient_error_decimal: max_abs_error.to_string_radix(10, Some(20)),
        max_abs_coefficient_error_db: amplitude_db(&max_abs_error, precision),
        max_phase_l1_error_decimal: max_phase_l1_error.to_string_radix(10, Some(20)),
        max_phase_l1_error_db: amplitude_db(&max_phase_l1_error, precision),
        required_accumulator_bits: bank.required_accumulator_bits(),
        coefficient_sha256: coefficient_identity(spec, input_delay_frames, &coefficients),
    };
    Ok(DesignedFilter {
        spec: spec.clone(),
        ratio: spec.ratio,
        input_delay_frames,
        bank,
        report,
    })
}

/// Design and quantize a rectangular, Hann, Blackman, or Dolph-Chebyshev
/// windowed sinc bank.
pub fn design_windowed_sinc(
    spec: &WindowedSincSpec,
) -> Result<DesignedWindowedFilter, DesignError> {
    spec.validate()?;
    let phase_count = usize::try_from(spec.ratio.up())
        .map_err(|_| DesignError::PhaseCountTooLarge(spec.ratio.up()))?;
    let coefficient_count = phase_count
        .checked_mul(spec.taps_per_phase)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    if coefficient_count > MAX_NATIVE_COEFFICIENTS {
        return Err(DesignError::CoefficientBudgetExceeded {
            requested: coefficient_count,
            maximum: MAX_NATIVE_COEFFICIENTS,
        });
    }
    let precision = spec.working_precision_bits;
    let input_delay_frames = u64::try_from((spec.taps_per_phase - 1) / 2)
        .map_err(|_| DesignError::CoefficientCountOverflow)?;
    let cutoff = cutoff_for_windowed(spec, precision);
    let pi = Float::with_val(precision, Constant::Pi);
    let dolph = if let WindowFunction::DolphChebyshev { attenuation_db } = spec.window {
        let requested = coefficient_count
            .checked_mul(spec.taps_per_phase)
            .ok_or(DesignError::CoefficientCountOverflow)?;
        if requested > MAX_DOLPH_SERIES_TERMS {
            return Err(DesignError::DolphSeriesBudgetExceeded {
                requested,
                maximum: MAX_DOLPH_SERIES_TERMS,
            });
        }
        Some(DolphWindow::new(
            spec.taps_per_phase,
            attenuation_db,
            precision,
        )?)
    } else {
        None
    };
    let mut coefficients = Vec::with_capacity(coefficient_count);
    let mut max_dc_correction_raw = 0_u64;
    let mut max_abs_error = Float::with_val(precision, 0);
    let mut max_phase_l1_error = Float::with_val(precision, 0);

    for phase in 0..phase_count {
        let mut ideal = Vec::with_capacity(spec.taps_per_phase);
        for tap in 0..spec.taps_per_phase {
            ideal.push(design_windowed_tap(
                spec,
                phase,
                tap,
                input_delay_frames,
                &cutoff,
                &pi,
                dolph.as_ref(),
            )?);
        }
        let mut sum = Float::with_val(precision, 0);
        for coefficient in &ideal {
            sum += coefficient;
        }
        if sum.is_zero() {
            return Err(DesignError::ZeroPhaseGain(phase));
        }
        for coefficient in &mut ideal {
            *coefficient /= &sum;
        }

        let phase_start = coefficients.len();
        let mut raw_sum = 0_i128;
        for (tap, coefficient) in ideal.iter().enumerate() {
            let raw = quantize_q2_62(coefficient, phase, tap)?;
            raw_sum = raw_sum
                .checked_add(i128::from(raw))
                .ok_or(DesignError::DcCorrectionOverflow { phase })?;
            coefficients.push(Q2_62::from_raw(raw));
        }
        let residual = i128::from(Q2_62::ONE.raw()) - raw_sum;
        let correction_index = coefficients[phase_start..]
            .iter()
            .enumerate()
            .max_by_key(|(_, coefficient)| coefficient.raw().unsigned_abs())
            .map(|(index, _)| phase_start + index)
            .expect("validated non-empty phase");
        let corrected = i128::from(coefficients[correction_index].raw()) + residual;
        let corrected =
            i64::try_from(corrected).map_err(|_| DesignError::DcCorrectionOverflow { phase })?;
        coefficients[correction_index] = Q2_62::from_raw(corrected);
        max_dc_correction_raw = max_dc_correction_raw.max(
            u64::try_from(residual.unsigned_abs())
                .map_err(|_| DesignError::DcCorrectionOverflow { phase })?,
        );

        let mut phase_l1_error = Float::with_val(precision, 0);
        for (tap, coefficient) in ideal.iter().enumerate() {
            let mut quantized = Float::with_val(precision, coefficients[phase_start + tap].raw());
            quantized >>= Q2_62::FRACTIONAL_BITS;
            quantized -= coefficient;
            quantized.abs_mut();
            if quantized > max_abs_error {
                max_abs_error = quantized.clone();
            }
            phase_l1_error += quantized;
        }
        if phase_l1_error > max_phase_l1_error {
            max_phase_l1_error = phase_l1_error;
        }
    }

    let bank = PolyphaseFirQ63::for_ratio(spec.ratio, spec.taps_per_phase, coefficients.clone())?;
    let report = QuantizationReport {
        algorithm: spec.window.algorithm(),
        working_precision_bits: precision,
        coefficient_fractional_bits: Q2_62::FRACTIONAL_BITS,
        coefficient_count,
        max_dc_correction_raw,
        max_abs_coefficient_error_decimal: max_abs_error.to_string_radix(10, Some(20)),
        max_abs_coefficient_error_db: amplitude_db(&max_abs_error, precision),
        max_phase_l1_error_decimal: max_phase_l1_error.to_string_radix(10, Some(20)),
        max_phase_l1_error_db: amplitude_db(&max_phase_l1_error, precision),
        required_accumulator_bits: bank.required_accumulator_bits(),
        coefficient_sha256: coefficient_identity_windowed(spec, input_delay_frames, &coefficients),
    };
    Ok(DesignedWindowedFilter {
        spec: spec.clone(),
        input_delay_frames,
        bank,
        report,
    })
}

/// Design a fractional-delay polyphase low-pass bank by solving one discrete
/// weighted least-squares problem per phase. The shared normal matrix is
/// factored once with MPFR Cholesky decomposition; only fixed-point Q2.62 data
/// leaves the designer.
/// This reference method does not enforce cross-phase consistency in the
/// transition band; per-phase amplitude qualification alone is insufficient.
/// Prefer `optimized::Method::GlobalLeastSquares` and complete-image checks
/// for a global anti-imaging design.
pub fn design_least_squares(
    spec: &LeastSquaresSpec,
) -> Result<DesignedLeastSquaresFilter, DesignError> {
    spec.validate()?;
    let phase_count = usize::try_from(spec.ratio.up())
        .map_err(|_| DesignError::PhaseCountTooLarge(spec.ratio.up()))?;
    let coefficient_count = phase_count
        .checked_mul(spec.taps_per_phase)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    if coefficient_count > MAX_NATIVE_COEFFICIENTS {
        return Err(DesignError::CoefficientBudgetExceeded {
            requested: coefficient_count,
            maximum: MAX_NATIVE_COEFFICIENTS,
        });
    }
    let tap_cube = spec
        .taps_per_phase
        .checked_pow(3)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    let grid_work = usize::try_from(spec.grid_points_per_band)
        .map_err(|_| DesignError::CoefficientCountOverflow)?
        .checked_mul(spec.taps_per_phase)
        .and_then(|work| work.checked_mul(phase_count.saturating_add(2)))
        .ok_or(DesignError::CoefficientCountOverflow)?;
    let requested_work = tap_cube
        .checked_add(grid_work)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    if requested_work > MAX_LEAST_SQUARES_WORK {
        return Err(DesignError::LeastSquaresWorkBudgetExceeded {
            requested: requested_work,
            maximum: MAX_LEAST_SQUARES_WORK,
        });
    }

    let precision = spec.working_precision_bits;
    let input_delay_frames = u64::try_from((spec.taps_per_phase - 1) / 2)
        .map_err(|_| DesignError::CoefficientCountOverflow)?;
    let grid = build_least_squares_grid(spec);
    let correlation = least_squares_correlation(&grid, spec.taps_per_phase, precision);
    let factor = cholesky_toeplitz(&correlation, precision)?;
    let mut ideal_phases = Vec::with_capacity(phase_count);
    for phase in 0..phase_count {
        let right_hand_side =
            least_squares_right_hand_side(spec, phase, input_delay_frames, &grid, precision);
        ideal_phases.push(cholesky_solve(&factor, &right_hand_side, precision));
    }

    let mut coefficients = Vec::with_capacity(coefficient_count);
    let mut max_dc_correction_raw = 0_u64;
    let mut max_abs_error = Float::with_val(precision, 0);
    let mut max_phase_l1_error = Float::with_val(precision, 0);
    for (phase, mut ideal) in ideal_phases.into_iter().enumerate() {
        let mut sum = Float::with_val(precision, 0);
        for coefficient in &ideal {
            sum += coefficient;
        }
        if sum.is_zero() {
            return Err(DesignError::ZeroPhaseGain(phase));
        }
        for coefficient in &mut ideal {
            *coefficient /= &sum;
        }

        let phase_start = coefficients.len();
        let mut raw_sum = 0_i128;
        for (tap, coefficient) in ideal.iter().enumerate() {
            let raw = quantize_q2_62(coefficient, phase, tap)?;
            raw_sum = raw_sum
                .checked_add(i128::from(raw))
                .ok_or(DesignError::DcCorrectionOverflow { phase })?;
            coefficients.push(Q2_62::from_raw(raw));
        }
        let residual = i128::from(Q2_62::ONE.raw()) - raw_sum;
        let correction_index = coefficients[phase_start..]
            .iter()
            .enumerate()
            .max_by_key(|(_, coefficient)| coefficient.raw().unsigned_abs())
            .map(|(index, _)| phase_start + index)
            .expect("validated non-empty phase");
        let corrected = i128::from(coefficients[correction_index].raw()) + residual;
        let corrected =
            i64::try_from(corrected).map_err(|_| DesignError::DcCorrectionOverflow { phase })?;
        coefficients[correction_index] = Q2_62::from_raw(corrected);
        max_dc_correction_raw = max_dc_correction_raw.max(
            u64::try_from(residual.unsigned_abs())
                .map_err(|_| DesignError::DcCorrectionOverflow { phase })?,
        );

        let mut phase_l1_error = Float::with_val(precision, 0);
        for (tap, coefficient) in ideal.iter().enumerate() {
            let mut quantized = Float::with_val(precision, coefficients[phase_start + tap].raw());
            quantized >>= Q2_62::FRACTIONAL_BITS;
            quantized -= coefficient;
            quantized.abs_mut();
            if quantized > max_abs_error {
                max_abs_error = quantized.clone();
            }
            phase_l1_error += quantized;
        }
        if phase_l1_error > max_phase_l1_error {
            max_phase_l1_error = phase_l1_error;
        }
    }

    let bank = PolyphaseFirQ63::for_ratio(spec.ratio, spec.taps_per_phase, coefficients.clone())?;
    let report = QuantizationReport {
        algorithm: "least-squares-lowpass-v1",
        working_precision_bits: precision,
        coefficient_fractional_bits: Q2_62::FRACTIONAL_BITS,
        coefficient_count,
        max_dc_correction_raw,
        max_abs_coefficient_error_decimal: max_abs_error.to_string_radix(10, Some(20)),
        max_abs_coefficient_error_db: amplitude_db(&max_abs_error, precision),
        max_phase_l1_error_decimal: max_phase_l1_error.to_string_radix(10, Some(20)),
        max_phase_l1_error_db: amplitude_db(&max_phase_l1_error, precision),
        required_accumulator_bits: bank.required_accumulator_bits(),
        coefficient_sha256: coefficient_identity_least_squares(
            spec,
            input_delay_frames,
            &coefficients,
        ),
    };
    Ok(DesignedLeastSquaresFilter {
        spec: spec.clone(),
        input_delay_frames,
        bank,
        report,
    })
}

/// Design an odd-length Type-I equiripple global prototype with MPFR Remez
/// exchange, decompose it into rational polyphase rows, then quantize each row
/// under the same exact-DC and accumulator-proof rules as other native banks.
pub fn design_equiripple(spec: &EquirippleSpec) -> Result<DesignedEquirippleFilter, DesignError> {
    spec.validate()?;
    let phase_count = usize::try_from(spec.ratio.up())
        .map_err(|_| DesignError::PhaseCountTooLarge(spec.ratio.up()))?;
    let coefficient_count = phase_count
        .checked_mul(spec.taps_per_phase)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    if coefficient_count > MAX_NATIVE_COEFFICIENTS {
        return Err(DesignError::CoefficientBudgetExceeded {
            requested: coefficient_count,
            maximum: MAX_NATIVE_COEFFICIENTS,
        });
    }
    let global_length = spec
        .taps_per_phase
        .checked_sub(1)
        .and_then(|span| span.checked_mul(phase_count))
        .and_then(|span| span.checked_add(1))
        .ok_or(DesignError::CoefficientCountOverflow)?;
    let amplitude_count = global_length.div_ceil(2);
    let extrema_count = amplitude_count
        .checked_add(1)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    let points_per_band = usize::try_from(spec.grid_density)
        .map_err(|_| DesignError::CoefficientCountOverflow)?
        .checked_mul(extrema_count)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    let dense_grid_points = prototype_grid_points(spec.ratio, points_per_band as u128)
        .and_then(|count| usize::try_from(count).ok())
        .ok_or(DesignError::CoefficientCountOverflow)?;
    let requested_work = remez_work(
        extrema_count as u128,
        dense_grid_points as u128,
        spec.max_iterations,
    )
    .and_then(|work| work.checked_add(4 * coefficient_count as u128))
    .and_then(|work| usize::try_from(work).ok())
    .ok_or(DesignError::CoefficientCountOverflow)?;
    if requested_work > MAX_REMEZ_WORK {
        return Err(DesignError::RemezWorkBudgetExceeded {
            requested: requested_work,
            maximum: MAX_REMEZ_WORK,
        });
    }

    let precision = spec.working_precision_bits;
    let grid = build_remez_grid(spec, points_per_band, precision);
    let cache_cosines =
        remez_cosine_cache_bytes(grid.len() as u64, amplitude_count as u64, precision)
            .is_some_and(|bytes| bytes <= 8 * 1024 * 1024);
    let exchange = remez_exchange(spec, amplitude_count, &grid, precision, cache_cosines)?;
    let center = (global_length - 1) / 2;
    let mut prototype = vec![Float::with_val(precision, 0); global_length];
    prototype[center] = exchange.amplitudes[0].clone();
    for offset in 1..amplitude_count {
        let mut coefficient = exchange.amplitudes[offset].clone();
        coefficient /= 2;
        prototype[center - offset] = coefficient.clone();
        prototype[center + offset] = coefficient;
    }

    let input_delay_frames = u64::try_from((spec.taps_per_phase - 1) / 2)
        .map_err(|_| DesignError::CoefficientCountOverflow)?;
    let mut coefficients = Vec::with_capacity(coefficient_count);
    let mut max_dc_correction_raw = 0_u64;
    let mut max_abs_error = Float::with_val(precision, 0);
    let mut max_phase_l1_error = Float::with_val(precision, 0);
    for phase in 0..phase_count {
        let mut ideal = Vec::with_capacity(spec.taps_per_phase);
        for tap in 0..spec.taps_per_phase {
            let global_index = tap
                .checked_mul(phase_count)
                .and_then(|index| index.checked_add(phase))
                .ok_or(DesignError::CoefficientCountOverflow)?;
            ideal.push(if global_index < prototype.len() {
                prototype[global_index].clone()
            } else {
                Float::with_val(precision, 0)
            });
        }
        let mut sum = Float::with_val(precision, 0);
        for coefficient in &ideal {
            sum += coefficient;
        }
        if sum.is_zero() {
            return Err(DesignError::ZeroPhaseGain(phase));
        }
        for coefficient in &mut ideal {
            *coefficient /= &sum;
        }

        let phase_start = coefficients.len();
        let mut raw_sum = 0_i128;
        for (tap, coefficient) in ideal.iter().enumerate() {
            let raw = quantize_q2_62(coefficient, phase, tap)?;
            raw_sum = raw_sum
                .checked_add(i128::from(raw))
                .ok_or(DesignError::DcCorrectionOverflow { phase })?;
            coefficients.push(Q2_62::from_raw(raw));
        }
        let residual = i128::from(Q2_62::ONE.raw()) - raw_sum;
        let correction_index = coefficients[phase_start..]
            .iter()
            .enumerate()
            .max_by_key(|(_, coefficient)| coefficient.raw().unsigned_abs())
            .map(|(index, _)| phase_start + index)
            .expect("validated non-empty phase");
        let corrected = i128::from(coefficients[correction_index].raw()) + residual;
        let corrected =
            i64::try_from(corrected).map_err(|_| DesignError::DcCorrectionOverflow { phase })?;
        coefficients[correction_index] = Q2_62::from_raw(corrected);
        max_dc_correction_raw = max_dc_correction_raw.max(
            u64::try_from(residual.unsigned_abs())
                .map_err(|_| DesignError::DcCorrectionOverflow { phase })?,
        );

        let mut phase_l1_error = Float::with_val(precision, 0);
        for (tap, coefficient) in ideal.iter().enumerate() {
            let mut quantized = Float::with_val(precision, coefficients[phase_start + tap].raw());
            quantized >>= Q2_62::FRACTIONAL_BITS;
            quantized -= coefficient;
            quantized.abs_mut();
            if quantized > max_abs_error {
                max_abs_error = quantized.clone();
            }
            phase_l1_error += quantized;
        }
        if phase_l1_error > max_phase_l1_error {
            max_phase_l1_error = phase_l1_error;
        }
    }

    let bank = PolyphaseFirQ63::for_ratio(spec.ratio, spec.taps_per_phase, coefficients.clone())?;
    let quantization = QuantizationReport {
        algorithm: if spec.ratio.up() == spec.ratio.down() {
            "parks-mcclellan-lowpass-unity-v3"
        } else {
            "parks-mcclellan-lowpass-v2"
        },
        working_precision_bits: precision,
        coefficient_fractional_bits: Q2_62::FRACTIONAL_BITS,
        coefficient_count,
        max_dc_correction_raw,
        max_abs_coefficient_error_decimal: max_abs_error.to_string_radix(10, Some(20)),
        max_abs_coefficient_error_db: amplitude_db(&max_abs_error, precision),
        max_phase_l1_error_decimal: max_phase_l1_error.to_string_radix(10, Some(20)),
        max_phase_l1_error_db: amplitude_db(&max_phase_l1_error, precision),
        required_accumulator_bits: bank.required_accumulator_bits(),
        coefficient_sha256: coefficient_identity_equiripple(
            spec,
            input_delay_frames,
            &coefficients,
        ),
    };
    Ok(DesignedEquirippleFilter {
        spec: spec.clone(),
        input_delay_frames,
        bank,
        report: EquirippleReport {
            quantization,
            exchange_iterations: exchange.iterations,
            dense_grid_points,
            extremal_frequencies_decimal: exchange
                .extrema
                .iter()
                .map(|index| grid[*index].frequency.to_string_radix(10, Some(20)))
                .collect(),
            prototype_weighted_error_decimal: exchange
                .max_weighted_error
                .to_string_radix(10, Some(20)),
        },
    })
}

/// Design the same normalized Kaiser-windowed sinc bank as [`design_kaiser`],
/// quantized directly to an arbitrary-width Q2.n table. MPFR is confined to
/// coefficient construction and measurement; execution data is integer-only.
pub fn design_kaiser_big(spec: &BigKaiserSpec) -> Result<DesignedBigFilter, DesignError> {
    spec.validate()?;
    let core = &spec.core;
    let phase_count = usize::try_from(core.ratio.up())
        .map_err(|_| DesignError::PhaseCountTooLarge(core.ratio.up()))?;
    let coefficient_count = phase_count
        .checked_mul(core.taps_per_phase)
        .ok_or(DesignError::CoefficientCountOverflow)?;
    if coefficient_count > MAX_BIG_COEFFICIENTS {
        return Err(DesignError::BigCoefficientBudgetExceeded {
            requested: coefficient_count,
            maximum: MAX_BIG_COEFFICIENTS,
        });
    }
    let precision = core.working_precision_bits;
    let input_delay_frames = u64::try_from((core.taps_per_phase - 1) / 2)
        .map_err(|_| DesignError::CoefficientCountOverflow)?;
    let coefficient_format = BigQFormat::new(2, spec.coefficient_fractional_bits)?;
    let cutoff = cutoff_for(core, precision);
    let beta = float_from_fraction(core.beta, precision);
    let i0_beta = bessel_i0(&beta, precision)?;
    let pi = Float::with_val(precision, Constant::Pi);
    let mut coefficients = Vec::with_capacity(coefficient_count);
    let mut max_dc_correction_raw = Integer::new();
    let mut max_abs_error = Float::with_val(precision, 0);
    let mut max_phase_l1_error = Float::with_val(precision, 0);
    let unity_raw = Integer::from(1) << spec.coefficient_fractional_bits;

    for phase in 0..phase_count {
        let mut ideal = Vec::with_capacity(core.taps_per_phase);
        for tap in 0..core.taps_per_phase {
            ideal.push(design_tap(
                core,
                phase,
                tap,
                input_delay_frames,
                &cutoff,
                &beta,
                &i0_beta,
                &pi,
            )?);
        }

        let mut sum = Float::with_val(precision, 0);
        for coefficient in &ideal {
            sum += coefficient;
        }
        if sum.is_zero() {
            return Err(DesignError::ZeroPhaseGain(phase));
        }
        for coefficient in &mut ideal {
            *coefficient /= &sum;
        }

        let phase_start = coefficients.len();
        let mut raw_sum = Integer::new();
        for (tap, coefficient) in ideal.iter().enumerate() {
            let raw = quantize_big(coefficient, spec.coefficient_fractional_bits, phase, tap)?;
            raw_sum += &raw;
            coefficients.push(BigQ::from_raw(raw, coefficient_format)?);
        }

        let residual = &unity_raw - raw_sum;
        let mut correction_index = phase_start;
        let mut largest_magnitude = Integer::new();
        for (offset, coefficient) in coefficients[phase_start..].iter().enumerate() {
            let mut magnitude = coefficient.raw().clone();
            magnitude.abs_mut();
            if magnitude > largest_magnitude {
                largest_magnitude = magnitude;
                correction_index = phase_start + offset;
            }
        }
        let corrected_raw = Integer::from(coefficients[correction_index].raw() + &residual);
        coefficients[correction_index] = BigQ::from_raw(corrected_raw, coefficient_format)?;
        let mut correction_magnitude = residual;
        correction_magnitude.abs_mut();
        if correction_magnitude > max_dc_correction_raw {
            max_dc_correction_raw = correction_magnitude;
        }

        let mut phase_l1_error = Float::with_val(precision, 0);
        for (tap, coefficient) in ideal.iter().enumerate() {
            let mut quantized = Float::with_val(precision, coefficients[phase_start + tap].raw());
            quantized >>= spec.coefficient_fractional_bits;
            quantized -= coefficient;
            quantized.abs_mut();
            if quantized > max_abs_error {
                max_abs_error = quantized.clone();
            }
            phase_l1_error += quantized;
        }
        if phase_l1_error > max_phase_l1_error {
            max_phase_l1_error = phase_l1_error;
        }
    }

    let bank = PolyphaseFirBigQ63::for_ratio(
        core.ratio,
        core.taps_per_phase,
        coefficient_format,
        spec.accumulator_bits,
        coefficients.clone(),
    )?;
    let report = BigQuantizationReport {
        algorithm: "kaiser-windowed-sinc-big-v1",
        working_precision_bits: precision,
        coefficient_fractional_bits: spec.coefficient_fractional_bits,
        coefficient_count,
        max_dc_correction_raw: max_dc_correction_raw.to_string(),
        max_abs_coefficient_error_decimal: max_abs_error.to_string_radix(10, Some(20)),
        max_abs_coefficient_error_db: amplitude_db(&max_abs_error, precision),
        max_phase_l1_error_decimal: max_phase_l1_error.to_string_radix(10, Some(20)),
        max_phase_l1_error_db: amplitude_db(&max_phase_l1_error, precision),
        configured_accumulator_bits: spec.accumulator_bits,
        required_accumulator_bits: bank.required_accumulator_bits(),
        coefficient_sha256: coefficient_identity_big(spec, input_delay_frames, &coefficients),
    };
    Ok(DesignedBigFilter {
        spec: spec.clone(),
        input_delay_frames,
        bank,
        report,
    })
}

/// MPFR response-grid measurements of the quantized execution bank.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseAnalysis {
    pub analyzed_phases: usize,
    pub grid_points_per_band: u32,
    pub passband_end_decimal: String,
    pub stopband_start_decimal: String,
    pub passband_ripple_db: String,
    pub max_passband_deviation_decimal: String,
    pub stopband_peak_db: String,
}

/// Result of checking all error-producing parts measured by the designer
/// against one explicit amplitude floor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseCompliance {
    pub target: ErrorFloor,
    pub passband_deviation_meets_target: bool,
    pub stopband_meets_target: bool,
    pub coefficient_quantization_meets_target: bool,
}

impl ResponseCompliance {
    pub const fn meets_target(self) -> bool {
        self.passband_deviation_meets_target
            && self.stopband_meets_target
            && self.coefficient_quantization_meets_target
    }
}

/// Measure every phase of the quantized bank. Trigonometry and accumulation
/// use the same recorded MPFR working precision as coefficient design.
pub fn analyze_quantized_response(
    filter: &DesignedFilter,
    grid_points_per_band: u32,
) -> Result<ResponseAnalysis, DesignError> {
    analyze_quantized_response_internal(filter, grid_points_per_band, None)
        .map(|(analysis, _)| analysis)
}

/// Analyze the bank and verify passband deviation, stopband magnitude, and
/// the coefficient-quantization response bound against an explicit floor.
pub fn analyze_quantized_response_against(
    filter: &DesignedFilter,
    grid_points_per_band: u32,
    target: ErrorFloor,
) -> Result<(ResponseAnalysis, ResponseCompliance), DesignError> {
    let (analysis, compliance) =
        analyze_quantized_response_internal(filter, grid_points_per_band, Some(target))?;
    Ok((analysis, compliance.expect("target was provided")))
}

pub fn analyze_big_quantized_response(
    filter: &DesignedBigFilter,
    grid_points_per_band: u32,
) -> Result<ResponseAnalysis, DesignError> {
    analyze_big_quantized_response_internal(filter, grid_points_per_band, None)
        .map(|(analysis, _)| analysis)
}

pub fn analyze_big_quantized_response_against(
    filter: &DesignedBigFilter,
    grid_points_per_band: u32,
    target: ErrorFloor,
) -> Result<(ResponseAnalysis, ResponseCompliance), DesignError> {
    let (analysis, compliance) =
        analyze_big_quantized_response_internal(filter, grid_points_per_band, Some(target))?;
    Ok((analysis, compliance.expect("target was provided")))
}

pub fn analyze_windowed_quantized_response(
    filter: &DesignedWindowedFilter,
    grid_points_per_band: u32,
) -> Result<ResponseAnalysis, DesignError> {
    analyze_windowed_quantized_response_internal(filter, grid_points_per_band, None)
        .map(|(analysis, _)| analysis)
}

pub fn analyze_windowed_quantized_response_against(
    filter: &DesignedWindowedFilter,
    grid_points_per_band: u32,
    target: ErrorFloor,
) -> Result<(ResponseAnalysis, ResponseCompliance), DesignError> {
    let (analysis, compliance) =
        analyze_windowed_quantized_response_internal(filter, grid_points_per_band, Some(target))?;
    Ok((analysis, compliance.expect("target was provided")))
}

pub fn analyze_least_squares_quantized_response(
    filter: &DesignedLeastSquaresFilter,
    grid_points_per_band: u32,
) -> Result<ResponseAnalysis, DesignError> {
    analyze_least_squares_quantized_response_internal(filter, grid_points_per_band, None)
        .map(|(analysis, _)| analysis)
}

pub fn analyze_least_squares_quantized_response_against(
    filter: &DesignedLeastSquaresFilter,
    grid_points_per_band: u32,
    target: ErrorFloor,
) -> Result<(ResponseAnalysis, ResponseCompliance), DesignError> {
    let (analysis, compliance) = analyze_least_squares_quantized_response_internal(
        filter,
        grid_points_per_band,
        Some(target),
    )?;
    Ok((analysis, compliance.expect("target was provided")))
}

pub fn analyze_equiripple_quantized_response(
    filter: &DesignedEquirippleFilter,
    grid_points_per_band: u32,
) -> Result<ResponseAnalysis, DesignError> {
    analyze_equiripple_quantized_response_internal(filter, grid_points_per_band, None)
        .map(|(analysis, _)| analysis)
}

pub fn analyze_equiripple_quantized_response_against(
    filter: &DesignedEquirippleFilter,
    grid_points_per_band: u32,
    target: ErrorFloor,
) -> Result<(ResponseAnalysis, ResponseCompliance), DesignError> {
    let (analysis, compliance) =
        analyze_equiripple_quantized_response_internal(filter, grid_points_per_band, Some(target))?;
    Ok((analysis, compliance.expect("target was provided")))
}

fn analyze_quantized_response_internal(
    filter: &DesignedFilter,
    grid_points_per_band: u32,
    target: Option<ErrorFloor>,
) -> Result<(ResponseAnalysis, Option<ResponseCompliance>), DesignError> {
    let precision = filter.spec.working_precision_bits;
    analyze_response_internal(
        ResponseContext {
            ratio: filter.spec.ratio,
            rolloff: filter.spec.rolloff,
            precision: filter.spec.working_precision_bits,
            phase_count: filter.bank.phase_count(),
            coefficient_quantization_error_decimal: &filter.report.max_phase_l1_error_decimal,
            grid_points_per_band,
            target,
        },
        |phase| {
            Ok(filter
                .bank
                .phase(phase as u64)?
                .iter()
                .map(|coefficient| {
                    let mut value = Float::with_val(precision, coefficient.raw());
                    value >>= Q2_62::FRACTIONAL_BITS;
                    value
                })
                .collect())
        },
    )
}

fn analyze_big_quantized_response_internal(
    filter: &DesignedBigFilter,
    grid_points_per_band: u32,
    target: Option<ErrorFloor>,
) -> Result<(ResponseAnalysis, Option<ResponseCompliance>), DesignError> {
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(filter.bank.phase_count());
    analyze_big_quantized_response_internal_with_workers(
        filter,
        grid_points_per_band,
        target,
        workers,
    )
}

fn analyze_big_quantized_response_internal_with_workers(
    filter: &DesignedBigFilter,
    grid_points_per_band: u32,
    target: Option<ErrorFloor>,
    workers: usize,
) -> Result<(ResponseAnalysis, Option<ResponseCompliance>), DesignError> {
    let precision = filter.spec.core.working_precision_bits;
    let coefficient_fractional_bits = filter.spec.coefficient_fractional_bits;
    let context = ResponseContext {
        ratio: filter.spec.core.ratio,
        rolloff: filter.spec.core.rolloff,
        precision,
        phase_count: filter.bank.phase_count(),
        coefficient_quantization_error_decimal: &filter.report.max_phase_l1_error_decimal,
        grid_points_per_band,
        target,
    };
    let geometry = response_geometry(&context)?;
    let workers = workers.clamp(1, context.phase_count);
    if workers == 1 {
        return analyze_response_internal(context, |phase| {
            big_phase_coefficients(filter, phase, precision, coefficient_fractional_bits)
        });
    }

    let phases_per_worker = context.phase_count.div_ceil(workers);
    let measured = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for start in (0..context.phase_count).step_by(phases_per_worker) {
            let end = (start + phases_per_worker).min(context.phase_count);
            let geometry = &geometry;
            handles.push(scope.spawn(move || {
                let mut phases = Vec::with_capacity(end - start);
                for phase in start..end {
                    let coefficients = big_phase_coefficients(
                        filter,
                        phase,
                        precision,
                        coefficient_fractional_bits,
                    )?;
                    phases.push(measure_phase_response(&coefficients, geometry, precision));
                }
                Ok::<_, DesignError>(phases)
            }));
        }
        let mut phases = Vec::with_capacity(context.phase_count);
        for handle in handles {
            phases.extend(handle.join().expect("response worker must not panic")?);
        }
        Ok::<_, DesignError>(phases)
    })?;
    finish_response(context, geometry, measured)
}

fn big_phase_coefficients(
    filter: &DesignedBigFilter,
    phase: usize,
    precision: u32,
    coefficient_fractional_bits: u32,
) -> Result<Vec<Float>, DesignError> {
    Ok(filter
        .bank
        .phase(phase as u64)?
        .iter()
        .map(|coefficient| {
            let mut value = Float::with_val(precision, coefficient.raw());
            value >>= coefficient_fractional_bits;
            value
        })
        .collect())
}

fn analyze_windowed_quantized_response_internal(
    filter: &DesignedWindowedFilter,
    grid_points_per_band: u32,
    target: Option<ErrorFloor>,
) -> Result<(ResponseAnalysis, Option<ResponseCompliance>), DesignError> {
    let precision = filter.spec.working_precision_bits;
    analyze_response_internal(
        ResponseContext {
            ratio: filter.spec.ratio,
            rolloff: filter.spec.rolloff,
            precision,
            phase_count: filter.bank.phase_count(),
            coefficient_quantization_error_decimal: &filter.report.max_phase_l1_error_decimal,
            grid_points_per_band,
            target,
        },
        |phase| {
            Ok(filter
                .bank
                .phase(phase as u64)?
                .iter()
                .map(|coefficient| {
                    let mut value = Float::with_val(precision, coefficient.raw());
                    value >>= Q2_62::FRACTIONAL_BITS;
                    value
                })
                .collect())
        },
    )
}

fn analyze_least_squares_quantized_response_internal(
    filter: &DesignedLeastSquaresFilter,
    grid_points_per_band: u32,
    target: Option<ErrorFloor>,
) -> Result<(ResponseAnalysis, Option<ResponseCompliance>), DesignError> {
    let precision = filter.spec.working_precision_bits;
    analyze_response_internal(
        ResponseContext {
            ratio: filter.spec.ratio,
            rolloff: filter.spec.rolloff,
            precision,
            phase_count: filter.bank.phase_count(),
            coefficient_quantization_error_decimal: &filter.report.max_phase_l1_error_decimal,
            grid_points_per_band,
            target,
        },
        |phase| {
            Ok(filter
                .bank
                .phase(phase as u64)?
                .iter()
                .map(|coefficient| {
                    let mut value = Float::with_val(precision, coefficient.raw());
                    value >>= Q2_62::FRACTIONAL_BITS;
                    value
                })
                .collect())
        },
    )
}

fn analyze_equiripple_quantized_response_internal(
    filter: &DesignedEquirippleFilter,
    grid_points_per_band: u32,
    target: Option<ErrorFloor>,
) -> Result<(ResponseAnalysis, Option<ResponseCompliance>), DesignError> {
    let precision = filter.spec.working_precision_bits;
    analyze_response_internal(
        ResponseContext {
            ratio: filter.spec.ratio,
            rolloff: filter.spec.rolloff,
            precision,
            phase_count: filter.bank.phase_count(),
            coefficient_quantization_error_decimal: &filter
                .report
                .quantization
                .max_phase_l1_error_decimal,
            grid_points_per_band,
            target,
        },
        |phase| {
            Ok(filter
                .bank
                .phase(phase as u64)?
                .iter()
                .map(|coefficient| {
                    let mut value = Float::with_val(precision, coefficient.raw());
                    value >>= Q2_62::FRACTIONAL_BITS;
                    value
                })
                .collect())
        },
    )
}

struct ResponseContext<'a> {
    ratio: RateRatio,
    rolloff: Fraction,
    precision: u32,
    phase_count: usize,
    coefficient_quantization_error_decimal: &'a str,
    grid_points_per_band: u32,
    target: Option<ErrorFloor>,
}

struct ResponseGeometry {
    passband_end: Float,
    stopband_start: Float,
    passband_frequencies: Vec<Float>,
    stopband_frequencies: Vec<Float>,
}

struct PhaseResponseExtrema {
    passband_min: Float,
    passband_max: Float,
    max_passband_deviation: Float,
    stopband_max: Float,
}

fn analyze_response_internal<F>(
    context: ResponseContext<'_>,
    mut coefficients_for_phase: F,
) -> Result<(ResponseAnalysis, Option<ResponseCompliance>), DesignError>
where
    F: FnMut(usize) -> Result<Vec<Float>, DesignError>,
{
    let geometry = response_geometry(&context)?;
    let mut measured = Vec::with_capacity(context.phase_count);
    for phase in 0..context.phase_count {
        let coefficients = coefficients_for_phase(phase)?;
        measured.push(measure_phase_response(
            &coefficients,
            &geometry,
            context.precision,
        ));
    }
    finish_response(context, geometry, measured)
}

fn response_geometry(context: &ResponseContext<'_>) -> Result<ResponseGeometry, DesignError> {
    if context.grid_points_per_band < 2 {
        return Err(DesignError::AnalysisGridTooSmall(
            context.grid_points_per_band,
        ));
    }
    let lower_nyquist = lower_nyquist(context.ratio, context.precision);
    let mut passband_rolloff = float_from_fraction(context.rolloff, context.precision);
    passband_rolloff *= 2;
    passband_rolloff -= 1;
    let mut passband_end = lower_nyquist.clone();
    passband_end *= passband_rolloff;
    let stopband_start = lower_nyquist;
    let mut nyquist = Float::with_val(context.precision, 1);
    nyquist /= 2;
    let zero = Float::with_val(context.precision, 0);
    let passband_frequencies = (0..context.grid_points_per_band)
        .map(|point| {
            grid_frequency(
                &zero,
                &passband_end,
                point,
                context.grid_points_per_band,
                context.precision,
            )
        })
        .collect();
    let stopband_points = if stopband_start == nyquist {
        1
    } else {
        context.grid_points_per_band
    };
    let stopband_frequencies = (0..stopband_points)
        .map(|point| {
            if stopband_points == 1 {
                stopband_start.clone()
            } else {
                grid_frequency(
                    &stopband_start,
                    &nyquist,
                    point,
                    stopband_points,
                    context.precision,
                )
            }
        })
        .collect();
    Ok(ResponseGeometry {
        passband_end,
        stopband_start,
        passband_frequencies,
        stopband_frequencies,
    })
}

fn measure_phase_response(
    coefficients: &[Float],
    geometry: &ResponseGeometry,
    precision: u32,
) -> PhaseResponseExtrema {
    let mut passband = geometry
        .passband_frequencies
        .iter()
        .map(|frequency| magnitude_response(coefficients, frequency, precision));
    let first = passband
        .next()
        .expect("response geometry has at least two passband points");
    let mut passband_min = first.clone();
    let mut passband_max = first.clone();
    let mut max_passband_deviation = first;
    max_passband_deviation -= 1;
    max_passband_deviation.abs_mut();
    for magnitude in passband {
        if magnitude < passband_min {
            passband_min = magnitude.clone();
        }
        if magnitude > passband_max {
            passband_max = magnitude.clone();
        }
        let mut deviation = magnitude;
        deviation -= 1;
        deviation.abs_mut();
        if deviation > max_passband_deviation {
            max_passband_deviation = deviation;
        }
    }
    let mut stopband = geometry
        .stopband_frequencies
        .iter()
        .map(|frequency| magnitude_response(coefficients, frequency, precision));
    let mut stopband_max = stopband
        .next()
        .expect("response geometry has at least one stopband point");
    for magnitude in stopband {
        if magnitude > stopband_max {
            stopband_max = magnitude;
        }
    }
    PhaseResponseExtrema {
        passband_min,
        passband_max,
        max_passband_deviation,
        stopband_max,
    }
}

fn finish_response(
    context: ResponseContext<'_>,
    geometry: ResponseGeometry,
    measured: Vec<PhaseResponseExtrema>,
) -> Result<(ResponseAnalysis, Option<ResponseCompliance>), DesignError> {
    let mut measured = measured.into_iter();
    let first = measured.next().expect("validated non-empty phase bank");
    let mut passband_min = first.passband_min;
    let mut passband_max = first.passband_max;
    let mut max_passband_deviation = first.max_passband_deviation;
    let mut stopband_max = first.stopband_max;
    for phase in measured {
        if phase.passband_min < passband_min {
            passband_min = phase.passband_min;
        }
        if phase.passband_max > passband_max {
            passband_max = phase.passband_max;
        }
        if phase.max_passband_deviation > max_passband_deviation {
            max_passband_deviation = phase.max_passband_deviation;
        }
        if phase.stopband_max > stopband_max {
            stopband_max = phase.stopband_max;
        }
    }

    let compliance = if let Some(target) = context.target {
        let parsed = Float::parse(context.coefficient_quantization_error_decimal)
            .map_err(|_| DesignError::InvalidInternalMeasurement)?;
        let quantization_error = Float::with_val(context.precision, parsed);
        Some(ResponseCompliance {
            target,
            passband_deviation_meets_target: amplitude_meets_floor(
                &max_passband_deviation,
                target,
                context.precision,
            ),
            stopband_meets_target: amplitude_meets_floor(&stopband_max, target, context.precision),
            coefficient_quantization_meets_target: amplitude_meets_floor(
                &quantization_error,
                target,
                context.precision,
            ),
        })
    } else {
        None
    };
    let mut ripple_ratio = passband_max;
    ripple_ratio /= passband_min;
    Ok((
        ResponseAnalysis {
            analyzed_phases: context.phase_count,
            grid_points_per_band: context.grid_points_per_band,
            passband_end_decimal: geometry.passband_end.to_string_radix(10, Some(16)),
            stopband_start_decimal: geometry.stopband_start.to_string_radix(10, Some(16)),
            passband_ripple_db: amplitude_db(&ripple_ratio, context.precision),
            max_passband_deviation_decimal: max_passband_deviation.to_string_radix(10, Some(16)),
            stopband_peak_db: amplitude_db(&stopband_max, context.precision),
        },
        compliance,
    ))
}

fn amplitude_meets_floor(value: &Float, target: ErrorFloor, precision: u32) -> bool {
    if value.is_zero() {
        return true;
    }
    let mut measured_db = Float::with_val(precision, value);
    measured_db.log10_mut();
    measured_db *= 20;
    let target_db = Float::with_val(precision, -i64::from(target.attenuation_db()));
    measured_db <= target_db
}

fn lower_nyquist(ratio: RateRatio, precision: u32) -> Float {
    let mut result = Float::with_val(precision, 1);
    if ratio.up() < ratio.down() {
        result *= ratio.up();
        result /= ratio.down();
    }
    result /= 2;
    result
}

fn grid_frequency(start: &Float, end: &Float, point: u32, points: u32, precision: u32) -> Float {
    let mut span = Float::with_val(precision, end);
    span -= start;
    span *= point;
    span /= points - 1;
    span += start;
    span
}

#[derive(Clone, Debug)]
struct LeastSquaresGridPoint {
    frequency: Float,
    weight: Float,
    passband: bool,
}

fn build_least_squares_grid(spec: &LeastSquaresSpec) -> Vec<LeastSquaresGridPoint> {
    let precision = spec.working_precision_bits;
    let lower_nyquist = lower_nyquist(spec.ratio, precision);
    let mut passband_scale = float_from_fraction(spec.rolloff, precision);
    passband_scale *= 2;
    passband_scale -= 1;
    let mut passband_end = lower_nyquist.clone();
    passband_end *= passband_scale;
    let mut nyquist = Float::with_val(precision, 1);
    nyquist /= 2;
    let zero = Float::with_val(precision, 0);
    let passband_weight = float_from_fraction(spec.passband_weight, precision);
    let stopband_weight = float_from_fraction(spec.stopband_weight, precision);
    let stopband_points = if lower_nyquist == nyquist {
        1
    } else {
        spec.grid_points_per_band
    };
    let mut grid = Vec::with_capacity(
        usize::try_from(spec.grid_points_per_band + stopband_points)
            .expect("u32 point count fits usize on supported targets"),
    );
    for point in 0..spec.grid_points_per_band {
        grid.push(LeastSquaresGridPoint {
            frequency: grid_frequency(
                &zero,
                &passband_end,
                point,
                spec.grid_points_per_band,
                precision,
            ),
            weight: passband_weight.clone(),
            passband: true,
        });
    }
    for point in 0..stopband_points {
        grid.push(LeastSquaresGridPoint {
            frequency: if stopband_points == 1 {
                lower_nyquist.clone()
            } else {
                grid_frequency(&lower_nyquist, &nyquist, point, stopband_points, precision)
            },
            weight: stopband_weight.clone(),
            passband: false,
        });
    }
    grid
}

fn cosine_frequency_offset(frequency: &Float, offset: &Float, precision: u32) -> Float {
    let mut angle = Float::with_val(precision, frequency);
    angle *= 2;
    angle *= offset;
    Float::with_val(precision, angle.cos_pi_ref())
}

fn least_squares_correlation(
    grid: &[LeastSquaresGridPoint],
    taps: usize,
    precision: u32,
) -> Vec<Float> {
    let mut correlation = Vec::with_capacity(taps);
    for lag in 0..taps {
        let offset = Float::with_val(precision, lag);
        let mut sum = Float::with_val(precision, 0);
        for point in grid {
            let mut term = cosine_frequency_offset(&point.frequency, &offset, precision);
            term *= &point.weight;
            sum += term;
        }
        correlation.push(sum);
    }
    correlation
}

fn least_squares_right_hand_side(
    spec: &LeastSquaresSpec,
    phase: usize,
    delay: u64,
    grid: &[LeastSquaresGridPoint],
    precision: u32,
) -> Vec<Float> {
    let mut fractional_delay = Float::with_val(precision, delay);
    let mut phase_offset = Float::with_val(precision, phase);
    phase_offset /= spec.ratio.up();
    fractional_delay -= phase_offset;
    let mut result = Vec::with_capacity(spec.taps_per_phase);
    for tap in 0..spec.taps_per_phase {
        let mut offset = Float::with_val(precision, tap);
        offset -= &fractional_delay;
        let mut sum = Float::with_val(precision, 0);
        for point in grid.iter().filter(|point| point.passband) {
            let mut term = cosine_frequency_offset(&point.frequency, &offset, precision);
            term *= &point.weight;
            sum += term;
        }
        result.push(sum);
    }
    result
}

fn cholesky_toeplitz(
    correlation: &[Float],
    precision: u32,
) -> Result<Vec<Vec<Float>>, DesignError> {
    cholesky_matrix(correlation.len(), precision, |row, column| {
        correlation[row.abs_diff(column)].clone()
    })
}

fn cholesky_matrix(
    size: usize,
    precision: u32,
    element: impl Fn(usize, usize) -> Float,
) -> Result<Vec<Vec<Float>>, DesignError> {
    let zero = Float::with_val(precision, 0);
    let mut lower = vec![vec![zero; size]; size];
    for row in 0..size {
        for column in 0..=row {
            let mut value = element(row, column);
            for (row_value, column_value) in
                lower[row][..column].iter().zip(&lower[column][..column])
            {
                let mut product = Float::with_val(precision, row_value);
                product *= column_value;
                value -= product;
            }
            if row == column {
                if value <= 0 {
                    return Err(DesignError::SingularLeastSquaresSystem { pivot: row });
                }
                value.sqrt_mut();
                lower[row][column] = value;
            } else {
                value /= &lower[column][column];
                lower[row][column] = value;
            }
        }
    }
    Ok(lower)
}

fn cholesky_solve(lower: &[Vec<Float>], right_hand_side: &[Float], precision: u32) -> Vec<Float> {
    let size = lower.len();
    let mut forward = Vec::with_capacity(size);
    for row in 0..size {
        let mut value = right_hand_side[row].clone();
        for (column, solved) in forward.iter().enumerate() {
            let mut product = Float::with_val(precision, &lower[row][column]);
            product *= solved;
            value -= product;
        }
        value /= &lower[row][row];
        forward.push(value);
    }
    let mut result = vec![Float::with_val(precision, 0); size];
    for row in (0..size).rev() {
        let mut value = forward[row].clone();
        for column in (row + 1)..size {
            let mut product = Float::with_val(precision, &lower[column][row]);
            product *= &result[column];
            value -= product;
        }
        value /= &lower[row][row];
        result[row] = value;
    }
    result
}

#[derive(Clone, Debug)]
struct RemezGridPoint {
    frequency: Float,
    desired: Float,
    weight: Float,
    band: u8,
}

#[derive(Clone, Debug)]
struct RemezExchangeResult {
    amplitudes: Vec<Float>,
    extrema: Vec<usize>,
    iterations: u32,
    max_weighted_error: Float,
}

fn build_remez_grid(
    spec: &EquirippleSpec,
    points_per_band: usize,
    precision: u32,
) -> Vec<RemezGridPoint> {
    build_prototype_grid(
        spec.ratio,
        spec.rolloff,
        (spec.passband_weight, spec.stopband_weight),
        points_per_band,
        precision,
    )
}

fn build_prototype_grid(
    ratio: RateRatio,
    rolloff: Fraction,
    weights: (Fraction, Fraction),
    points_per_band: usize,
    precision: u32,
) -> Vec<RemezGridPoint> {
    let lower_nyquist = lower_nyquist(ratio, precision);
    let mut passband_scale = float_from_fraction(rolloff, precision);
    passband_scale *= 2;
    passband_scale -= 1;
    let mut passband_end = lower_nyquist.clone();
    passband_end *= passband_scale;
    passband_end /= ratio.up();
    let mut stopband_start = lower_nyquist;
    stopband_start /= ratio.up();
    let mut nyquist = Float::with_val(precision, 1);
    nyquist /= 2;
    let zero = Float::with_val(precision, 0);
    let passband_weight = float_from_fraction(weights.0, precision);
    let stopband_weight = float_from_fraction(weights.1, precision);
    let passband_desired = Float::with_val(precision, ratio.up());
    let grid_count = prototype_grid_points(ratio, points_per_band as u128)
        .and_then(|count| usize::try_from(count).ok())
        .expect("validated prototype-grid budget");
    let mut grid = Vec::with_capacity(grid_count);
    let point_count = u32::try_from(points_per_band)
        .expect("validated prototype-grid budget keeps the grid within u32");
    for point in 0..point_count {
        grid.push(RemezGridPoint {
            frequency: grid_frequency(&zero, &passband_end, point, point_count, precision),
            desired: passband_desired.clone(),
            weight: passband_weight.clone(),
            band: 0,
        });
    }
    let stop_count =
        u32::try_from(grid_count - points_per_band).expect("validated prototype stopband grid");
    for point in 0..stop_count {
        grid.push(RemezGridPoint {
            frequency: if stop_count == 1 {
                stopband_start.clone()
            } else {
                grid_frequency(&stopband_start, &nyquist, point, stop_count, precision)
            },
            desired: Float::with_val(precision, 0),
            weight: stopband_weight.clone(),
            band: 1,
        });
    }
    grid
}

fn prototype_grid_points(ratio: RateRatio, per_band: u128) -> Option<u128> {
    per_band.checked_add(if ratio.up() == ratio.down() {
        1
    } else {
        per_band
    })
}

fn remez_work(extrema: u128, grid: u128, iterations: u32) -> Option<u128> {
    let candidates = extrema.checked_mul(2)?.checked_add(4)?;
    let solve = extrema
        .checked_pow(3)?
        .checked_add(extrema.checked_pow(2)?)?;
    let evaluation = grid.checked_mul(extrema)?.checked_mul(2)?;
    let selection = extrema.checked_mul(candidates.checked_pow(2)?)?;
    let scans = grid.checked_mul(8)?.checked_add(extrema.checked_mul(8)?)?;
    let initialization = grid.checked_mul(4)?.checked_add(extrema.checked_mul(12)?)?;
    solve
        .checked_add(evaluation)?
        .checked_add(selection)?
        .checked_add(scans)?
        .checked_mul(u128::from(iterations))?
        .checked_add(initialization)
}

fn initial_remez_extrema(grid: &[RemezGridPoint], required: usize, precision: u32) -> Vec<usize> {
    let band_split = grid
        .iter()
        .position(|point| point.band != grid[0].band)
        .expect("validated two-band Remez grid");
    if band_split + 1 == grid.len() {
        let mut extrema: Vec<_> = (0..required - 1)
            .map(|ordinal| {
                (ordinal as u128 * (band_split - 1) as u128 / (required - 2) as u128) as usize
            })
            .collect();
        extrema.push(grid.len() - 1);
        return extrema;
    }
    let mut passband_width = Float::with_val(precision, &grid[band_split - 1].frequency);
    passband_width -= &grid[0].frequency;
    let mut total_width = Float::with_val(precision, &grid[grid.len() - 1].frequency);
    total_width -= &grid[band_split].frequency;
    total_width += &passband_width;
    let mut extrema = Vec::with_capacity(required);
    let mut cursor = 0;
    for ordinal in 0..required {
        let mut target = Float::with_val(precision, &total_width);
        target *= ordinal;
        target /= required - 1;
        if target <= passband_width {
            target += &grid[0].frequency;
        } else {
            target -= &passband_width;
            target += &grid[band_split].frequency;
        }
        while cursor + 1 < grid.len() && grid[cursor + 1].frequency <= target {
            cursor += 1;
        }
        let mut selected = cursor;
        if cursor + 1 < grid.len() {
            let mut left_distance = Float::with_val(precision, &target);
            left_distance -= &grid[cursor].frequency;
            left_distance.abs_mut();
            let mut right_distance = Float::with_val(precision, &grid[cursor + 1].frequency);
            right_distance -= target;
            right_distance.abs_mut();
            if right_distance < left_distance {
                selected += 1;
            }
        }
        let minimum = extrema.last().map_or(0, |previous| previous + 1);
        let maximum = grid.len() - (required - ordinal);
        extrema.push(selected.clamp(minimum, maximum));
    }
    extrema
}

fn remez_grid_converged(
    errors: &[Float],
    extrema: &[usize],
    solved_ripple: &Float,
    precision: u32,
) -> bool {
    if solved_ripple <= &0
        || !solved_ripple.is_finite()
        || extrema
            .windows(2)
            .any(|pair| (errors[pair[0]] > 0) == (errors[pair[1]] > 0))
    {
        return false;
    }
    let mut tolerance = Float::with_val(precision, solved_ripple);
    tolerance >>= precision / 3;
    let mut lower = Float::with_val(precision, solved_ripple);
    lower -= &tolerance;
    let mut upper = Float::with_val(precision, solved_ripple);
    upper += tolerance;
    errors
        .iter()
        .all(|error| error.is_finite() && error.clone().abs() <= upper)
        && extrema
            .iter()
            .all(|index| errors[*index].clone().abs() >= lower)
}

fn remez_exchange(
    spec: &EquirippleSpec,
    amplitude_count: usize,
    grid: &[RemezGridPoint],
    precision: u32,
    cache_cosines: bool,
) -> Result<RemezExchangeResult, DesignError> {
    let cosines = RemezCosines::new(grid, amplitude_count, precision, cache_cosines);
    let extrema_count = amplitude_count + 1;
    let mut extrema = initial_remez_extrema(grid, extrema_count, precision);
    for iteration in 1..=spec.max_iterations {
        let solution = solve_remez_system(&cosines, &extrema, amplitude_count, precision)?;
        let amplitudes = solution[..amplitude_count].to_vec();
        let mut solved_ripple = solution[amplitude_count].clone();
        solved_ripple.abs_mut();
        let errors = evaluate_remez_errors(&cosines, &amplitudes, precision);
        let next_extrema = select_remez_extrema(grid, &errors, extrema_count)?;
        let max_weighted_error = errors
            .iter()
            .map(|error| {
                let mut magnitude = error.clone();
                magnitude.abs_mut();
                magnitude
            })
            .max_by(|left, right| left.partial_cmp(right).expect("finite MPFR errors"))
            .expect("validated non-empty Remez grid");
        if remez_grid_converged(&errors, &extrema, &solved_ripple, precision) {
            return Ok(RemezExchangeResult {
                amplitudes,
                extrema,
                iterations: iteration,
                max_weighted_error,
            });
        }
        extrema = next_extrema;
    }
    Err(DesignError::RemezDidNotConverge {
        iterations: spec.max_iterations,
    })
}

fn solve_remez_system(
    cosines: &RemezCosines<'_>,
    extrema: &[usize],
    amplitude_count: usize,
    precision: u32,
) -> Result<Vec<Float>, DesignError> {
    let size = amplitude_count + 1;
    let mut matrix = vec![vec![Float::with_val(precision, 0); size]; size];
    let mut right_hand_side = Vec::with_capacity(size);
    for (row, grid_index) in extrema.iter().copied().enumerate() {
        let point = &cosines.grid[grid_index];
        for (harmonic, value) in matrix[row][..amplitude_count].iter_mut().enumerate() {
            *value = cosines.value(grid_index, harmonic);
        }
        let mut alternation =
            Float::with_val(precision, if row.is_multiple_of(2) { 1 } else { -1 });
        alternation /= &point.weight;
        matrix[row][amplitude_count] = alternation;
        right_hand_side.push(point.desired.clone());
    }
    solve_linear_system(matrix, right_hand_side, precision)
}

fn solve_linear_system(
    mut matrix: Vec<Vec<Float>>,
    mut right_hand_side: Vec<Float>,
    precision: u32,
) -> Result<Vec<Float>, DesignError> {
    let size = matrix.len();
    for column in 0..size {
        let mut pivot = column;
        let mut pivot_magnitude = matrix[column][column].clone();
        pivot_magnitude.abs_mut();
        for (row, values) in matrix.iter().enumerate().skip(column + 1) {
            let mut magnitude = values[column].clone();
            magnitude.abs_mut();
            if magnitude > pivot_magnitude {
                pivot = row;
                pivot_magnitude = magnitude;
            }
        }
        if pivot_magnitude.is_zero() {
            return Err(DesignError::SingularRemezSystem { pivot: column });
        }
        if pivot != column {
            matrix.swap(pivot, column);
            right_hand_side.swap(pivot, column);
        }
        let pivot_row = matrix[column].clone();
        let pivot_right = right_hand_side[column].clone();
        for row in (column + 1)..size {
            let mut factor = Float::with_val(precision, &matrix[row][column]);
            factor /= &pivot_row[column];
            matrix[row][column] = Float::with_val(precision, 0);
            for (target, source) in matrix[row][(column + 1)..]
                .iter_mut()
                .zip(&pivot_row[(column + 1)..])
            {
                let mut product = Float::with_val(precision, source);
                product *= &factor;
                *target -= product;
            }
            let mut product = pivot_right.clone();
            product *= factor;
            right_hand_side[row] -= product;
        }
    }

    let mut solution = vec![Float::with_val(precision, 0); size];
    for row in (0..size).rev() {
        let mut value = right_hand_side[row].clone();
        for (coefficient, solved) in matrix[row][(row + 1)..].iter().zip(&solution[(row + 1)..]) {
            let mut product = Float::with_val(precision, coefficient);
            product *= solved;
            value -= product;
        }
        value /= &matrix[row][row];
        solution[row] = value;
    }
    Ok(solution)
}

fn remez_cosine_cache_bytes(grid: u64, amplitudes: u64, precision: u32) -> Option<u64> {
    let float_bytes = 64_u128 + u128::from(precision.div_ceil(64)) * 8;
    let bytes = u128::from(grid)
        .checked_mul(u128::from(amplitudes))?
        .checked_mul(float_bytes)?
        .checked_add(u128::from(grid).checked_mul(32)?)?;
    u64::try_from(bytes).ok()
}

struct RemezCosines<'grid> {
    grid: &'grid [RemezGridPoint],
    precision: u32,
    table: Option<Vec<Vec<Float>>>,
}

impl<'grid> RemezCosines<'grid> {
    fn new(grid: &'grid [RemezGridPoint], amplitudes: usize, precision: u32, cached: bool) -> Self {
        let table = cached.then(|| {
            grid.iter()
                .map(|point| {
                    (0..amplitudes)
                        .map(|harmonic| {
                            let offset = Float::with_val(precision, harmonic);
                            cosine_frequency_offset(&point.frequency, &offset, precision)
                        })
                        .collect()
                })
                .collect()
        });
        Self {
            grid,
            precision,
            table,
        }
    }
    fn value(&self, point: usize, harmonic: usize) -> Float {
        if let Some(table) = &self.table {
            table[point][harmonic].clone()
        } else {
            let offset = Float::with_val(self.precision, harmonic);
            cosine_frequency_offset(&self.grid[point].frequency, &offset, self.precision)
        }
    }
}

fn evaluate_remez_errors(
    cosines: &RemezCosines<'_>,
    amplitudes: &[Float],
    precision: u32,
) -> Vec<Float> {
    cosines
        .grid
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let mut response = Float::with_val(precision, 0);
            for (harmonic, amplitude) in amplitudes.iter().enumerate() {
                let mut term = cosines.value(index, harmonic);
                term *= amplitude;
                response += term;
            }
            response -= &point.desired;
            response *= &point.weight;
            response
        })
        .collect()
}

fn select_remez_extrema(
    grid: &[RemezGridPoint],
    errors: &[Float],
    required: usize,
) -> Result<Vec<usize>, DesignError> {
    // Explicit resource policy, not a theorem about rounded MPFR evaluations.
    // Plateaus may create arbitrarily many candidates: reject rather than
    // truncate them or allocate an unbudgeted dynamic-programming table.
    let maximum = required.saturating_mul(2).saturating_add(4);
    let mut local_extrema = Vec::new();
    let mut band_start = 0;
    while band_start < grid.len() {
        let band = grid[band_start].band;
        let mut band_end = band_start + 1;
        while band_end < grid.len() && grid[band_end].band == band {
            band_end += 1;
        }
        for index in band_start..band_end {
            let mut magnitude = errors[index].clone();
            magnitude.abs_mut();
            let band_endpoint = index == band_start || index + 1 == band_end;
            let left_ok = if index == band_start {
                true
            } else {
                let mut left = errors[index - 1].clone();
                left.abs_mut();
                (errors[index] > 0) != (errors[index - 1] > 0) || magnitude >= left
            };
            let right_ok = if index + 1 == band_end {
                true
            } else {
                let mut right = errors[index + 1].clone();
                right.abs_mut();
                (errors[index] > 0) != (errors[index + 1] > 0) || magnitude >= right
            };
            if (band_endpoint || (left_ok && right_ok)) && !magnitude.is_zero() {
                if local_extrema.len() == maximum {
                    return Err(DesignError::RemezExtremaBudgetExceeded {
                        found: maximum.saturating_add(1),
                        maximum,
                    });
                }
                local_extrema.push(index);
            }
        }
        band_start = band_end;
    }
    if local_extrema.len() < required {
        return Err(DesignError::RemezExtremaUnavailable {
            required,
            found: local_extrema.len(),
        });
    }

    let candidate_count = local_extrema.len();
    let mut scores = vec![vec![None::<Float>; candidate_count]; required + 1];
    let mut predecessors = vec![vec![None::<usize>; candidate_count]; required + 1];
    for (position, index) in local_extrema.iter().copied().enumerate() {
        let mut magnitude = errors[index].clone();
        magnitude.abs_mut();
        scores[1][position] = Some(magnitude);
    }
    for selected in 2..=required {
        for end in 0..candidate_count {
            let end_index = local_extrema[end];
            let end_positive = errors[end_index] > 0;
            let mut magnitude = errors[end_index].clone();
            magnitude.abs_mut();
            let mut best_score: Option<Float> = None;
            let mut best_predecessor = None;
            for predecessor in 0..end {
                let predecessor_index = local_extrema[predecessor];
                if (errors[predecessor_index] > 0) == end_positive {
                    continue;
                }
                if let Some(previous_score) = &scores[selected - 1][predecessor] {
                    let mut candidate_score = previous_score.clone();
                    candidate_score += &magnitude;
                    if best_score
                        .as_ref()
                        .is_none_or(|current| candidate_score > *current)
                    {
                        best_score = Some(candidate_score);
                        best_predecessor = Some(predecessor);
                    }
                }
            }
            scores[selected][end] = best_score;
            predecessors[selected][end] = best_predecessor;
        }
    }

    let best_end = scores[required]
        .iter()
        .enumerate()
        .filter_map(|(position, score)| score.as_ref().map(|score| (position, score)))
        .max_by(|(_, left), (_, right)| left.partial_cmp(right).expect("finite MPFR errors"))
        .map(|(position, _)| position);
    if let Some(mut end) = best_end {
        let mut result = vec![0_usize; required];
        for selected in (1..=required).rev() {
            result[selected - 1] = local_extrema[end];
            if selected > 1 {
                end = predecessors[selected][end].expect("dynamic path has a predecessor");
            }
        }
        return Ok(result);
    }

    // Early exchange iterations can lack a complete alternating sequence.
    // Force the strongest local extrema into the next interpolation system;
    // the solve itself imposes alternation and the next pass returns to the
    // dynamic alternating selection above.
    local_extrema.sort_by(|left, right| {
        let mut left_magnitude = errors[*left].clone();
        left_magnitude.abs_mut();
        let mut right_magnitude = errors[*right].clone();
        right_magnitude.abs_mut();
        right_magnitude
            .partial_cmp(&left_magnitude)
            .expect("finite MPFR errors")
            .then_with(|| left.cmp(right))
    });
    local_extrema.truncate(required);
    local_extrema.sort_unstable();
    Ok(local_extrema)
}

fn magnitude_response(coefficients: &[Float], frequency: &Float, precision: u32) -> Float {
    let mut angle = Float::with_val(precision, frequency);
    angle *= 2;
    let cosine_step = Float::with_val(precision, angle.cos_pi_ref());
    let sine_step = Float::with_val(precision, angle.sin_pi_ref());
    let mut cosine = Float::with_val(precision, 1);
    let mut sine = Float::with_val(precision, 0);
    let mut real = Float::with_val(precision, 0);
    let mut imaginary = Float::with_val(precision, 0);

    for coefficient in coefficients {
        let mut real_term = Float::with_val(precision, coefficient);
        real_term *= &cosine;
        real += real_term;
        let mut imaginary_term = Float::with_val(precision, coefficient);
        imaginary_term *= &sine;
        imaginary -= imaginary_term;

        let mut next_cosine = Float::with_val(precision, &cosine);
        next_cosine *= &cosine_step;
        let mut cross = Float::with_val(precision, &sine);
        cross *= &sine_step;
        next_cosine -= cross;

        let mut next_sine = Float::with_val(precision, &sine);
        next_sine *= &cosine_step;
        let mut cross = Float::with_val(precision, &cosine);
        cross *= &sine_step;
        next_sine += cross;
        cosine = next_cosine;
        sine = next_sine;
    }

    let real_copy = real.clone();
    real *= real_copy;
    let imaginary_copy = imaginary.clone();
    imaginary *= imaginary_copy;
    real += imaginary;
    real.sqrt_mut();
    real
}

fn amplitude_db(value: &Float, precision: u32) -> String {
    if value.is_zero() {
        return "-inf".to_owned();
    }
    let mut db = Float::with_val(precision, value);
    db.log10_mut();
    db *= 20;
    db.to_string_radix(10, Some(16))
}

fn cutoff_for(spec: &KaiserSpec, precision: u32) -> Float {
    let mut cutoff = Float::with_val(precision, 1);
    cutoff /= 2;
    if spec.ratio.up() < spec.ratio.down() {
        cutoff *= spec.ratio.up();
        cutoff /= spec.ratio.down();
    }
    cutoff *= spec.rolloff.numerator;
    cutoff /= spec.rolloff.denominator;
    cutoff
}

fn cutoff_for_windowed(spec: &WindowedSincSpec, precision: u32) -> Float {
    let mut cutoff = Float::with_val(precision, 1);
    cutoff /= 2;
    if spec.ratio.up() < spec.ratio.down() {
        cutoff *= spec.ratio.up();
        cutoff /= spec.ratio.down();
    }
    cutoff *= spec.rolloff.numerator;
    cutoff /= spec.rolloff.denominator;
    cutoff
}

fn float_from_fraction(value: Fraction, precision: u32) -> Float {
    let mut result = Float::with_val(precision, value.numerator);
    result /= value.denominator;
    result
}

#[allow(clippy::too_many_arguments)]
fn design_tap(
    spec: &KaiserSpec,
    phase: usize,
    tap: usize,
    delay: u64,
    cutoff: &Float,
    beta: &Float,
    i0_beta: &Float,
    pi: &Float,
) -> Result<Float, DesignError> {
    let precision = spec.working_precision_bits;
    let mut x_numerator = Integer::from(tap);
    x_numerator *= spec.ratio.up();
    x_numerator += phase;
    let mut delay_numerator = Integer::from(delay);
    delay_numerator *= spec.ratio.up();
    x_numerator -= delay_numerator;
    let mut x = Float::with_val(precision, x_numerator);
    x /= spec.ratio.up();

    let mut abs_x = x.clone();
    abs_x.abs_mut();
    if abs_x > delay {
        return Ok(Float::with_val(precision, 0));
    }

    let window = kaiser_window(&x, delay, beta, i0_beta, precision)?;
    let mut argument = Float::with_val(precision, &x);
    argument *= cutoff;
    argument *= 2;
    let sinc = if argument.is_zero() {
        Float::with_val(precision, 1)
    } else {
        let mut value = Float::with_val(precision, argument.sin_pi_ref());
        let mut denominator = Float::with_val(precision, pi);
        denominator *= &argument;
        value /= denominator;
        value
    };
    let mut coefficient = Float::with_val(precision, cutoff);
    coefficient *= 2;
    coefficient *= sinc;
    coefficient *= window;
    Ok(coefficient)
}

fn design_windowed_tap(
    spec: &WindowedSincSpec,
    phase: usize,
    tap: usize,
    delay: u64,
    cutoff: &Float,
    pi: &Float,
    dolph: Option<&DolphWindow>,
) -> Result<Float, DesignError> {
    let precision = spec.working_precision_bits;
    let mut x_numerator = Integer::from(tap);
    x_numerator *= spec.ratio.up();
    x_numerator += phase;
    let mut delay_numerator = Integer::from(delay);
    delay_numerator *= spec.ratio.up();
    x_numerator -= delay_numerator;
    let mut x = Float::with_val(precision, x_numerator);
    x /= spec.ratio.up();

    let mut abs_x = x.clone();
    abs_x.abs_mut();
    if abs_x > delay {
        return Ok(Float::with_val(precision, 0));
    }
    let window = simple_window(spec.window, &x, delay, precision, dolph)?;
    let mut argument = Float::with_val(precision, &x);
    argument *= cutoff;
    argument *= 2;
    let sinc = if argument.is_zero() {
        Float::with_val(precision, 1)
    } else {
        let mut value = Float::with_val(precision, argument.sin_pi_ref());
        let mut denominator = Float::with_val(precision, pi);
        denominator *= &argument;
        value /= denominator;
        value
    };
    let mut coefficient = Float::with_val(precision, cutoff);
    coefficient *= 2;
    coefficient *= sinc;
    coefficient *= window;
    Ok(coefficient)
}

#[derive(Clone, Debug)]
struct DolphWindow {
    length: usize,
    precision: u32,
    spectrum: Vec<Float>,
    center_normalization: Float,
}

impl DolphWindow {
    fn new(length: usize, attenuation_db: u32, precision: u32) -> Result<Self, DesignError> {
        if attenuation_db == 0 {
            return Err(DesignError::InvalidDolphAttenuation(attenuation_db));
        }
        let order = length - 1;
        let mut log_ten = Float::with_val(precision, 10);
        log_ten.ln_mut();
        log_ten *= attenuation_db;
        log_ten /= 20;
        log_ten.exp_mut();
        let mut beta = log_ten;
        beta.acosh_mut();
        beta /= order;
        beta.cosh_mut();

        let mut spectrum = Vec::with_capacity(length);
        for index in 0..length {
            let mut angle = Float::with_val(precision, index);
            angle /= length;
            let mut x = Float::with_val(precision, angle.cos_pi_ref());
            x *= &beta;
            let value = if x > 1 {
                x.acosh_mut();
                x *= order;
                x.cosh_mut();
                x
            } else if x < -1 {
                x = -x;
                x.acosh_mut();
                x *= order;
                x.cosh_mut();
                if order.is_multiple_of(2) { x } else { -x }
            } else {
                x.acos_mut();
                x *= order;
                x.cos_mut();
                x
            };
            spectrum.push(value);
        }
        let mut center_normalization = Float::with_val(precision, 0);
        for value in &spectrum {
            center_normalization += value;
        }
        if center_normalization.is_zero() {
            return Err(DesignError::ZeroDolphNormalization);
        }
        Ok(Self {
            length,
            precision,
            spectrum,
            center_normalization,
        })
    }

    fn at_distance(&self, distance: &Float) -> Float {
        let mut result = Float::with_val(self.precision, 0);
        for (index, frequency_sample) in self.spectrum.iter().enumerate() {
            let mut angle = Float::with_val(self.precision, distance);
            // Inverse-DFT bins above N/2 represent negative frequencies.
            // Positive aliases agree at integer x only, not at polyphase positions.
            angle *= 2 * index.min(self.length - index);
            angle /= self.length;
            let mut term = Float::with_val(self.precision, angle.cos_pi_ref());
            term *= frequency_sample;
            result += term;
        }
        result /= &self.center_normalization;
        result
    }
}

fn simple_window(
    kind: WindowFunction,
    x: &Float,
    delay: u64,
    precision: u32,
    dolph: Option<&DolphWindow>,
) -> Result<Float, DesignError> {
    Ok(match kind {
        WindowFunction::Rectangular => Float::with_val(precision, 1),
        WindowFunction::Hann => {
            let mut normalized = Float::with_val(precision, x);
            normalized /= delay;
            let mut value = Float::with_val(precision, normalized.cos_pi_ref());
            value += 1;
            value /= 2;
            value
        }
        WindowFunction::Blackman => {
            let mut normalized = Float::with_val(precision, x);
            normalized /= delay;
            let mut first = Float::with_val(precision, normalized.cos_pi_ref());
            first /= 2;
            let mut doubled = normalized;
            doubled *= 2;
            let mut second = Float::with_val(precision, doubled.cos_pi_ref());
            second *= 2;
            second /= 25;
            let mut value = Float::with_val(precision, 21);
            value /= 50;
            value += first;
            value += second;
            value
        }
        WindowFunction::DolphChebyshev { .. } => dolph
            .ok_or(DesignError::InvalidInternalMeasurement)?
            .at_distance(x),
    })
}

fn kaiser_window(
    x: &Float,
    delay: u64,
    beta: &Float,
    i0_beta: &Float,
    precision: u32,
) -> Result<Float, DesignError> {
    let mut normalized = Float::with_val(precision, x);
    normalized /= delay;
    let copy = normalized.clone();
    normalized *= copy;
    let mut radial = Float::with_val(precision, 1);
    radial -= normalized;
    if radial < 0 {
        return Ok(Float::with_val(precision, 0));
    }
    radial.sqrt_mut();
    radial *= beta;
    let mut window = bessel_i0(&radial, precision)?;
    window /= i0_beta;
    Ok(window)
}

fn bessel_i0(value: &Float, precision: u32) -> Result<Float, DesignError> {
    let mut factor = Float::with_val(precision, value);
    factor *= value;
    factor /= 4;
    let mut term = Float::with_val(precision, 1);
    let mut sum = Float::with_val(precision, 1);
    let iteration_limit = precision.saturating_mul(4).saturating_add(256);
    for index in 1..=iteration_limit {
        term *= &factor;
        let divisor = u64::from(index) * u64::from(index);
        term /= divisor;
        let previous = sum.clone();
        sum += &term;
        if sum == previous {
            return Ok(sum);
        }
    }
    Err(DesignError::BesselDidNotConverge)
}

fn quantize_q2_62(value: &Float, phase: usize, tap: usize) -> Result<i64, DesignError> {
    let mut scaled = value.clone();
    scaled <<= Q2_62::FRACTIONAL_BITS;
    let (integer, _) = scaled
        .to_integer_round(Round::Nearest)
        .ok_or(DesignError::NonFiniteCoefficient { phase, tap })?;
    integer
        .to_i64()
        .ok_or(DesignError::CoefficientOutOfRange { phase, tap })
}

fn quantize_big(
    value: &Float,
    fractional_bits: u32,
    phase: usize,
    tap: usize,
) -> Result<Integer, DesignError> {
    let mut scaled = value.clone();
    scaled <<= fractional_bits;
    scaled
        .to_integer_round(Round::Nearest)
        .map(|(integer, _)| integer)
        .ok_or(DesignError::NonFiniteCoefficient { phase, tap })
}

fn coefficient_identity<'a>(
    spec: &KaiserSpec,
    delay: u64,
    coefficients: impl IntoIterator<Item = &'a Q2_62>,
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"sex/sexfir/kaiser-windowed-sinc-v1\0");
    hash.update(spec.ratio.up().to_le_bytes());
    hash.update(spec.ratio.down().to_le_bytes());
    hash.update((spec.taps_per_phase as u64).to_le_bytes());
    hash.update(spec.rolloff.numerator.to_le_bytes());
    hash.update(spec.rolloff.denominator.to_le_bytes());
    hash.update(spec.beta.numerator.to_le_bytes());
    hash.update(spec.beta.denominator.to_le_bytes());
    hash.update(spec.working_precision_bits.to_le_bytes());
    hash.update(delay.to_le_bytes());
    for coefficient in coefficients {
        hash.update(coefficient.raw().to_le_bytes());
    }
    let digest = hash.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn coefficient_identity_windowed(
    spec: &WindowedSincSpec,
    delay: u64,
    coefficients: &[Q2_62],
) -> String {
    let mut hash = Sha256::new();
    hash.update(
        if matches!(spec.window, WindowFunction::DolphChebyshev { .. }) {
            b"sex/sexfir/windowed-sinc-dolph-v2\0".as_slice()
        } else {
            b"sex/sexfir/windowed-sinc-v1\0".as_slice()
        },
    );
    hash.update([match spec.window {
        WindowFunction::Rectangular => 0,
        WindowFunction::Hann => 1,
        WindowFunction::Blackman => 2,
        WindowFunction::DolphChebyshev { .. } => 3,
    }]);
    if let WindowFunction::DolphChebyshev { attenuation_db } = spec.window {
        hash.update(attenuation_db.to_le_bytes());
    }
    hash.update(spec.ratio.up().to_le_bytes());
    hash.update(spec.ratio.down().to_le_bytes());
    hash.update((spec.taps_per_phase as u64).to_le_bytes());
    hash.update(spec.rolloff.numerator.to_le_bytes());
    hash.update(spec.rolloff.denominator.to_le_bytes());
    hash.update(spec.working_precision_bits.to_le_bytes());
    hash.update(delay.to_le_bytes());
    for coefficient in coefficients {
        hash.update(coefficient.raw().to_le_bytes());
    }
    encode_sha256(hash)
}

fn coefficient_identity_least_squares(
    spec: &LeastSquaresSpec,
    delay: u64,
    coefficients: &[Q2_62],
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"sex/sexfir/least-squares-lowpass-v1\0");
    hash.update(spec.ratio.up().to_le_bytes());
    hash.update(spec.ratio.down().to_le_bytes());
    hash.update((spec.taps_per_phase as u64).to_le_bytes());
    hash.update(spec.rolloff.numerator.to_le_bytes());
    hash.update(spec.rolloff.denominator.to_le_bytes());
    hash.update(spec.passband_weight.numerator.to_le_bytes());
    hash.update(spec.passband_weight.denominator.to_le_bytes());
    hash.update(spec.stopband_weight.numerator.to_le_bytes());
    hash.update(spec.stopband_weight.denominator.to_le_bytes());
    hash.update(spec.grid_points_per_band.to_le_bytes());
    hash.update(spec.working_precision_bits.to_le_bytes());
    hash.update(delay.to_le_bytes());
    for coefficient in coefficients {
        hash.update(coefficient.raw().to_le_bytes());
    }
    encode_sha256(hash)
}

fn coefficient_identity_equiripple(
    spec: &EquirippleSpec,
    delay: u64,
    coefficients: &[Q2_62],
) -> String {
    let mut hash = Sha256::new();
    if spec.ratio.up() == spec.ratio.down() {
        hash.update(b"sex/sexfir/parks-mcclellan-lowpass-unity-v3\0");
    } else {
        hash.update(b"sex/sexfir/parks-mcclellan-lowpass-v2\0");
    }
    hash.update(spec.ratio.up().to_le_bytes());
    hash.update(spec.ratio.down().to_le_bytes());
    hash.update((spec.taps_per_phase as u64).to_le_bytes());
    hash.update(spec.rolloff.numerator.to_le_bytes());
    hash.update(spec.rolloff.denominator.to_le_bytes());
    hash.update(spec.passband_weight.numerator.to_le_bytes());
    hash.update(spec.passband_weight.denominator.to_le_bytes());
    hash.update(spec.stopband_weight.numerator.to_le_bytes());
    hash.update(spec.stopband_weight.denominator.to_le_bytes());
    hash.update(spec.grid_density.to_le_bytes());
    hash.update(spec.max_iterations.to_le_bytes());
    hash.update(spec.working_precision_bits.to_le_bytes());
    hash.update(delay.to_le_bytes());
    for coefficient in coefficients {
        hash.update(coefficient.raw().to_le_bytes());
    }
    encode_sha256(hash)
}

fn coefficient_identity_big<'a>(
    spec: &BigKaiserSpec,
    delay: u64,
    coefficients: impl IntoIterator<Item = &'a BigQ>,
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"sex/sexfir/kaiser-windowed-sinc-big-v1\0");
    hash.update(spec.core.ratio.up().to_le_bytes());
    hash.update(spec.core.ratio.down().to_le_bytes());
    hash.update((spec.core.taps_per_phase as u64).to_le_bytes());
    hash.update(spec.core.rolloff.numerator.to_le_bytes());
    hash.update(spec.core.rolloff.denominator.to_le_bytes());
    hash.update(spec.core.beta.numerator.to_le_bytes());
    hash.update(spec.core.beta.denominator.to_le_bytes());
    hash.update(spec.core.working_precision_bits.to_le_bytes());
    hash.update(spec.coefficient_fractional_bits.to_le_bytes());
    hash.update(spec.accumulator_bits.to_le_bytes());
    hash.update(delay.to_le_bytes());
    for coefficient in coefficients {
        let encoded = coefficient.raw().to_string_radix(16);
        hash.update((encoded.len() as u64).to_le_bytes());
        hash.update(encoded.as_bytes());
    }
    encode_sha256(hash)
}

fn encode_sha256(hash: Sha256) -> String {
    let digest = hash.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use sexq::{OverflowPolicy, Q1_63};
    use sexrate::{
        FrameCountPolicy, InterleavedResamplerQ63, InterleavedStreamStats, StreamError,
        output_frames_for_input,
    };
    use std::sync::Arc;

    fn small_spec() -> KaiserSpec {
        KaiserSpec {
            ratio: RateRatio::from_fraction(3, 2).unwrap(),
            taps_per_phase: 7,
            rolloff: Fraction::new(9, 10).unwrap(),
            beta: Fraction::new(5, 1).unwrap(),
            working_precision_bits: 128,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        }
    }

    fn small_big_spec() -> BigKaiserSpec {
        BigKaiserSpec {
            core: small_spec(),
            coefficient_fractional_bits: 96,
            accumulator_bits: 192,
        }
    }

    fn small_windowed_spec(window: WindowFunction) -> WindowedSincSpec {
        WindowedSincSpec {
            ratio: RateRatio::from_fraction(3, 2).unwrap(),
            taps_per_phase: 7,
            rolloff: Fraction::new(9, 10).unwrap(),
            window,
            working_precision_bits: 128,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        }
    }

    fn small_least_squares_spec() -> LeastSquaresSpec {
        LeastSquaresSpec {
            ratio: RateRatio::from_fraction(3, 2).unwrap(),
            taps_per_phase: 7,
            rolloff: Fraction::new(9, 10).unwrap(),
            passband_weight: Fraction::new(1, 1).unwrap(),
            stopband_weight: Fraction::new(10, 1).unwrap(),
            grid_points_per_band: 32,
            working_precision_bits: 128,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        }
    }

    fn small_equiripple_spec() -> EquirippleSpec {
        EquirippleSpec {
            ratio: RateRatio::from_fraction(2, 3).unwrap(),
            taps_per_phase: 7,
            rolloff: Fraction::new(9, 10).unwrap(),
            passband_weight: Fraction::new(1, 1).unwrap(),
            stopband_weight: Fraction::new(10, 1).unwrap(),
            grid_density: 16,
            max_iterations: 64,
            working_precision_bits: 128,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        }
    }

    fn run_mono(
        designed: &DesignedFilter,
        input: &[Q1_63],
        overflow: OverflowPolicy,
    ) -> Result<(Vec<Q1_63>, InterleavedStreamStats), StreamError> {
        let target = output_frames_for_input(
            input.len() as u64,
            designed.ratio(),
            FrameCountPolicy::NearestTiesToEven,
        )?;
        let mut stream = InterleavedResamplerQ63::new_with_input_delay(
            1,
            designed.ratio(),
            Arc::new(designed.bank().clone()),
            designed.input_delay_frames(),
            RoundingMode::NearestTiesToEven,
            overflow,
        )?;
        let mut output = Vec::new();
        stream.push_interleaved_finite_into(
            input,
            FrameCountPolicy::NearestTiesToEven,
            &mut output,
        )?;
        let stats = stream.finish_exact_frames(target, &mut output)?;
        Ok((output, stats))
    }

    fn sine_tone_q63(
        frequency_hz: u64,
        sample_rate: u64,
        frames: usize,
        amplitude_raw: i64,
    ) -> Vec<Q1_63> {
        const PRECISION: u32 = 160;
        (0..frames)
            .map(|index| {
                let mut angle = Float::with_val(
                    PRECISION,
                    2_u64 * frequency_hz * u64::try_from(index).unwrap(),
                );
                angle /= sample_rate;
                angle.sin_pi_mut();
                angle *= amplitude_raw;
                let raw = angle
                    .to_integer_round(Round::Nearest)
                    .unwrap()
                    .0
                    .to_i64()
                    .unwrap();
                Q1_63::from_raw(raw)
            })
            .collect()
    }

    fn swept_sine_q63(
        start_hz: u64,
        end_hz: u64,
        sample_rate: u64,
        frames: usize,
        amplitude_raw: i64,
    ) -> Vec<Q1_63> {
        const PRECISION: u32 = 160;
        assert!(end_hz >= start_hz);
        assert!(frames >= 2);
        (0..frames)
            .map(|index| {
                let index = u64::try_from(index).unwrap();
                let mut angle = Float::with_val(PRECISION, 2_u64 * start_hz * index);
                angle /= sample_rate;
                let mut sweep = Float::with_val(PRECISION, (end_hz - start_hz) * index * index);
                sweep /= sample_rate * u64::try_from(frames - 1).unwrap();
                angle += sweep;
                angle.sin_pi_mut();
                angle *= amplitude_raw;
                let raw = angle
                    .to_integer_round(Round::Nearest)
                    .unwrap()
                    .0
                    .to_i64()
                    .unwrap();
                Q1_63::from_raw(raw)
            })
            .collect()
    }

    fn run_interleaved_chunks(
        designed: &DesignedFilter,
        channels: u16,
        input: &[Q1_63],
        chunk_frames: &[usize],
        overflow: OverflowPolicy,
    ) -> Result<(Vec<Q1_63>, InterleavedStreamStats), StreamError> {
        let channels_usize = usize::from(channels);
        assert!(input.len().is_multiple_of(channels_usize));
        assert!(!chunk_frames.is_empty());
        assert!(chunk_frames.iter().all(|frames| *frames != 0));
        let input_frames = input.len() / channels_usize;
        let target = output_frames_for_input(
            input_frames as u64,
            designed.ratio(),
            FrameCountPolicy::NearestTiesToEven,
        )?;
        let mut stream = InterleavedResamplerQ63::new_with_input_delay(
            channels,
            designed.ratio(),
            Arc::new(designed.bank().clone()),
            designed.input_delay_frames(),
            RoundingMode::NearestTiesToEven,
            overflow,
        )?;
        let mut output = Vec::new();
        let mut frame = 0;
        let mut chunk = 0;
        while frame < input_frames {
            let frames = chunk_frames[chunk % chunk_frames.len()].min(input_frames - frame);
            let start = frame * channels_usize;
            let end = (frame + frames) * channels_usize;
            stream.push_interleaved_finite_into(
                &input[start..end],
                FrameCountPolicy::NearestTiesToEven,
                &mut output,
            )?;
            frame += frames;
            chunk += 1;
        }
        let stats = stream.finish_exact_frames(target, &mut output)?;
        Ok((output, stats))
    }

    #[test]
    fn configuration_is_exact_and_validated() {
        assert_eq!(Fraction::new(6, 8).unwrap(), Fraction::new(3, 4).unwrap());
        let mut spec = small_spec();
        spec.taps_per_phase = 6;
        assert_eq!(design_kaiser(&spec), Err(DesignError::EvenTapCount(6)));
        spec.taps_per_phase = 7;
        spec.working_precision_bits = 64;
        assert_eq!(
            design_kaiser(&spec),
            Err(DesignError::WorkingPrecisionTooLow(64))
        );
    }

    #[test]
    fn planner_materialization_is_exact_and_rejects_wider_backends() {
        let ratio = RateRatio::from_rates(44_100, 48_000).unwrap();
        let sane_plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio,
            preset: sexplan::QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        assert_eq!(
            KaiserSpec::from_precision_plan(&sane_plan).unwrap(),
            KaiserSpec::native_candidate(ratio)
        );

        let high_plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio,
            preset: sexplan::QualityPreset::High,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        assert_eq!(
            KaiserSpec::from_precision_plan(&high_plan),
            Err(DesignError::PlannedCoefficientPrecisionUnavailable {
                required: 96,
                available: 62,
            })
        );
        let big = BigKaiserSpec::from_precision_plan(&high_plan).unwrap();
        assert_eq!(big.coefficient_fractional_bits, 96);
        assert_eq!(big.accumulator_bits, 192);
        assert_eq!(big.core.taps_per_phase, 555);
    }

    #[test]
    fn bigint_design_has_exact_dc_and_reproducible_identity() {
        let first = design_kaiser_big(&small_big_spec()).unwrap();
        let second = design_kaiser_big(&small_big_spec()).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.bank().coefficient_format(),
            BigQFormat::new(2, 96).unwrap()
        );
        assert_eq!(first.report().coefficient_fractional_bits, 96);
        assert_eq!(first.report().configured_accumulator_bits, 192);
        assert_eq!(first.report().coefficient_sha256.len(), 64);
        let unity = Integer::from(1) << 96;
        for phase in 0..first.bank().phase_count() {
            let mut sum = Integer::new();
            for coefficient in first.bank().phase(phase as u64).unwrap() {
                sum += coefficient.raw();
            }
            assert_eq!(sum, unity);
        }
    }

    #[test]
    fn windowed_sinc_families_are_distinct_normalized_and_measurable() {
        let mut identities = Vec::new();
        for window in [
            WindowFunction::Rectangular,
            WindowFunction::Hann,
            WindowFunction::Blackman,
        ] {
            let designed = design_windowed_sinc(&small_windowed_spec(window)).unwrap();
            assert_eq!(designed.report().algorithm, window.algorithm());
            assert_eq!(designed.bank().phase_count(), 3);
            for phase in 0..3 {
                let sum = designed
                    .bank()
                    .phase(phase)
                    .unwrap()
                    .iter()
                    .map(|coefficient| i128::from(coefficient.raw()))
                    .sum::<i128>();
                assert_eq!(sum, i128::from(Q2_62::ONE.raw()));
            }
            let response = analyze_windowed_quantized_response(&designed, 9).unwrap();
            assert_eq!(response.analyzed_phases, 3);
            identities.push(designed.report().coefficient_sha256.clone());
        }
        identities.sort();
        identities.dedup();
        assert_eq!(identities.len(), 3);
    }

    #[test]
    fn least_squares_design_is_reproducible_normalized_and_measurable() {
        let spec = small_least_squares_spec();
        let first = design_least_squares(&spec).unwrap();
        let second = design_least_squares(&spec).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.report().algorithm, "least-squares-lowpass-v1");
        assert_eq!(first.input_delay_frames(), 3);
        assert_eq!(first.bank().phase_count(), 3);
        for phase in 0..first.bank().phase_count() {
            let sum = first
                .bank()
                .phase(phase as u64)
                .unwrap()
                .iter()
                .map(|coefficient| i128::from(coefficient.raw()))
                .sum::<i128>();
            assert_eq!(sum, i128::from(Q2_62::ONE.raw()));
        }
        let response = analyze_least_squares_quantized_response(&first, 17).unwrap();
        assert_eq!(response.analyzed_phases, 3);
        assert_eq!(response.grid_points_per_band, 17);
    }

    #[test]
    fn least_squares_weights_change_the_quantized_solution() {
        let baseline = design_least_squares(&small_least_squares_spec()).unwrap();
        let weighted = design_least_squares(&LeastSquaresSpec {
            stopband_weight: Fraction::new(1_000, 1).unwrap(),
            ..small_least_squares_spec()
        })
        .unwrap();
        assert_ne!(baseline.bank(), weighted.bank());
        assert_ne!(
            baseline.report().coefficient_sha256,
            weighted.report().coefficient_sha256
        );
    }

    #[test]
    fn least_squares_execution_is_chunk_invariant() {
        let designed = design_least_squares(&small_least_squares_spec()).unwrap();
        let input = [
            Q1_63::from_raw(1_i64 << 60),
            Q1_63::from_raw(-(1_i64 << 59)),
            Q1_63::from_raw(1_i64 << 58),
            Q1_63::ZERO,
        ];
        let target = output_frames_for_input(
            input.len() as u64,
            designed.ratio(),
            FrameCountPolicy::NearestTiesToEven,
        )
        .unwrap();
        let run = |chunks: &[&[Q1_63]]| {
            let mut stream = InterleavedResamplerQ63::new_with_input_delay(
                1,
                designed.ratio(),
                Arc::new(designed.bank().clone()),
                designed.input_delay_frames(),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                stream
                    .push_interleaved_finite_into(
                        chunk,
                        FrameCountPolicy::NearestTiesToEven,
                        &mut output,
                    )
                    .unwrap();
            }
            let stats = stream.finish_exact_frames(target, &mut output).unwrap();
            (output, stats)
        };
        assert_eq!(
            run(&[&input]),
            run(&[&input[..1], &input[1..3], &input[3..]])
        );
    }

    #[test]
    fn least_squares_validation_and_work_budget_fail_early() {
        assert_eq!(
            design_least_squares(&LeastSquaresSpec {
                rolloff: Fraction::new(1, 2).unwrap(),
                ..small_least_squares_spec()
            }),
            Err(DesignError::InvalidLeastSquaresRolloff(
                Fraction::new(1, 2).unwrap()
            ))
        );
        assert_eq!(
            design_least_squares(&LeastSquaresSpec {
                passband_weight: Fraction::new(0, 1).unwrap(),
                ..small_least_squares_spec()
            }),
            Err(DesignError::ZeroLeastSquaresWeight("passband"))
        );
        assert!(matches!(
            design_least_squares(&LeastSquaresSpec {
                taps_per_phase: 1_025,
                ..small_least_squares_spec()
            }),
            Err(DesignError::LeastSquaresWorkBudgetExceeded { .. })
        ));
    }

    #[test]
    fn equiripple_design_converges_reproducibly_and_has_exact_phase_dc() {
        let spec = small_equiripple_spec();
        let first = design_equiripple(&spec).unwrap();
        let second = design_equiripple(&spec).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.report().quantization.algorithm,
            "parks-mcclellan-lowpass-v2"
        );
        assert!((1..=spec.max_iterations).contains(&first.report().exchange_iterations));
        assert_eq!(first.report().dense_grid_points, 256);
        assert_eq!(first.report().extremal_frequencies_decimal.len(), 8);
        assert_eq!(first.input_delay_frames(), 3);
        assert_eq!(first.bank().phase(1).unwrap()[6], Q2_62::ZERO);
        for phase in 0..first.bank().phase_count() {
            let sum = first
                .bank()
                .phase(phase as u64)
                .unwrap()
                .iter()
                .map(|coefficient| i128::from(coefficient.raw()))
                .sum::<i128>();
            assert_eq!(sum, i128::from(Q2_62::ONE.raw()));
        }
        let response = analyze_equiripple_quantized_response(&first, 17).unwrap();
        assert_eq!(response.analyzed_phases, 2);
    }

    #[test]
    fn remez_solution_satisfies_dense_grid_alternation() {
        let spec = small_equiripple_spec();
        let global_length = (spec.taps_per_phase - 1) * spec.ratio.up() as usize + 1;
        let amplitude_count = global_length.div_ceil(2);
        let points_per_band = spec.grid_density as usize * (amplitude_count + 1);
        let grid = build_remez_grid(&spec, points_per_band, spec.working_precision_bits);
        let exchange = remez_exchange(
            &spec,
            amplitude_count,
            &grid,
            spec.working_precision_bits,
            false,
        )
        .unwrap();
        let cosines = RemezCosines::new(&grid, amplitude_count, spec.working_precision_bits, false);
        let errors =
            evaluate_remez_errors(&cosines, &exchange.amplitudes, spec.working_precision_bits);
        assert_eq!(exchange.extrema.len(), amplitude_count + 1);
        for pair in exchange.extrema.windows(2) {
            assert_ne!(errors[pair[0]] > 0, errors[pair[1]] > 0);
        }
        let mut extremal_ripple = errors[exchange.extrema[0]].clone();
        extremal_ripple.abs_mut();
        let tolerance =
            Float::with_val(spec.working_precision_bits, Float::parse("1e-30").unwrap());
        for index in &exchange.extrema {
            let mut magnitude = errors[*index].clone();
            magnitude.abs_mut();
            magnitude -= &extremal_ripple;
            magnitude.abs_mut();
            assert!(magnitude < tolerance);
        }
        let mut permitted_maximum = extremal_ripple;
        permitted_maximum *= Float::with_val(
            spec.working_precision_bits,
            Float::parse("1.0000000001").unwrap(),
        );
        assert!(exchange.max_weighted_error <= permitted_maximum);
    }

    #[test]
    fn equiripple_execution_is_chunk_invariant() {
        let designed = design_equiripple(&small_equiripple_spec()).unwrap();
        let input = [
            Q1_63::from_raw(1_i64 << 60),
            Q1_63::from_raw(-(1_i64 << 59)),
            Q1_63::from_raw(1_i64 << 58),
            Q1_63::ZERO,
            Q1_63::from_raw(-(1_i64 << 57)),
        ];
        let target = output_frames_for_input(
            input.len() as u64,
            designed.ratio(),
            FrameCountPolicy::NearestTiesToEven,
        )
        .unwrap();
        let run = |chunks: &[&[Q1_63]]| {
            let mut stream = InterleavedResamplerQ63::new_with_input_delay(
                1,
                designed.ratio(),
                Arc::new(designed.bank().clone()),
                designed.input_delay_frames(),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                stream
                    .push_interleaved_finite_into(
                        chunk,
                        FrameCountPolicy::NearestTiesToEven,
                        &mut output,
                    )
                    .unwrap();
            }
            let stats = stream.finish_exact_frames(target, &mut output).unwrap();
            (output, stats)
        };
        assert_eq!(
            run(&[&input]),
            run(&[&input[..1], &input[1..3], &input[3..]])
        );
    }

    #[test]
    fn equiripple_validation_and_work_budget_are_explicit() {
        assert_eq!(
            design_equiripple(&EquirippleSpec {
                grid_density: 3,
                ..small_equiripple_spec()
            }),
            Err(DesignError::RemezGridDensityTooSmall(3))
        );
        assert_eq!(
            design_equiripple(&EquirippleSpec {
                max_iterations: 0,
                ..small_equiripple_spec()
            }),
            Err(DesignError::ZeroRemezIterations)
        );
        assert!(matches!(
            design_equiripple(&EquirippleSpec {
                taps_per_phase: 1_025,
                ..small_equiripple_spec()
            }),
            Err(DesignError::RemezWorkBudgetExceeded { .. })
        ));
    }

    #[test]
    fn hann_and_blackman_windows_have_exact_center_and_zero_edges() {
        let precision = 128;
        let center = Float::with_val(precision, 0);
        let edge = Float::with_val(precision, 3);
        for window in [WindowFunction::Hann, WindowFunction::Blackman] {
            assert_eq!(
                simple_window(window, &center, 3, precision, None).unwrap(),
                Float::with_val(precision, 1)
            );
            assert!(
                simple_window(window, &edge, 3, precision, None)
                    .unwrap()
                    .is_zero()
            );
        }
        assert_eq!(
            simple_window(WindowFunction::Rectangular, &edge, 3, precision, None).unwrap(),
            Float::with_val(precision, 1)
        );
    }

    #[test]
    fn dolph_chebyshev_matches_the_reference_odd_window() {
        let precision = 192;
        let window = DolphWindow::new(7, 100, precision).unwrap();
        let expected = [
            "0.05650405062850233",
            "0.316608530648474",
            "0.7601208123539079",
            "1.0",
            "0.7601208123539079",
            "0.316608530648474",
            "0.05650405062850233",
        ];
        let tolerance = Float::with_val(precision, Float::parse("1e-14").unwrap());
        for (index, expected) in expected.into_iter().enumerate() {
            let distance = i32::try_from(index).unwrap() - 3;
            let actual = window.at_distance(&Float::with_val(precision, distance));
            let expected = Float::with_val(precision, Float::parse(expected).unwrap());
            let mut error = actual;
            error -= expected;
            error.abs_mut();
            assert!(error < tolerance);
        }

        let spec = small_windowed_spec(WindowFunction::DolphChebyshev {
            attenuation_db: 100,
        });
        let designed = design_windowed_sinc(&spec).unwrap();
        assert_eq!(
            designed.report().algorithm,
            "windowed-sinc-dolph-chebyshev-v2"
        );
        assert_eq!(
            design_windowed_sinc(&WindowedSincSpec {
                window: WindowFunction::DolphChebyshev { attenuation_db: 0 },
                ..spec
            }),
            Err(DesignError::InvalidDolphAttenuation(0))
        );
    }

    #[test]
    fn direct_dolph_series_has_an_explicit_work_budget() {
        let spec = WindowedSincSpec {
            ratio: RateRatio::from_fraction(160, 147).unwrap(),
            taps_per_phase: 1_025,
            rolloff: Fraction::new(99, 100).unwrap(),
            window: WindowFunction::DolphChebyshev {
                attenuation_db: 160,
            },
            working_precision_bits: 256,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        };
        assert!(matches!(
            design_windowed_sinc(&spec),
            Err(DesignError::DolphSeriesBudgetExceeded { .. })
        ));
    }

    #[test]
    fn bigint_designed_stream_is_exact_across_input_chunks() {
        let designed = design_kaiser_big(&small_big_spec()).unwrap();
        let input = [
            Q1_63::from_raw(1_i64 << 60),
            Q1_63::from_raw(-(1_i64 << 59)),
            Q1_63::from_raw(1_i64 << 58),
            Q1_63::ZERO,
        ];
        let target = output_frames_for_input(
            input.len() as u64,
            designed.ratio(),
            FrameCountPolicy::NearestTiesToEven,
        )
        .unwrap();
        let run = |chunks: &[&[Q1_63]]| {
            let mut stream = InterleavedResamplerQ63::new_big_with_input_delay(
                1,
                designed.ratio(),
                Arc::new(designed.bank().clone()),
                designed.input_delay_frames(),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                stream
                    .push_interleaved_finite_into(
                        chunk,
                        FrameCountPolicy::NearestTiesToEven,
                        &mut output,
                    )
                    .unwrap();
            }
            let stats = stream.finish_exact_frames(target, &mut output).unwrap();
            (output, stats)
        };
        assert_eq!(
            run(&[&input]),
            run(&[&input[..1], &input[1..3], &input[3..]])
        );
    }

    #[test]
    fn every_quantized_phase_has_exact_unity_dc_gain() {
        let designed = design_kaiser(&small_spec()).unwrap();
        assert_eq!(designed.input_delay_frames(), 3);
        assert_eq!(designed.bank().phase_count(), 3);
        assert_eq!(designed.bank().taps_per_phase(), 7);
        for phase in 0..3 {
            let sum = designed
                .bank()
                .phase(phase)
                .unwrap()
                .iter()
                .map(|coefficient| i128::from(coefficient.raw()))
                .sum::<i128>();
            assert_eq!(sum, i128::from(Q2_62::ONE.raw()));
        }
    }

    #[test]
    fn global_prototype_support_zeroes_the_last_tap_of_nonzero_phases() {
        let designed = design_kaiser(&small_spec()).unwrap();
        assert_ne!(designed.bank().phase(0).unwrap()[6], Q2_62::ZERO);
        assert_eq!(designed.bank().phase(1).unwrap()[6], Q2_62::ZERO);
        assert_eq!(designed.bank().phase(2).unwrap()[6], Q2_62::ZERO);
    }

    #[test]
    fn sha256_standard_vectors_are_cpu_independent() {
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        for (input, expected) in [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ] {
            assert_eq!(hex(&Sha256::digest(input.as_bytes())), expected);
            let mut incremental = Sha256::new();
            for byte in input.as_bytes() {
                incremental.update([*byte]);
            }
            assert_eq!(hex(&incremental.finalize()), expected);
        }
    }

    #[test]
    fn design_is_bit_reproducible_with_a_stable_identity() {
        let first = design_kaiser(&small_spec()).unwrap();
        let second = design_kaiser(&small_spec()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.report().coefficient_sha256.len(), 64);
        assert_eq!(first.report().coefficient_count, 21);
        assert!(first.report().max_dc_correction_raw <= 4);
    }

    fn legacy_native_candidate(ratio: RateRatio) -> KaiserSpec {
        KaiserSpec {
            ratio,
            taps_per_phase: 2
                * (128_u128 * u128::from(ratio.down()))
                    .div_ceil(u128::from(ratio.up()))
                    .max(128) as usize
                + 1,
            rolloff: Fraction::new(19, 20).unwrap(),
            beta: Fraction::new(10, 1).unwrap(),
            working_precision_bits: 192,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        }
    }

    #[test]
    fn legacy_common_ratio_candidate_keeps_its_regression_identity() {
        let ratio = RateRatio::from_rates(44_100, 48_000).unwrap();
        let designed = design_kaiser(&legacy_native_candidate(ratio)).unwrap();
        assert_eq!(designed.report().required_accumulator_bits, 128);
        assert_eq!(
            designed.report().coefficient_sha256,
            "d2c520186b24e159d9f336c31e23a582b4571924fadc35c77828e5695ea70cf0"
        );
        assert_eq!(
            designed.report().max_phase_l1_error_decimal,
            "1.6739264407570411384e-17"
        );
    }

    #[test]
    fn designed_stream_is_bit_exact_across_input_chunks() {
        let spec = small_spec();
        let designed = design_kaiser(&spec).unwrap();
        let delay = designed.input_delay_frames();
        let bank = Arc::new(designed.into_bank());
        let input = (0..41)
            .map(|index| Q1_63::from_raw(i64::from(index) * 1_000_003 - 20_000_060))
            .collect::<Vec<_>>();
        let target = output_frames_for_input(
            input.len() as u64,
            spec.ratio,
            FrameCountPolicy::NearestTiesToEven,
        )
        .unwrap();

        let run = |chunks: &[&[Q1_63]]| {
            let mut stream = InterleavedResamplerQ63::new_with_input_delay(
                1,
                spec.ratio,
                Arc::clone(&bank),
                delay,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                stream
                    .push_interleaved_finite_into(
                        chunk,
                        FrameCountPolicy::NearestTiesToEven,
                        &mut output,
                    )
                    .unwrap();
            }
            let stats = stream.finish_exact_frames(target, &mut output).unwrap();
            (output, stats)
        };

        assert_eq!(
            run(&[&input]),
            run(&[&input[..1], &input[1..9], &[], &input[9..27], &input[27..]])
        );
    }

    #[test]
    fn quantized_response_analysis_is_reproducible() {
        let designed = design_kaiser(&small_spec()).unwrap();
        let first = analyze_quantized_response(&designed, 9).unwrap();
        let second = analyze_quantized_response(&designed, 9).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.analyzed_phases, 3);
        assert_eq!(first.grid_points_per_band, 9);
        assert!(!first.passband_ripple_db.is_empty());
        assert!(!first.stopband_peak_db.is_empty());
        assert_eq!(
            analyze_quantized_response(&designed, 1),
            Err(DesignError::AnalysisGridTooSmall(1))
        );

        let (_, loose) =
            analyze_quantized_response_against(&designed, 9, ErrorFloor::new(1).unwrap()).unwrap();
        assert!(loose.meets_target());
        let (_, strict) =
            analyze_quantized_response_against(&designed, 9, ErrorFloor::new(300).unwrap())
                .unwrap();
        assert!(!strict.meets_target());
    }

    #[test]
    fn bigint_response_parallelism_is_bit_identical_and_keeps_all_phases() {
        let designed = design_kaiser_big(&small_big_spec()).unwrap();
        let target = ErrorFloor::new(80).unwrap();
        let expected =
            analyze_big_quantized_response_internal_with_workers(&designed, 17, Some(target), 1)
                .unwrap();
        for workers in [2, 3, 32] {
            assert_eq!(
                analyze_big_quantized_response_internal_with_workers(
                    &designed,
                    17,
                    Some(target),
                    workers,
                )
                .unwrap(),
                expected,
                "workers={workers}"
            );
        }
        assert_eq!(expected.0.analyzed_phases, 3);
        assert_eq!(expected.0.grid_points_per_band, 17);
        assert_eq!(
            analyze_big_quantized_response_internal_with_workers(&designed, 1, Some(target), 3,),
            Err(DesignError::AnalysisGridTooSmall(1))
        );
    }

    #[test]
    fn explicit_300db_plan_is_verified_after_quantization() {
        let ratio = RateRatio::from_rates(44_100, 48_000).unwrap();
        let target = ErrorFloor::new(300).unwrap();
        let plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio,
            preset: sexplan::QualityPreset::Sane,
            error_floor: Some(target),
            working_precision_bits: Some(256),
        })
        .unwrap();
        let spec = KaiserSpec::from_precision_plan(&plan).unwrap();
        let designed = design_kaiser(&spec).unwrap();
        let (response, compliance) =
            analyze_quantized_response_against(&designed, 65, target).unwrap();

        assert!(compliance.meets_target());
        assert_eq!(response.stopband_peak_db, "-307.4143649543181");
        assert_eq!(
            designed.report().max_phase_l1_error_db,
            "-326.6973457092834"
        );
    }

    #[test]
    fn sane_default_target_is_qualified_beyond_the_old_sparse_grid() {
        let target = ErrorFloor::new(110).unwrap();
        let old = design_kaiser(&legacy_native_candidate(
            RateRatio::from_rates(48_000, 16_000).unwrap(),
        ))
        .unwrap();
        let (_, old_compliance) = analyze_quantized_response_against(&old, 513, target).unwrap();
        assert!(!old_compliance.passband_deviation_meets_target);
        assert!(!old_compliance.stopband_meets_target);

        for (from, to) in [(48_000, 16_000), (16_000, 24_000), (48_000, 1_000)] {
            let plan = sexplan::plan_precision(sexplan::PlanRequest {
                ratio: RateRatio::from_rates(from, to).unwrap(),
                preset: sexplan::QualityPreset::Sane,
                error_floor: None,
                working_precision_bits: None,
            })
            .unwrap();
            let corrected =
                design_kaiser(&KaiserSpec::from_precision_plan(&plan).unwrap()).unwrap();
            let (response, compliance) =
                analyze_quantized_response_against(&corrected, 1025, target).unwrap();
            assert!(
                compliance.meets_target(),
                "{from}->{to}: {response:?}, {compliance:?}"
            );
        }
    }

    #[test]
    fn legacy_candidate_44100_to_48000_response_is_locked() {
        let ratio = RateRatio::from_rates(44_100, 48_000).unwrap();
        let designed = design_kaiser(&legacy_native_candidate(ratio)).unwrap();
        let response = analyze_quantized_response(&designed, 129).unwrap();

        assert_eq!(designed.bank().taps_per_phase(), 257);
        assert_eq!(response.analyzed_phases, 160);
        assert_eq!(response.passband_end_decimal, "4.500000000000000e-1");
        assert_eq!(response.passband_ripple_db, "6.265820113691220e-5");
        assert_eq!(
            response.max_passband_deviation_decimal,
            "3.637947248714758e-6"
        );
        assert_eq!(response.stopband_start_decimal, "5.000000000000000e-1");
        assert_eq!(response.stopband_peak_db, "-117.0862953781437");
    }

    #[test]
    fn legacy_candidate_three_to_one_downsample_response_is_locked() {
        let ratio = RateRatio::from_rates(48_000, 16_000).unwrap();
        let designed = design_kaiser(&legacy_native_candidate(ratio)).unwrap();
        let response = analyze_quantized_response(&designed, 129).unwrap();

        assert_eq!(designed.bank().taps_per_phase(), 769);
        assert_eq!(response.analyzed_phases, 1);
        assert_eq!(response.passband_end_decimal, "1.500000000000000e-1");
        assert_eq!(response.passband_ripple_db, "3.411283202005583e-5");
        assert_eq!(
            response.max_passband_deviation_decimal,
            "2.353316615337044e-6"
        );
        assert_eq!(response.stopband_start_decimal, "1.666666666666667e-1");
        assert_eq!(response.stopband_peak_db, "-110.5568805632797");
    }

    #[test]
    fn designed_pipeline_survives_exact_torture_vectors() {
        let designed = design_kaiser(&small_spec()).unwrap();

        let silence = vec![Q1_63::ZERO; 64];
        let (silence_output, silence_stats) =
            run_mono(&designed, &silence, OverflowPolicy::Error).unwrap();
        assert_eq!(silence_output, vec![Q1_63::ZERO; 96]);
        assert_eq!(silence_stats.saturated_samples, 0);

        let quarter_scale = Q1_63::from_raw(1_i64 << 61);
        let dc = vec![quarter_scale; 64];
        let (dc_output, dc_stats) = run_mono(&designed, &dc, OverflowPolicy::Error).unwrap();
        assert!(
            dc_output[5..92]
                .iter()
                .all(|&sample| sample == quarter_scale)
        );
        assert_eq!(dc_stats.saturated_samples, 0);

        let mut impulse = vec![Q1_63::ZERO; 64];
        impulse[20] = quarter_scale;
        let negative_impulse = impulse
            .iter()
            .map(|sample| Q1_63::from_raw(-sample.raw()))
            .collect::<Vec<_>>();
        let (positive_response, positive_stats) =
            run_mono(&designed, &impulse, OverflowPolicy::Error).unwrap();
        let (negative_response, negative_stats) =
            run_mono(&designed, &negative_impulse, OverflowPolicy::Error).unwrap();
        assert!(
            positive_response
                .iter()
                .any(|sample| *sample != Q1_63::ZERO)
        );
        assert!(
            positive_response
                .iter()
                .zip(&negative_response)
                .all(|(positive, negative)| positive.raw() == -negative.raw())
        );
        assert_eq!(positive_stats.saturated_samples, 0);
        assert_eq!(negative_stats.saturated_samples, 0);

        let alternating = (0..128)
            .map(|index| {
                if index % 2 == 0 {
                    Q1_63::MIN
                } else {
                    Q1_63::MAX
                }
            })
            .collect::<Vec<_>>();
        let (alternating_output, alternating_stats) =
            run_mono(&designed, &alternating, OverflowPolicy::Saturate).unwrap();
        assert_eq!(alternating_output.len(), 192);
        assert_eq!(alternating_stats.saturated_samples, 0);
    }

    #[test]
    fn spectral_torture_matrix_is_multichannel_and_chunk_invariant() {
        const SAMPLE_RATE: u64 = 48_000;
        const FRAMES: usize = 4_096;
        let designed = design_kaiser(&small_spec()).unwrap();
        let silence = vec![Q1_63::ZERO; FRAMES];
        let full_scale = sine_tone_q63(3_000, SAMPLE_RATE, FRAMES, i64::MAX);
        let one_hz = sine_tone_q63(1, SAMPLE_RATE, FRAMES, i64::MAX / 2);
        let near_nyquist = sine_tone_q63(23_999, SAMPLE_RATE, FRAMES, i64::MAX / 2);
        let tone_a = sine_tone_q63(37, SAMPLE_RATE, FRAMES, i64::MAX / 8);
        let tone_b = sine_tone_q63(997, SAMPLE_RATE, FRAMES, i64::MAX / 8);
        let tone_c = sine_tone_q63(11_731, SAMPLE_RATE, FRAMES, i64::MAX / 8);
        let multitone = tone_a
            .iter()
            .zip(&tone_b)
            .zip(&tone_c)
            .map(|((&a, &b), &c)| {
                Q1_63::from_raw(
                    i64::try_from(i128::from(a.raw()) + i128::from(b.raw()) + i128::from(c.raw()))
                        .unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let sweep = swept_sine_q63(1, 23_999, SAMPLE_RATE, FRAMES, i64::MAX / 2);
        let channel_data = [silence, full_scale, one_hz, near_nyquist, multitone, sweep];
        let channels = u16::try_from(channel_data.len()).unwrap();
        let mut interleaved = Vec::with_capacity(FRAMES * channel_data.len());
        for frame in 0..FRAMES {
            for channel in &channel_data {
                interleaved.push(channel[frame]);
            }
        }

        let contiguous = run_interleaved_chunks(
            &designed,
            channels,
            &interleaved,
            &[FRAMES],
            OverflowPolicy::Saturate,
        )
        .unwrap();
        let irregular = run_interleaved_chunks(
            &designed,
            channels,
            &interleaved,
            &[1, 17, 3, 257, 64, 5, 1_023],
            OverflowPolicy::Saturate,
        )
        .unwrap();
        assert_eq!(contiguous, irregular);
        assert_eq!(contiguous.1.input_frames, FRAMES as u64);
        assert_eq!(
            contiguous.1.output_frames,
            output_frames_for_input(
                FRAMES as u64,
                designed.ratio(),
                FrameCountPolicy::NearestTiesToEven,
            )
            .unwrap()
        );
        let output_channels = usize::from(channels);
        assert!(
            contiguous
                .0
                .iter()
                .skip(output_channels)
                .step_by(output_channels)
                .all(|sample| *sample == Q1_63::ZERO)
        );
        for channel in 1..output_channels {
            assert!(
                contiguous
                    .0
                    .iter()
                    .skip(channel)
                    .step_by(output_channels)
                    .any(|sample| *sample != Q1_63::ZERO),
                "channel {channel} unexpectedly became silence"
            );
        }
    }
}
