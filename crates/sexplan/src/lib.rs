//! Exact, policy-level precision planning for SeX.
//!
//! This crate turns a named quality preset plus an optional error floor into
//! an auditable filter/arithmetic plan. Configuration arithmetic is integer or
//! rational; it never passes through `f32` or `f64`.

use core::fmt;
use core::str::FromStr;
use sexrate::RateRatio;

pub mod numerical;

const MAX_ERROR_FLOOR_DB: u32 = 1_000_000;
// 6.020599913 dB is deliberately a tiny lower approximation of
// 20*log10(2). Dividing by it conservatively rounds the required bit count up.
const DB_PER_BIT_LOWER_NUMERATOR: u128 = 6_020_599_913;
const DB_PER_BIT_LOWER_DENOMINATOR: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Rational {
    numerator: u64,
    denominator: u64,
}

impl Rational {
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, PlanError> {
        if denominator == 0 {
            return Err(PlanError::ZeroDenominator);
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    const fn from_reduced(numerator: u64, denominator: u64) -> Self {
        Self {
            numerator,
            denominator,
        }
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

/// Positive attenuation target written as `-N dB` at the CLI.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ErrorFloor {
    attenuation_db: u32,
}

impl ErrorFloor {
    pub fn new(attenuation_db: u32) -> Result<Self, PlanError> {
        if attenuation_db == 0 {
            return Err(PlanError::ZeroErrorFloor);
        }
        if attenuation_db > MAX_ERROR_FLOOR_DB {
            return Err(PlanError::ErrorFloorTooLarge(attenuation_db));
        }
        Ok(Self { attenuation_db })
    }

    pub const fn attenuation_db(self) -> u32 {
        self.attenuation_db
    }
}

impl fmt::Display for ErrorFloor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "-{}dB", self.attenuation_db)
    }
}

impl FromStr for ErrorFloor {
    type Err = PlanError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let value = raw
            .strip_suffix("dB")
            .or_else(|| raw.strip_suffix("db"))
            .unwrap_or(raw);
        let magnitude = value
            .strip_prefix('-')
            .ok_or_else(|| PlanError::InvalidErrorFloor(raw.to_owned()))?;
        if magnitude.is_empty() || magnitude.starts_with('+') {
            return Err(PlanError::InvalidErrorFloor(raw.to_owned()));
        }
        let attenuation_db = magnitude
            .parse::<u32>()
            .map_err(|_| PlanError::InvalidErrorFloor(raw.to_owned()))?;
        Self::new(attenuation_db)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum QualityPreset {
    Fast,
    #[default]
    Sane,
    High,
    Absurd,
    Pointless,
    Until40k,
}

impl QualityPreset {
    pub const ALL: [Self; 6] = [
        Self::Fast,
        Self::Sane,
        Self::High,
        Self::Absurd,
        Self::Pointless,
        Self::Until40k,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Sane => "sane",
            Self::High => "high",
            Self::Absurd => "absurd",
            Self::Pointless => "pointless",
            Self::Until40k => "until-40k",
        }
    }

