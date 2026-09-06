//! All-phase certification bound to an immutable, actual Kaiser coefficient bank.
use super::*;
use crate::{DesignedBigFilter, DesignedFilter, KaiserSpec};
use rug::Integer;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BankLimits {
    pub phase: PhaseLimits,
    pub max_phases: u64,
    pub max_coefficients: u64,
    /// Aggregate nonconstant I0 terms, across ALL phases and refinement passes.
    pub max_total_series_terms: u64,
    /// Refine endpoint precision by doubling, capped exactly at this value.
    pub max_precision_bits: u32,
    /// Optional exact target for the maximum joint per-phase L1 coefficient error.
    /// Refines enclosure arithmetic, not the supplied bank or its designer.
    pub target_max_phase_l1: Option<Rational>,
}
impl Default for BankLimits {
    fn default() -> Self {
        Self {
            phase: PhaseLimits::default(),
            max_phases: 4096,
            max_coefficients: 1_048_576,
            max_total_series_terms: 10_000_000,
            max_precision_bits: 4096,
            target_max_phase_l1: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CertificationError {
    Enclosure(Error),
    InvalidLimits,
    BankLayout,
    IdentityMismatch,
    PhaseBudget,
    CoefficientBudget,
    TotalSeriesBudget,
    NegativeInputBound,
    TargetNotMet {
        precision_bits: u32,
        max_phase_l1: Rational,
    },
    TargetViolated {
        precision_bits: u32,
        lower_bound: Rational,
        upper_bound: Rational,
        target: Rational,
        total_series_terms: u64,
    },
}
impl From<Error> for CertificationError {
    fn from(value: Error) -> Self {
        Self::Enclosure(value)
    }
}
impl fmt::Display for CertificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enclosure(error) => error.fmt(f),
            Self::InvalidLimits => f.write_str("invalid bank certification limits or target"),
            Self::BankLayout => {
                f.write_str("coefficient bank layout differs from its Kaiser specification")
            }
            Self::IdentityMismatch => {
                f.write_str("actual coefficient identity differs from the stored design report")
            }
            Self::PhaseBudget => f.write_str("bank certification exceeds its phase budget"),
            Self::CoefficientBudget => {
                f.write_str("bank certification exceeds its coefficient budget")
            }
            Self::TotalSeriesBudget => {
                f.write_str("bank certification exhausted its aggregate I0 work budget")
            }
            Self::NegativeInputBound => f.write_str("input peak bound must be nonnegative"),
            Self::TargetNotMet { precision_bits, .. } => write!(
                f,
                "joint coefficient error target not met: inconclusive enclosure at {precision_bits} bits; increase enclosure precision; bank was not changed"
            ),
            Self::TargetViolated { precision_bits, .. } => write!(
                f,
                "joint coefficient error target not met: proved violation at {precision_bits} enclosure bits; bank redesign required"
            ),
        }
    }
}
impl std::error::Error for CertificationError {}

/// Only produced after checking EVERY phase at the returned endpoint precision.
/// This bounds actual coefficients against the mathematical normalized Kaiser
/// bank, not against an ideal brick-wall filter or final rounded PCM.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BankCertificate {
    spec: KaiserSpec,
    coefficient_sha256: String,
    coefficient_fractional_bits: u32,
    phase_errors: Vec<CoefficientErrorBounds>,
    max_absolute: Rational,
    max_phase_l1: Rational,
    precision_bits: u32,
    passes: u32,
    total_series_terms: u64,
    limits: BankLimits,
}
impl BankCertificate {
    pub fn spec(&self) -> &KaiserSpec {
        &self.spec
    }
    pub fn coefficient_sha256(&self) -> &str {
        &self.coefficient_sha256
    }
    pub fn coefficient_fractional_bits(&self) -> u32 {
        self.coefficient_fractional_bits
    }
    /// In phase order 0..ratio.up(), with no omitted phases.
    pub fn phase_errors(&self) -> &[CoefficientErrorBounds] {
        &self.phase_errors
    }
    pub fn max_absolute(&self) -> &Rational {
        &self.max_absolute
    }
    pub fn max_phase_l1(&self) -> &Rational {
        &self.max_phase_l1
    }
    pub fn precision_bits(&self) -> u32 {
        self.precision_bits
    }
    pub fn passes(&self) -> u32 {
        self.passes
    }
    pub fn total_series_terms(&self) -> u64 {
        self.total_series_terms
    }
    pub fn limits(&self) -> &BankLimits {
        &self.limits
    }

