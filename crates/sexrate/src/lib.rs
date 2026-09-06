//! Exact-rational scheduling primitives for the SeX resampler.
//!
//! This crate deliberately contains no floating-point rate or phase state.
//! Given an output/input ratio `up/down`, output frame `t` is located at the
//! exact input coordinate `t * down / up`.

use core::fmt;
use sexq::{
    ArithmeticError, ArithmeticOutcome, BigMac, BigQ, BigQError, BigQFormat, ExactDotBackend,
    ExactDotError, MacQ125, OverflowPolicy, Q1_63, Q2_62, RoundingMode, WideQ63,
    automatic_exact_dot_backend, exact_dot_backend_available, exact_dot_q1_63_q2_62_with_backend,
};
use std::sync::Arc;

mod arbitrary;
mod frequency;
mod sample;
mod timeline;
pub use arbitrary::{BigFirSpec, CausalResamplerBig, InterleavedResamplerBig, PolyphaseFirBig};
pub use frequency::{ParseRateError, SampleRate};
pub use sample::ResamplerSample;
use timeline::Timeline;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateError {
    ZeroInputRate,
    ZeroOutputRate,
    ZeroRatioNumerator,
    ZeroRatioDenominator,
    InputPositionOverflow,
    OutputLengthOverflow,
}

impl fmt::Display for RateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroInputRate => f.write_str("input sample rate must be non-zero"),
            Self::ZeroOutputRate => f.write_str("output sample rate must be non-zero"),
            Self::ZeroRatioNumerator => f.write_str("ratio numerator must be non-zero"),
            Self::ZeroRatioDenominator => f.write_str("ratio denominator must be non-zero"),
            Self::InputPositionOverflow => f.write_str("exact input frame position overflowed u64"),
            Self::OutputLengthOverflow => f.write_str("exact output frame count overflowed u64"),
        }
    }
}

impl std::error::Error for RateError {}

/// Construction or evaluation failure for a quantized polyphase FIR bank.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolyphaseError {
    ZeroPhases,
    ZeroTaps,
    PhaseCountTooLarge(u64),
    CoefficientCount {
        expected: usize,
        actual: usize,
    },
    SampleCount {
        expected: usize,
        actual: usize,
    },
    PhaseOutOfRange {
        phase: u64,
        phase_count: usize,
    },
    AccumulatorTooNarrow {
        phase: usize,
        required_bits: u16,
    },
    BigAccumulatorTooNarrow {
        phase: usize,
        required_bits: u32,
        configured_bits: u32,
    },
    ExactDot(ExactDotError),
    Arithmetic(ArithmeticError),
    BigArithmetic(BigQError),
}

impl fmt::Display for PolyphaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ZeroPhases => f.write_str("a polyphase FIR needs at least one phase"),
            Self::ZeroTaps => f.write_str("a polyphase FIR needs at least one tap per phase"),
            Self::PhaseCountTooLarge(count) => {
                write!(f, "phase count {count} is not addressable on this platform")
            }
            Self::CoefficientCount { expected, actual } => {
                write!(f, "expected {expected} coefficients, got {actual}")
            }
            Self::SampleCount { expected, actual } => {
                write!(f, "expected {expected} input samples, got {actual}")
            }
            Self::PhaseOutOfRange { phase, phase_count } => {
                write!(f, "phase {phase} is outside 0..{phase_count}")
            }
            Self::AccumulatorTooNarrow {
                phase,
                required_bits,
            } => write!(
                f,
                "phase {phase} requires a {required_bits}-bit signed accumulator"
            ),
            Self::BigAccumulatorTooNarrow {
                phase,
                required_bits,
                configured_bits,
            } => write!(
                f,
                "phase {phase} requires a {required_bits}-bit signed accumulator; configured width is {configured_bits}"
            ),
            Self::ExactDot(ref error) => error.fmt(f),
            Self::Arithmetic(error) => error.fmt(f),
            Self::BigArithmetic(ref error) => error.fmt(f),
        }
    }
}

impl std::error::Error for PolyphaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ExactDot(error) => Some(error),
            Self::Arithmetic(error) => Some(error),
            Self::BigArithmetic(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ArithmeticError> for PolyphaseError {
    fn from(value: ArithmeticError) -> Self {
        Self::Arithmetic(value)
    }
}

impl From<ExactDotError> for PolyphaseError {
    fn from(value: ExactDotError) -> Self {
        Self::ExactDot(value)
    }
}

impl From<BigQError> for PolyphaseError {
    fn from(value: BigQError) -> Self {
        Self::BigArithmetic(value)
    }
}

/// Failure while advancing the causal streaming engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamError {
    ZeroChannels,
    PhaseCountMismatch { ratio_up: u64, bank_phases: usize },
    PartialInterleavedFrame { samples: usize, channels: u16 },
    ChannelOutputMismatch,
    InterleavedCountOverflow,
    AllocationFailed,
    InputFrameOverflow,
    OutputFrameOverflow,
    SaturationCountOverflow,
    InternalPositionLag { requested: u64, current: u64 },
    HistoryUnavailable { requested: u64, oldest: u64 },
    TargetBeforeProduced { target: u64, produced: u64 },
    FiniteModeStartedLate,
    FinitePolicyMismatch,
    FiniteAndUnboundedModeMismatch,
    FiniteTargetMismatch { expected: u64, actual: u64 },
    OutputDomainMismatch,
    Poisoned,
    Rate(RateError),
    Polyphase(PolyphaseError),
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ZeroChannels => f.write_str("resampler must have at least one channel"),
            Self::PhaseCountMismatch {
                ratio_up,
                bank_phases,
            } => write!(
                f,
                "rate ratio needs {ratio_up} phases, coefficient bank has {bank_phases}"
            ),
            Self::PartialInterleavedFrame { samples, channels } => write!(
                f,
                "chunk has {samples} samples, not a multiple of {channels} channels"
            ),
            Self::ChannelOutputMismatch => {
                f.write_str("channel resamplers produced different frame counts")
            }
            Self::InterleavedCountOverflow => {
                f.write_str("interleaved sample count overflowed usize")
            }
            Self::AllocationFailed => f.write_str("could not allocate resampler storage"),
            Self::InputFrameOverflow => f.write_str("input frame counter overflowed u64"),
            Self::OutputFrameOverflow => f.write_str("output frame counter overflowed u64"),
            Self::SaturationCountOverflow => f.write_str("saturation counter overflowed u64"),
            Self::InternalPositionLag { requested, current } => write!(
                f,
                "resampler requested past input frame {requested} after reaching {current}"
            ),
            Self::HistoryUnavailable { requested, oldest } => write!(
                f,
                "resampler needs input frame {requested}, but retained history starts at {oldest}"
            ),
            Self::TargetBeforeProduced { target, produced } => write!(
                f,
                "cannot finish at {target} output frames after already producing {produced}"
            ),
            Self::FiniteModeStartedLate => {
                f.write_str("finite-duration push mode must start with the first input frame")
            }
            Self::FinitePolicyMismatch => {
                f.write_str("finite-duration frame-count policy changed within one stream")
            }
            Self::FiniteAndUnboundedModeMismatch => {
                f.write_str("cannot mix finite-duration and unbounded push methods")
            }
            Self::FiniteTargetMismatch { expected, actual } => write!(
                f,
                "finite-duration stream expects {expected} output frames, got target {actual}"
            ),
            Self::OutputDomainMismatch => {
                f.write_str("cannot mix narrow and wide output methods on one resampler")
            }
            Self::Poisoned => f.write_str("resampler cannot continue after an earlier error"),
            Self::Rate(error) => error.fmt(f),
            Self::Polyphase(ref error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rate(error) => Some(error),
            Self::Polyphase(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RateError> for StreamError {
    fn from(value: RateError) -> Self {
        Self::Rate(value)
    }
}

impl From<PolyphaseError> for StreamError {
    fn from(value: PolyphaseError) -> Self {
        Self::Polyphase(value)
    }
}

/// A reduced positive output/input rate ratio.
///
/// `up` is both the interpolation factor and the number of exact fractional
/// phases in the direct rational polyphase representation. `down` is the
/// decimation factor.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RateRatio {
    up: u64,
    down: u64,
}

impl RateRatio {
    /// Construct `output_hz / input_hz` and reduce it by the exact GCD.
    pub fn from_rates(input_hz: u64, output_hz: u64) -> Result<Self, RateError> {
        if input_hz == 0 {
            return Err(RateError::ZeroInputRate);
        }
        if output_hz == 0 {
            return Err(RateError::ZeroOutputRate);
        }
        Self::from_fraction(output_hz, input_hz)
    }

    /// Construct and reduce an explicit output/input fraction.
    pub fn from_fraction(numerator: u64, denominator: u64) -> Result<Self, RateError> {
        if numerator == 0 {
            return Err(RateError::ZeroRatioNumerator);
        }
        if denominator == 0 {
            return Err(RateError::ZeroRatioDenominator);
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self {
            up: numerator / divisor,
            down: denominator / divisor,
        })
    }

    pub const fn up(self) -> u64 {
        self.up
    }

    pub const fn down(self) -> u64 {
        self.down
    }

    /// Exact input-coordinate increment between adjacent output frames.
    pub const fn input_step(self) -> (u64, u64) {
        (self.down, self.up)
    }
}

/// Explicit duration rounding for finite streams.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrameCountPolicy {
    Floor,
    Ceiling,
    #[default]
    NearestTiesToEven,
}

/// Compute `input_frames * up / down` without a floating-point duration.
pub fn output_frames_for_input(
    input_frames: u64,
    ratio: RateRatio,
    policy: FrameCountPolicy,
) -> Result<u64, RateError> {
    let numerator = u128::from(input_frames) * u128::from(ratio.up);
    let denominator = u128::from(ratio.down);
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let increment = match policy {
        FrameCountPolicy::Floor => false,
        FrameCountPolicy::Ceiling => remainder != 0,
        FrameCountPolicy::NearestTiesToEven => {
            let twice_remainder = remainder * 2;
            twice_remainder > denominator || (twice_remainder == denominator && quotient & 1 == 1)
        }
    };
    u64::try_from(quotient + u128::from(increment)).map_err(|_| RateError::OutputLengthOverflow)
}

const fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

/// One exact resampling position: `input_index + phase / phase_count`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhasePosition {
    pub input_index: u64,
    pub phase: u64,
    pub phase_count: u64,
}

/// A division-free-per-sample phase clock after ratio construction.
///
/// Internally, the update uses `u128` so adding two valid `u64` ratio fields
/// cannot overflow. No state depends on chunk size or CPU floating-point mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhaseClock {
    ratio: RateRatio,
    input_index: u64,
    phase: u64,
}

impl PhaseClock {
    pub const fn new(ratio: RateRatio) -> Self {
        Self::with_input_offset(ratio, 0)
    }

    /// Start at an exact integer input coordinate, used to compensate a
    /// causal designer delay without approximating it in output frames.
    pub const fn with_input_offset(ratio: RateRatio, input_index: u64) -> Self {
        Self {
            ratio,
            input_index,
            phase: 0,
        }
    }