    pub const fn definition(self) -> PresetDefinition {
        match self {
            Self::Fast => PresetDefinition {
                preset: self,
                stopband_attenuation_db: 80,
                transition_width_of_lower_nyquist: Rational::from_reduced(1, 5),
                rolloff: Rational::from_reduced(9, 10),
                kaiser_beta: Rational::from_reduced(8, 1),
                half_taps_at_unity: 64,
                minimum_coefficient_fractional_bits: 40,
                minimum_accumulator_bits: 128,
                minimum_working_precision_bits: 128,
                dither: DitherPolicy::Tpdf,
            },
            Self::Sane => PresetDefinition {
                preset: self,
                stopband_attenuation_db: 110,
                transition_width_of_lower_nyquist: Rational::from_reduced(1, 10),
                rolloff: Rational::from_reduced(19, 20),
                kaiser_beta: Rational::from_reduced(10, 1),
                half_taps_at_unity: 128,
                minimum_coefficient_fractional_bits: 62,
                minimum_accumulator_bits: 128,
                minimum_working_precision_bits: 192,
                dither: DitherPolicy::HighPassTpdf,
            },
            Self::High => PresetDefinition {
                preset: self,
                stopband_attenuation_db: 160,
                transition_width_of_lower_nyquist: Rational::from_reduced(1, 20),
                rolloff: Rational::from_reduced(39, 40),
                kaiser_beta: Rational::from_reduced(16, 1),
                half_taps_at_unity: 256,
                minimum_coefficient_fractional_bits: 96,
                minimum_accumulator_bits: 192,
                minimum_working_precision_bits: 256,
                dither: DitherPolicy::NoiseShaped { order: 5 },
            },
            Self::Absurd => PresetDefinition {
                preset: self,
                stopband_attenuation_db: 240,
                transition_width_of_lower_nyquist: Rational::from_reduced(1, 40),
                rolloff: Rational::from_reduced(79, 80),
                kaiser_beta: Rational::from_reduced(25, 1),
                half_taps_at_unity: 1_024,
                minimum_coefficient_fractional_bits: 160,
                minimum_accumulator_bits: 384,
                minimum_working_precision_bits: 384,
                dither: DitherPolicy::NoiseShaped { order: 9 },
            },
            Self::Pointless => PresetDefinition {
                preset: self,
                stopband_attenuation_db: 360,
                transition_width_of_lower_nyquist: Rational::from_reduced(1, 80),
                rolloff: Rational::from_reduced(159, 160),
                kaiser_beta: Rational::from_reduced(39, 1),
                half_taps_at_unity: 2_048,
                minimum_coefficient_fractional_bits: 256,
                minimum_accumulator_bits: 640,
                minimum_working_precision_bits: 512,
                dither: DitherPolicy::NoiseShaped { order: 9 },
            },
            Self::Until40k => PresetDefinition {
                preset: self,
                stopband_attenuation_db: 600,
                transition_width_of_lower_nyquist: Rational::from_reduced(1, 500),
                rolloff: Rational::from_reduced(999, 1_000),
                kaiser_beta: Rational::from_reduced(65, 1),
                half_taps_at_unity: 32_768,
                minimum_coefficient_fractional_bits: 512,
                minimum_accumulator_bits: 1_024,
                minimum_working_precision_bits: 1_024,
                dither: DitherPolicy::NoiseShaped { order: 9 },
            },
        }
    }
}

