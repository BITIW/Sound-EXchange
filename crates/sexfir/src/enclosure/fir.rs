//! Exact-rational geometry and signed interval propagation for Kaiser phases.
use super::*;
use crate::KaiserSpec;
use rug::{Integer, float::Constant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhaseLimits {
    pub arithmetic: Limits,
    pub max_taps: usize,
    /// Across denominator and every numerator I0 evaluation in this phase.
    pub max_total_series_terms: u64,
}
impl Default for PhaseLimits {
    fn default() -> Self {
        Self {
            arithmetic: Limits::default(),
            max_taps: 4097,
            max_total_series_terms: 1_000_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhaseEnclosure {
    coefficients: Vec<Enclosure>,
    unnormalized_gain: Enclosure,
    series_terms: u64,
    limits: PhaseLimits,
}
impl PhaseEnclosure {
    pub fn coefficients(&self) -> &[Enclosure] {
        &self.coefficients
    }
    pub fn unnormalized_gain(&self) -> &Enclosure {
        &self.unnormalized_gain
    }
    pub fn series_terms(&self) -> u64 {
        self.series_terms
    }

    /// Compare the ACTUAL final integer bank, including any DC correction,
    /// against the mathematical normalized Kaiser phase. This jointly bounds
    /// design arithmetic and quantization, not either contribution in isolation.
    pub fn compare_quantized<'a>(
        &self,
        raw: impl IntoIterator<Item = &'a Integer>,
        fractional_bits: u32,
    ) -> Result<CoefficientErrorBounds, Error> {
        if fractional_bits > self.limits.arithmetic.max_input_bits {
            return Err(Error::InputTooWide);
        }
        let denominator = Integer::from(1) << fractional_bits;
        let mut raw = raw.into_iter();
        let mut maximum = Rational::from(0);
        let mut l1 = Rational::from(0);
        let mut l1_lower = Rational::from(0);
        for enclosure in &self.coefficients {
            let integer = raw.next().ok_or(Error::CoefficientLayout)?;
            if integer.significant_bits() > self.limits.arithmetic.max_input_bits {
                return Err(Error::InputTooWide);
            }
            let value = Rational::from((integer.clone(), denominator.clone()));
            let error = enclosure.rational_error_bound(&value);
            let lower = enclosure.lower.to_rational().expect("finite endpoint");
            let upper = enclosure.upper.to_rational().expect("finite endpoint");
            l1_lower += if value < lower {
                lower - &value
            } else if value > upper {
                value - upper
            } else {
                Rational::from(0)
            };
            if error > maximum {
                maximum = error.clone();
            }
            l1 += error;
        }
        if raw.next().is_some() {
            return Err(Error::CoefficientLayout);
        }
        Ok(CoefficientErrorBounds {
            max_absolute: maximum,
            phase_l1: l1,
            phase_l1_lower: l1_lower,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoefficientErrorBounds {
    pub max_absolute: Rational,
    /// Also bounds the complex response difference of this one phase at ANY
    /// frequency, by the triangle inequality. Not a complete-image certificate.
    pub phase_l1: Rational,
    /// Proved LOWER bound on this phase's true coefficient L1 difference.
    /// Distinguishes a genuinely inaccurate bank from a wide enclosure.
    pub phase_l1_lower: Rational,
}

fn from_rational(value: &Rational, p: u32) -> Result<Enclosure, Error> {
    checked(
        Float::with_val_round(p, value, Round::Down).0,
        Float::with_val_round(p, value, Round::Up).0,
    )
}
fn checked(lower: Float, upper: Float) -> Result<Enclosure, Error> {
    if !lower.is_finite() || !upper.is_finite() || lower > upper {
        return Err(Error::NonFinite);
    }
    Ok(Enclosure { lower, upper })
}
fn multiply(a: &Enclosure, b: &Enclosure, p: u32) -> Result<Enclosure, Error> {
    let mut lower = Float::with_val(p, rug::float::Special::Infinity);
    let mut upper = Float::with_val(p, rug::float::Special::NegInfinity);
    for x in [&a.lower, &a.upper] {
        for y in [&b.lower, &b.upper] {
            let lo = Float::with_val_round(p, x * y, Round::Down).0;
            let hi = Float::with_val_round(p, x * y, Round::Up).0;
            if lo < lower {
                lower = lo;
            }
            if hi > upper {
                upper = hi;
            }
        }
    }
    checked(lower, upper)
}
fn divide(a: &Enclosure, b: &Enclosure, p: u32) -> Result<Enclosure, Error> {
    if b.lower <= 0 && b.upper >= 0 {
        return Err(Error::UnresolvedPhaseGain);
    }
    let mut lower = Float::with_val(p, rug::float::Special::Infinity);
    let mut upper = Float::with_val(p, rug::float::Special::NegInfinity);
    for x in [&a.lower, &a.upper] {
        for y in [&b.lower, &b.upper] {
            let lo = Float::with_val_round(p, x / y, Round::Down).0;
            let hi = Float::with_val_round(p, x / y, Round::Up).0;
            if lo < lower {
                lower = lo;
            }
            if hi > upper {
                upper = hi;
            }
        }
    }
    checked(lower, upper)
}

/// Enclose sin(pi*x) without first rounding x's integer part or quadrant.
pub fn sin_pi_rational(x: &Rational, limits: Limits) -> Result<Enclosure, Error> {
    limits.validate()?;
    limits.check_input(x)?;
    let p = limits.precision_bits;
    let magnitude = x.clone().abs();
    let whole = Integer::from(magnitude.numer() / magnitude.denom());
    let mut fraction = magnitude;
    fraction -= &whole;
    if fraction == 0 {
        return Ok(Enclosure::exact(p, 0));
    }
    // sin(pi*f) = cos(pi*|f-1/2|), with the cosine argument in [0,1/2].
    // cos(pi*t) is decreasing there: round its input in the opposite direction.
    let coordinate = (fraction - Rational::from((1, 2))).abs();
    let mut lower = Float::with_val_round(p, &coordinate, Round::Up).0;
    let mut upper = Float::with_val_round(p, &coordinate, Round::Down).0;
    lower.cos_pi_round(Round::Down);
    upper.cos_pi_round(Round::Up);
    if whole.is_odd() ^ (x < &0) {
        (lower, upper) = (-upper, -lower);
    }
    checked(lower, upper)
}

/// Enclose the exact mathematical, unit-DC Kaiser phase. The actual existing
/// designer is not invoked. Its finite-precision arithmetic and final integer
/// coefficients can subsequently be compared against these independent bounds.
pub fn kaiser_phase(
    spec: &KaiserSpec,
    phase: u64,
    limits: PhaseLimits,
) -> Result<PhaseEnclosure, Error> {
    spec.validate()
        .map_err(|error| Error::InvalidSpecification(error.to_string()))?;
    limits.arithmetic.validate()?;
    if limits.max_taps == 0
        || limits.max_taps > 1_048_576
        || limits.max_total_series_terms == 0
        || limits.max_total_series_terms > 1_000_000_000
    {
        return Err(Error::InvalidLimits);
    }
    if phase >= spec.ratio.up() {
        return Err(Error::PhaseOutOfRange);
    }
    if spec.taps_per_phase > limits.max_taps {
        return Err(Error::TapBudget);
    }
    let p = limits.arithmetic.precision_bits;
    let radius = (spec.taps_per_phase / 2) as u64;
    let beta = Rational::from((spec.beta.numerator(), spec.beta.denominator()));
    let window = KaiserWindow::new(
        radius,
        &beta,
        Limits {
            max_series_terms: limits
                .max_total_series_terms
                .min(u64::from(limits.arithmetic.max_series_terms))
                as u32,
            ..limits.arithmetic
        },
    )?;
    let mut work = u64::from(window.denominator().series_terms);
    let mut twice_cutoff = Rational::from((spec.rolloff.numerator(), spec.rolloff.denominator()));
    if spec.ratio.up() < spec.ratio.down() {
        twice_cutoff *= Rational::from((spec.ratio.up(), spec.ratio.down()));
    }
    limits.arithmetic.check_input(&twice_cutoff)?;
    let pi = checked(
        Float::with_val_round(p, Constant::Pi, Round::Down).0,
        Float::with_val_round(p, Constant::Pi, Round::Up).0,
    )?;
    let mut raw = Vec::with_capacity(spec.taps_per_phase);
    let mut gain = Enclosure::exact(p, 0);
    for tap in 0..spec.taps_per_phase {
        let numerator =
            Integer::from(tap) * spec.ratio.up() + phase - Integer::from(radius) * spec.ratio.up();
        let x = Rational::from((numerator, spec.ratio.up()));
        let remaining = limits.max_total_series_terms - work;
        let (weight, terms) = window.at_counted(
            &x,
            remaining.min(u64::from(limits.arithmetic.max_series_terms)) as u32,
        )?;
        work += u64::from(terms);
        let value = if weight.upper == 0 {
            Enclosure::exact(p, 0)
        } else if x == 0 {
            from_rational(&twice_cutoff, p)?
        } else {
            let distance = x.abs();
            let argument = Rational::from(&twice_cutoff * &distance);
            let sine = sin_pi_rational(&argument, limits.arithmetic)?;
            // 2*c*sinc(2*c*x) = sin(pi*2*c*|x|)/(pi*|x|), preserving sign.
            let denominator = multiply(&pi, &from_rational(&distance, p)?, p)?;
            multiply(&divide(&sine, &denominator, p)?, &weight, p)?
        };
        gain.lower.add_assign_round(&value.lower, Round::Down);
        gain.upper.add_assign_round(&value.upper, Round::Up);
        raw.push(value);
    }
    let gain = checked(gain.lower, gain.upper)?;
    if gain.lower <= 0 && gain.upper >= 0 {
        return Err(Error::UnresolvedPhaseGain);
    }
    let coefficients = raw
        .iter()
        .map(|value| divide(value, &gain, p))
        .collect::<Result<_, _>>()?;
    Ok(PhaseEnclosure {
        coefficients,
        unnormalized_gain: gain,
        series_terms: work,
        limits,
    })
}

#[cfg(test)]
mod tests;