    pub const fn ratio(self) -> RateRatio {
        self.ratio
    }

    pub const fn position(self) -> PhasePosition {
        PhasePosition {
            input_index: self.input_index,
            phase: self.phase,
            phase_count: self.ratio.up,
        }
    }

    /// Return the current position and advance by exactly one output frame.
    pub fn next_position(&mut self) -> Result<PhasePosition, RateError> {
        let current = self.position();
        self.skip_outputs(1)?;
        Ok(current)
    }

    /// Advance by many output frames with the same result as repeated
    /// [`Self::next_position`] calls.
    pub fn skip_outputs(&mut self, output_frames: u64) -> Result<(), RateError> {
        let total_phase =
            u128::from(self.phase) + u128::from(self.ratio.down) * u128::from(output_frames);
        let input_advance = total_phase / u128::from(self.ratio.up);
        let next_phase = total_phase % u128::from(self.ratio.up);
        let input_advance =
            u64::try_from(input_advance).map_err(|_| RateError::InputPositionOverflow)?;
        self.input_index = self
            .input_index
            .checked_add(input_advance)
            .ok_or(RateError::InputPositionOverflow)?;
        self.phase = next_phase as u64;
        Ok(())
    }
}

/// Immutable, phase-major Q1.63 FIR coefficient bank.
///
/// This is the execution-side representation. Filter design and coefficient
/// quantization intentionally live outside it. Coefficients for phase `p`
/// occupy `p * taps_per_phase..(p + 1) * taps_per_phase`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolyphaseFirQ63 {
    phase_count: usize,
    taps_per_phase: usize,
    required_accumulator_bits: u16,
    minimum_accumulator_bits: u16,
    minimum_wide_accumulator_bits: u32,
    dot_backend: ExactDotBackend,
    coefficients: Box<[Q2_62]>,
}

impl PolyphaseFirQ63 {
    /// Q65.63 input preserves pre-rate DSP headroom. Try the same exact i128
    /// sum first; if any intermediate needs more bits, recompute in GMP.
    pub fn convolve_wide_input(
        &self,
        phase: u64,
        samples: &[WideQ63],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        if samples.len() != self.taps_per_phase() {
            return Err(PolyphaseError::SampleCount {
                expected: self.taps_per_phase(),
                actual: samples.len(),
            });
        }
        let coefficients = self.phase(phase)?;
        let native =
            samples
                .iter()
                .zip(coefficients)
                .try_fold(0_i128, |sum, (sample, coefficient)| {
                    sum.checked_add(sample.raw().checked_mul(i128::from(coefficient.raw()))?)
                });
        if let Some(raw) = native {
            return Ok(WideQ63::from_raw(sexq::round_shift_i128(
                raw, 62, rounding,
            )?));
        }
        let format = BigQFormat::new(2, 62)?;
        // Exact signed admission can tighten the narrow bound asymmetrically;
        // the full wide domain has its own once-computed proof, not a +64 guess.
        let mut mac = BigMac::new(
            BigQFormat::new(65, 63)?,
            format,
            Some(self.minimum_wide_accumulator_bits),
        )?;
        for (&sample, coefficient) in samples.iter().zip(coefficients) {
            mac.accumulate_wide(sample, &BigQ::from_i64(coefficient.raw(), format)?)?;
        }
        Ok(mac.finish_wide(rounding)?)
    }

    pub fn new(
        phase_count: usize,
        taps_per_phase: usize,
        coefficients: Vec<Q2_62>,
    ) -> Result<Self, PolyphaseError> {
        Self::new_with_dot_backend(
            phase_count,
            taps_per_phase,
            coefficients,
            automatic_exact_dot_backend(),
        )
    }

    /// Construct a bank with an explicit exact multiplication backend. This is
    /// useful for reproducibility qualification and controlled benchmarks;
    /// both backends retain identical product order and checked accumulation.
    pub fn new_with_dot_backend(
        phase_count: usize,
        taps_per_phase: usize,
        coefficients: Vec<Q2_62>,
        dot_backend: ExactDotBackend,
    ) -> Result<Self, PolyphaseError> {
        if phase_count == 0 {
            return Err(PolyphaseError::ZeroPhases);
        }
        if taps_per_phase == 0 {
            return Err(PolyphaseError::ZeroTaps);
        }
        let expected =
            phase_count
                .checked_mul(taps_per_phase)
                .ok_or(PolyphaseError::CoefficientCount {
                    expected: usize::MAX,
                    actual: coefficients.len(),
                })?;
        if coefficients.len() != expected {
            return Err(PolyphaseError::CoefficientCount {
                expected,
                actual: coefficients.len(),
            });
        }
        if !exact_dot_backend_available(dot_backend) {
            return Err(PolyphaseError::ExactDot(ExactDotError::BackendUnavailable(
                dot_backend,
            )));
        }
        let mut required_accumulator_bits = 0;
        let mut minimum_accumulator_bits = 0;
        let mut minimum_wide_accumulator_bits = 0;
        let wide_format = BigQFormat::new(65, 63)?;
        let coefficient_format = BigQFormat::new(2, 62)?;
        for (phase, phase_coefficients) in coefficients.chunks_exact(taps_per_phase).enumerate() {
            let requirements = MacQ125::exact_requirements_for(phase_coefficients)?;
            if !requirements.fits_i128() {
                return Err(PolyphaseError::AccumulatorTooNarrow {
                    phase,
                    required_bits: requirements.signed_bits,
                });
            }
            minimum_accumulator_bits = minimum_accumulator_bits.max(requirements.signed_bits);
            required_accumulator_bits = required_accumulator_bits
                .max(MacQ125::requirements_for(phase_coefficients)?.signed_bits);
            // Scratch is limited to one phase, never a second complete bank.
            let promoted = phase_coefficients
                .iter()
                .map(|c| BigQ::from_i64(c.raw(), coefficient_format))
                .collect::<Result<Vec<_>, _>>()?;
            minimum_wide_accumulator_bits = minimum_wide_accumulator_bits.max(
                BigMac::exact_requirements_for(wide_format, coefficient_format, &promoted)?
                    .signed_bits,
            );
        }
        Ok(Self {
            phase_count,
            taps_per_phase,
            required_accumulator_bits,
            minimum_accumulator_bits,
            minimum_wide_accumulator_bits,
            dot_backend,
            coefficients: coefficients.into_boxed_slice(),
        })
    }

    /// Construct a bank whose phase count is the exact interpolation factor.
    pub fn for_ratio(
        ratio: RateRatio,
        taps_per_phase: usize,
        coefficients: Vec<Q2_62>,
    ) -> Result<Self, PolyphaseError> {
        let phase_count = usize::try_from(ratio.up())
            .map_err(|_| PolyphaseError::PhaseCountTooLarge(ratio.up()))?;
        Self::new(phase_count, taps_per_phase, coefficients)
    }

    pub fn for_ratio_with_dot_backend(
        ratio: RateRatio,
        taps_per_phase: usize,
        coefficients: Vec<Q2_62>,
        dot_backend: ExactDotBackend,
    ) -> Result<Self, PolyphaseError> {
        let phase_count = usize::try_from(ratio.up())
            .map_err(|_| PolyphaseError::PhaseCountTooLarge(ratio.up()))?;
        Self::new_with_dot_backend(phase_count, taps_per_phase, coefficients, dot_backend)
    }

    pub const fn phase_count(&self) -> usize {
        self.phase_count
    }

    pub const fn taps_per_phase(&self) -> usize {
        self.taps_per_phase
    }

    pub const fn dot_backend(&self) -> ExactDotBackend {
        self.dot_backend
    }

    /// Legacy symmetric L1 estimate retained for designer/cache compatibility.
    /// This can be 129 for a bank whose exact signed interval fits i128.
    /// Use `minimum_accumulator_bits` for the actual required signed width.
    pub const fn required_accumulator_bits(&self) -> u16 {
        self.required_accumulator_bits
    }

    /// Exact minimum full-Q1.63-input signed width across all phases, at most 128.
    pub const fn minimum_accumulator_bits(&self) -> u16 {
        self.minimum_accumulator_bits
    }

    /// Exact full-Q65.63-input width for GMP fallback across all phases.
    /// This may be 193 even though every Q1.63 phase fits i128.
    pub const fn minimum_wide_accumulator_bits(&self) -> u32 {
        self.minimum_wide_accumulator_bits
    }

    pub fn phase(&self, phase: u64) -> Result<&[Q2_62], PolyphaseError> {
        let phase = usize::try_from(phase).map_err(|_| PolyphaseError::PhaseOutOfRange {
            phase,
            phase_count: self.phase_count,
        })?;
        if phase >= self.phase_count {
            return Err(PolyphaseError::PhaseOutOfRange {
                phase: phase as u64,
                phase_count: self.phase_count,
            });
        }
        let start = phase * self.taps_per_phase;
        Ok(&self.coefficients[start..start + self.taps_per_phase])
    }

    /// Evaluate exactly one phase. Every product is accumulated as Q3.125;
    /// rounding and output overflow handling happen once, after the final tap.
    pub fn convolve(
        &self,
        phase: u64,
        samples: &[Q1_63],
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Q1_63>, PolyphaseError> {
        if samples.len() != self.taps_per_phase {
            return Err(PolyphaseError::SampleCount {
                expected: self.taps_per_phase,
                actual: samples.len(),
            });
        }

        let wide = self.convolve_wide(phase, samples, rounding)?;
        Ok(wide.to_q1_63(overflow)?)
    }

    pub fn convolve_wide(
        &self,
        phase: u64,
        samples: &[Q1_63],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        if samples.len() != self.taps_per_phase {
            return Err(PolyphaseError::SampleCount {
                expected: self.taps_per_phase,
                actual: samples.len(),
            });
        }
        let raw =
            exact_dot_q1_63_q2_62_with_backend(samples, self.phase(phase)?, self.dot_backend)?;
        let mut accumulator = MacQ125::new();
        accumulator.accumulate_raw_product(raw)?;
        Ok(accumulator.finish_wide(rounding)?)
    }
}

/// Immutable phase-major FIR bank with arbitrary-width coefficients and MAC.
///
/// PCM-facing samples remain exactly Q1.63, but each sample is losslessly
/// promoted to GMP before multiplication. Products and the full phase sum stay
/// arbitrary-width; only the final result is rounded once back to Q1.63.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolyphaseFirBigQ63 {
    phase_count: usize,
    taps_per_phase: usize,
    coefficient_format: BigQFormat,
    accumulator_bits: u32,
    required_accumulator_bits: u32,
    minimum_accumulator_bits: u32,
    minimum_wide_accumulator_bits: u32,
    coefficients: Arc<[BigQ]>,
}