    /// Exact, pre-rounding FIR output error for |input sample| <= peak.
    /// Includes design/quantization/DC correction; excludes signal arithmetic
    /// rounding, ideal-filter approximation, other effects, dither and clipping.
    pub fn signal_error_bound(&self, peak: &Rational) -> Result<Rational, CertificationError> {
        self.limits.phase.arithmetic.check_input(peak)?;
        if peak < &0 {
            return Err(CertificationError::NegativeInputBound);
        }
        Ok(Rational::from(peak * &self.max_phase_l1))
    }
}

fn validate(
    spec: &KaiserSpec,
    phases: usize,
    taps: usize,
    bits: u32,
    limits: &BankLimits,
) -> Result<(), CertificationError> {
    spec.validate()
        .map_err(|error| Error::InvalidSpecification(error.to_string()))?;
    limits.phase.arithmetic.validate()?;
    if limits.max_phases == 0
        || limits.max_phases > 1_048_576
        || limits.max_coefficients == 0
        || limits.max_coefficients > 1_073_741_824
        || limits.max_total_series_terms == 0
        || limits.max_total_series_terms > 1_000_000_000
        || limits.max_precision_bits < limits.phase.arithmetic.precision_bits
        || limits.max_precision_bits > 1_048_576
        || limits.phase.max_taps == 0
        || limits.phase.max_taps > 1_048_576
        || limits.phase.max_total_series_terms == 0
        || limits.phase.max_total_series_terms > 1_000_000_000
    {
        return Err(CertificationError::InvalidLimits);
    }
    if let Some(target) = &limits.target_max_phase_l1 {
        limits.phase.arithmetic.check_input(target)?;
        if target < &0 {
            return Err(CertificationError::InvalidLimits);
        }
    }
    if phases as u64 != spec.ratio.up() || taps != spec.taps_per_phase {
        return Err(CertificationError::BankLayout);
    }
    if spec.ratio.up() > limits.max_phases {
        return Err(CertificationError::PhaseBudget);
    }
    if taps > limits.phase.max_taps {
        return Err(Error::TapBudget.into());
    }
    if bits > limits.phase.arithmetic.max_input_bits {
        return Err(Error::InputTooWide.into());
    }
    if spec
        .ratio
        .up()
        .checked_mul(taps as u64)
        .is_none_or(|n| n > limits.max_coefficients)
    {
        return Err(CertificationError::CoefficientBudget);
    }
    Ok(())
}

/// Certify a native Q2.62 bank. Identity is recomputed from actual coefficients.
pub fn certify_kaiser(
    designed: &DesignedFilter,
    limits: BankLimits,
) -> Result<BankCertificate, CertificationError> {
    let bank = designed.bank();
    validate(
        designed.spec(),
        bank.phase_count(),
        bank.taps_per_phase(),
        62,
        &limits,
    )?;
    let identity = crate::coefficient_identity(
        designed.spec(),
        designed.input_delay_frames(),
        (0..designed.ratio().up())
            .flat_map(|phase| bank.phase(phase).expect("validated immutable bank layout")),
    );
    if identity != designed.report().coefficient_sha256 {
        return Err(CertificationError::IdentityMismatch);
    }
    certify(designed.spec(), identity, 62, limits, |phase| {
        bank.phase(phase)
            .expect("validated immutable bank layout")
            .iter()
            .map(|q| Integer::from(q.raw()))
            .collect()
    })
}

