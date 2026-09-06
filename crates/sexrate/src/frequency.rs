//! Exact positive frequencies, parsed without binary floating-point conversion.
use super::{RateRatio, gcd};
use rug::{Integer, ops::Pow};
use std::{fmt, str::FromStr};

/// Reduced positive Hz; each component fits the scheduler's u64 contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SampleRate {
    numerator: u64,
    denominator: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseRateError {
    InvalidSyntax,
    Zero,
    TooLong,
    OutOfRange,
    RatioOverflow,
    NonIntegralContainerRate,
    ContainerRateOverflow,
}
impl fmt::Display for ParseRateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidSyntax => "invalid exact rate: use positive Hz, a fraction, or decimal/scientific notation with optional k/M suffix",
            Self::Zero => "sample rate and its denominator must be non-zero",
            Self::TooLong => "sample-rate text exceeds 256 bytes",
            Self::OutOfRange => "reduced sample rate exceeds the positive u64 rational range",
            Self::RatioOverflow => "exact output/input rate ratio exceeds the u64 scheduler range",
            Self::NonIntegralContainerRate => "output container adapters require integer Hz; fractional Hz is supported by plan/analyze and the resampler API, never silently rounded",
            Self::ContainerRateOverflow => "output sample rate exceeds the container's u32 Hz range",
        })
    }
}
impl std::error::Error for ParseRateError {}

impl SampleRate {
    pub fn from_hz(hz: u64) -> Result<Self, ParseRateError> {
        Self::from_fraction(hz, 1)
    }
    pub fn from_fraction(numerator: u64, denominator: u64) -> Result<Self, ParseRateError> {
        if numerator == 0 || denominator == 0 {
            return Err(ParseRateError::Zero);
        }
        let common = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / common,
            denominator: denominator / common,
        })
    }
    pub const fn numerator(self) -> u64 {
        self.numerator
    }
    pub const fn denominator(self) -> u64 {
        self.denominator
    }
    /// Compute output/input with cross-cancellation BEFORE checked products.
    pub fn ratio_from(self, input: Self) -> Result<RateRatio, ParseRateError> {
        let a = gcd(self.numerator, input.numerator);
        let b = gcd(input.denominator, self.denominator);
        let up = (self.numerator / a)
            .checked_mul(input.denominator / b)
            .ok_or(ParseRateError::RatioOverflow)?;
        let down = (self.denominator / b)
            .checked_mul(input.numerator / a)
            .ok_or(ParseRateError::RatioOverflow)?;
        Ok(RateRatio::from_fraction(up, down).expect("positive checked products"))
    }
    pub fn container_hz(self) -> Result<u32, ParseRateError> {
        if self.denominator != 1 {
            return Err(ParseRateError::NonIntegralContainerRate);
        }
        u32::try_from(self.numerator).map_err(|_| ParseRateError::ContainerRateOverflow)
    }
}
impl fmt::Display for SampleRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.denominator == 1 {
            write!(f, "{}", self.numerator)
        } else {
            write!(f, "{}/{}", self.numerator, self.denominator)
        }
    }
}

fn digits(text: &str) -> Result<Integer, ParseRateError> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseRateError::InvalidSyntax);
    }
    Integer::from_str_radix(text, 10).map_err(|_| ParseRateError::InvalidSyntax)
}
fn reduced(mut numerator: Integer, mut denominator: Integer) -> Result<SampleRate, ParseRateError> {
    if numerator == 0 || denominator == 0 {
        return Err(ParseRateError::Zero);
    }
    let common = numerator.clone().gcd(&denominator);
    numerator /= &common;
    denominator /= common;
    SampleRate::from_fraction(
        numerator.to_u64().ok_or(ParseRateError::OutOfRange)?,
        denominator.to_u64().ok_or(ParseRateError::OutOfRange)?,
    )
}

impl FromStr for SampleRate {
    type Err = ParseRateError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() > 256 {
            return Err(ParseRateError::TooLong);
        }
        if let Some((n, d)) = text.split_once('/') {
            return reduced(digits(n)?, digits(d)?);
        }
        let (text, suffix) =
            if let Some(body) = text.strip_suffix('k').or_else(|| text.strip_suffix('K')) {
                (body, 3_i64)
            } else if let Some(body) = text.strip_suffix('M') {
                (body, 6)
            } else {
                (text, 0)
            };
        let (mantissa, exponent) = if let Some((m, e)) = text.split_once(['e', 'E']) {
            let unsigned = e.strip_prefix(['+', '-']).unwrap_or(e);
            if unsigned.is_empty() || !unsigned.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ParseRateError::InvalidSyntax);
            }
            (
                m,
                e.parse::<i32>().map_err(|_| ParseRateError::OutOfRange)? as i64,
            )
        } else {
            (text, 0)
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if whole.is_empty() && fraction.is_empty() {
            return Err(ParseRateError::InvalidSyntax);
        }
        if !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
        {
            return Err(ParseRateError::InvalidSyntax);
        }
        let combined = format!("{whole}{fraction}");
        let nonzero = combined.trim_start_matches('0');
        if nonzero.is_empty() {
            return Err(ParseRateError::Zero);
        }
        let significant = nonzero.trim_end_matches('0');
        let trailing = nonzero.len() - significant.len();
        let power = exponent + suffix - fraction.len() as i64 + trailing as i64;
        // The input text cap bounds cancellation. Powers beyond this cannot
        // yield a positive u64/u64 value with a nonzero <=256-digit mantissa.
        if !(-256..=256).contains(&power) {
            return Err(ParseRateError::OutOfRange);
        }
        let mut numerator = digits(significant)?;
        let mut denominator = Integer::from(1);
        let factor = Integer::from(10).pow(power.unsigned_abs() as u32);
        if power >= 0 {
            numerator *= factor;
        } else {
            denominator = factor;
        }
        reduced(numerator, denominator)
    }
}

#[cfg(test)]
mod tests;