impl PolyphaseFirBigQ63 {
    pub fn convolve_wide_input(
        &self,
        phase: u64,
        samples: &[WideQ63],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        if samples.len() != self.taps_per_phase() {
            return Err(PolyphaseError::SampleCount {
                expected: self.taps_per_phase(),
                actual: samples.len(),
            });
        }
        let bits = self
            .accumulator_bits
            .checked_add(64)
            .ok_or(BigQError::FractionalWidthOverflow)?
            .max(self.minimum_wide_accumulator_bits);
        let mut mac = BigMac::new(
            BigQFormat::new(65, 63)?,
            self.coefficient_format,
            Some(bits),
        )?;
        for (&sample, coefficient) in samples.iter().zip(self.phase(phase)?) {
            mac.accumulate_wide(sample, coefficient)?;
        }
        Ok(mac.finish_wide(rounding)?)
    }

    pub fn new(
        phase_count: usize,
        taps_per_phase: usize,
        coefficient_format: BigQFormat,
        accumulator_bits: u32,
        coefficients: Vec<BigQ>,
    ) -> Result<Self, PolyphaseError> {
        if phase_count == 0 {
            return Err(PolyphaseError::ZeroPhases);
        }
        if taps_per_phase == 0 {
            return Err(PolyphaseError::ZeroTaps);
        }
        let expected =
            phase_count
                .checked_mul(taps_per_phase)
                .ok_or(PolyphaseError::CoefficientCount {
                    expected: usize::MAX,
                    actual: coefficients.len(),
                })?;
        if coefficients.len() != expected {
            return Err(PolyphaseError::CoefficientCount {
                expected,
                actual: coefficients.len(),
            });
        }
        let sample_format = BigQFormat::new(1, Q1_63::FRACTIONAL_BITS)?;
        BigMac::new(sample_format, coefficient_format, Some(accumulator_bits))?;
        let mut required_accumulator_bits = 0;
        let mut minimum_accumulator_bits = 0;
        let mut minimum_wide_accumulator_bits = 0;
        for (phase, phase_coefficients) in coefficients.chunks_exact(taps_per_phase).enumerate() {
            let requirements = BigMac::exact_requirements_for(
                sample_format,
                coefficient_format,
                phase_coefficients,
            )?;
            if !requirements.fits_signed_bits(accumulator_bits) {
                return Err(PolyphaseError::BigAccumulatorTooNarrow {
                    phase,
                    required_bits: requirements.signed_bits,
                    configured_bits: accumulator_bits,
                });
            }
            minimum_accumulator_bits = minimum_accumulator_bits.max(requirements.signed_bits);
            minimum_wide_accumulator_bits = minimum_wide_accumulator_bits.max(
                BigMac::exact_requirements_for(
                    BigQFormat::new(65, 63)?,
                    coefficient_format,
                    phase_coefficients,
                )?
                .signed_bits,
            );
            required_accumulator_bits = required_accumulator_bits.max(
                BigMac::requirements_for(sample_format, coefficient_format, phase_coefficients)?
                    .signed_bits,
            );
        }
        Ok(Self {
            phase_count,
            taps_per_phase,
            coefficient_format,
            accumulator_bits,
            required_accumulator_bits,
            minimum_accumulator_bits,
            minimum_wide_accumulator_bits,
            coefficients: coefficients.into(),
        })
    }

    pub fn for_ratio(
        ratio: RateRatio,
        taps_per_phase: usize,
        coefficient_format: BigQFormat,
        accumulator_bits: u32,
        coefficients: Vec<BigQ>,
    ) -> Result<Self, PolyphaseError> {
        let phase_count = usize::try_from(ratio.up())
            .map_err(|_| PolyphaseError::PhaseCountTooLarge(ratio.up()))?;
        Self::new(
            phase_count,
            taps_per_phase,
            coefficient_format,
            accumulator_bits,
            coefficients,
        )
    }

    pub const fn phase_count(&self) -> usize {
        self.phase_count
    }

    pub const fn taps_per_phase(&self) -> usize {
        self.taps_per_phase
    }

    pub const fn coefficient_format(&self) -> BigQFormat {
        self.coefficient_format
    }

    pub const fn accumulator_bits(&self) -> u32 {
        self.accumulator_bits
    }

    /// Legacy symmetric L1 estimate retained in designer/cache reports.
    /// It may exceed the exact admitted accumulator width by one bit.
    pub const fn required_accumulator_bits(&self) -> u32 {
        self.required_accumulator_bits
    }

    /// Exact minimum signed width across all phases for Q1.63 input.
    pub const fn minimum_accumulator_bits(&self) -> u32 {
        self.minimum_accumulator_bits
    }

    /// Exact minimum for Q65.63 input; not necessarily the Q1.63 minimum + 64.
    pub const fn minimum_wide_accumulator_bits(&self) -> u32 {
        self.minimum_wide_accumulator_bits
    }

    pub fn phase(&self, phase: u64) -> Result<&[BigQ], PolyphaseError> {
        let phase = usize::try_from(phase).map_err(|_| PolyphaseError::PhaseOutOfRange {
            phase,
            phase_count: self.phase_count,
        })?;
        if phase >= self.phase_count {
            return Err(PolyphaseError::PhaseOutOfRange {
                phase: phase as u64,
                phase_count: self.phase_count,
            });
        }
        let start = phase * self.taps_per_phase;
        Ok(&self.coefficients[start..start + self.taps_per_phase])
    }

    pub fn convolve(
        &self,
        phase: u64,
        samples: &[Q1_63],
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Q1_63>, PolyphaseError> {
        if samples.len() != self.taps_per_phase {
            return Err(PolyphaseError::SampleCount {
                expected: self.taps_per_phase,
                actual: samples.len(),
            });
        }
        let sample_format = BigQFormat::new(1, Q1_63::FRACTIONAL_BITS)?;
        let accumulator = self.accumulate_phase(phase, samples, sample_format)?;
        let outcome = accumulator.finish(sample_format, rounding, overflow)?;
        let raw = outcome
            .value
            .raw_i64()
            .expect("Q1.63 BigQ raw value always fits i64");
        Ok(ArithmeticOutcome {
            value: Q1_63::from_raw(raw),
            saturated: outcome.saturated,
        })
    }

    pub fn convolve_wide(
        &self,
        phase: u64,
        samples: &[Q1_63],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        if samples.len() != self.taps_per_phase {
            return Err(PolyphaseError::SampleCount {
                expected: self.taps_per_phase,
                actual: samples.len(),
            });
        }
        let sample_format = BigQFormat::new(1, Q1_63::FRACTIONAL_BITS)?;
        let accumulator = self.accumulate_phase(phase, samples, sample_format)?;
        let wide_format = BigQFormat::new(65, WideQ63::FRACTIONAL_BITS)?;
        let outcome = accumulator.finish(wide_format, rounding, OverflowPolicy::Error)?;
        Ok(WideQ63::from_raw(
            outcome
                .value
                .raw_i128()
                .expect("Q65.63 BigQ raw value always fits i128"),
        ))
    }

    fn accumulate_phase(
        &self,
        phase: u64,
        samples: &[Q1_63],
        sample_format: BigQFormat,
    ) -> Result<BigMac, PolyphaseError> {
        let mut accumulator = BigMac::new(
            sample_format,
            self.coefficient_format,
            Some(self.accumulator_bits),
        )?;
        for (&sample, coefficient) in samples.iter().zip(self.phase(phase)?) {
            let sample = BigQ::from_i64(sample.raw(), sample_format)?;
            accumulator.accumulate(&sample, coefficient)?;
        }
        Ok(accumulator)
    }
}

#[derive(Debug)]
enum FirQ63Kernel {
    Native(Arc<PolyphaseFirQ63>),
    Big(Arc<PolyphaseFirBigQ63>),
}

impl FirQ63Kernel {
    fn phase_count(&self) -> usize {
        match self {
            Self::Native(fir) => fir.phase_count(),
            Self::Big(fir) => fir.phase_count(),
        }
    }

    fn taps_per_phase(&self) -> usize {
        match self {
            Self::Native(fir) => fir.taps_per_phase(),
            Self::Big(fir) => fir.taps_per_phase(),
        }
    }

