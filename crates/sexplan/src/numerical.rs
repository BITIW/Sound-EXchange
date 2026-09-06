//! Outward-rounded, integer-only bounds for an entire processing chain.
//!
//! Reference: exact rational effects and the same selected normalization scale.
//! DesignedFir excludes MPFR design error; CertifiedFir instead propagates an
//! externally proved joint error against the caller's mathematical FIR reference.
//! Both exclude ideal-filter approximation and final PCM/dither/clipping.

use rug::Integer;
use sexq::{RoundingMode, round_div_integer};
use sexrate::{PolyphaseFirBigQ63, PolyphaseFirQ63};
use std::fmt;

const MAX_BITS: u32 = 1_048_576;

/// Exact maximum over phase sums of absolute quantized coefficients, in Q2.C
/// raw units. Constructed from a validated bank, never a floating-point report.
/// The caller must use this bound with the same bank during execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirL1Bound {
    taps_per_phase: u64,
    coefficient_fractional_bits: u32,
    maximum_raw: Integer,
}

impl FirL1Bound {
    pub fn from_native(bank: &PolyphaseFirQ63) -> Self {
        let maximum_raw = (0..bank.phase_count())
            .map(|phase| {
                bank.phase(phase as u64)
                    .expect("validated phase")
                    .iter()
                    .map(|c| Integer::from(c.raw()).abs())
                    .sum::<Integer>()
            })
            .max()
            .expect("nonempty validated bank");
        Self {
            taps_per_phase: bank.taps_per_phase() as u64,
            coefficient_fractional_bits: 62,
            maximum_raw,
        }
    }

    pub fn from_big(bank: &PolyphaseFirBigQ63) -> Result<Self, BudgetError> {
        if bank.coefficient_format().integer_bits() != 2 {
            return Err(BudgetError::InvalidFir);
        }
        let maximum_raw = (0..bank.phase_count())
            .map(|phase| {
                bank.phase(phase as u64)
                    .expect("validated phase")
                    .iter()
                    .map(|c| c.raw().clone().abs())
                    .sum::<Integer>()
            })
            .max()
            .expect("nonempty validated bank");
        Ok(Self {
            taps_per_phase: bank.taps_per_phase() as u64,
            coefficient_fractional_bits: bank.coefficient_format().fractional_bits(),
            maximum_raw,
        })
    }

