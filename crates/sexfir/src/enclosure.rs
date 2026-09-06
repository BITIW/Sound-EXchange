//! Directed-rounding enclosures for mathematical normalized Kaiser FIR phases.
//!
//! These are design-validation primitives, not a replacement designer or a
//! complete rate-transform response certificate. Actual integer coefficients
//! can be compared against the normalized phase enclosures. See
//! `docs/kaiser-design-enclosures.md` for the series and remainder proof.
use rug::float::Round;
use rug::ops::{AddAssignRound, DivAssignRound, MulAssignRound, SubAssignRound};
use rug::{Float, Rational};
use std::fmt;

mod fir;
pub use fir::{CoefficientErrorBounds, PhaseEnclosure, PhaseLimits, kaiser_phase, sin_pi_rational};
mod bank;
pub use bank::{
    BankCertificate, BankLimits, CertificationError, certify_kaiser, certify_kaiser_big,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// MPFR endpoint precision, not a promised relative enclosure width.
    pub precision_bits: u32,
    /// Maximum nonconstant terms per I0 evaluation.
    pub max_series_terms: u32,
    /// Maximum bits in either numerator or denominator of an input rational.
    pub max_input_bits: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            precision_bits: 192,
            max_series_terms: 16384,
            max_input_bits: 65536,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidLimits,
    InputTooWide,
    NegativeArgument,
    ZeroRadius,
    NonFinite,
    SeriesBudget { maximum: u32 },
    InvalidSpecification(String),
    PhaseOutOfRange,
    TapBudget,
    TotalSeriesBudget,
    UnresolvedPhaseGain,
    CoefficientLayout,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits => f.write_str("invalid enclosure precision/resource limits"),
            Self::InputTooWide => f.write_str("enclosure rational input exceeds its bit budget"),
            Self::NegativeArgument => {
                f.write_str("I0 squared argument and Kaiser beta must be nonnegative")
            }
            Self::ZeroRadius => f.write_str("Kaiser enclosure radius must be positive"),
            Self::NonFinite => f.write_str("enclosure calculation exceeded the finite MPFR range"),
            Self::SeriesBudget { maximum } => write!(
                f,
                "I0 enclosure did not finish within {maximum} series terms"
            ),
            Self::InvalidSpecification(message) => write!(f,"invalid enclosed FIR specification: {message}"),
            Self::PhaseOutOfRange => f.write_str("enclosed FIR phase is outside the rational phase count"),
            Self::TapBudget => f.write_str("enclosed FIR exceeds its tap budget"),
            Self::TotalSeriesBudget => f.write_str("enclosed FIR exhausted its total I0 series budget"),
            Self::UnresolvedPhaseGain => f.write_str("phase gain interval contains zero; increase enclosure precision or reject the design"),
            Self::CoefficientLayout => f.write_str("quantized coefficients do not match the enclosed phase"),
        }
    }
}
impl std::error::Error for Error {}

impl Limits {
    fn validate(self) -> Result<(), Error> {
        if !(16..=1_048_576).contains(&self.precision_bits)
            || !(1..=1_048_576).contains(&self.max_series_terms)
            || !(1..=1_048_576).contains(&self.max_input_bits)
        {
            return Err(Error::InvalidLimits);
        }
        Ok(())
    }

    fn check_input(self, value: &Rational) -> Result<(), Error> {
        if value.numer().significant_bits() > self.max_input_bits
            || value.denom().significant_bits() > self.max_input_bits
        {
            return Err(Error::InputTooWide);
        }
        Ok(())
    }
}

/// Finite dyadic bounds with private endpoints to preserve their invariants.
#[derive(Clone, Debug, PartialEq)]
pub struct Enclosure {
    lower: Float,
    upper: Float,
}

impl Enclosure {
    pub fn lower(&self) -> &Float {
        &self.lower
    }
    pub fn upper(&self) -> &Float {
        &self.upper
    }