    fn convolve<S: ResamplerSample>(
        &self,
        phase: u64,
        samples: &[S],
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<Q1_63>, PolyphaseError> {
        Ok(self
            .convolve_wide(phase, samples, rounding)?
            .to_q1_63(overflow)?)
    }

    fn convolve_wide<S: ResamplerSample>(
        &self,
        phase: u64,
        samples: &[S],
        rounding: RoundingMode,
    ) -> Result<WideQ63, PolyphaseError> {
        match self {
            Self::Native(fir) => S::native_fir(fir, phase, samples, rounding),
            Self::Big(fir) => S::big_fir(fir, phase, samples, rounding),
        }
    }
}

/// Auditable counters from one streaming resampler instance.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamStats {
    pub input_frames: u64,
    pub output_frames: u64,
    pub saturated_outputs: u64,
}

/// Causal, zero-padded, mono polyphase stream with Q1.63 or Q65.63 input.
///
/// Tap zero multiplies `x[input_index]`, tap one multiplies
/// `x[input_index - 1]`, and so on. Missing samples before the stream are zero.
/// [`Self::finish_into`] appends exactly `taps_per_phase - 1` zero input frames
/// to expose the untrimmed causal FIR tail. A later linear-phase façade may
/// trim a designer-declared group delay without changing this kernel.
///
/// The implementation stores indexed FIR history and a scratch window. Finite
/// streams can temporarily retain more than one tap window when exact frame
/// count rounding defers an otherwise-ready phase; input is never retained for
/// the full stream and unused phases are never computed.
#[derive(Debug)]
pub struct CausalResampler<S: ResamplerSample> {
    timeline: Timeline<S>,
    fir: FirQ63Kernel,
    rounding: RoundingMode,
    overflow: OverflowPolicy,
    output_domain: Option<OutputDomain>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputDomain {
    Narrow,
    Wide,
}

impl<S: ResamplerSample> CausalResampler<S> {
    pub fn new(
        ratio: RateRatio,
        fir: Arc<PolyphaseFirQ63>,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, StreamError> {
        Self::new_with_input_delay(ratio, fir, 0, rounding, overflow)
    }

    /// Align output frame zero with an exact integer input delay.
    pub fn new_with_input_delay(
        ratio: RateRatio,
        fir: Arc<PolyphaseFirQ63>,
        input_delay: u64,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, StreamError> {
        Self::new_with_kernel(
            ratio,
            FirQ63Kernel::Native(fir),
            input_delay,
            rounding,
            overflow,
        )
    }

    pub fn new_big_with_input_delay(
        ratio: RateRatio,
        fir: Arc<PolyphaseFirBigQ63>,
        input_delay: u64,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, StreamError> {
        Self::new_with_kernel(
            ratio,
            FirQ63Kernel::Big(fir),
            input_delay,
            rounding,
            overflow,
        )
    }

    fn new_with_kernel(
        ratio: RateRatio,
        fir: FirQ63Kernel,
        input_delay: u64,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, StreamError> {
        if u64::try_from(fir.phase_count()).ok() != Some(ratio.up()) {
            return Err(StreamError::PhaseCountMismatch {
                ratio_up: ratio.up(),
                bank_phases: fir.phase_count(),
            });
        }
        let timeline = Timeline::new(ratio, input_delay, fir.taps_per_phase(), S::ZERO)?;
        Ok(Self {
            timeline,
            fir,
            rounding,
            overflow,
            output_domain: None,
        })
    }

    pub const fn stats(&self) -> StreamStats {
        self.timeline.stats()
    }
    /// Number of retained input samples, excluding the reusable FIR scratch window.
    pub fn retained_input_frames(&self) -> usize {
        self.timeline.retained_frames()
    }

    /// Append causal output. A processing error preserves any completed prefix
    /// but poisons the stream; resuming after a skipped phase is forbidden.
    pub fn push_into(&mut self, input: &[S], output: &mut Vec<Q1_63>) -> Result<(), StreamError> {
        self.push_narrow(input, None, output)
    }

    /// Append no more than the exact rounded output length of the input prefix.
    pub fn push_finite_into(
        &mut self,
        input: &[S],
        policy: FrameCountPolicy,
        output: &mut Vec<Q1_63>,
    ) -> Result<(), StreamError> {
        self.push_narrow(input, Some(policy), output)
    }

    fn push_narrow(
        &mut self,
        input: &[S],
        policy: Option<FrameCountPolicy>,
        output: &mut Vec<Q1_63>,
    ) -> Result<(), StreamError> {
        self.timeline.ensure_ready()?;
        self.timeline.select_mode(policy)?;
        self.select_output_domain(OutputDomain::Narrow)?;
        self.timeline.push(input, policy, output, |phase, samples| {
            self.fir
                .convolve(phase, samples, self.rounding, self.overflow)
        })
    }

    pub fn finish_into(self, output: &mut Vec<Q1_63>) -> Result<StreamStats, StreamError> {
        self.finish_narrow(None, output)
    }

    pub fn finish_exact_frames(
        self,
        target_output_frames: u64,
        output: &mut Vec<Q1_63>,
    ) -> Result<StreamStats, StreamError> {
        self.finish_narrow(Some(target_output_frames), output)
    }

    fn finish_narrow(
        mut self,
        target: Option<u64>,
        output: &mut Vec<Q1_63>,
    ) -> Result<StreamStats, StreamError> {
        self.timeline.ensure_ready()?;
        if target.is_none() {
            self.timeline.select_mode(None)?;
        }
        self.select_output_domain(OutputDomain::Narrow)?;
        let Self {
            timeline,
            fir,
            rounding,
            overflow,
            ..
        } = self;
        timeline.finish(target, output, |phase, samples| {
            fir.convolve(phase, samples, rounding, overflow)
        })
    }

    pub fn push_wide_into(
        &mut self,
        input: &[S],
        output: &mut Vec<WideQ63>,
    ) -> Result<(), StreamError> {
        self.push_wide(input, None, output)
    }

    pub fn push_finite_wide_into(
        &mut self,
        input: &[S],
        policy: FrameCountPolicy,
        output: &mut Vec<WideQ63>,
    ) -> Result<(), StreamError> {
        self.push_wide(input, Some(policy), output)
    }

    fn push_wide(
        &mut self,
        input: &[S],
        policy: Option<FrameCountPolicy>,
        output: &mut Vec<WideQ63>,
    ) -> Result<(), StreamError> {
        self.timeline.ensure_ready()?;
        self.timeline.select_mode(policy)?;
        self.select_output_domain(OutputDomain::Wide)?;
        self.timeline.push(input, policy, output, |phase, samples| {
            self.fir
                .convolve_wide(phase, samples, self.rounding)
                .map(|value| ArithmeticOutcome {
                    value,
                    saturated: false,
                })
        })
    }

    pub fn finish_wide_exact_frames(
        mut self,
        target_output_frames: u64,
        output: &mut Vec<WideQ63>,
    ) -> Result<StreamStats, StreamError> {
        self.select_output_domain(OutputDomain::Wide)?;
        let Self {
            timeline,
            fir,
            rounding,
            ..
        } = self;
        timeline.finish(Some(target_output_frames), output, |phase, samples| {
            fir.convolve_wide(phase, samples, rounding)
                .map(|value| ArithmeticOutcome {
                    value,
                    saturated: false,
                })
        })
    }

    fn select_output_domain(&mut self, requested: OutputDomain) -> Result<(), StreamError> {
        self.timeline.ensure_ready()?;
        match self.output_domain {
            None => {
                self.output_domain = Some(requested);
                Ok(())
            }
            Some(existing) if existing == requested => Ok(()),
            Some(_) => Err(StreamError::OutputDomainMismatch),
        }
    }
}

/// Aggregate counters for an interleaved multichannel stream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InterleavedStreamStats {
    pub input_frames: u64,
    pub output_frames: u64,
    pub saturated_samples: u64,
}

/// One independent exact FIR state per channel with a shared immutable bank.
#[derive(Debug)]
pub struct InterleavedResampler<S: ResamplerSample> {
    channels: u16,
    streams: Vec<CausalResampler<S>>,
    poisoned: bool,
}

impl<S: ResamplerSample> InterleavedResampler<S> {
    pub fn new_with_input_delay(
        channels: u16,
        ratio: RateRatio,
        fir: Arc<PolyphaseFirQ63>,
        input_delay: u64,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, StreamError> {
        if channels == 0 {
            return Err(StreamError::ZeroChannels);
        }
        let mut streams = Vec::with_capacity(usize::from(channels));
        for _ in 0..channels {
            streams.push(CausalResampler::<S>::new_with_input_delay(
                ratio,
                Arc::clone(&fir),
                input_delay,
                rounding,
                overflow,
            )?);
        }
        Ok(Self {
            channels,
            streams,
            poisoned: false,
        })
    }

    /// Construct one bigint-kernel stream per channel while retaining the
    /// shared exact-rational scheduler and immutable coefficient bank.
    pub fn new_big_with_input_delay(
        channels: u16,
        ratio: RateRatio,
        fir: Arc<PolyphaseFirBigQ63>,
        input_delay: u64,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, StreamError> {
        if channels == 0 {
            return Err(StreamError::ZeroChannels);
        }
        let mut streams = Vec::with_capacity(usize::from(channels));
        for _ in 0..channels {
            streams.push(CausalResampler::<S>::new_big_with_input_delay(
                ratio,
                Arc::clone(&fir),
                input_delay,
                rounding,
                overflow,
            )?);
        }
        Ok(Self {
            channels,
            streams,
            poisoned: false,
        })
    }

    pub const fn channels(&self) -> u16 {
        self.channels
    }

    pub fn push_interleaved_into(
        &mut self,
        input: &[S],
        output: &mut Vec<Q1_63>,
    ) -> Result<(), StreamError> {
        if self.poisoned {
            return Err(StreamError::Poisoned);
        }
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            return Err(StreamError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        let frames = input.len() / channels;
        let mut channel_inputs = (0..channels)
            .map(|_| Vec::with_capacity(frames))
            .collect::<Vec<_>>();
        for frame in input.chunks_exact(channels) {
            for (channel, &sample) in frame.iter().enumerate() {
                channel_inputs[channel].push(sample);
            }
        }

        let mut channel_outputs = (0..channels).map(|_| Vec::new()).collect::<Vec<_>>();
        for channel in 0..channels {
            if let Err(error) = self.streams[channel]
                .push_into(&channel_inputs[channel], &mut channel_outputs[channel])
            {
                self.poisoned = true;
                return Err(error);
            }
        }
        append_interleaved(&channel_outputs, output)
    }

    pub fn push_interleaved_finite_into(
        &mut self,
        input: &[S],
        policy: FrameCountPolicy,
        output: &mut Vec<Q1_63>,
    ) -> Result<(), StreamError> {
        if self.poisoned {
            return Err(StreamError::Poisoned);
        }
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            return Err(StreamError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        let frames = input.len() / channels;
        let mut channel_inputs = (0..channels)
            .map(|_| Vec::with_capacity(frames))
            .collect::<Vec<_>>();
        for frame in input.chunks_exact(channels) {
            for (channel, &sample) in frame.iter().enumerate() {
                channel_inputs[channel].push(sample);
            }
        }
        let mut channel_outputs = (0..channels).map(|_| Vec::new()).collect::<Vec<_>>();
        for channel in 0..channels {
            if let Err(error) = self.streams[channel].push_finite_into(
                &channel_inputs[channel],
                policy,
                &mut channel_outputs[channel],
            ) {
                self.poisoned = true;
                return Err(error);
            }
        }
        append_interleaved(&channel_outputs, output)
    }

    pub fn finish_exact_frames(
        self,
        target_output_frames: u64,
        output: &mut Vec<Q1_63>,
    ) -> Result<InterleavedStreamStats, StreamError> {
        if self.poisoned {
            return Err(StreamError::Poisoned);
        }
        let mut channel_outputs = (0..usize::from(self.channels))
            .map(|_| Vec::new())
            .collect::<Vec<_>>();
        let mut channel_stats = Vec::with_capacity(usize::from(self.channels));
        for (channel, stream) in self.streams.into_iter().enumerate() {
            channel_stats.push(
                stream.finish_exact_frames(target_output_frames, &mut channel_outputs[channel])?,
            );
        }
        append_interleaved(&channel_outputs, output)?;

        let first = channel_stats[0];
        if channel_stats.iter().any(|stats| {
            stats.input_frames != first.input_frames || stats.output_frames != first.output_frames
        }) {
            return Err(StreamError::ChannelOutputMismatch);
        }
        let saturated_samples = channel_stats.iter().try_fold(0_u64, |total, stats| {
            total
                .checked_add(stats.saturated_outputs)
                .ok_or(StreamError::SaturationCountOverflow)
        })?;
        Ok(InterleavedStreamStats {
            input_frames: first.input_frames,
            output_frames: first.output_frames,
            saturated_samples,
        })
    }

    pub fn push_interleaved_wide_into(
        &mut self,
        input: &[S],
        output: &mut Vec<WideQ63>,
    ) -> Result<(), StreamError> {
        if self.poisoned {
            return Err(StreamError::Poisoned);
        }
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            return Err(StreamError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        let frames = input.len() / channels;
        let mut channel_inputs = (0..channels)
            .map(|_| Vec::with_capacity(frames))
            .collect::<Vec<_>>();
        for frame in input.chunks_exact(channels) {
            for (channel, &sample) in frame.iter().enumerate() {
                channel_inputs[channel].push(sample);
            }
        }
        let mut channel_outputs = (0..channels).map(|_| Vec::new()).collect::<Vec<_>>();
        for channel in 0..channels {
            if let Err(error) = self.streams[channel]
                .push_wide_into(&channel_inputs[channel], &mut channel_outputs[channel])
            {
                self.poisoned = true;
                return Err(error);
            }
        }
        append_interleaved(&channel_outputs, output)
    }

    pub fn push_interleaved_finite_wide_into(
        &mut self,
        input: &[S],
        policy: FrameCountPolicy,
        output: &mut Vec<WideQ63>,
    ) -> Result<(), StreamError> {
        if self.poisoned {
            return Err(StreamError::Poisoned);
        }
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            return Err(StreamError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        let frames = input.len() / channels;
        let mut channel_inputs = (0..channels)
            .map(|_| Vec::with_capacity(frames))
            .collect::<Vec<_>>();
        for frame in input.chunks_exact(channels) {
            for (channel, &sample) in frame.iter().enumerate() {
                channel_inputs[channel].push(sample);
            }
        }
        let mut channel_outputs = (0..channels).map(|_| Vec::new()).collect::<Vec<_>>();
        for channel in 0..channels {
            if let Err(error) = self.streams[channel].push_finite_wide_into(
                &channel_inputs[channel],
                policy,
                &mut channel_outputs[channel],
            ) {
                self.poisoned = true;
                return Err(error);
            }
        }
        append_interleaved(&channel_outputs, output)
    }

    pub fn finish_wide_exact_frames(
        self,
        target_output_frames: u64,
        output: &mut Vec<WideQ63>,
    ) -> Result<InterleavedStreamStats, StreamError> {
        if self.poisoned {
            return Err(StreamError::Poisoned);
        }
        let mut channel_outputs = (0..usize::from(self.channels))
            .map(|_| Vec::new())
            .collect::<Vec<_>>();
        let mut channel_stats = Vec::with_capacity(usize::from(self.channels));
        for (channel, stream) in self.streams.into_iter().enumerate() {
            channel_stats.push(
                stream.finish_wide_exact_frames(
                    target_output_frames,
                    &mut channel_outputs[channel],
                )?,
            );
        }
        append_interleaved(&channel_outputs, output)?;
        let first = channel_stats[0];
        if channel_stats.iter().any(|stats| {
            stats.input_frames != first.input_frames || stats.output_frames != first.output_frames
        }) {
            return Err(StreamError::ChannelOutputMismatch);
        }
        Ok(InterleavedStreamStats {
            input_frames: first.input_frames,
            output_frames: first.output_frames,
            saturated_samples: 0,
        })
    }
}

/// Q1.63 input API, preserving the original scheduler surface.
pub type CausalResamplerQ63 = CausalResampler<Q1_63>;
/// Q65.63 input, with the identical scheduler and extended FIR accumulation.
pub type CausalResamplerWideQ63 = CausalResampler<WideQ63>;
pub type InterleavedResamplerQ63 = InterleavedResampler<Q1_63>;
pub type InterleavedResamplerWideQ63 = InterleavedResampler<WideQ63>;

fn append_interleaved<T: Clone>(
    channels: &[Vec<T>],
    output: &mut Vec<T>,
) -> Result<(), StreamError> {
    let Some(first) = channels.first() else {
        return Err(StreamError::ZeroChannels);
    };
    if channels.iter().any(|channel| channel.len() != first.len()) {
        return Err(StreamError::ChannelOutputMismatch);
    }
    let additional = first
        .len()
        .checked_mul(channels.len())
        .ok_or(StreamError::InterleavedCountOverflow)?;
    output
        .try_reserve(additional)
        .map_err(|_| StreamError::InterleavedCountOverflow)?;
    for frame in 0..first.len() {
        for channel in channels {
            output.push(channel[frame].clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_fir_products_cancel_exactly_beyond_i128() {
        let coefficients = vec![
            Q2_62::from_raw(3_i64 << 61),
            Q2_62::from_raw(-(3_i64 << 61)),
        ];
        let bank = PolyphaseFirQ63::new(1, 2, coefficients.clone()).unwrap();
        let format = BigQFormat::new(2, 62).unwrap();
        let big = PolyphaseFirBigQ63::new(
            1,
            2,
            format,
            128,
            coefficients
                .iter()
                .map(|c| BigQ::from_i64(c.raw(), format).unwrap())
                .collect(),
        )
        .unwrap();
        for sample in [WideQ63::MIN, WideQ63::MAX, WideQ63::from_raw(3_i128 << 63)] {
            assert_eq!(
                bank.convolve_wide_input(0, &[sample, sample], RoundingMode::NearestTiesToEven)
                    .unwrap(),
                WideQ63::ZERO
            );
            assert_eq!(
                big.convolve_wide_input(0, &[sample, sample], RoundingMode::NearestTiesToEven)
                    .unwrap(),
                WideQ63::ZERO
            );
        }
    }

    #[test]
    fn extended_input_uses_the_same_deferred_finite_schedule() {
        let policy = FrameCountPolicy::NearestTiesToEven;
        for (up, down) in [(1, 5), (5, 4), (3, 7), (160, 147)] {
            let ratio = RateRatio::from_fraction(up, down).unwrap();
            let coefficients = vec![Q2_62::from_raw(1_i64 << 60); 3 * up as usize];
            let bank =
                Arc::new(PolyphaseFirQ63::for_ratio(ratio, 3, coefficients.clone()).unwrap());
            let format = BigQFormat::new(2, 62).unwrap();
            let big = Arc::new(
                PolyphaseFirBigQ63::for_ratio(
                    ratio,
                    3,
                    format,
                    128,
                    coefficients
                        .iter()
                        .map(|c| BigQ::from_i64(c.raw(), format).unwrap())
                        .collect(),
                )
                .unwrap(),
            );
            let input = (0..37)
                .map(|i| WideQ63::from_raw((i128::from(i) - 18) << 63))
                .collect::<Vec<_>>();
            let target = output_frames_for_input(input.len() as u64, ratio, policy).unwrap();
            let mut outputs = Vec::new();
            for block in [1, 7, 37] {
                for is_big in [false, true] {
                    let mut stream = if is_big {
                        CausalResamplerWideQ63::new_big_with_input_delay(
                            ratio,
                            big.clone(),
                            1,
                            RoundingMode::NearestTiesToEven,
                            OverflowPolicy::Error,
                        )
                        .unwrap()
                    } else {
                        CausalResamplerWideQ63::new_with_input_delay(
                            ratio,
                            bank.clone(),
                            1,
                            RoundingMode::NearestTiesToEven,
                            OverflowPolicy::Error,
                        )
                        .unwrap()
                    };
                    let mut output = Vec::new();
                    for chunk in input.chunks(block) {
                        stream
                            .push_finite_wide_into(chunk, policy, &mut output)
                            .unwrap();
                    }
                    let stats = stream
                        .finish_wide_exact_frames(target, &mut output)
                        .unwrap();
                    assert_eq!(stats.input_frames, 37);
                    assert_eq!(stats.output_frames, target);
                    outputs.push(output);
                }
            }
            assert!(outputs.iter().all(|output| output == &outputs[0]));
            assert!(
                outputs[0]
                    .iter()
                    .any(|sample| sample.unsigned_abs() > 1_u128 << 63)
            );
        }
    }

    #[test]
    fn common_audio_ratio_is_reduced_exactly() {
        let ratio = RateRatio::from_rates(44_100, 48_000).unwrap();
        assert_eq!(ratio.up(), 160);
        assert_eq!(ratio.down(), 147);
        assert_eq!(ratio.input_step(), (147, 160));
    }

    #[test]
    fn finite_output_length_uses_explicit_exact_rounding() {
        let half = RateRatio::from_fraction(1, 2).unwrap();
        assert_eq!(
            output_frames_for_input(1, half, FrameCountPolicy::Floor),
            Ok(0)
        );
        assert_eq!(
            output_frames_for_input(1, half, FrameCountPolicy::Ceiling),
            Ok(1)
        );
        assert_eq!(
            output_frames_for_input(1, half, FrameCountPolicy::NearestTiesToEven),
            Ok(0)
        );
        assert_eq!(
            output_frames_for_input(3, half, FrameCountPolicy::NearestTiesToEven),
            Ok(2)
        );
        let common = RateRatio::from_rates(44_100, 48_000).unwrap();
        assert_eq!(
            output_frames_for_input(44_100, common, FrameCountPolicy::NearestTiesToEven),
            Ok(48_000)
        );
    }

    #[test]
    fn invalid_rates_are_rejected() {
        assert_eq!(
            RateRatio::from_rates(0, 48_000),
            Err(RateError::ZeroInputRate)
        );
        assert_eq!(
            RateRatio::from_rates(48_000, 0),
            Err(RateError::ZeroOutputRate)
        );
        assert_eq!(
            RateRatio::from_fraction(1, 0),
            Err(RateError::ZeroRatioDenominator)
        );
    }

    #[test]
    fn phase_sequence_for_441_to_480_is_exact() {
        let mut clock = PhaseClock::new(RateRatio::from_rates(44_100, 48_000).unwrap());
        assert_eq!(
            clock.next_position().unwrap(),
            PhasePosition {
                input_index: 0,
                phase: 0,
                phase_count: 160
            }
        );
        assert_eq!(
            clock.next_position().unwrap(),
            PhasePosition {
                input_index: 0,
                phase: 147,
                phase_count: 160
            }
        );
        assert_eq!(
            clock.next_position().unwrap(),
            PhasePosition {
                input_index: 1,
                phase: 134,
                phase_count: 160
            }
        );
    }

    #[test]
    fn a_complete_ratio_period_returns_to_phase_zero() {
        let ratio = RateRatio::from_rates(44_100, 48_000).unwrap();
        let mut clock = PhaseClock::new(ratio);
        clock.skip_outputs(ratio.up()).unwrap();
        assert_eq!(
            clock.position(),
            PhasePosition {
                input_index: ratio.down(),
                phase: 0,
                phase_count: ratio.up()
            }
        );
    }

    #[test]
    fn downsampling_can_advance_multiple_input_frames() {
        let mut clock = PhaseClock::new(RateRatio::from_rates(48_000, 8_000).unwrap());
        assert_eq!(clock.next_position().unwrap().input_index, 0);
        assert_eq!(clock.next_position().unwrap().input_index, 6);
        assert_eq!(clock.next_position().unwrap().input_index, 12);
    }

    #[test]
    fn scheduling_is_independent_of_chunk_boundaries() {
        let ratio = RateRatio::from_rates(44_100, 48_000).unwrap();
        let mut one_by_one = PhaseClock::new(ratio);
        for _ in 0..10_003 {
            one_by_one.next_position().unwrap();
        }

        let mut chunked = PhaseClock::new(ratio);
        for chunk in [1, 127, 4096, 17, 5762] {
            chunked.skip_outputs(chunk).unwrap();
        }
        assert_eq!(one_by_one, chunked);
    }

    #[test]
    fn large_ratio_products_use_u128_not_wrapping_u64() {
        let ratio = RateRatio::from_fraction(u64::MAX - 1, u64::MAX).unwrap();
        let mut clock = PhaseClock::new(ratio);
        clock.skip_outputs(u64::MAX - 2).unwrap();
        let position = clock.position();
        assert!(position.input_index > 0);
        assert!(position.phase < position.phase_count);
    }

    #[test]
    fn irrational_looking_rates_and_hundred_gib_schedule_remain_exact() {
        let ratio = RateRatio::from_rates(44_117, 47_999).unwrap();
        assert_eq!(ratio, RateRatio::from_fraction(47_999, 44_117).unwrap());

        // Virtual eight-channel, 32-bit PCM payload: schedule the equivalent
        // of 100 GiB without allocating or touching that disk space.
        let input_frames = (100_u64 * 1024 * 1024 * 1024) / (8 * 4);
        let target =
            output_frames_for_input(input_frames, ratio, FrameCountPolicy::NearestTiesToEven)
                .unwrap();
        let product = u128::from(input_frames) * u128::from(ratio.up());
        let quotient = product / u128::from(ratio.down());
        let remainder = product % u128::from(ratio.down());
        let twice_remainder = remainder * 2;
        let expected = quotient
            + u128::from(
                twice_remainder > u128::from(ratio.down())
                    || (twice_remainder == u128::from(ratio.down()) && quotient & 1 == 1),
            );
        assert_eq!(u128::from(target), expected);

        let mut clock = PhaseClock::new(ratio);
        clock.skip_outputs(target).unwrap();
        let exact_phase = u128::from(target) * u128::from(ratio.down());
        assert_eq!(
            u128::from(clock.position().input_index),
            exact_phase / u128::from(ratio.up())
        );
        assert_eq!(
            u128::from(clock.position().phase),
            exact_phase % u128::from(ratio.up())
        );
    }

    #[test]
    fn polyphase_bank_shape_is_validated() {
        assert_eq!(
            PolyphaseFirQ63::new(2, 2, vec![Q2_62::ZERO; 3]),
            Err(PolyphaseError::CoefficientCount {
                expected: 4,
                actual: 3
            })
        );
        assert_eq!(
            PolyphaseFirQ63::new(0, 2, Vec::new()),
            Err(PolyphaseError::ZeroPhases)
        );
        assert_eq!(
            PolyphaseFirQ63::new(1, 2, vec![Q2_62::MIN, Q2_62::MIN]),
            Err(PolyphaseError::AccumulatorTooNarrow {
                phase: 0,
                required_bits: 129
            })
        );
    }

    #[test]
    fn native_wide_fallback_distinguishes_exact_sum_from_destination_overflow() {
        let coefficients = [Q2_62::from_raw(i64::MAX), Q2_62::from_raw(2), Q2_62::MIN];
        let bank = PolyphaseFirQ63::new(1, 3, coefficients.to_vec()).unwrap();
        assert_eq!(bank.minimum_accumulator_bits(), 128);
        assert_eq!(bank.minimum_wide_accumulator_bits(), 193);
        let samples = [
            WideQ63::from_raw(i128::MAX),
            WideQ63::from_raw(i128::MAX),
            WideQ63::from_raw(i128::MIN),
        ];
        let cf = BigQFormat::new(2, 62).unwrap();
        let mut oracle = BigMac::new(BigQFormat::new(65, 63).unwrap(), cf, None).unwrap();
        for (sample, coefficient) in samples.iter().zip(coefficients) {
            oracle
                .accumulate_wide(*sample, &BigQ::from_i64(coefficient.raw(), cf).unwrap())
                .unwrap();
        }
        let expected = oracle
            .finish_wide(RoundingMode::NearestTiesToEven)
            .unwrap_err();
        assert_eq!(expected, BigQError::Overflow(sexq::Operation::Accumulate));
        assert_eq!(
            bank.convolve_wide_input(0, &samples, RoundingMode::NearestTiesToEven),
            Err(expected.into())
        );
    }

    #[test]
    fn native_wide_fallback_matches_unbounded_oracle_for_all_phases_and_roundings() {
        let cf = BigQFormat::new(2, 62).unwrap();
        let sf = BigQFormat::new(65, 63).unwrap();
        let coefficients = vec![
            Q2_62::ONE,
            Q2_62::ZERO,
            Q2_62::ZERO,
            Q2_62::from_raw(i64::MAX),
            Q2_62::from_raw(2),
            Q2_62::MIN,
        ];
        let bank = PolyphaseFirQ63::new(2, 3, coefficients).unwrap();
        assert_eq!(bank.minimum_wide_accumulator_bits(), 193);
        let edges = [
            i128::MIN,
            i128::MAX,
            -1,
            0,
            1,
            -(1_i128 << 126),
            1_i128 << 126,
        ];
        for phase in 0..2 {
            for a in edges {
                for b in edges {
                    for c in edges {
                        let samples = [a, b, c].map(WideQ63::from_raw);
                        let mut oracle = BigMac::new(sf, cf, None).unwrap();
                        for (sample, coefficient) in samples.iter().zip(bank.phase(phase).unwrap())
                        {
                            oracle
                                .accumulate_wide(
                                    *sample,
                                    &BigQ::from_i64(coefficient.raw(), cf).unwrap(),
                                )
                                .unwrap();
                        }
                        for rounding in [
                            RoundingMode::TowardZero,
                            RoundingMode::Floor,
                            RoundingMode::Ceiling,
                            RoundingMode::NearestTiesToEven,
                            RoundingMode::NearestTiesAwayFromZero,
                        ] {
                            assert_eq!(
                                bank.convolve_wide_input(phase, &samples, rounding),
                                oracle
                                    .clone()
                                    .finish_wide(rounding)
                                    .map_err(PolyphaseError::from)
                            );
                        }
                    }
                }
            }
        }
        for (raw, bits) in [(0, 1), (Q2_62::ONE.raw(), 190), (-Q2_62::ONE.raw(), 191)] {
            let bank = PolyphaseFirQ63::new(1, 1, vec![Q2_62::from_raw(raw)]).unwrap();
            assert_eq!(bank.minimum_wide_accumulator_bits(), bits);
        }
    }

    #[test]
    fn native_admission_accepts_exact_i128_endpoints_for_every_available_backend() {
        let round = RoundingMode::NearestTiesToEven;
        for backend in [
            ExactDotBackend::Scalar,
            ExactDotBackend::Avx2,
            ExactDotBackend::Avx512,
        ] {
            if !exact_dot_backend_available(backend) {
                continue;
            }
            eprintln!("native admission backend: {}", backend.name());
            let mut coefficients = vec![Q2_62::ONE; 4];
            coefficients.resize(8, Q2_62::ZERO); // Exercise a full AVX-512 vector too.
            let bank = PolyphaseFirQ63::new_with_dot_backend(1, 8, coefficients, backend).unwrap();
            assert_eq!(bank.required_accumulator_bits(), 129); // legacy cache/report estimate
            assert_eq!(bank.minimum_accumulator_bits(), 128);
            assert_eq!(
                bank.convolve_wide(0, &[Q1_63::MIN; 8], round)
                    .unwrap()
                    .raw(),
                -(1_i128 << 65)
            );
            assert_eq!(
                bank.convolve_wide(0, &[Q1_63::MAX; 8], round)
                    .unwrap()
                    .raw(),
                4 * i128::from(i64::MAX)
            );
            assert!(
                bank.convolve(0, &[Q1_63::MIN; 8], round, OverflowPolicy::Error)
                    .is_err()
            );
            let clipped = bank
                .convolve(0, &[Q1_63::MIN; 8], round, OverflowPolicy::Saturate)
                .unwrap();
            assert_eq!(clipped.value, Q1_63::MIN);
            assert!(clipped.saturated);

            // Both i128 endpoints are attainable, despite a symmetric bound >2^127.
            let mut coefficients = vec![Q2_62::from_raw(i64::MAX), Q2_62::from_raw(2), Q2_62::MIN];
            coefficients.resize(8, Q2_62::ZERO);
            let bank =
                PolyphaseFirQ63::new_with_dot_backend(1, 8, coefficients.clone(), backend).unwrap();
            assert_eq!(bank.minimum_accumulator_bits(), 128);
            for (samples, expected) in [
                ([Q1_63::MIN, Q1_63::MIN, Q1_63::MAX], i128::MIN),
                ([Q1_63::MAX, Q1_63::MAX, Q1_63::MIN], i128::MAX),
            ] {
                let mut samples = samples.to_vec();
                samples.resize(8, Q1_63::ZERO);
                assert_eq!(
                    exact_dot_q1_63_q2_62_with_backend(&samples, &coefficients, backend).unwrap(),
                    expected
                );
                assert_eq!(
                    bank.convolve_wide(0, &samples, round).unwrap().raw(),
                    sexq::round_shift_i128(expected, 62, round).unwrap()
                );
            }
            let mut coefficients = vec![Q2_62::ONE; 4];
            coefficients.extend([Q2_62::from_raw(-Q2_62::ONE.raw()); 4]);
            assert!(matches!(
                PolyphaseFirQ63::new_with_dot_backend(2, 4, coefficients, backend),
                Err(PolyphaseError::AccumulatorTooNarrow {
                    phase: 1,
                    required_bits: 129
                })
            ));
        }
    }

    #[test]
    fn bigint_admission_uses_exact_q63_width_and_reproves_extended_input_width() {
        let round = RoundingMode::NearestTiesToEven;
        let cf = BigQFormat::new(2, 96).unwrap();
        let coefficient: rug::Integer = (rug::Integer::from(1) << 95) + 1;
        let coefficients = vec![
            BigQ::from_raw(coefficient.clone(), cf).unwrap(),
            BigQ::from_raw(-coefficient, cf).unwrap(),
            BigQ::from_i64(-3, cf).unwrap(),
        ];
        let bank = PolyphaseFirBigQ63::new(1, 3, cf, 160, coefficients.clone()).unwrap();
        assert_eq!(bank.required_accumulator_bits(), 161);
        assert_eq!(bank.minimum_accumulator_bits(), 160);
        assert_eq!(bank.minimum_wide_accumulator_bits(), 225);
        let samples = [
            WideQ63::from_raw(i128::MAX),
            WideQ63::from_raw(i128::MIN),
            WideQ63::from_raw(i128::MAX),
        ];
        let mut reference = BigMac::new(BigQFormat::new(65, 63).unwrap(), cf, None).unwrap();
        for (sample, coefficient) in samples.iter().zip(&coefficients) {
            reference.accumulate_wide(*sample, coefficient).unwrap();
        }
        let expected = reference.finish_wide(round).unwrap();
        assert_eq!(
            bank.convolve_wide_input(0, &samples, round).unwrap(),
            expected
        );
        assert_eq!(expected.raw(), i128::MAX - ((1_i128 << 31) - 1));
        // The old '+64' extension fails on tap two, even though final output fits.
        let mut insufficient =
            BigMac::new(BigQFormat::new(65, 63).unwrap(), cf, Some(224)).unwrap();
        insufficient
            .accumulate_wide(samples[0], &coefficients[0])
            .unwrap();
        assert!(matches!(
            insufficient.accumulate_wide(samples[1], &coefficients[1]),
            Err(BigQError::AccumulatorOverflow { bits: 224 })
        ));
        assert!(matches!(
            PolyphaseFirBigQ63::new(1, 3, cf, 159, coefficients),
            Err(PolyphaseError::BigAccumulatorTooNarrow {
                required_bits: 160,
                ..
            })
        ));
    }

    #[test]
    fn bigint_polyphase_uses_one_final_rounding_and_planned_width() {
        let ratio = RateRatio::from_fraction(2, 1).unwrap();
        let coefficient_format = BigQFormat::new(2, 96).unwrap();
        let unity = BigQ::one(coefficient_format).unwrap();
        assert_eq!(
            PolyphaseFirBigQ63::for_ratio(
                ratio,
                1,
                coefficient_format,
                159,
                vec![unity.clone(), unity.clone()],
            ),
            Err(PolyphaseError::BigAccumulatorTooNarrow {
                phase: 0,
                required_bits: 160,
                configured_bits: 159,
            })
        );

        let bank = PolyphaseFirBigQ63::for_ratio(
            ratio,
            1,
            coefficient_format,
            160,
            vec![unity.clone(), unity],
        )
        .unwrap();
        assert_eq!(bank.required_accumulator_bits(), 161);
        assert_eq!(bank.minimum_accumulator_bits(), 160);
        let sample = Q1_63::from_raw(0x0123_4567_89ab_cdef);
        for phase in 0..2 {
            assert_eq!(
                bank.convolve(
                    phase,
                    &[sample],
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error,
                )
                .unwrap(),
                ArithmeticOutcome {
                    value: sample,
                    saturated: false,
                }
            );
        }
    }

    #[test]
    fn only_the_requested_polyphase_branch_is_evaluated() {
        let half = Q2_62::from_raw(1_i64 << 61);
        let quarter = Q2_62::from_raw(1_i64 << 60);
        let three_quarters = Q2_62::from_raw(3_i64 << 60);
        let bank = PolyphaseFirQ63::new(2, 2, vec![half, half, quarter, three_quarters]).unwrap();
        assert_eq!(bank.required_accumulator_bits(), 127);
        let samples = [
            Q1_63::from_raw(1_i64 << 62),
            Q1_63::from_raw(-(1_i64 << 61)),
        ];

        let phase_zero = bank
            .convolve(
                0,
                &samples,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
        let phase_one = bank
            .convolve(
                1,
                &samples,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();

        assert_eq!(phase_zero.value, Q1_63::from_raw(1_i64 << 60));
        assert_eq!(phase_one.value, Q1_63::from_raw(-(1_i64 << 59)));
    }

    #[test]
    fn polyphase_fir_rounds_once_after_the_last_tap() {
        let half = Q2_62::from_raw(1_i64 << 61);
        let bank = PolyphaseFirQ63::new(1, 2, vec![half, half]).unwrap();
        let one_raw_unit = Q1_63::from_raw(1);
        let output = bank
            .convolve(
                0,
                &[one_raw_unit, one_raw_unit],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();

        assert_eq!(output.value, one_raw_unit);
        assert!(!output.saturated);
    }

    #[test]
    fn scalar_and_avx2_fir_backends_are_stream_bit_exact() {
        if !exact_dot_backend_available(ExactDotBackend::Avx2) {
            return;
        }
        let ratio = RateRatio::from_fraction(3, 2).unwrap();
        let taps = 7;
        let raw_weights = [1_i64, 2, 3, 4, 3, 2, 1];
        let weight_sum = raw_weights.iter().sum::<i64>();
        let mut phase = raw_weights
            .map(|weight| Q2_62::from_raw(Q2_62::ONE.raw() / weight_sum * weight))
            .to_vec();
        let residual = Q2_62::ONE.raw() - phase.iter().map(|value| value.raw()).sum::<i64>();
        phase[taps / 2] = Q2_62::from_raw(phase[taps / 2].raw() + residual);
        let coefficients = phase.repeat(ratio.up() as usize);
        let scalar_bank = Arc::new(
            PolyphaseFirQ63::for_ratio_with_dot_backend(
                ratio,
                taps,
                coefficients.clone(),
                ExactDotBackend::Scalar,
            )
            .unwrap(),
        );
        let vector_bank = Arc::new(
            PolyphaseFirQ63::for_ratio_with_dot_backend(
                ratio,
                taps,
                coefficients,
                ExactDotBackend::Avx2,
            )
            .unwrap(),
        );
        assert_eq!(scalar_bank.dot_backend(), ExactDotBackend::Scalar);
        assert_eq!(vector_bank.dot_backend(), ExactDotBackend::Avx2);

        let input = (0_i64..257)
            .map(|index| Q1_63::from_raw((index * 0x0123_4567) ^ -(index * 0x1020_3040)))
            .collect::<Vec<_>>();
        let render = |bank: Arc<PolyphaseFirQ63>| {
            let mut stream = CausalResamplerQ63::new_with_input_delay(
                ratio,
                bank,
                3,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in input.chunks(11) {
                stream
                    .push_finite_into(chunk, FrameCountPolicy::NearestTiesToEven, &mut output)
                    .unwrap();
            }
            let target = output_frames_for_input(
                input.len() as u64,
                ratio,
                FrameCountPolicy::NearestTiesToEven,
            )
            .unwrap();
            let stats = stream.finish_exact_frames(target, &mut output).unwrap();
            (output, stats)
        };
        assert_eq!(render(scalar_bank), render(vector_bank));
    }

    #[test]
    fn wide_fir_output_preserves_overshoot_before_clipping_policy() {
        let bank =
            PolyphaseFirQ63::new(1, 2, vec![Q2_62::ONE, Q2_62::from_raw(1_i64 << 61)]).unwrap();
        let samples = [Q1_63::MAX, Q1_63::MAX];
        let wide = bank
            .convolve_wide(0, &samples, RoundingMode::NearestTiesToEven)
            .unwrap();
        assert!(wide.raw() > i128::from(i64::MAX));
        assert_eq!(
            wide.to_q1_63(OverflowPolicy::Saturate).unwrap(),
            ArithmeticOutcome {
                value: Q1_63::MAX,
                saturated: true,
            }
        );
        assert!(wide.to_q1_63(OverflowPolicy::Error).is_err());
    }

    #[test]
    fn phase_and_sample_bounds_are_errors() {
        let bank = PolyphaseFirQ63::new(1, 1, vec![Q2_62::ONE]).unwrap();
        assert_eq!(
            bank.convolve(
                1,
                &[Q1_63::ZERO],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error
            ),
            Err(PolyphaseError::PhaseOutOfRange {
                phase: 1,
                phase_count: 1
            })
        );
        assert_eq!(
            bank.convolve(
                0,
                &[],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error
            ),
            Err(PolyphaseError::SampleCount {
                expected: 1,
                actual: 0
            })
        );
    }

    fn unity_bank(ratio: RateRatio) -> Arc<PolyphaseFirQ63> {
        Arc::new(
            PolyphaseFirQ63::for_ratio(ratio, 1, vec![Q2_62::ONE; ratio.up() as usize]).unwrap(),
        )
    }

    fn bigint_unity_bank(ratio: RateRatio) -> Arc<PolyphaseFirBigQ63> {
        let format = BigQFormat::new(2, 512).unwrap();
        Arc::new(
            PolyphaseFirBigQ63::for_ratio(
                ratio,
                1,
                format,
                1_024,
                vec![BigQ::one(format).unwrap(); ratio.up() as usize],
            )
            .unwrap(),
        )
    }

    #[test]
    fn streaming_identity_is_bit_exact() {
        let ratio = RateRatio::from_rates(48_000, 48_000).unwrap();
        let input = [Q1_63::MIN, Q1_63::from_raw(-1), Q1_63::ZERO, Q1_63::MAX];
        let mut output = Vec::new();
        let mut stream = CausalResamplerQ63::new(
            ratio,
            unity_bank(ratio),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        stream.push_into(&input, &mut output).unwrap();
        let stats = stream.finish_into(&mut output).unwrap();

        assert_eq!(output, input);
        assert_eq!(
            stats,
            StreamStats {
                input_frames: 4,
                output_frames: 4,
                saturated_outputs: 0
            }
        );
    }

    #[test]
    fn wide_streaming_output_is_chunk_exact_and_cannot_mix_domains() {
        let ratio = RateRatio::from_fraction(1, 1).unwrap();
        let input = [Q1_63::MIN, Q1_63::ZERO, Q1_63::MAX];
        let run = |chunks: &[&[Q1_63]]| {
            let mut stream = CausalResamplerQ63::new(
                ratio,
                unity_bank(ratio),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                stream.push_wide_into(chunk, &mut output).unwrap();
            }
            let stats = stream.finish_wide_exact_frames(3, &mut output).unwrap();
            (output, stats)
        };
        let contiguous = run(&[&input]);
        let chunked = run(&[&input[..1], &input[1..]]);
        assert_eq!(contiguous, chunked);
        assert_eq!(contiguous.0, input.map(WideQ63::from_q1_63).to_vec());

        let mut mixed = CausalResamplerQ63::new(
            ratio,
            unity_bank(ratio),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        mixed.push_wide_into(&input[..1], &mut Vec::new()).unwrap();
        assert_eq!(
            mixed.push_into(&input[1..], &mut Vec::new()),
            Err(StreamError::OutputDomainMismatch)
        );
    }

    #[test]
    fn bigint_streaming_identity_is_bit_exact_across_chunks() {
        let ratio = RateRatio::from_rates(48_000, 48_000).unwrap();
        let input = [Q1_63::MIN, Q1_63::from_raw(-1), Q1_63::ZERO, Q1_63::MAX];
        let run = |chunks: &[&[Q1_63]]| {
            let mut output = Vec::new();
            let mut stream = CausalResamplerQ63::new_big_with_input_delay(
                ratio,
                bigint_unity_bank(ratio),
                0,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            for chunk in chunks {
                stream.push_into(chunk, &mut output).unwrap();
            }
            let stats = stream.finish_into(&mut output).unwrap();
            (output, stats)
        };

        let contiguous = run(&[&input]);
        let chunked = run(&[&input[..1], &input[1..3], &[], &input[3..]]);
        assert_eq!(contiguous, chunked);
        assert_eq!(contiguous.0, input);
        assert_eq!(contiguous.1.saturated_outputs, 0);
    }

    #[test]
    fn streaming_result_does_not_depend_on_chunk_boundaries() {
        let ratio = RateRatio::from_fraction(3, 2).unwrap();
        let input = [
            Q1_63::from_raw(1),
            Q1_63::from_raw(2),
            Q1_63::from_raw(3),
            Q1_63::from_raw(4),
            Q1_63::from_raw(5),
        ];

        let run = |chunks: &[&[Q1_63]]| {
            let mut stream = CausalResamplerQ63::new(
                ratio,
                unity_bank(ratio),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                stream.push_into(chunk, &mut output).unwrap();
            }
            let stats = stream.finish_into(&mut output).unwrap();
            (output, stats)
        };

        let contiguous = run(&[&input]);
        let chunked = run(&[&input[..1], &input[1..3], &[], &input[3..]]);
        assert_eq!(contiguous, chunked);
        assert_eq!(
            contiguous.0,
            vec![
                input[0], input[0], input[1], input[2], input[2], input[3], input[4], input[4]
            ]
        );
    }

    #[test]
    fn million_frame_pathological_downsample_retains_bounded_history() {
        let ratio = RateRatio::from_fraction(1, 1_001).unwrap();
        let quarter = Q2_62::from_raw(1_i64 << 60);
        let half = Q2_62::from_raw(1_i64 << 61);
        let bank = Arc::new(PolyphaseFirQ63::new(1, 3, vec![quarter, quarter, half]).unwrap());
        let mut stream = CausalResamplerQ63::new(
            ratio,
            bank,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        let mut output = Vec::new();
        let mut maximum_retained = 0;
        const INPUT_FRAMES: u64 = 1_000_003;
        for _ in 0..INPUT_FRAMES {
            stream
                .push_finite_into(
                    &[Q1_63::ZERO],
                    FrameCountPolicy::NearestTiesToEven,
                    &mut output,
                )
                .unwrap();
            maximum_retained = maximum_retained.max(stream.retained_input_frames());
            output.clear();
        }
        let target =
            output_frames_for_input(INPUT_FRAMES, ratio, FrameCountPolicy::NearestTiesToEven)
                .unwrap();
        let stats = stream.finish_exact_frames(target, &mut output).unwrap();
        assert_eq!(stats.input_frames, INPUT_FRAMES);
        assert_eq!(stats.output_frames, target);
        assert!(
            maximum_retained <= 503,
            "retained {maximum_retained} frames"
        );
    }

    #[test]
    fn finish_exposes_causal_fir_tail() {
        let ratio = RateRatio::from_fraction(1, 1).unwrap();
        let echo = Arc::new(
            PolyphaseFirQ63::new(1, 2, vec![Q2_62::ONE, Q2_62::from_raw(1_i64 << 61)]).unwrap(),
        );
        let mut stream = CausalResamplerQ63::new(
            ratio,
            echo,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        let mut output = Vec::new();
        stream
            .push_into(&[Q1_63::from_raw(8)], &mut output)
            .unwrap();
        let stats = stream.finish_into(&mut output).unwrap();

        assert_eq!(output, vec![Q1_63::from_raw(8), Q1_63::from_raw(4)]);
        assert_eq!(stats.input_frames, 1);
        assert_eq!(stats.output_frames, 2);
    }

    #[test]
    fn input_delay_and_exact_finish_align_finite_identity() {
        let ratio = RateRatio::from_fraction(3, 2).unwrap();
        let mut stream = CausalResamplerQ63::new_with_input_delay(
            ratio,
            unity_bank(ratio),
            3,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        let input = [
            Q1_63::from_raw(10),
            Q1_63::from_raw(20),
            Q1_63::from_raw(30),
            Q1_63::from_raw(40),
        ];
        let mut output = Vec::new();
        stream.push_into(&input, &mut output).unwrap();
        assert_eq!(output, vec![input[3], input[3]]);
        let target = output_frames_for_input(
            input.len() as u64,
            ratio,
            FrameCountPolicy::NearestTiesToEven,
        )
        .unwrap();
        let stats = stream.finish_exact_frames(target, &mut output).unwrap();
        assert_eq!(stats.output_frames, 6);
        assert_eq!(output.len(), 6);
    }

    #[test]
    fn finite_prefix_rounding_retains_the_window_for_a_deferred_phase() {
        // At 1/5, the output anchored at input frame 5 is ready after six
        // inputs, but nearest rounding does not admit the second output until
        // the eighth input. Its three-tap window must still contain frame 3.
        let ratio = RateRatio::from_fraction(1, 5).unwrap();
        let quarter = Q2_62::from_raw(1_i64 << 60);
        let half = Q2_62::from_raw(1_i64 << 61);
        let bank = Arc::new(PolyphaseFirQ63::new(1, 3, vec![quarter, quarter, half]).unwrap());
        let input = (1_i64..=8)
            .map(|sample| Q1_63::from_raw(sample * 10))
            .collect::<Vec<_>>();

        let render = |chunks: &[&[Q1_63]]| {
            let mut stream = CausalResamplerQ63::new(
                ratio,
                Arc::clone(&bank),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                stream
                    .push_finite_into(chunk, FrameCountPolicy::NearestTiesToEven, &mut output)
                    .unwrap();
            }
            let target = output_frames_for_input(
                input.len() as u64,
                ratio,
                FrameCountPolicy::NearestTiesToEven,
            )
            .unwrap();
            let stats = stream.finish_exact_frames(target, &mut output).unwrap();
            (output, stats)
        };

        let contiguous = render(&[&input]);
        let one_frame_chunks = input.iter().map(std::slice::from_ref).collect::<Vec<_>>();
        let chunked = render(&one_frame_chunks);
        assert_eq!(contiguous, chunked);
        assert_eq!(contiguous.0, vec![Q1_63::from_raw(2), Q1_63::from_raw(48)]);
        assert_eq!(contiguous.1.input_frames, 8);
        assert_eq!(contiguous.1.output_frames, 2);
    }

    #[test]
    fn finite_push_never_emits_a_frame_that_exact_length_would_retract() {
        let ratio = RateRatio::from_fraction(5, 4).unwrap();
        let input = [Q1_63::from_raw(7)];

        let mut unbounded = CausalResamplerQ63::new(
            ratio,
            unity_bank(ratio),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        let mut unbounded_output = Vec::new();
        unbounded.push_into(&input, &mut unbounded_output).unwrap();
        assert_eq!(unbounded_output.len(), 2);
        assert_eq!(
            unbounded.finish_exact_frames(1, &mut unbounded_output),
            Err(StreamError::TargetBeforeProduced {
                target: 1,
                produced: 2,
            })
        );

        let mut finite = CausalResamplerQ63::new(
            ratio,
            unity_bank(ratio),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        let mut finite_output = Vec::new();
        finite
            .push_finite_into(
                &input,
                FrameCountPolicy::NearestTiesToEven,
                &mut finite_output,
            )
            .unwrap();
        let stats = finite.finish_exact_frames(1, &mut finite_output).unwrap();
        assert_eq!(finite_output, input);
        assert_eq!(stats.output_frames, 1);
    }

    #[test]
    fn finite_mode_validates_policy_target_and_output_domain() {
        let ratio = RateRatio::from_fraction(1, 1).unwrap();

        let mut started_unbounded = CausalResamplerQ63::new(
            ratio,
            unity_bank(ratio),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        started_unbounded
            .push_into(&[Q1_63::ZERO], &mut Vec::new())
            .unwrap();
        assert_eq!(
            started_unbounded.push_finite_into(
                &[],
                FrameCountPolicy::NearestTiesToEven,
                &mut Vec::new(),
            ),
            Err(StreamError::FiniteModeStartedLate)
        );

        let mut finite = CausalResamplerQ63::new(
            ratio,
            unity_bank(ratio),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        finite
            .push_finite_wide_into(&[Q1_63::ZERO], FrameCountPolicy::Floor, &mut Vec::new())
            .unwrap();
        assert_eq!(
            finite.push_finite_wide_into(&[], FrameCountPolicy::Ceiling, &mut Vec::new(),),
            Err(StreamError::FinitePolicyMismatch)
        );
        assert_eq!(
            finite.push_into(&[], &mut Vec::new()),
            Err(StreamError::FiniteAndUnboundedModeMismatch)
        );

        let mut wrong_target = CausalResamplerQ63::new(
            ratio,
            unity_bank(ratio),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        wrong_target
            .push_finite_into(
                &[Q1_63::ZERO],
                FrameCountPolicy::NearestTiesToEven,
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(
            wrong_target.finish_exact_frames(2, &mut Vec::new()),
            Err(StreamError::FiniteTargetMismatch {
                expected: 1,
                actual: 2,
            })
        );
    }

    #[test]
    fn interleaved_channels_keep_independent_state_and_order() {
        let ratio = RateRatio::from_fraction(2, 1).unwrap();
        let mut stream = InterleavedResamplerQ63::new_with_input_delay(
            2,
            ratio,
            unity_bank(ratio),
            0,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        let left_one = Q1_63::from_raw(1);
        let right_ten = Q1_63::from_raw(10);
        let left_two = Q1_63::from_raw(2);
        let right_twenty = Q1_63::from_raw(20);
        let mut output = Vec::new();
        stream
            .push_interleaved_into(&[left_one, right_ten], &mut output)
            .unwrap();
        stream
            .push_interleaved_into(&[left_two, right_twenty], &mut output)
            .unwrap();
        let stats = stream.finish_exact_frames(4, &mut output).unwrap();

        assert_eq!(
            output,
            vec![
                left_one,
                right_ten,
                left_one,
                right_ten,
                left_two,
                right_twenty,
                left_two,
                right_twenty
            ]
        );
        assert_eq!(
            stats,
            InterleavedStreamStats {
                input_frames: 2,
                output_frames: 4,
                saturated_samples: 0
            }
        );
    }

    #[test]
    fn interleaved_stream_rejects_partial_frames() {
        let ratio = RateRatio::from_fraction(1, 1).unwrap();
        let mut stream = InterleavedResamplerQ63::new_with_input_delay(
            2,
            ratio,
            unity_bank(ratio),
            0,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        assert_eq!(
            stream.push_interleaved_into(&[Q1_63::ZERO], &mut Vec::new()),
            Err(StreamError::PartialInterleavedFrame {
                samples: 1,
                channels: 2
            })
        );
    }

    #[test]
    fn saturation_is_counted_and_error_policy_poisons() {
        let ratio = RateRatio::from_fraction(1, 1).unwrap();
        let gain = Arc::new(PolyphaseFirQ63::new(1, 1, vec![Q2_62::MAX]).unwrap());
        let input = [Q1_63::from_raw(3_i64 << 61)];

        let mut saturating = CausalResamplerQ63::new(
            ratio,
            Arc::clone(&gain),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Saturate,
        )
        .unwrap();
        let mut saturated_output = Vec::new();
        saturating.push_into(&input, &mut saturated_output).unwrap();
        let stats = saturating.finish_into(&mut saturated_output).unwrap();
        assert_eq!(saturated_output, vec![Q1_63::MAX]);
        assert_eq!(stats.saturated_outputs, 1);

        let mut exact = CausalResamplerQ63::new(
            ratio,
            gain,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        let mut exact_output = Vec::new();
        assert!(matches!(
            exact.push_into(&input, &mut exact_output),
            Err(StreamError::Polyphase(PolyphaseError::Arithmetic(
                ArithmeticError::Overflow(_)
            )))
        ));
        assert_eq!(
            exact.push_into(&[], &mut exact_output),
            Err(StreamError::Poisoned)
        );
    }
}