    pub fn maximum_raw(&self) -> &Integer {
        &self.maximum_raw
    }
    pub const fn coefficient_fractional_bits(&self) -> u32 {
        self.coefficient_fractional_bits
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExactRatio {
    pub numerator: i128,
    pub denominator: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NumericalStage {
    /// Rows are output-channel rows, or one FIR/convolution/gain row.
    Linear {
        name: String,
        rows: Vec<Vec<ExactRatio>>,
    },
    DcBlock {
        radius: ExactRatio,
    },
    /// A nearest-quantized Q2.C bank, followed by exact DC correction of one
    /// coefficient in each phase. Does not allocate/materialize its taps.
    DesignedFir {
        taps_per_phase: u64,
        coefficient_fractional_bits: u32,
    },
    /// Q2.C FIR with an externally proved all-phase L1 coefficient error
    /// relative to its mathematical reference. This REPLACES the generic
    /// quantization allowance; callers must bind the proof to the actual bank.
    CertifiedFir {
        taps_per_phase: u64,
        coefficient_fractional_bits: u32,
        error_numerator: Integer,
        error_denominator: Integer,
    },
    /// Attenuation by the SAME exact rational <=1 in execution and reference.
    /// Use the safe worst case 1 here, then add its one final rounding.
    Normalize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageBound {
    pub name: String,
    /// All raw values use NumericalBudget::bound_fractional_bits.
    pub ideal_peak_raw: Integer,
    pub rounding_error_raw: Integer,
    pub effect_coefficient_error_raw: Integer,
    pub fir_coefficient_error_raw: Integer,
}

impl StageBound {
    pub fn total_error_raw(&self) -> Integer {
        self.rounding_error_raw.clone()
            + &self.effect_coefficient_error_raw
            + &self.fir_coefficient_error_raw
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumericalBudget {
    pub bound_fractional_bits: u32,
    pub stages: Vec<StageBound>,
    pub maximum_signal_peak_raw: Integer,
    pub final_bound: StageBound,
}

impl NumericalBudget {
    /// Smallest e with raw/2^W <= 2^e; None means exactly zero.
    pub fn binary_ceiling(&self, raw: &Integer) -> Option<i64> {
        if raw == &0 {
            return None;
        }
        Some(
            i64::from(Integer::from(raw - 1).significant_bits())
                - i64::from(self.bound_fractional_bits),
        )
    }

    pub fn meets_target_bits(&self, bits: u32) -> bool {
        let raw = self.final_bound.total_error_raw();
        if raw == 0 {
            return true;
        }
        if bits > self.bound_fractional_bits {
            return false;
        }
        raw <= Integer::from(1) << (self.bound_fractional_bits - bits)
    }

    pub fn minimum_integer_bits(&self) -> u32 {
        self.binary_ceiling(&self.maximum_signal_peak_raw)
            .map_or(1, |exponent| {
                u32::try_from((exponent + 2).max(1)).unwrap_or(u32::MAX)
            })
    }

    /// Conservative precision increment for one source of error. One extra
    /// bit reserves room for the other independent components and bound-grid
    /// rounding. The caller re-evaluates, never assumes this proves success.
    pub fn additional_bits(&self, raw: &Integer, target_bits: u32) -> u32 {
        self.binary_ceiling(raw).map_or(0, |e| {
            u32::try_from((e + i64::from(target_bits) + 2).max(0)).unwrap_or(u32::MAX)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BudgetError {
    InvalidPrecision,
    InvalidRatio,
    EmptyLinearStage,
    UnstableDcPole,
    InvalidFir,
}
impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPrecision => "numerical budget precision exceeds the safety contract",
            Self::InvalidRatio => "numerical budget ratio denominator must be non-zero",
            Self::EmptyLinearStage => "numerical budget requires non-empty linear rows",
            Self::UnstableDcPole => "DC radius must stay in [0,1) after coefficient quantization",
            Self::InvalidFir => "numerical budget requires a non-empty Q2.C FIR",
        })
    }
}
impl std::error::Error for BudgetError {}

struct Grid {
    bits: u32,
}
impl Grid {
    fn one(&self) -> Integer {
        Integer::from(1) << self.bits
    }
    fn mul_up(&self, a: &Integer, b: &Integer) -> Integer {
        ceil_div(&(a.clone() * b), &self.one())
    }
    fn coefficient(
        &self,
        ratio: ExactRatio,
        bits: u32,
    ) -> Result<(Integer, Integer, Integer, Integer), BudgetError> {
        if ratio.denominator == 0 {
            return Err(BudgetError::InvalidRatio);
        }
        let denominator = Integer::from(ratio.denominator);
        let numerator = Integer::from(ratio.numerator);
        let scaled = numerator.clone() << bits;
        let quantized = round_div_integer(&scaled, &denominator, RoundingMode::NearestTiesToEven)
            .expect("non-zero positive denominator");
        let ideal = ceil_div(&(numerator.abs() << self.bits), &denominator);
        let quantized_magnitude = quantized.clone().abs() << (self.bits - bits);
        let difference = (quantized.clone() * &denominator - scaled).abs();
        let error = ceil_div(&(difference << (self.bits - bits)), &denominator);
        Ok((ideal, quantized_magnitude, error, quantized))
    }
}

fn ceil_div(numerator: &Integer, denominator: &Integer) -> Integer {
    debug_assert!(numerator >= &0 && denominator > &0);
    let (quotient, remainder) = numerator.clone().div_rem(denominator.clone());
    quotient + u32::from(remainder != 0)
}

/// Source PCM is exact and bounded by one. Each bound-grid operation rounds
/// UP; the bound grid has 64 more fractional bits than the widest signal or
/// coefficient. No sum of rational denominators is formed.
pub fn evaluate_numerical_budget(
    stages: &[NumericalStage],
    signal_fractional_bits: u32,
    effect_fractional_bits: u32,
) -> Result<NumericalBudget, BudgetError> {
    evaluate_numerical_budget_with_fir_l1(
        stages,
        signal_fractional_bits,
        effect_fractional_bits,
        None,
    )
}

/// As above, optionally tightening the gain of exactly one FIR stage using the
/// executing bank's exact L1. Error allowances/reference semantics are unchanged.
/// An absent bound retains the non-materializing `2*taps` Q2.C range bound.
pub fn evaluate_numerical_budget_with_fir_l1(
    stages: &[NumericalStage],
    signal_fractional_bits: u32,
    effect_fractional_bits: u32,
    fir_l1: Option<&FirL1Bound>,
) -> Result<NumericalBudget, BudgetError> {
    if fir_l1.is_some()
        && stages
            .iter()
            .filter(|stage| {
                matches!(
                    stage,
                    NumericalStage::DesignedFir { .. } | NumericalStage::CertifiedFir { .. }
                )
            })
            .count()
            != 1
    {
        return Err(BudgetError::InvalidFir);
    }
    let coefficient_bits = stages
        .iter()
        .fold(effect_fractional_bits, |bits, stage| match stage {
            NumericalStage::DesignedFir {
                coefficient_fractional_bits,
                ..
            }
            | NumericalStage::CertifiedFir {
                coefficient_fractional_bits,
                ..
            } => bits.max(*coefficient_fractional_bits),
            _ => bits,
        });
    let maximum = signal_fractional_bits.max(coefficient_bits);
    if maximum > MAX_BITS {
        return Err(BudgetError::InvalidPrecision);
    }
    let grid = Grid { bits: maximum + 64 };
    let epsilon = Integer::from(1) << (grid.bits - signal_fractional_bits - 1);
    let mut current = StageBound {
        name: "input PCM".to_owned(),
        ideal_peak_raw: grid.one(),
        rounding_error_raw: Integer::new(),
        effect_coefficient_error_raw: Integer::new(),
        fir_coefficient_error_raw: Integer::new(),
    };
    let mut maximum_peak = grid.one();
    let mut reports = Vec::with_capacity(stages.len());
    for stage in stages {
        match stage {
            NumericalStage::Linear { name, rows } => {
                if rows.is_empty() || rows.iter().any(Vec::is_empty) {
                    return Err(BudgetError::EmptyLinearStage);
                }
                let mut ideal_l1 = Integer::new();
                let mut quantized_l1 = Integer::new();
                let mut error_l1 = Integer::new();
                for row in rows {
                    let (mut ideal, mut quantized, mut error) =
                        (Integer::new(), Integer::new(), Integer::new());
                    for coefficient in row {
                        let (a, b, c, _) =
                            grid.coefficient(*coefficient, effect_fractional_bits)?;
                        ideal += a;
                        quantized += b;
                        error += c;
                    }
                    ideal_l1 = ideal_l1.max(ideal);
                    quantized_l1 = quantized_l1.max(quantized);
                    error_l1 = error_l1.max(error);
                }
                current.rounding_error_raw =
                    grid.mul_up(&quantized_l1, &current.rounding_error_raw) + &epsilon;
                current.effect_coefficient_error_raw = grid
                    .mul_up(&quantized_l1, &current.effect_coefficient_error_raw)
                    + grid.mul_up(&error_l1, &current.ideal_peak_raw);
                current.fir_coefficient_error_raw =
                    grid.mul_up(&quantized_l1, &current.fir_coefficient_error_raw);
                current.ideal_peak_raw = grid.mul_up(&ideal_l1, &current.ideal_peak_raw);
                current.name.clone_from(name);
            }
            NumericalStage::DcBlock { radius } => {
                if radius.numerator < 0 || radius.numerator >= i128::from(radius.denominator) {
                    return Err(BudgetError::UnstableDcPole);
                }
                let (_, _, delta, raw) = grid.coefficient(*radius, effect_fractional_bits)?;
                let gap = (Integer::from(1) << effect_fractional_bits) - raw;
                if gap <= 0 {
                    return Err(BudgetError::UnstableDcPole);
                }
                // H_R has l1=2 for 0<=R<1. Coefficient and rounding forcing
                // enter through 1/(1-Rq), not through H_R.
                let forcing = grid.mul_up(&delta, &(current.ideal_peak_raw.clone() * 2));
                current.effect_coefficient_error_raw *= 2;
                current.effect_coefficient_error_raw +=
                    ceil_div(&(forcing << effect_fractional_bits), &gap);
                current.rounding_error_raw *= 2;
                current.rounding_error_raw +=
                    ceil_div(&(epsilon.clone() << effect_fractional_bits), &gap);
                current.fir_coefficient_error_raw *= 2;
                current.ideal_peak_raw *= 2;
                current.name = "DC removal".to_owned();
            }
            NumericalStage::DesignedFir {
                taps_per_phase,
                coefficient_fractional_bits,
            }
            | NumericalStage::CertifiedFir {
                taps_per_phase,
                coefficient_fractional_bits,
                ..
            } => {
                if *taps_per_phase == 0 {
                    return Err(BudgetError::InvalidFir);
                }
                let quantized_l1 = if let Some(bound) = fir_l1 {
                    if bound.taps_per_phase != *taps_per_phase
                        || bound.coefficient_fractional_bits != *coefficient_fractional_bits
                    {
                        return Err(BudgetError::InvalidFir);
                    }
                    bound.maximum_raw.clone() << (grid.bits - coefficient_fractional_bits)
                } else {
                    (Integer::from(*taps_per_phase) * 2) << grid.bits
                };
                // All but the DC-corrected tap are nearest-quantized; its
                // error is minus their sum. Thus sum |delta h| <= (T-1)2^-C
                // relative to an EXACT-DC-corrected unquantized reference.
                let delta = match stage {
                    NumericalStage::CertifiedFir {
                        error_numerator,
                        error_denominator,
                        ..
                    } => {
                        if error_numerator < &0
                            || error_denominator <= &0
                            || error_numerator.significant_bits() > MAX_BITS + 64
                            || error_denominator.significant_bits() > MAX_BITS + 64
                        {
                            return Err(BudgetError::InvalidFir);
                        }
                        ceil_div(&(error_numerator.clone() << grid.bits), error_denominator)
                    }
                    _ => {
                        Integer::from(taps_per_phase - 1)
                            << (grid.bits - coefficient_fractional_bits)
                    }
                };
                current.fir_coefficient_error_raw = grid
                    .mul_up(&quantized_l1, &current.fir_coefficient_error_raw)
                    + grid.mul_up(&delta, &current.ideal_peak_raw);
                current.effect_coefficient_error_raw =
                    grid.mul_up(&quantized_l1, &current.effect_coefficient_error_raw);
                current.rounding_error_raw =
                    grid.mul_up(&quantized_l1, &current.rounding_error_raw) + &epsilon;
                current.ideal_peak_raw =
                    grid.mul_up(&(quantized_l1 + delta), &current.ideal_peak_raw);
                current.name = if matches!(stage, NumericalStage::CertifiedFir { .. }) {
                    "certified polyphase FIR"
                } else {
                    "polyphase FIR"
                }
                .to_owned();
            }
            NumericalStage::Normalize => {
                current.rounding_error_raw += &epsilon;
                current.name = "normalization (same selected scale)".to_owned();
            }
        }
        maximum_peak = maximum_peak.max(current.ideal_peak_raw.clone() + current.total_error_raw());
        if maximum_peak.significant_bits() > grid.bits + MAX_BITS {
            return Err(BudgetError::InvalidPrecision);
        }
        reports.push(current.clone());
    }
    Ok(NumericalBudget {
        bound_fractional_bits: grid.bits,
        stages: reports,
        maximum_signal_peak_raw: maximum_peak,
        final_bound: current,
    })
}

#[cfg(test)]
mod tests;
