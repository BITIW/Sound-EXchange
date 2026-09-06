use super::*;
use crate::optimized;
use sexplan::{PlanRequest, QualityPreset, plan_precision, reserve_full_range_fir_accumulator};
use sexrate::RateRatio;

fn initial() -> PrecisionPlan {
    let mut plan = plan_precision(PlanRequest {
        ratio: RateRatio::from_fraction(2, 3).unwrap(),
        preset: QualityPreset::Fast,
        error_floor: None,
        working_precision_bits: None,
    })
    .unwrap();
    plan.taps_per_phase = 7;
    plan.coefficient_count = 14;
    plan.coefficient_fractional_bits = 8;
    plan.working_precision_bits = 128;
    plan.planned_accumulator_bits = 128;
    plan
}

fn ill_conditioned() -> PrecisionPlan {
    let mut plan = initial();
    plan.taps_per_phase = 129;
    plan.coefficient_count = 258;
    plan.coefficient_fractional_bits = 64;
    plan.planned_accumulator_bits = 256;
    plan.rolloff = sexplan::Rational::new(3, 5).unwrap();
    plan.transition_width_of_lower_nyquist = sexplan::Rational::new(4, 5).unwrap();
    plan
}

#[test]
fn solver_precision_recovers_real_ls_without_changing_geometry_or_cache_charges() {
    let initial = ill_conditioned();
    let request = Request {
        certificate: Some(certificate::Limits::default()),
        ..request(Designer::GlobalLeastSquares)
    };
    let directory = std::env::temp_dir().join(format!(
        "sex-solver-retry-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let mut previous = None;
    for run in 0..2 {
        let result = qualify(
            initial.clone(),
            request,
            Ok,
            |plan| {
                let spec = request.designer.optimized_spec(plan)?.unwrap();
                let (bank, status) = crate::cache::design_optimized_big_cached(
                    &directory,
                    &spec,
                    request.designer_limits,
                )?;
                assert_eq!(
                    status,
                    if run == 0 {
                        crate::cache::CacheStatus::DesignedAndStored
                    } else {
                        crate::cache::CacheStatus::Hit
                    }
                );
                Ok(bank)
            },
            assess_optimized,
        )
        .unwrap();
        assert_eq!(result.solver_failures.len(), 1);
        let failure = &result.solver_failures[0];
        assert_eq!(failure.plan, initial);
        assert_eq!(
            failure.error,
            crate::DesignError::SingularLeastSquaresSystem { pivot: 110 }
        );
        assert_eq!(failure.next_working_bits, Some(256));
        let mut expected = initial.clone();
        expected.working_precision_bits = 256;
        assert_eq!(result.plan, expected);
        assert_eq!(
            result.design.report().quantization.coefficient_sha256,
            "572cec99f26cfb24ad1aa1c71770e8db45f2ff14e632e883d1fa52e4c070f707"
        );
        assert_eq!(result.attempts.len(), 1);
        assert!(result.attempts[0].assessment.response_passed());
        let first_work = request
            .designer
            .optimized_spec(&initial)
            .unwrap()
            .unwrap()
            .preflight(request.designer_limits)
            .unwrap();
        assert_eq!(
            result.charged_design_terms,
            first_work.terms + result.design.report().work.terms
        );
        assert_eq!(result.charged_coefficients, 516);
        if let Some((bank, charged)) = previous {
            assert_eq!(result.design, bank);
            assert_eq!(result.charged_design_terms, charged);
        }
        previous = Some((result.design, result.charged_design_terms));
    }
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    let mut reference_plan = initial;
    reference_plan.working_precision_bits = 512;
    let reference = optimized::design(
        &request
            .designer
            .optimized_spec(&reference_plan)
            .unwrap()
            .unwrap(),
        request.designer_limits,
    )
    .unwrap();
    let (recovered, _) = previous.unwrap();
    for phase in 0..2 {
        assert_eq!(recovered.bank().phase(phase), reference.bank().phase(phase));
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn solver_precision_respects_pinned_precision_and_all_retry_budgets() {
    for mode in 0..5 {
        let initial = ill_conditioned();
        let mut request = request(Designer::GlobalLeastSquares);
        match mode {
            0 => request.explicit_working_bits = Some(128),
            1 => request.limits.max_precision_bits = 128,
            2 => request.designer_limits.max_precision_bits = 128,
            3 => request.limits.max_attempts = 1,
            _ => {
                request.max_design_terms = request
                    .designer
                    .optimized_spec(&initial)
                    .unwrap()
                    .unwrap()
                    .preflight(request.designer_limits)
                    .unwrap()
                    .terms
            }
        }
        let mut calls = 0;
        let error = qualify(
            initial,
            request,
            Ok,
            |plan| {
                calls += 1;
                Ok(optimized::design(
                    &request.designer.optimized_spec(plan)?.unwrap(),
                    request.designer_limits,
                )?)
            },
            assess_optimized,
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(error.attempts.is_empty());
        assert_eq!(error.solver_failures.len(), 1);
        assert_eq!(
            error.solver_failures[0].next_working_bits,
            if mode == 4 { Some(256) } else { None }
        );
        if mode == 4 {
            assert!(error.reason.contains("cumulative designer work"));
        }
    }
}

#[test]
fn solver_precision_classifies_typed_causes_not_error_text() {
    use crate::DesignError;
    for cause in [
        DesignError::SingularLeastSquaresSystem { pivot: 3 },
        DesignError::SingularRemezSystem { pivot: 3 },
        DesignError::RemezExtremaUnavailable {
            required: 4,
            found: 3,
        },
        DesignError::RemezDidNotConverge { iterations: 64 },
    ] {
        assert_eq!(numerical_solver_error(&cause), Some(cause.clone()));
        let wrapped = crate::cache::CacheError::Optimized(optimized::Error::Design(cause.clone()));
        assert_eq!(numerical_solver_error(&wrapped), Some(cause));
    }
    for cause in [
        DesignError::RemezExtremaBudgetExceeded {
            found: 5,
            maximum: 4,
        },
        DesignError::CoefficientOutOfRange { phase: 0, tap: 1 },
        DesignError::WorkingPrecisionTooLow(32),
    ] {
        assert_eq!(numerical_solver_error(&cause), None);
    }
    assert_eq!(
        numerical_solver_error(&crate::cache::CacheError::Corrupt("singular least squares")),
        None
    );
    assert_eq!(
        numerical_solver_error(&std::io::Error::other("Remez did not converge")),
        None
    );
}

fn request(designer: Designer) -> Request {
    Request {
        designer,
        grid: 17,
        harmonics: Some(harmonics::Limits::default()),
        designer_limits: optimized::Limits {
            max_work: 4_000_000_000,
            ..Default::default()
        },
        max_design_terms: 8_000_000_000,
        ..Default::default()
    }
}

#[test]
fn optimized_quality_feedback_repairs_precision_and_shape_without_changing_target() {
    for designer in [Designer::GlobalLeastSquares, Designer::Equiripple] {
        let initial = initial();
        let request = request(designer);
        let result = qualify(
            initial.clone(),
            request,
            |plan| Ok(reserve_full_range_fir_accumulator(&plan, None)?),
            |plan| {
                Ok(optimized::design(
                    &designer.optimized_spec(plan)?.unwrap(),
                    request.designer_limits,
                )?)
            },
            assess_optimized,
        )
        .unwrap();
        assert!(
            result
                .attempts
                .iter()
                .any(|attempt| attempt.next_change == Some(Change::CoefficientPrecision))
        );
        assert!(
            result
                .attempts
                .iter()
                .any(|attempt| attempt.next_change == Some(Change::FilterShape))
        );
        assert!(result.plan.taps_per_phase > 7);
        assert!(result.plan.coefficient_fractional_bits > 8);
        assert_eq!(result.plan.target_error_floor, initial.target_error_floor);
        assert_eq!(result.plan.rolloff, initial.rolloff);
        let last = result.attempts.last().unwrap();
        assert!(last.assessment.coefficient_budget_passed && last.assessment.response_passed());
        let work: u64 = result
            .attempts
            .iter()
            .map(|attempt| {
                designer
                    .optimized_spec(&attempt.plan)
                    .unwrap()
                    .unwrap()
                    .preflight(request.designer_limits)
                    .unwrap()
                    .terms
            })
            .sum();
        assert_eq!(work, result.charged_design_terms);
        assert!(work <= request.max_design_terms);
    }
}

#[test]
fn optimized_quality_feedback_bounds_solver_work_and_rejects_wrong_banks() {
    for designer in [Designer::GlobalLeastSquares, Designer::Equiripple] {
        let initial = initial();
        let mut request = request(designer);
        request.max_design_terms = designer
            .optimized_spec(&initial)
            .unwrap()
            .unwrap()
            .preflight(request.designer_limits)
            .unwrap()
            .terms;
        let mut materialized = 0;
        let error = qualify(
            initial.clone(),
            request,
            Ok,
            |plan| {
                materialized += 1;
                Ok(optimized::design(
                    &designer.optimized_spec(plan)?.unwrap(),
                    request.designer_limits,
                )?)
            },
            assess_optimized,
        )
        .unwrap_err();
        assert_eq!(materialized, 1);
        assert_eq!(error.attempts.len(), 1);
        assert!(error.reason.contains("cumulative designer work"));
        let bank = optimized::design(
            &designer.optimized_spec(&initial).unwrap().unwrap(),
            request.designer_limits,
        )
        .unwrap();
        let other = if designer == Designer::Equiripple {
            Designer::GlobalLeastSquares
        } else {
            Designer::Equiripple
        };
        assert!(
            assess_optimized(
                &bank,
                &initial,
                Request {
                    designer: other,
                    ..request
                }
            )
            .is_err()
        );
    }
}