impl fmt::Display for QualityPreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for QualityPreset {
    type Err = PlanError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.as_str() == value)
            .ok_or_else(|| PlanError::UnknownPreset(value.to_owned()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DitherPolicy {
    Tpdf,
    HighPassTpdf,
    NoiseShaped { order: u8 },
}

impl fmt::Display for DitherPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tpdf => f.write_str("TPDF"),
            Self::HighPassTpdf => f.write_str("high-pass TPDF"),
            Self::NoiseShaped { order } => write!(f, "noise-shaped (order {order})"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendRequirement {
    NativeI128,
    WideInteger { minimum_bits: u32 },
    BigInt { minimum_bits: u32 },
}

impl fmt::Display for BackendRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NativeI128 => f.write_str("native i128"),
            Self::WideInteger { minimum_bits } => {
                write!(f, "wide integer (at least {minimum_bits} bits)")
            }
            Self::BigInt { minimum_bits } => {
                write!(f, "bigint (at least {minimum_bits} bits)")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresetDefinition {
    pub preset: QualityPreset,
    pub stopband_attenuation_db: u32,
    /// Full transition-band width divided by the lower Nyquist.
    pub transition_width_of_lower_nyquist: Rational,
    pub rolloff: Rational,
    pub kaiser_beta: Rational,
    /// Half of the odd tap count before downsampling-ratio scaling.
    pub half_taps_at_unity: u64,
    pub minimum_coefficient_fractional_bits: u32,
    pub minimum_accumulator_bits: u32,
    pub minimum_working_precision_bits: u32,
    pub dither: DitherPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanRequest {
    pub ratio: RateRatio,
    pub preset: QualityPreset,
    pub error_floor: Option<ErrorFloor>,
    /// Explicit MPFR precision. Values below the calculated minimum fail.
    pub working_precision_bits: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrecisionPlan {
    pub ratio: RateRatio,
    pub preset: QualityPreset,
    pub target_error_floor: ErrorFloor,
    /// Attenuation used to size the Kaiser design. It may exceed the public
    /// target to cover finite-order approximation and response-grid margin.
    pub design_attenuation_db: u32,
    pub transition_width_of_lower_nyquist: Rational,
    pub rolloff: Rational,
    pub kaiser_beta: Rational,
    pub taps_per_phase: u64,
    pub coefficient_count: u128,
    pub amplitude_error_bits: u32,
    /// Bits reserved so the sum of all coefficient quantization errors still
    /// fits under the requested amplitude-error budget.
    pub coefficient_sum_guard_bits: u32,
    pub coefficient_fractional_bits: u32,
    pub planned_accumulator_bits: u32,
    pub working_precision_bits: u32,
    pub backend: BackendRequirement,
    pub dither: DitherPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalPrecisionRequest {
    pub minimum_integer_bits: u32,
    /// Number of rounded stages along one signal path, not the number of taps.
    pub rounded_stages: u32,
    /// Conservative log2 bound for downstream amplification of rounding errors.
    pub roundoff_amplification_bits: u32,
    pub fractional_bits: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalPrecisionPlan {
    pub integer_bits: u32,
    pub fractional_bits: u32,
    pub effect_coefficient_fractional_bits: u32,
    pub rounded_stages: u32,
    pub roundoff_amplification_bits: u32,
}

/// Budget each stage's nearest-rounding error below the amplitude target after
/// a supplied amplification bound. This does not include filter approximation,
/// coefficient error, dither, or final PCM quantization in an end-to-end claim.
pub fn plan_signal_precision(
    filter: &PrecisionPlan,
    request: SignalPrecisionRequest,
) -> Result<SignalPrecisionPlan, PlanError> {
    let roundoff_bits = filter
        .amplitude_error_bits
        .checked_add(ceil_log2(u64::from(request.rounded_stages.max(1))))
        .and_then(|bits| bits.checked_add(request.roundoff_amplification_bits))
        .and_then(|bits| bits.checked_add(2))
        .ok_or(PlanError::ArithmeticOverflow)?;
    let minimum = 63
        .max(filter.coefficient_fractional_bits)
        .max(roundoff_bits);
    let fractional_bits = request.fractional_bits.unwrap_or(minimum);
    if fractional_bits < minimum {
        return Err(PlanError::SignalPrecisionBelowMinimum {
            requested: fractional_bits,
            minimum,
        });
    }
    let integer_bits = request.minimum_integer_bits.max(65);
    if integer_bits
        .checked_add(fractional_bits)
        .is_none_or(|bits| bits > 1_048_576)
    {
        return Err(PlanError::SignalWidthSafetyLimit);
    }
    Ok(SignalPrecisionPlan {
        integer_bits,
        fractional_bits,
        effect_coefficient_fractional_bits: if fractional_bits == 63 {
            62
        } else {
            fractional_bits
        },
        rounded_stages: request.rounded_stages,
        roundoff_amplification_bits: request.roundoff_amplification_bits,
    })
}

#[cfg(test)]
mod signal_tests {
    use super::*;

    #[test]
    fn signal_precision_follows_presets_targets_and_amplification() {
        let ratio = RateRatio::from_fraction(160, 147).unwrap();
        let request = SignalPrecisionRequest {
            minimum_integer_bits: 65,
            rounded_stages: 1,
            roundoff_amplification_bits: 12,
            fractional_bits: None,
        };
        let mut previous = 0;
        for preset in QualityPreset::ALL {
            let filter = plan_precision(PlanRequest {
                ratio,
                preset,
                error_floor: None,
                working_precision_bits: None,
            })
            .unwrap();
            let signal = plan_signal_precision(&filter, request).unwrap();
            assert!(signal.fractional_bits >= previous);
            assert!(signal.fractional_bits >= filter.coefficient_fractional_bits);
            previous = signal.fractional_bits;
            if preset == QualityPreset::Until40k {
                assert!(signal.fractional_bits >= 512);
            }
            assert_eq!(
                plan_signal_precision(
                    &filter,
                    SignalPrecisionRequest {
                        fractional_bits: Some(4096),
                        ..request
                    }
                )
                .unwrap()
                .fractional_bits,
                4096
            );
            assert!(
                plan_signal_precision(
                    &filter,
                    SignalPrecisionRequest {
                        fractional_bits: Some(signal.fractional_bits - 1),
                        ..request
                    }
                )
                .is_err()
            );
            let amplified = plan_signal_precision(
                &filter,
                SignalPrecisionRequest {
                    roundoff_amplification_bits: 256,
                    rounded_stages: 7,
                    ..request
                },
            )
            .unwrap();
            assert!(amplified.fractional_bits >= filter.amplitude_error_bits + 256 + 3 + 2);
        }
        let strict = plan_precision(PlanRequest {
            ratio,
            preset: QualityPreset::Sane,
            error_floor: Some(ErrorFloor::new(400).unwrap()),
            working_precision_bits: None,
        })
        .unwrap();
        assert!(
            plan_signal_precision(&strict, request)
                .unwrap()
                .fractional_bits
                > 63
        );
        assert!(
            plan_signal_precision(
                &strict,
                SignalPrecisionRequest {
                    fractional_bits: Some(u32::MAX),
                    ..request
                }
            )
            .is_err()
        );
    }
}

pub fn plan_precision(request: PlanRequest) -> Result<PrecisionPlan, PlanError> {
    let definition = request.preset.definition();
    let requested_attenuation = request.error_floor.map_or(
        definition.stopband_attenuation_db,
        ErrorFloor::attenuation_db,
    );
    let target_attenuation = requested_attenuation.max(definition.stopband_attenuation_db);
    let target_error_floor = ErrorFloor::new(target_attenuation)?;
    // Sane's original beta=10 candidate misses -110 dB between the old 129-point
    // grid samples (48 -> 16 kHz: -106.35 dB on 513 points). Until-40k's original
    // beta=65 also missed -600 dB. High's beta=16 bank has an exact continuous
    // passband violation at 48 -> 24 kHz. These reserve the same 12 dB margin
    // as stricter custom targets, including implicit defaults. Still not a proof.
    let design_attenuation_db = if target_attenuation == definition.stopband_attenuation_db
        && !matches!(
            request.preset,
            QualityPreset::Sane | QualityPreset::High | QualityPreset::Until40k
        ) {
        target_attenuation
    } else {
        target_attenuation
            .checked_add(12)
            .ok_or(PlanError::ArithmeticOverflow)?
    };

    let quality_scaled_half_taps = if design_attenuation_db == definition.stopband_attenuation_db {
        u128::from(definition.half_taps_at_unity)
    } else {
        let numerator = u128::from(definition.half_taps_at_unity)
            .checked_mul(u128::from(design_attenuation_db - 8))
            .ok_or(PlanError::ArithmeticOverflow)?;
        div_ceil(
            numerator,
            u128::from(definition.stopband_attenuation_db - 8),
        )
    };
    let ratio_scaled_half_taps = if request.ratio.down() <= request.ratio.up() {
        quality_scaled_half_taps
    } else {
        div_ceil(
            quality_scaled_half_taps
                .checked_mul(u128::from(request.ratio.down()))
                .ok_or(PlanError::ArithmeticOverflow)?,
            u128::from(request.ratio.up()),
        )
    };
    let taps_per_phase = ratio_scaled_half_taps
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or(PlanError::ArithmeticOverflow)?;
    let taps_per_phase =
        u64::try_from(taps_per_phase).map_err(|_| PlanError::ArithmeticOverflow)?;
    let coefficient_count = u128::from(taps_per_phase)
        .checked_mul(u128::from(request.ratio.up()))
        .ok_or(PlanError::ArithmeticOverflow)?;

    let amplitude_error_bits = amplitude_bits_for_db(target_attenuation)?;
    let coefficient_sum_guard_bits = ceil_log2(taps_per_phase).saturating_add(1);
    let coefficient_fractional_bits = definition.minimum_coefficient_fractional_bits.max(
        amplitude_error_bits
            .checked_add(coefficient_sum_guard_bits)
            .ok_or(PlanError::ArithmeticOverflow)?,
    );
    let precision_floor = definition
        .minimum_working_precision_bits
        .max(coefficient_fractional_bits.saturating_add(64));
    let working_precision_bits = match request.working_precision_bits {
        Some(bits) if bits < precision_floor => {
            return Err(PlanError::WorkingPrecisionBelowMinimum {
                requested: bits,
                minimum: precision_floor,
            });
        }
        Some(bits) => bits,
        None => round_up_to_multiple(precision_floor, 32)?,
    };
    let planned_accumulator_bits = definition.minimum_accumulator_bits.max(
        128_u32
            .checked_add(coefficient_fractional_bits.saturating_sub(62))
            .ok_or(PlanError::ArithmeticOverflow)?,
    );
    let backend = if coefficient_fractional_bits <= 62 && planned_accumulator_bits <= 128 {
        BackendRequirement::NativeI128
    } else if coefficient_fractional_bits <= 126 && planned_accumulator_bits <= 256 {
        BackendRequirement::WideInteger {
            minimum_bits: planned_accumulator_bits,
        }
    } else {
        BackendRequirement::BigInt {
            minimum_bits: planned_accumulator_bits,
        }
    };
    let kaiser_beta = if design_attenuation_db == definition.stopband_attenuation_db {
        definition.kaiser_beta
    } else {
        kaiser_beta_for(design_attenuation_db)?
    };

    Ok(PrecisionPlan {
        ratio: request.ratio,
        preset: request.preset,
        target_error_floor,
        design_attenuation_db,
        transition_width_of_lower_nyquist: definition.transition_width_of_lower_nyquist,
        rolloff: definition.rolloff,
        kaiser_beta,
        taps_per_phase,
        coefficient_count,
        amplitude_error_bits,
        coefficient_sum_guard_bits,
        coefficient_fractional_bits,
        planned_accumulator_bits,
        working_precision_bits,
        backend,
        dither: definition.dither,
    })
}

fn amplitude_bits_for_db(attenuation_db: u32) -> Result<u32, PlanError> {
    let numerator = u128::from(attenuation_db)
        .checked_mul(DB_PER_BIT_LOWER_DENOMINATOR)
        .ok_or(PlanError::ArithmeticOverflow)?;
    u32::try_from(div_ceil(numerator, DB_PER_BIT_LOWER_NUMERATOR))
        .map_err(|_| PlanError::ArithmeticOverflow)
}

/// Conservative binary amplitude budget for an already validated dB target.
pub fn required_amplitude_bits(target: ErrorFloor) -> u32 {
    amplitude_bits_for_db(target.attenuation_db()).expect("validated attenuation fits u32 bits")
}

/// Increase Kaiser design attenuation and length while preserving the public
/// target, exact rate, transition edges, dither, and existing precision floors.
/// This is a candidate heuristic, not evidence that the new bank meets quality.
pub fn strengthen_filter(
    plan: &PrecisionPlan,
    explicit_working_bits: Option<u32>,
) -> Result<PrecisionPlan, PlanError> {
    if plan.taps_per_phase < 3
        || plan.taps_per_phase.is_multiple_of(2)
        || plan.design_attenuation_db <= 8
    {
        return Err(PlanError::ArithmeticOverflow);
    }
    let mut result = plan.clone();
    result.design_attenuation_db = plan
        .design_attenuation_db
        .checked_add(12)
        .ok_or(PlanError::ArithmeticOverflow)?;
    let half = u128::from((plan.taps_per_phase - 1) / 2);
    let new_half = div_ceil(
        half * u128::from(result.design_attenuation_db - 8),
        u128::from(plan.design_attenuation_db - 8),
    );
    result.taps_per_phase =
        u64::try_from(new_half * 2 + 1).map_err(|_| PlanError::ArithmeticOverflow)?;
    result.coefficient_count = u128::from(result.taps_per_phase) * u128::from(plan.ratio.up());
    let beta = kaiser_beta_for(result.design_attenuation_db)?;
    if u128::from(beta.numerator()) * u128::from(result.kaiser_beta.denominator())
        > u128::from(result.kaiser_beta.numerator()) * u128::from(beta.denominator())
    {
        result.kaiser_beta = beta;
    }
    result.coefficient_sum_guard_bits = ceil_log2(result.taps_per_phase) + 1;
    let minimum = required_amplitude_bits(plan.target_error_floor)
        .checked_add(result.coefficient_sum_guard_bits)
        .ok_or(PlanError::ArithmeticOverflow)?;
    raise_coefficient_precision(&result, minimum, explicit_working_bits)
}

/// Raise coefficient precision without changing the filter's rate, shape,
/// acceptance floor, or tap count. Cache identities must use the returned plan.
pub fn raise_coefficient_precision(
    plan: &PrecisionPlan,
    minimum_bits: u32,
    explicit_working_bits: Option<u32>,
) -> Result<PrecisionPlan, PlanError> {
    let mut result = plan.clone();
    result.coefficient_fractional_bits = minimum_bits.max(plan.coefficient_fractional_bits);
    if result.coefficient_fractional_bits > 1_048_512 {
        return Err(PlanError::SignalWidthSafetyLimit);
    }
    let minimum_work = result
        .working_precision_bits
        .max(result.coefficient_fractional_bits + 64);
    result.working_precision_bits = match explicit_working_bits {
        Some(bits) if bits < minimum_work => {
            return Err(PlanError::WorkingPrecisionBelowMinimum {
                requested: bits,
                minimum: minimum_work,
            });
        }
        Some(bits) => bits,
        None => round_up_to_multiple(minimum_work, 32)?,
    };
    result.planned_accumulator_bits = result
        .planned_accumulator_bits
        .max(128 + result.coefficient_fractional_bits.saturating_sub(62));
    result.backend = if result.coefficient_fractional_bits <= 62
        && result.planned_accumulator_bits <= 128
    {
        BackendRequirement::NativeI128
    } else if result.coefficient_fractional_bits <= 126 && result.planned_accumulator_bits <= 256 {
        BackendRequirement::WideInteger {
            minimum_bits: result.planned_accumulator_bits,
        }
    } else {
        BackendRequirement::BigInt {
            minimum_bits: result.planned_accumulator_bits,
        }
    };
    Ok(result)
}

/// Window-independent search step: double the half length, preserving the
/// requested bands and target. This is a candidate heuristic, not a response bound.
pub fn double_filter_length(
    plan: &PrecisionPlan,
    explicit_working_bits: Option<u32>,
) -> Result<PrecisionPlan, PlanError> {
    if plan.taps_per_phase < 3 || plan.taps_per_phase.is_multiple_of(2) {
        return Err(PlanError::ArithmeticOverflow);
    }
    let mut result = plan.clone();
    result.taps_per_phase = plan
        .taps_per_phase
        .checked_sub(1)
        .and_then(|n| n.checked_mul(2))
        .and_then(|n| n.checked_add(1))
        .ok_or(PlanError::ArithmeticOverflow)?;
    result.coefficient_count = u128::from(result.taps_per_phase) * u128::from(plan.ratio.up());
    result.coefficient_sum_guard_bits = ceil_log2(result.taps_per_phase) + 1;
    let minimum = required_amplitude_bits(plan.target_error_floor)
        .checked_add(result.coefficient_sum_guard_bits)
        .ok_or(PlanError::ArithmeticOverflow)?;
    let result = raise_coefficient_precision(&result, minimum, explicit_working_bits)?;
    reserve_full_range_fir_accumulator(&result, explicit_working_bits)
}

/// Conservative Q1.63 x Q2.C bound without assuming any particular window's
/// L1 norm: T * 2^(C+64), plus sign and the exact power-of-two endpoint.
pub fn reserve_full_range_fir_accumulator(
    plan: &PrecisionPlan,
    explicit_working_bits: Option<u32>,
) -> Result<PrecisionPlan, PlanError> {
    if plan.taps_per_phase == 0 {
        return Err(PlanError::ArithmeticOverflow);
    }
    let mut result = plan.clone();
    let width = plan
        .coefficient_fractional_bits
        .checked_add(66)
        .and_then(|bits| bits.checked_add(ceil_log2(plan.taps_per_phase)))
        .ok_or(PlanError::ArithmeticOverflow)?;
    result.planned_accumulator_bits = result.planned_accumulator_bits.max(width);
    raise_coefficient_precision(
        &result,
        plan.coefficient_fractional_bits,
        explicit_working_bits,
    )
}

fn kaiser_beta_for(attenuation_db: u32) -> Result<Rational, PlanError> {
    // High-attenuation Kaiser approximation: beta = 0.1102 * (A - 8.7).
    let offset_tenths = attenuation_db
        .checked_mul(10)
        .and_then(|value| value.checked_sub(87))
        .ok_or(PlanError::ArithmeticOverflow)?;
    let numerator = u64::from(offset_tenths)
        .checked_mul(1_102)
        .ok_or(PlanError::ArithmeticOverflow)?;
    Rational::new(numerator, 100_000)
}

const fn ceil_log2(value: u64) -> u32 {
    if value <= 1 {
        0
    } else {
        u64::BITS - (value - 1).leading_zeros()
    }
}

fn round_up_to_multiple(value: u32, multiple: u32) -> Result<u32, PlanError> {
    let adjusted = value
        .checked_add(multiple - 1)
        .ok_or(PlanError::ArithmeticOverflow)?;
    Ok((adjusted / multiple) * multiple)
}

fn div_ceil(numerator: u128, denominator: u128) -> u128 {
    numerator / denominator + u128::from(!numerator.is_multiple_of(denominator))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    ZeroDenominator,
    ZeroErrorFloor,
    ErrorFloorTooLarge(u32),
    InvalidErrorFloor(String),
    UnknownPreset(String),
    WorkingPrecisionBelowMinimum { requested: u32, minimum: u32 },
    SignalPrecisionBelowMinimum { requested: u32, minimum: u32 },
    SignalWidthSafetyLimit,
    ArithmeticOverflow,
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignalPrecisionBelowMinimum { requested, minimum } => write!(
                f,
                "signal precision {requested} is below the calculated minimum {minimum} fractional bits"
            ),
            Self::SignalWidthSafetyLimit => {
                f.write_str("signal Q width exceeds the 1048576-bit resource limit")
            }
            Self::ZeroDenominator => f.write_str("rational denominator must be non-zero"),
            Self::ZeroErrorFloor => f.write_str("error floor attenuation must be non-zero"),
            Self::ErrorFloorTooLarge(value) => write!(
                f,
                "error floor -{value}dB exceeds the planner safety limit -{MAX_ERROR_FLOOR_DB}dB"
            ),
            Self::InvalidErrorFloor(value) => {
                write!(
                    f,
                    "invalid error floor {value:?}; expected a value such as -300dB"
                )
            }
            Self::UnknownPreset(value) => write!(
                f,
                "unknown preset {value:?}; expected fast, sane, high, absurd, pointless, or until-40k"
            ),
            Self::WorkingPrecisionBelowMinimum { requested, minimum } => write!(
                f,
                "working precision {requested} bits is below the calculated minimum {minimum} bits"
            ),
            Self::ArithmeticOverflow => f.write_str("precision plan exceeds integer safety limits"),
        }
    }
}

impl std::error::Error for PlanError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_length_refinement_reserves_the_full_coefficient_range() {
        let plan = plan_precision(PlanRequest {
            ratio: RateRatio::from_fraction(3, 2).unwrap(),
            preset: QualityPreset::Fast,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        let mut plan = raise_coefficient_precision(&plan, 64, None).unwrap();
        let target = plan.target_error_floor;
        let attenuation = plan.design_attenuation_db;
        for _ in 0..10 {
            let next = double_filter_length(&plan, None).unwrap();
            assert_eq!(next.taps_per_phase, 2 * (plan.taps_per_phase - 1) + 1);
            assert_eq!(next.target_error_floor, target);
            assert_eq!(next.design_attenuation_db, attenuation);
            assert_eq!(next.rolloff, plan.rolloff);
            assert_eq!(next.kaiser_beta, plan.kaiser_beta);
            assert_eq!(next.coefficient_count, u128::from(next.taps_per_phase) * 3);
            assert!(
                next.planned_accumulator_bits
                    >= 66 + next.coefficient_fractional_bits + ceil_log2(next.taps_per_phase)
            );
            plan = next;
        }
        plan.taps_per_phase = u64::MAX;
        assert!(double_filter_length(&plan, None).is_err());
    }

    fn ratio(input: u64, output: u64) -> RateRatio {
        RateRatio::from_rates(input, output).unwrap()
    }

    #[test]
    fn names_and_error_floors_parse_without_floats() {
        for preset in QualityPreset::ALL {
            assert_eq!(preset.as_str().parse::<QualityPreset>(), Ok(preset));
        }
        assert_eq!(
            "-300dB".parse::<ErrorFloor>().unwrap().attenuation_db(),
            300
        );
        assert_eq!("-90".parse::<ErrorFloor>().unwrap().attenuation_db(), 90);
        assert!("300dB".parse::<ErrorFloor>().is_err());
        assert!("-1.5dB".parse::<ErrorFloor>().is_err());
    }

    #[test]
    fn sane_plan_reserves_margin_without_weakening_its_target() {
        let plan = plan_precision(PlanRequest {
            ratio: ratio(44_100, 48_000),
            preset: QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        assert_eq!(plan.target_error_floor, ErrorFloor::new(110).unwrap());
        assert_eq!(plan.design_attenuation_db, 122);
        assert_eq!(plan.taps_per_phase, 289);
        assert_eq!(plan.coefficient_count, 46_240);
        assert_eq!(plan.coefficient_fractional_bits, 62);
        assert_eq!(plan.coefficient_sum_guard_bits, 10);
        assert_eq!(plan.planned_accumulator_bits, 128);
        assert_eq!(plan.working_precision_bits, 192);
        assert_eq!(plan.backend, BackendRequirement::NativeI128);
    }

    #[test]
    fn high_default_and_explicit_target_reserve_design_margin() {
        for error_floor in [None, Some(ErrorFloor::new(160).unwrap())] {
            let plan = plan_precision(PlanRequest {
                ratio: ratio(48_000, 24_000),
                preset: QualityPreset::High,
                error_floor,
                working_precision_bits: None,
            })
            .unwrap();
            assert_eq!(plan.target_error_floor, ErrorFloor::new(160).unwrap());
            assert_eq!(plan.design_attenuation_db, 172);
            assert_eq!(plan.taps_per_phase, 1109);
            assert_eq!(plan.coefficient_fractional_bits, 96);
        }
    }

    #[test]
    fn response_refinement_preserves_target_geometry_and_precision_floors() {
        let initial = plan_precision(PlanRequest {
            ratio: ratio(48_000, 16_000),
            preset: QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        let wider = raise_coefficient_precision(&initial, 160, None).unwrap();
        let next = strengthen_filter(&wider, None).unwrap();
        assert_eq!(next.target_error_floor, wider.target_error_floor);
        assert_eq!(next.ratio, wider.ratio);
        assert_eq!(next.rolloff, wider.rolloff);
        assert_eq!(
            next.transition_width_of_lower_nyquist,
            wider.transition_width_of_lower_nyquist
        );
        assert_eq!(next.dither, wider.dither);
        assert_eq!(next.design_attenuation_db, wider.design_attenuation_db + 12);
        assert!(
            next.taps_per_phase > wider.taps_per_phase && !next.taps_per_phase.is_multiple_of(2)
        );
        assert_eq!(next.coefficient_count, u128::from(next.taps_per_phase));
        assert!(next.coefficient_fractional_bits >= 160);
        assert!(next.planned_accumulator_bits >= wider.planned_accumulator_bits);
    }

    #[test]
    fn response_refinement_rejects_unrepresentable_shapes_and_attenuation() {
        let mut plan = plan_precision(PlanRequest {
            ratio: ratio(48_000, 16_000),
            preset: QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        plan.taps_per_phase = u64::MAX;
        assert!(strengthen_filter(&plan, None).is_err());
        plan.taps_per_phase = 3;
        plan.design_attenuation_db = u32::MAX;
        assert!(strengthen_filter(&plan, None).is_err());
        plan.design_attenuation_db = 8;
        assert!(strengthen_filter(&plan, None).is_err());
    }

    #[test]
    fn downsampling_scales_taps_using_the_exact_rate_ratio() {
        let plan = plan_precision(PlanRequest {
            ratio: ratio(48_000, 16_000),
            preset: QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        assert_eq!(plan.taps_per_phase, 865);
        assert_eq!(plan.coefficient_count, 865);
    }

    #[test]
    fn custom_error_floor_increases_filter_and_precision_budget() {
        let plan = plan_precision(PlanRequest {
            ratio: ratio(44_100, 48_000),
            preset: QualityPreset::Sane,
            error_floor: Some(ErrorFloor::new(300).unwrap()),
            working_precision_bits: None,
        })
        .unwrap();
        assert_eq!(plan.taps_per_phase, 765);
        assert_eq!(plan.amplitude_error_bits, 50);
        assert_eq!(plan.coefficient_sum_guard_bits, 11);
        assert_eq!(plan.coefficient_fractional_bits, 62);
        assert_eq!(plan.backend, BackendRequirement::NativeI128);
        assert_eq!(plan.design_attenuation_db, 312);
        assert_eq!(plan.kaiser_beta, Rational::new(1_671_183, 50_000).unwrap());
    }

    #[test]
    fn every_preset_has_concrete_monotonic_requirements() {
        let mut previous: Option<PresetDefinition> = None;
        for preset in QualityPreset::ALL {
            let definition = preset.definition();
            if let Some(previous) = previous {
                assert!(definition.stopband_attenuation_db > previous.stopband_attenuation_db);
                assert!(
                    definition.minimum_coefficient_fractional_bits
                        > previous.minimum_coefficient_fractional_bits
                );
                assert!(definition.half_taps_at_unity > previous.half_taps_at_unity);
            }
            previous = Some(definition);
        }
    }

    #[test]
    fn wide_presets_never_silently_fall_back_to_native() {
        let high = plan_precision(PlanRequest {
            ratio: ratio(44_100, 48_000),
            preset: QualityPreset::High,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        assert_eq!(
            high.backend,
            BackendRequirement::WideInteger { minimum_bits: 192 }
        );

        let until = plan_precision(PlanRequest {
            ratio: ratio(44_100, 48_000),
            preset: QualityPreset::Until40k,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        assert_eq!(
            until.backend,
            BackendRequirement::BigInt {
                minimum_bits: 1_024
            }
        );
        assert_eq!(until.taps_per_phase, 66_867);
        assert_eq!(until.coefficient_fractional_bits, 512);
    }

    #[test]
    fn until_default_and_explicit_target_both_reserve_design_margin() {
        for error_floor in [None, Some(ErrorFloor::new(600).unwrap())] {
            let plan = plan_precision(PlanRequest {
                ratio: ratio(48_000, 24_000),
                preset: QualityPreset::Until40k,
                error_floor,
                working_precision_bits: None,
            })
            .unwrap();
            assert_eq!(plan.target_error_floor, ErrorFloor::new(600).unwrap());
            assert_eq!(plan.design_attenuation_db, 612);
            assert_eq!(plan.kaiser_beta, kaiser_beta_for(612).unwrap());
            assert_eq!(plan.taps_per_phase, 133_733);
            assert_eq!(plan.coefficient_fractional_bits, 512);
        }
    }

    #[test]
    fn explicit_working_precision_cannot_undercut_the_plan() {
        assert_eq!(
            plan_precision(PlanRequest {
                ratio: ratio(44_100, 48_000),
                preset: QualityPreset::Sane,
                error_floor: None,
                working_precision_bits: Some(128),
            }),
            Err(PlanError::WorkingPrecisionBelowMinimum {
                requested: 128,
                minimum: 192
            })
        );
    }
}
