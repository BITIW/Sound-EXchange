//! Feedback-based quality refinement, separate from numerical signal planning.
//! Generic hooks let a caller replan its whole chain and materialize cached banks.
//! All accepted results retain the requested target and frequency geometry.
use crate::{
    DesignedBigFilter, DesignedFilter, ResponseAnalysis, ResponseCompliance, certificate, harmonics,
};
use sexplan::{
    PrecisionPlan, raise_coefficient_precision, required_amplitude_bits, strengthen_filter,
};
use std::fmt;

#[cfg(test)]
mod optimized_tests;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Designer {
    #[default]
    Kaiser,
    Rectangular,
    Hann,
    Blackman,
    DolphChebyshev,
    GlobalLeastSquares,
    Equiripple,
}
impl Designer {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kaiser => "kaiser",
            Self::Rectangular => "rectangular",
            Self::Hann => "hann",
            Self::Blackman => "blackman",
            Self::DolphChebyshev => "dolph-chebyshev",
            Self::GlobalLeastSquares => "global-ls",
            Self::Equiripple => "remez",
        }
    }
    pub fn window(self, plan: &PrecisionPlan) -> Option<crate::WindowFunction> {
        use crate::WindowFunction as W;
        match self {
            Self::Kaiser | Self::GlobalLeastSquares | Self::Equiripple => None,
            Self::Rectangular => Some(W::Rectangular),
            Self::Hann => Some(W::Hann),
            Self::Blackman => Some(W::Blackman),
            Self::DolphChebyshev => Some(W::DolphChebyshev {
                attenuation_db: plan.design_attenuation_db,
            }),
        }
    }
    pub fn preflight(self, plan: &PrecisionPlan) -> Result<(), crate::DesignError> {
        if let Some(window) = self.window(plan) {
            crate::windowed::Spec::from_precision_plan(plan, window)?.preflight()?;
        }
        Ok(())
    }
    pub fn optimized_spec(
        self,
        plan: &PrecisionPlan,
    ) -> Result<Option<crate::optimized::Spec>, crate::DesignError> {
        match self {
            Self::GlobalLeastSquares => {
                crate::optimized::Spec::from_precision_plan(plan, false).map(Some)
            }
            Self::Equiripple => crate::optimized::Spec::from_precision_plan(plan, true).map(Some),
            _ => Ok(None),
        }
    }
}
impl std::str::FromStr for Designer {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "kaiser" => Ok(Self::Kaiser),
            "rectangular" => Ok(Self::Rectangular),
            "hann" => Ok(Self::Hann),
            "blackman" => Ok(Self::Blackman),
            "dolph-chebyshev" => Ok(Self::DolphChebyshev),
            "global-ls" => Ok(Self::GlobalLeastSquares),
            "remez" => Ok(Self::Equiripple),
            _ => Err(format!(
                "unknown FIR window: {s}; expected kaiser|rectangular|hann|blackman|dolph-chebyshev"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_attempts: u32,
    pub max_total_coefficients: u64,
    pub max_response_terms: u64,
    pub max_precision_bits: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            max_total_coefficients: 4_000_000,
            max_response_terms: 200_000_000,
            max_precision_bits: 16384,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub designer: Designer,
    pub designer_limits: crate::optimized::Limits,
    pub max_design_terms: u64,
    pub grid: u32,
    pub limits: Limits,
    pub explicit_working_bits: Option<u32>,
    pub certificate: Option<certificate::Limits>,
    pub harmonics: Option<harmonics::Limits>,
}
impl Default for Request {
    fn default() -> Self {
        Self {
            designer: Designer::Kaiser,
            designer_limits: crate::optimized::Limits::default(),
            max_design_terms: 200_000_000,
            grid: 65,
            limits: Limits::default(),
            explicit_working_bits: None,
            certificate: None,
            harmonics: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assessment {
    pub actual_coefficient_fractional_bits: u32,
    /// Exact bound (T-1)*2^-C <= 2^(-required_amplitude_bits-3), plus
    /// the designer's measured quantization gate. The exact reference has DC correction.
    pub coefficient_budget_passed: bool,
    pub sampled: ResponseCompliance,
    pub response: ResponseAnalysis,
    pub continuous: Option<certificate::Certificate>,
    pub harmonic: Option<harmonics::Report>,
}
impl Assessment {
    pub fn response_passed(&self) -> bool {
        let phase_pass = match self.continuous.as_ref().map(|proof| proof.outcome) {
            Some(certificate::Outcome::Certified) => true,
            Some(_) => false,
            None => {
                self.sampled.passband_deviation_meets_target && self.sampled.stopband_meets_target
            }
        };
        phase_pass
            && self
                .harmonic
                .as_ref()
                .is_none_or(harmonics::Report::meets_target)
    }
}

fn coefficient_budget(plan: &PrecisionPlan, taps: usize, bits: u32) -> bool {
    coefficient_budget_for(plan.target_error_floor, taps, bits)
}

pub(crate) fn coefficient_budget_for(target: sexplan::ErrorFloor, taps: usize, bits: u32) -> bool {
    let error_bits = required_amplitude_bits(target) + 3;
    let remaining = bits.saturating_sub(error_bits);
    bits >= error_bits
        && (remaining >= usize::BITS || taps.saturating_sub(1) <= (1_usize << remaining))
}

pub fn assess_native(
    filter: &DesignedFilter,
    plan: &PrecisionPlan,
    request: Request,
) -> Result<Assessment, Box<dyn std::error::Error>> {
    if request.designer != Designer::Kaiser
        || *filter.spec() != crate::KaiserSpec::from_precision_plan(plan)?
    {
        return Err("materialized native filter does not match the requested plan".into());
    }
    let (response, sampled) =
        crate::analyze_quantized_response_against(filter, request.grid, plan.target_error_floor)?;
    let coefficient_budget_passed = coefficient_budget(plan, filter.bank().taps_per_phase(), 62)
        && sampled.coefficient_quantization_meets_target;
    let request = if coefficient_budget_passed {
        request
    } else {
        Request {
            certificate: None,
            harmonics: None,
            ..request
        }
    };
    Ok(Assessment {
        actual_coefficient_fractional_bits: 62,
        coefficient_budget_passed,
        sampled,
        response,
        continuous: request
            .certificate
            .map(|limits| certificate::certify(filter, plan.target_error_floor, limits))
            .transpose()?,
        harmonic: request
            .harmonics
            .map(|limits| harmonics::analyze(filter, request.grid, plan.target_error_floor, limits))
            .transpose()?,
    })
}

pub fn assess_big(
    filter: &DesignedBigFilter,
    plan: &PrecisionPlan,
    request: Request,
) -> Result<Assessment, Box<dyn std::error::Error>> {
    if request.designer != Designer::Kaiser
        || *filter.spec() != crate::BigKaiserSpec::from_precision_plan(plan)?
    {
        return Err("materialized bigint filter does not match the requested plan".into());
    }
    let (response, sampled) = crate::analyze_big_quantized_response_against(
        filter,
        request.grid,
        plan.target_error_floor,
    )?;
    let bits = filter.spec().coefficient_fractional_bits;
    let coefficient_budget_passed = coefficient_budget(plan, filter.bank().taps_per_phase(), bits)
        && sampled.coefficient_quantization_meets_target;
    let request = if coefficient_budget_passed {
        request
    } else {
        Request {
            certificate: None,
            harmonics: None,
            ..request
        }
    };
    Ok(Assessment {
        actual_coefficient_fractional_bits: bits,
        coefficient_budget_passed,
        sampled,
        response,
        continuous: request
            .certificate
            .map(|limits| certificate::certify_big(filter, plan.target_error_floor, limits))
            .transpose()?,
        harmonic: request
            .harmonics
            .map(|limits| {
                harmonics::analyze_big(filter, request.grid, plan.target_error_floor, limits)
            })
            .transpose()?,
    })
}

pub fn assess_windowed(
    filter: &crate::windowed::Designed,
    plan: &PrecisionPlan,
    request: Request,
) -> Result<Assessment, Box<dyn std::error::Error>> {
    let window = request
        .designer
        .window(plan)
        .ok_or("windowed bank requested for Kaiser plan")?;
    if *filter.spec() != crate::windowed::Spec::from_precision_plan(plan, window)? {
        return Err("materialized windowed filter does not match the requested plan".into());
    }
    let (response, sampled) =
        crate::windowed::analyze_against(filter, request.grid, plan.target_error_floor)?;
    let bits = filter.spec().coefficient_fractional_bits;
    let coefficient_budget_passed = coefficient_budget(plan, filter.bank().taps_per_phase(), bits)
        && sampled.coefficient_quantization_meets_target;
    let continuous = if coefficient_budget_passed {
        request
            .certificate
            .map(|limits| crate::windowed::certify(filter, plan.target_error_floor, limits))
            .transpose()?
    } else {
        None
    };
    let harmonic = if coefficient_budget_passed {
        request
            .harmonics
            .map(|limits| {
                crate::windowed::analyze_harmonics(
                    filter,
                    request.grid,
                    plan.target_error_floor,
                    limits,
                )
            })
            .transpose()?
    } else {
        None
    };
    Ok(Assessment {
        actual_coefficient_fractional_bits: bits,
        coefficient_budget_passed,
        sampled,
        response,
        continuous,
        harmonic,
    })
}

pub fn assess_optimized(
    filter: &crate::optimized::Designed,
    plan: &PrecisionPlan,
    request: Request,
) -> Result<Assessment, Box<dyn std::error::Error>> {
    if request.designer.optimized_spec(plan)?.as_ref() != Some(filter.spec()) {
        return Err("materialized optimized filter does not match the requested plan".into());
    }
    let (response, sampled) =
        crate::optimized::analyze_against(filter, request.grid, plan.target_error_floor)?;
    let coefficient_budget_passed =
        crate::optimized::coefficient_budget_passed(filter, plan.target_error_floor)?
            && sampled.coefficient_quantization_meets_target;
    let continuous = if coefficient_budget_passed {
        request
            .certificate
            .map(|limits| crate::optimized::certify(filter, plan.target_error_floor, limits))
            .transpose()?
    } else {
        None
    };
    let harmonic = if coefficient_budget_passed {
        request
            .harmonics
            .map(|limits| {
                crate::optimized::analyze_harmonics(
                    filter,
                    request.grid,
                    plan.target_error_floor,
                    limits,
                )
            })
            .transpose()?
    } else {
        None
    };
    Ok(Assessment {
        actual_coefficient_fractional_bits: filter.spec().coefficient_fractional_bits,
        coefficient_budget_passed,
        sampled,
        response,
        continuous,
        harmonic,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    CoefficientPrecision,
    FilterShape,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attempt {
    pub plan: PrecisionPlan,
    pub assessment: Assessment,
    pub next_change: Option<Change>,
}

#[derive(Debug)]
pub struct Qualified<T> {
    pub plan: PrecisionPlan,
    pub design: T,
    pub attempts: Vec<Attempt>,
    pub charged_coefficients: u64,
    pub charged_response_terms: u64,
    pub charged_design_terms: u64,
    pub solver_failures: Vec<SolverFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolverFailure {
    pub plan: PrecisionPlan,
    pub error: crate::DesignError,
    pub next_working_bits: Option<u32>,
}

fn numerical_solver_error(error: &(dyn std::error::Error + 'static)) -> Option<crate::DesignError> {
    let mut current = Some(error);
    for _ in 0..16 {
        let error = current?;
        if let Some(error) = error.downcast_ref::<crate::DesignError>() {
            return matches!(
                error,
                crate::DesignError::SingularLeastSquaresSystem { .. }
                    | crate::DesignError::SingularRemezSystem { .. }
                    | crate::DesignError::RemezExtremaUnavailable { .. }
                    | crate::DesignError::RemezDidNotConverge { .. }
            )
            .then(|| error.clone());
        }
        current = error.source();
    }
    None
}

#[derive(Debug)]
pub struct RefinementError {
    pub reason: String,
    pub attempts: Vec<Attempt>,
    pub solver_failures: Vec<SolverFailure>,
}
impl fmt::Display for RefinementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "quality refinement failed after {} assessed candidate(s): {}; no quality pass is claimed",
            self.attempts.len(),
            self.reason
        )
    }
}
impl std::error::Error for RefinementError {}

fn preflight(
    plan: &PrecisionPlan,
    request: Request,
    totals: &mut (u64, u64, u64),
) -> Result<(), String> {
    if plan.taps_per_phase < 3
        || plan.taps_per_phase.is_multiple_of(2)
        || plan.coefficient_count != u128::from(plan.taps_per_phase) * u128::from(plan.ratio.up())
        || plan.amplitude_error_bits != required_amplitude_bits(plan.target_error_floor)
    {
        return Err("inconsistent filter plan".to_owned());
    }
    if plan.working_precision_bits > request.limits.max_precision_bits {
        return Err(format!(
            "MPFR precision limit: requires {} bits, limit {}",
            plan.working_precision_bits, request.limits.max_precision_bits
        ));
    }
    if request
        .explicit_working_bits
        .is_some_and(|bits| bits != plan.working_precision_bits)
        || plan
            .coefficient_fractional_bits
            .checked_add(64)
            .is_none_or(|bits| bits > plan.working_precision_bits)
    {
        return Err("inconsistent or insufficient explicit/design precision".to_owned());
    }
    let coefficients =
        u64::try_from(plan.coefficient_count).map_err(|_| "coefficient count overflow")?;
    let new_coefficients = totals
        .0
        .checked_add(coefficients)
        .ok_or("coefficient work overflow")?;
    let terms = coefficients
        .checked_mul(u64::from(request.grid) * 2)
        .ok_or("response work overflow")?;
    let new_terms = totals
        .1
        .checked_add(terms)
        .ok_or("response work overflow")?;
    if new_coefficients > request.limits.max_total_coefficients {
        return Err(format!(
            "cumulative coefficient limit: requires {new_coefficients}, limit {} (this candidate: {coefficients})",
            request.limits.max_total_coefficients
        ));
    }
    if new_terms > request.limits.max_response_terms {
        return Err(format!(
            "cumulative response-term limit: requires {new_terms}, limit {} (this candidate: {terms})",
            request.limits.max_response_terms
        ));
    }
    if let Some(limits) = request.certificate
        && plan.taps_per_phase > limits.max_taps as u64
    {
        return Err("continuous certificate tap limit (inconclusive)".to_owned());
    }
    if let Some(limits) = request.harmonics {
        harmonics::preflight(
            harmonics::Spec {
                ratio: plan.ratio,
                rolloff: crate::Fraction::new(plan.rolloff.numerator(), plan.rolloff.denominator())
                    .map_err(|e| e.to_string())?,
                taps_per_phase: plan.taps_per_phase,
                fractional_bits: plan.coefficient_fractional_bits.max(62),
                input_delay_frames: (plan.taps_per_phase - 1) / 2,
                precision_bits: plan.working_precision_bits,
            },
            request.grid,
            limits,
        )
        .map_err(|e| e.to_string())?;
    }
    request
        .designer
        .preflight(plan)
        .map_err(|e| e.to_string())?;
    let mut design_terms = totals.2;
    if let Some(spec) = request
        .designer
        .optimized_spec(plan)
        .map_err(|error| error.to_string())?
    {
        let work = spec
            .preflight(request.designer_limits)
            .map_err(|error| error.to_string())?;
        design_terms = design_terms
            .checked_add(work.terms)
            .ok_or("designer work overflow")?;
        if design_terms > request.max_design_terms {
            return Err(format!(
                "cumulative designer work: requires {design_terms}, limit {}",
                request.max_design_terms
            ));
        }
    }
    *totals = (new_coefficients, new_terms, design_terms);
    Ok(())
}

/// `prepare` must return a consistent whole-chain-refined plan before design.
/// Cached candidates are charged identically to freshly computed candidates.
/// Supplemental certificate/harmonic budgets are per attempt; the two aggregate
/// budgets here bound coefficient materialization and ordinary response terms.
pub fn qualify<T>(
    initial: PrecisionPlan,
    request: Request,
    mut prepare: impl FnMut(PrecisionPlan) -> Result<PrecisionPlan, Box<dyn std::error::Error>>,
    mut materialize: impl FnMut(&PrecisionPlan) -> Result<T, Box<dyn std::error::Error>>,
    mut inspect: impl FnMut(
        &T,
        &PrecisionPlan,
        Request,
    ) -> Result<Assessment, Box<dyn std::error::Error>>,
) -> Result<Qualified<T>, RefinementError> {
    let mut history = Vec::new();
    let mut solver_failures = Vec::new();
    let mut totals = (0, 0, 0);
    let mut plan = initial.clone();
    let result = (|| -> Result<(PrecisionPlan, T), String> {
        if request.grid < 2
            || request.limits.max_attempts == 0
            || request.limits.max_attempts > 128
            || request.limits.max_total_coefficients == 0
            || request.limits.max_response_terms == 0
            || request.limits.max_precision_bits == 0
            || request.max_design_terms == 0
        {
            return Err("invalid refinement request".to_owned());
        }
        for candidate_index in 0..request.limits.max_attempts {
            let previous = plan.clone();
            plan = prepare(plan.clone()).map_err(|e| e.to_string())?;
            if plan.ratio != initial.ratio
                || plan.target_error_floor != initial.target_error_floor
                || plan.rolloff != initial.rolloff
                || plan.transition_width_of_lower_nyquist
                    != initial.transition_width_of_lower_nyquist
                || plan.preset != initial.preset
                || plan.dither != initial.dither
            {
                return Err("preparation changed the target or frequency geometry".to_owned());
            }
            if plan.coefficient_fractional_bits < previous.coefficient_fractional_bits
                || plan.working_precision_bits < previous.working_precision_bits
                || plan.planned_accumulator_bits < previous.planned_accumulator_bits
                || plan.taps_per_phase < previous.taps_per_phase
            {
                return Err("preparation undercut an existing precision or length floor".to_owned());
            }
            preflight(&plan, request, &mut totals)?;
            let design = match materialize(&plan) {
                Ok(design) => design,
                Err(error) => {
                    let optimized = matches!(
                        request.designer,
                        Designer::GlobalLeastSquares | Designer::Equiripple
                    );
                    if let Some(numerical) = optimized
                        .then(|| numerical_solver_error(error.as_ref()))
                        .flatten()
                    {
                        let next_working_bits =
                            plan.working_precision_bits.checked_mul(2).filter(|bits| {
                                request.explicit_working_bits.is_none()
                                    && candidate_index + 1 < request.limits.max_attempts
                                    && *bits <= request.limits.max_precision_bits
                                    && *bits <= request.designer_limits.max_precision_bits
                            });
                        solver_failures.push(SolverFailure {
                            plan: plan.clone(),
                            error: numerical,
                            next_working_bits,
                        });
                        if let Some(bits) = next_working_bits {
                            plan.working_precision_bits = bits;
                            continue;
                        }
                        let stop = if request.explicit_working_bits.is_some() {
                            "explicit working precision is fixed"
                        } else if candidate_index + 1 == request.limits.max_attempts {
                            "candidate limit exhausted"
                        } else {
                            "doubling working precision exceeds the precision limit"
                        };
                        return Err(format!("{error}; solver retry stopped: {stop}"));
                    }
                    return Err(error.to_string());
                }
            };
            let assessment = inspect(&design, &plan, request).map_err(|e| e.to_string())?;
            if assessment.sampled.target != plan.target_error_floor {
                return Err("assessment used a different acceptance target".to_owned());
            }
            if assessment.response.grid_points_per_band != request.grid
                || assessment.response.analyzed_phases as u64 != plan.ratio.up()
            {
                return Err("assessment used a different grid or omitted phases".to_owned());
            }
            if assessment.coefficient_budget_passed
                && (request.certificate.is_some() != assessment.continuous.is_some()
                    || request.harmonics.is_some() != assessment.harmonic.is_some())
            {
                return Err("assessment omitted a requested supplemental gate".to_owned());
            }
            let next = if !assessment.coefficient_budget_passed {
                Some(Change::CoefficientPrecision)
            } else if !assessment.response_passed() {
                Some(Change::FilterShape)
            } else {
                None
            };
            history.push(Attempt {
                plan: plan.clone(),
                assessment: assessment.clone(),
                next_change: next,
            });
            if let Some(certificate::Outcome::Inconclusive(reason)) =
                assessment.continuous.as_ref().map(|proof| proof.outcome)
            {
                history.last_mut().unwrap().next_change = None;
                return Err(format!(
                    "continuous certificate is inconclusive: {reason:?}"
                ));
            }
            let Some(change) = next else {
                return Ok((plan.clone(), design));
            };
            plan = match change {
                Change::CoefficientPrecision => raise_coefficient_precision(
                    &plan,
                    assessment
                        .actual_coefficient_fractional_bits
                        .checked_add(16)
                        .ok_or("coefficient precision overflow")?,
                    request.explicit_working_bits,
                ),
                Change::FilterShape if request.designer == Designer::Kaiser => {
                    strengthen_filter(&plan, request.explicit_working_bits)
                }
                Change::FilterShape => {
                    if request.designer == Designer::DolphChebyshev {
                        plan.design_attenuation_db = plan
                            .design_attenuation_db
                            .checked_add(12)
                            .ok_or("Dolph attenuation overflow")?;
                    }
                    sexplan::double_filter_length(&plan, request.explicit_working_bits)
                }
            }
            .map_err(|e| e.to_string())?;
        }
        Err("candidate limit exhausted".to_owned())
    })();
    match result {
        Ok((plan, design)) => Ok(Qualified {
            plan,
            design,
            attempts: history,
            charged_coefficients: totals.0,
            charged_response_terms: totals.1,
            charged_design_terms: totals.2,
            solver_failures,
        }),
        Err(reason) => Err(RefinementError {
            reason,
            attempts: history,
            solver_failures,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BigKaiserSpec, KaiserSpec};
    use sexplan::{BackendRequirement, PlanRequest, QualityPreset, Rational, plan_precision};
    use sexrate::RateRatio;

    fn coarse() -> PrecisionPlan {
        let mut plan = plan_precision(PlanRequest {
            ratio: RateRatio::from_fraction(2, 1).unwrap(),
            preset: QualityPreset::Fast,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        plan.taps_per_phase = 129;
        plan.coefficient_count = 258;
        plan.kaiser_beta = Rational::new(12, 1).unwrap();
        plan.coefficient_fractional_bits = 8;
        plan.planned_accumulator_bits = 192;
        plan.backend = BackendRequirement::WideInteger { minimum_bits: 192 };
        plan
    }
    fn big(plan: &PrecisionPlan) -> Result<DesignedBigFilter, Box<dyn std::error::Error>> {
        Ok(crate::design_kaiser_big(
            &BigKaiserSpec::from_precision_plan(plan)?,
        )?)
    }

    #[test]
    fn qualification_rejects_an_assessment_from_another_grid_or_partial_bank() {
        for wrong_grid in [true, false] {
            let error = qualify(
                coarse(),
                Request::default(),
                Ok,
                big,
                |filter, plan, request| {
                    let mut report = assess_big(filter, plan, request)?;
                    if wrong_grid {
                        report.response.grid_points_per_band += 1;
                    } else {
                        report.response.analyzed_phases -= 1;
                    }
                    Ok(report)
                },
            )
            .unwrap_err();
            assert!(error.reason.contains("different grid or omitted phases"));
        }
    }

    #[test]
    fn quantization_failure_increases_only_fractional_width_before_filter_shape() {
        let initial = coarse();
        let result = qualify(
            initial.clone(),
            Request {
                grid: 33,
                ..Request::default()
            },
            Ok,
            big,
            assess_big,
        )
        .unwrap();
        assert_eq!(result.attempts.len(), 2);
        assert_eq!(
            result.attempts[0].next_change,
            Some(Change::CoefficientPrecision)
        );
        assert!(!result.attempts[0].assessment.coefficient_budget_passed);
        assert_eq!(result.plan.coefficient_fractional_bits, 24);
        assert_eq!(result.plan.taps_per_phase, initial.taps_per_phase);
        assert_eq!(result.plan.kaiser_beta, initial.kaiser_beta);
        assert_eq!(result.plan.target_error_floor, initial.target_error_floor);
        assert!(result.attempts.last().unwrap().assessment.response_passed());
    }

    #[test]
    fn exact_legacy_sane_violation_grows_shape_and_is_recertified() {
        let mut initial = plan_precision(PlanRequest {
            ratio: RateRatio::from_fraction(1, 3).unwrap(),
            preset: QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        initial.design_attenuation_db = 110;
        initial.taps_per_phase = 769;
        initial.coefficient_count = 769;
        initial.kaiser_beta = Rational::new(10, 1).unwrap();
        let result = qualify(
            initial.clone(),
            Request {
                certificate: Some(certificate::Limits::default()),
                ..Request::default()
            },
            Ok,
            |plan| {
                Ok(crate::design_kaiser(&KaiserSpec::from_precision_plan(
                    plan,
                )?)?)
            },
            assess_native,
        )
        .unwrap();
        assert_eq!(result.attempts.len(), 2);
        assert_eq!(result.attempts[0].next_change, Some(Change::FilterShape));
        assert_eq!(
            result.attempts[0]
                .assessment
                .continuous
                .as_ref()
                .unwrap()
                .outcome,
            certificate::Outcome::Violated
        );
        assert_eq!(result.plan.taps_per_phase, 861);
        assert_eq!(result.plan.target_error_floor, initial.target_error_floor);
        assert_eq!(result.plan.rolloff, initial.rolloff);
        assert_eq!(result.plan.design_attenuation_db, 122);
        assert_eq!(
            result.attempts[1]
                .assessment
                .continuous
                .as_ref()
                .unwrap()
                .outcome,
            certificate::Outcome::Certified
        );
    }

    #[test]
    fn already_qualified_candidate_is_not_changed() {
        let plan = plan_precision(PlanRequest {
            ratio: RateRatio::from_fraction(1, 3).unwrap(),
            preset: QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        let direct =
            crate::design_kaiser(&KaiserSpec::from_precision_plan(&plan).unwrap()).unwrap();
        let result = qualify(
            plan.clone(),
            Request::default(),
            Ok,
            |plan| {
                Ok(crate::design_kaiser(&KaiserSpec::from_precision_plan(
                    plan,
                )?)?)
            },
            assess_native,
        )
        .unwrap();
        assert_eq!(result.plan, plan);
        assert_eq!(result.design, direct);
        assert_eq!(result.attempts.len(), 1);
    }

    #[test]
    fn inconclusive_proof_is_not_a_shape_failure_or_a_quality_pass() {
        let mut initial = coarse();
        initial.coefficient_fractional_bits = 62;
        let request = Request {
            certificate: Some(certificate::Limits {
                max_work: 1,
                ..certificate::Limits::default()
            }),
            ..Request::default()
        };
        let error = qualify(initial, request, Ok, big, assess_big).unwrap_err();
        assert!(error.reason.contains("inconclusive"));
        assert_eq!(error.attempts.len(), 1);
        assert!(error.attempts[0].next_change.is_none());
    }

    #[test]
    fn candidate_and_cumulative_limits_fail_closed() {
        let one = Request {
            limits: Limits {
                max_attempts: 1,
                ..Limits::default()
            },
            ..Request::default()
        };
        let error = qualify(coarse(), one, Ok, big, assess_big).unwrap_err();
        assert_eq!(error.attempts.len(), 1);
        assert!(error.reason.contains("candidate limit"));
        for limits in [
            Limits {
                max_response_terms: 1,
                ..Limits::default()
            },
            Limits {
                max_total_coefficients: 1,
                ..Limits::default()
            },
            Limits {
                max_precision_bits: 32,
                ..Limits::default()
            },
        ] {
            let error = qualify(
                coarse(),
                Request {
                    limits,
                    ..Request::default()
                },
                Ok,
                |_| -> Result<DesignedBigFilter, Box<dyn std::error::Error>> {
                    panic!("preflight must precede materialization")
                },
                assess_big,
            )
            .unwrap_err();
            assert!(error.attempts.is_empty());
        }
    }

    #[test]
    fn explicit_working_precision_is_not_silently_increased() {
        let mut plan = coarse();
        plan.taps_per_phase = 257;
        plan.coefficient_count = 514;
        plan.working_precision_bits = 96;
        let error = qualify(
            plan,
            Request {
                explicit_working_bits: Some(96),
                ..Request::default()
            },
            Ok,
            big,
            assess_big,
        )
        .unwrap_err();
        assert!(error.reason.contains("precision"), "{error}");
        assert!(
            error
                .attempts
                .iter()
                .all(|attempt| attempt.plan.working_precision_bits == 96)
        );
        assert!(
            error
                .attempts
                .iter()
                .all(|attempt| attempt.next_change == Some(Change::CoefficientPrecision))
        );
    }

    #[test]
    fn preparation_cannot_move_frequency_edges_to_fake_a_pass() {
        let error = qualify(
            coarse(),
            Request::default(),
            |mut plan| {
                plan.rolloff = Rational::new(1, 2).unwrap();
                Ok(plan)
            },
            big,
            assess_big,
        )
        .unwrap_err();
        assert!(error.reason.contains("frequency geometry"));
        assert!(error.attempts.is_empty());
    }

    #[test]
    fn exact_coefficient_budget_is_checked_at_its_boundary() {
        let p = coarse();
        let bits = required_amplitude_bits(p.target_error_floor) + 3;
        assert!(coefficient_budget(&p, 129, bits + 7));
        assert!(!coefficient_budget(&p, 130, bits + 7));
        assert!(!coefficient_budget(&p, 129, bits + 6));
        assert!(coefficient_budget(&p, usize::MAX, u32::MAX));
    }

    #[test]
    fn assessment_rejects_a_bank_materialized_for_different_filter_parameters() {
        let initial = coarse();
        let bank = big(&initial).unwrap();
        let mut wrong = initial;
        wrong.kaiser_beta = Rational::new(1, 1).unwrap();
        assert!(
            assess_big(&bank, &wrong, Request::default())
                .unwrap_err()
                .to_string()
                .contains("does not match")
        );
    }

    #[test]
    fn cached_candidates_have_distinct_keys_and_identical_refinement_decisions() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "sex-quality-refinement-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(path.clone());
        let mut runs = Vec::new();
        for expected in [
            crate::cache::CacheStatus::DesignedAndStored,
            crate::cache::CacheStatus::Hit,
        ] {
            let mut statuses = Vec::new();
            let result = qualify(
                coarse(),
                Request::default(),
                Ok,
                |plan| {
                    let (filter, status) = crate::cache::design_kaiser_big_cached(
                        &path,
                        &BigKaiserSpec::from_precision_plan(plan)?,
                    )?;
                    statuses.push(status);
                    Ok(filter)
                },
                assess_big,
            )
            .unwrap();
            assert_eq!(statuses, vec![expected; 2]);
            assert_eq!(std::fs::read_dir(&path).unwrap().count(), 2);
            runs.push(result);
        }
        assert_eq!(runs[0].plan, runs[1].plan);
        assert_eq!(runs[0].design, runs[1].design);
        assert_eq!(runs[0].attempts, runs[1].attempts);
        assert_eq!(
            runs[0].charged_response_terms,
            runs[1].charged_response_terms
        );
        assert_eq!(runs[0].charged_coefficients, runs[1].charged_coefficients);
    }
}