    fn exact(precision: u32, value: u32) -> Self {
        Self {
            lower: Float::with_val(precision, value),
            upper: Float::with_val(precision, value),
        }
    }

    /// Exact width of the returned dyadic interval, not a rounded display.
    pub fn width(&self) -> Rational {
        self.upper.to_rational().unwrap() - self.lower.to_rational().unwrap()
    }

    /// Absolute error enclosure for a finite MPFR approximation to this value.
    /// The bound includes endpoint rounding and the infinite series remainder.
    pub fn absolute_error_bound(&self, approximation: &Float) -> Result<Rational, Error> {
        let value = approximation.to_rational().ok_or(Error::NonFinite)?;
        Ok(self.rational_error_bound(&value))
    }

    /// Exact error bound for a rational approximation, including fixed-point
    /// coefficients represented as raw_integer / 2^fractional_bits.
    pub fn rational_error_bound(&self, value: &Rational) -> Rational {
        let lower = (value.clone() - self.lower.to_rational().unwrap()).abs();
        let upper = (value.clone() - self.upper.to_rational().unwrap()).abs();
        lower.max(upper)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BesselEnclosure {
    pub value: Enclosure,
    /// Number of included nonconstant terms. Zero for I0(0) = 1.
    pub series_terms: u32,
    /// Bounds omitted positive terms only; total uncertainty is value.width().
    pub tail_upper_bound: Float,
}

/// Enclose I0(sqrt(s)) for an exact rational s >= 0, without rounding a square
/// root. I0(sqrt(s)) = sum (s/4)^k / (k!)^2. All terms are nonnegative.
pub fn i0_squared(s: &Rational, limits: Limits) -> Result<BesselEnclosure, Error> {
    limits.validate()?;
    limits.check_input(s)?;
    if s < &0 {
        return Err(Error::NegativeArgument);
    }
    let p = limits.precision_bits;
    if s == &0 {
        return Ok(BesselEnclosure {
            value: Enclosure::exact(p, 1),
            series_terms: 0,
            tail_upper_bound: Float::with_val(p, 0),
        });
    }
    let mut factor_lo = Float::with_val_round(p, s, Round::Down).0;
    factor_lo.div_assign_round(4, Round::Down);
    let mut factor_hi = Float::with_val_round(p, s, Round::Up).0;
    factor_hi.div_assign_round(4, Round::Up);
    if !factor_hi.is_finite() {
        return Err(Error::NonFinite);
    }
    let (mut term_lo, mut term_hi) = (Float::with_val(p, 1), Float::with_val(p, 1));
    let (mut sum_lo, mut sum_hi) = (Float::with_val(p, 1), Float::with_val(p, 1));
    let tail_scale = rug::Integer::from(1) << p;
    for k in 1..=limits.max_series_terms {
        let divisor = u64::from(k) * u64::from(k);
        term_lo.mul_assign_round(&factor_lo, Round::Down);
        term_lo.div_assign_round(divisor, Round::Down);
        term_hi.mul_assign_round(&factor_hi, Round::Up);
        term_hi.div_assign_round(divisor, Round::Up);
        sum_lo.add_assign_round(&term_lo, Round::Down);
        sum_hi.add_assign_round(&term_hi, Round::Up);
        if !sum_hi.is_finite() {
            return Err(Error::NonFinite);
        }

        // After t_(k+1), all consecutive tail ratios are <= a/(k+2)^2.
        let mut ratio_hi = factor_hi.clone();
        ratio_hi.div_assign_round((u64::from(k) + 2).pow(2), Round::Up);
        if ratio_hi >= 1 {
            continue;
        }
        let mut denominator_lo = Float::with_val(p, 1);
        denominator_lo.sub_assign_round(&ratio_hi, Round::Down);
        if denominator_lo <= 0 {
            return Err(Error::NonFinite);
        }
        let mut tail_hi = term_hi.clone();
        tail_hi.mul_assign_round(&factor_hi, Round::Up);
        tail_hi.div_assign_round((u64::from(k) + 1).pow(2), Round::Up);
        tail_hi.div_assign_round(&denominator_lo, Round::Up);
        if !tail_hi.is_finite() {
            return Err(Error::NonFinite);
        }
        let mut threshold = sum_lo.clone();
        // An explicit directed division, rather than an unchecked exponent
        // shift, also preserves the stopping criterion at MPFR range limits.
        threshold.div_assign_round(&tail_scale, Round::Down);
        if threshold <= 0 {
            return Err(Error::NonFinite);
        }
        if tail_hi <= threshold {
            sum_hi.add_assign_round(&tail_hi, Round::Up);
            if !sum_hi.is_finite() {
                return Err(Error::NonFinite);
            }
            return Ok(BesselEnclosure {
                value: Enclosure {
                    lower: sum_lo,
                    upper: sum_hi,
                },
                series_terms: k,
                tail_upper_bound: tail_hi,
            });
        }
    }
    Err(Error::SeriesBudget {
        maximum: limits.max_series_terms,
    })
}

#[cfg(test)]
mod tests;

/// Reuses a certified I0(beta) denominator across a window's exact coordinates.
/// It encloses the mathematical Kaiser window, NOT sinc, DC normalization,
/// coefficient quantization, or a completed FIR's total design error.
#[derive(Clone, Debug)]
pub struct KaiserWindow {
    radius: u64,
    beta_squared: Rational,
    denominator: BesselEnclosure,
    limits: Limits,
}

impl KaiserWindow {
    pub fn new(radius: u64, beta: &Rational, limits: Limits) -> Result<Self, Error> {
        limits.validate()?;
        limits.check_input(beta)?;
        if radius == 0 {
            return Err(Error::ZeroRadius);
        }
        if beta < &0 {
            return Err(Error::NegativeArgument);
        }
        let beta_squared = Rational::from(beta * beta);
        let denominator = i0_squared(&beta_squared, limits)?;
        Ok(Self {
            radius,
            beta_squared,
            denominator,
            limits,
        })
    }

    pub fn denominator(&self) -> &BesselEnclosure {
        &self.denominator
    }

    pub fn at(&self, x: &Rational) -> Result<Enclosure, Error> {
        self.at_counted(x, self.limits.max_series_terms)
            .map(|(value, _)| value)
    }

    fn at_counted(&self, x: &Rational, remaining: u32) -> Result<(Enclosure, u32), Error> {
        self.limits.check_input(x)?;
        let p = self.limits.precision_bits;
        if x.clone().abs() > self.radius {
            return Ok((Enclosure::exact(p, 0), 0));
        }
        if x == &0 || self.beta_squared == 0 {
            return Ok((Enclosure::exact(p, 1), 0));
        }
        let mut normalized_squared = Rational::from(x * x);
        normalized_squared /= self.radius;
        normalized_squared /= self.radius;
        let mut argument_squared = Rational::from(1) - normalized_squared;
        argument_squared *= &self.beta_squared;
        if argument_squared != 0 && remaining == 0 {
            return Err(Error::TotalSeriesBudget);
        }
        let limits = Limits {
            max_series_terms: remaining.max(1).min(self.limits.max_series_terms),
            ..self.limits
        };
        let numerator = i0_squared(&argument_squared, limits)?;
        let mut lower = numerator.value.lower;
        lower.div_assign_round(&self.denominator.value.upper, Round::Down);
        let mut upper = numerator.value.upper;
        upper.div_assign_round(&self.denominator.value.lower, Round::Up);
        if !lower.is_finite() || !upper.is_finite() {
            return Err(Error::NonFinite);
        }
        // Monotonicity of I0 on nonnegative inputs gives the exact range [0,1].
        if upper > 1 {
            upper = Float::with_val(p, 1);
        }
        Ok((Enclosure { lower, upper }, numerator.series_terms))
    }
}