/// Certify an arbitrary-width bank without copying the entire coefficient table.
pub fn certify_kaiser_big(
    designed: &DesignedBigFilter,
    limits: BankLimits,
) -> Result<BankCertificate, CertificationError> {
    let bank = designed.bank();
    let bits = designed.spec().coefficient_fractional_bits;
    validate(
        &designed.spec().core,
        bank.phase_count(),
        bank.taps_per_phase(),
        bits,
        &limits,
    )?;
    if bank.coefficient_format().fractional_bits() != bits {
        return Err(CertificationError::BankLayout);
    }
    let identity = crate::coefficient_identity_big(
        designed.spec(),
        designed.input_delay_frames(),
        (0..designed.ratio().up())
            .flat_map(|phase| bank.phase(phase).expect("validated immutable bank layout")),
    );
    if identity != designed.report().coefficient_sha256 {
        return Err(CertificationError::IdentityMismatch);
    }
    certify(&designed.spec().core, identity, bits, limits, |phase| {
        bank.phase(phase)
            .expect("validated immutable bank layout")
            .iter()
            .map(|q| q.raw().clone())
            .collect()
    })
}

fn certify(
    spec: &KaiserSpec,
    identity: String,
    bits: u32,
    limits: BankLimits,
    raw_phase: impl Fn(u64) -> Vec<Integer>,
) -> Result<BankCertificate, CertificationError> {
    let mut p = limits.phase.arithmetic.precision_bits;
    let mut passes = 0;
    let mut work = 0;
    loop {
        passes += 1;
        let mut phase_errors = Vec::with_capacity(spec.ratio.up() as usize);
        let mut max_absolute = Rational::from(0);
        let mut max_phase_l1 = Rational::from(0);
        let mut min_phase_l1 = Rational::from(0);
        for phase in 0..spec.ratio.up() {
            let remaining = limits.max_total_series_terms - work;
            if remaining == 0 {
                return Err(CertificationError::TotalSeriesBudget);
            }
            let enclosed = kaiser_phase(
                spec,
                phase,
                PhaseLimits {
                    arithmetic: Limits {
                        precision_bits: p,
                        ..limits.phase.arithmetic
                    },
                    max_total_series_terms: remaining.min(limits.phase.max_total_series_terms),
                    ..limits.phase
                },
            )?;
            work += enclosed.series_terms();
            let error = enclosed.compare_quantized(&raw_phase(phase), bits)?;
            max_absolute = max_absolute.max(error.max_absolute.clone());
            max_phase_l1 = max_phase_l1.max(error.phase_l1.clone());
            min_phase_l1 = min_phase_l1.max(error.phase_l1_lower.clone());
            phase_errors.push(error);
        }
        if let Some(target) = &limits.target_max_phase_l1
            && min_phase_l1 > *target
        {
            return Err(CertificationError::TargetViolated {
                precision_bits: p,
                lower_bound: min_phase_l1,
                upper_bound: max_phase_l1,
                target: target.clone(),
                total_series_terms: work,
            });
        }
        if limits
            .target_max_phase_l1
            .as_ref()
            .is_none_or(|target| max_phase_l1 <= *target)
        {
            return Ok(BankCertificate {
                spec: spec.clone(),
                coefficient_sha256: identity,
                coefficient_fractional_bits: bits,
                phase_errors,
                max_absolute,
                max_phase_l1,
                precision_bits: p,
                passes,
                total_series_terms: work,
                limits,
            });
        }
        if p == limits.max_precision_bits {
            return Err(CertificationError::TargetNotMet {
                precision_bits: p,
                max_phase_l1,
            });
        }
        p = (p * 2).min(limits.max_precision_bits);
    }
}

#[cfg(test)]
mod tests;
