use super::*;
use crate::{BigKaiserSpec, Fraction, design_kaiser, design_kaiser_big};
use sexq::RoundingMode;
use sexrate::RateRatio;

fn core(up: u64, down: u64) -> KaiserSpec {
    KaiserSpec {
        ratio: RateRatio::from_fraction(up, down).unwrap(),
        taps_per_phase: 17,
        rolloff: Fraction::new(9, 10).unwrap(),
        beta: Fraction::new(8, 1).unwrap(),
        working_precision_bits: 256,
        quantization_rounding: RoundingMode::NearestTiesToEven,
    }
}
fn big() -> DesignedBigFilter {
    design_kaiser_big(&BigKaiserSpec {
        core: core(3, 2),
        coefficient_fractional_bits: 96,
        accumulator_bits: 256,
    })
    .unwrap()
}
fn small_precision(p: u32) -> BankLimits {
    BankLimits {
        phase: PhaseLimits {
            arithmetic: Limits {
                precision_bits: p,
                ..Limits::default()
            },
            ..PhaseLimits::default()
        },
        ..BankLimits::default()
    }
}
fn binary_bound(bits: u32) -> Rational {
    Rational::from((Integer::from(1), Integer::from(1) << bits))
}

#[test]
fn native_certificate_covers_every_phase_and_matches_independent_phase_calls() {
    for (up, down) in [(1, 3), (3, 2), (7, 11)] {
        let designed = design_kaiser(&core(up, down)).unwrap();
        let certificate = certify_kaiser(&designed, BankLimits::default()).unwrap();
        assert_eq!(certificate.spec(), designed.spec());
        assert_eq!(
            certificate.coefficient_sha256(),
            designed.report().coefficient_sha256
        );
        assert_eq!(certificate.coefficient_fractional_bits(), 62);
        assert_eq!(certificate.phase_errors().len(), up as usize);
        let mut work = 0;
        let mut largest = Rational::from(0);
        for phase in 0..up {
            let enclosed = kaiser_phase(designed.spec(), phase, PhaseLimits::default()).unwrap();
            work += enclosed.series_terms();
            let raw: Vec<_> = designed
                .bank()
                .phase(phase)
                .unwrap()
                .iter()
                .map(|q| Integer::from(q.raw()))
                .collect();
            let error = enclosed.compare_quantized(&raw, 62).unwrap();
            assert_eq!(certificate.phase_errors()[phase as usize], error);
            largest = largest.max(error.phase_l1);
        }
        assert_eq!(certificate.total_series_terms(), work);
        assert_eq!(certificate.max_phase_l1(), &largest);
        assert_eq!(certificate.passes(), 1);
    }
}

#[test]
fn bigint_certificate_is_bound_to_actual_width_and_identity() {
    let designed = big();
    let before = designed.clone();
    let certificate = certify_kaiser_big(&designed, BankLimits::default()).unwrap();
    assert_eq!(certificate.coefficient_fractional_bits(), 96);
    assert_eq!(
        certificate.coefficient_sha256(),
        designed.report().coefficient_sha256
    );
    assert_eq!(certificate.phase_errors().len(), 3);
    assert!(certificate.max_phase_l1() < &binary_bound(80));
    assert!(certificate.max_absolute() <= certificate.max_phase_l1());
    assert_eq!(designed, before);
    let native =
        certify_kaiser(&design_kaiser(&core(3, 2)).unwrap(), BankLimits::default()).unwrap();
    assert_ne!(
        native.coefficient_sha256(),
        certificate.coefficient_sha256()
    );
}

#[test]
fn refinement_honors_non_power_of_two_ceiling_and_accounts_all_passes() {
    let designed = big();
    let limits = BankLimits {
        max_precision_bits: 100,
        target_max_phase_l1: Some(binary_bound(75)),
        ..small_precision(32)
    };
    let refined = certify_kaiser_big(&designed, limits.clone()).unwrap();
    assert_eq!(refined.precision_bits(), 100);
    assert_eq!(refined.passes(), 3);
    assert_eq!(refined.limits(), &limits);
    assert!(refined.max_phase_l1() <= &binary_bound(75));
    let mut work = 0;
    for p in [32, 64, 100] {
        work += certify_kaiser_big(&designed, small_precision(p))
            .unwrap()
            .total_series_terms();
    }
    assert_eq!(refined.total_series_terms(), work);
    let exact_budget = BankLimits {
        max_total_series_terms: work,
        ..limits.clone()
    };
    assert!(certify_kaiser_big(&designed, exact_budget).is_ok());
    assert!(
        certify_kaiser_big(
            &designed,
            BankLimits {
                max_total_series_terms: work - 1,
                ..limits
            }
        )
        .is_err()
    );
}

#[test]
fn target_failure_does_not_return_success_or_mutate_coefficients() {
    let designed = big();
    let before = designed.clone();
    let failure = certify_kaiser_big(
        &designed,
        BankLimits {
            max_precision_bits: 128,
            target_max_phase_l1: Some(Rational::from(0)),
            ..small_precision(32)
        },
    )
    .unwrap_err();
    match failure {
        CertificationError::TargetViolated {
            precision_bits,
            lower_bound,
            ..
        } => {
            assert!(precision_bits <= 128);
            assert!(lower_bound > 0);
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(designed, before);
}

#[test]
fn wide_enclosure_is_inconclusive_not_a_proved_bank_violation() {
    let limits = BankLimits {
        max_precision_bits: 16,
        target_max_phase_l1: Some(binary_bound(75)),
        ..small_precision(16)
    };
    assert!(matches!(
        certify_kaiser_big(&big(), limits),
        Err(CertificationError::TargetNotMet {
            precision_bits: 16,
            ..
        })
    ));
}

#[test]
fn proved_violation_reports_brackets_and_actual_work_without_refining_to_ceiling() {
    let bank = big();
    let target = binary_bound(150);
    let limits = BankLimits {
        target_max_phase_l1: Some(target.clone()),
        ..BankLimits::default()
    };
    let baseline = certify_kaiser_big(&bank, BankLimits::default()).unwrap();
    match certify_kaiser_big(&bank, limits).unwrap_err() {
        CertificationError::TargetViolated {
            precision_bits,
            lower_bound,
            upper_bound,
            target: actual,
            total_series_terms,
        } => {
            assert_eq!(actual, target);
            assert!(lower_bound > target && lower_bound <= upper_bound);
            assert_eq!(precision_bits, 192);
            assert_eq!(upper_bound, *baseline.max_phase_l1());
            assert_eq!(total_series_terms, baseline.total_series_terms());
        }
        error => panic!("expected proved violation, got {error}"),
    }
}

#[test]
fn phase_and_coefficient_budgets_reject_partial_coverage() {
    let designed = big();
    assert_eq!(
        certify_kaiser_big(
            &designed,
            BankLimits {
                max_phases: 2,
                ..BankLimits::default()
            }
        ),
        Err(CertificationError::PhaseBudget)
    );
    assert_eq!(
        certify_kaiser_big(
            &designed,
            BankLimits {
                max_coefficients: 50,
                ..BankLimits::default()
            }
        ),
        Err(CertificationError::CoefficientBudget)
    );
    let total = certify_kaiser_big(&designed, BankLimits::default())
        .unwrap()
        .total_series_terms();
    assert!(
        certify_kaiser_big(
            &designed,
            BankLimits {
                max_total_series_terms: total - 1,
                ..BankLimits::default()
            }
        )
        .is_err()
    );
    assert!(
        certify_kaiser_big(
            &designed,
            BankLimits {
                max_total_series_terms: total,
                ..BankLimits::default()
            }
        )
        .is_ok()
    );
}

#[test]
fn identity_is_recomputed_and_layout_is_checked_before_certification() {
    let mut native = design_kaiser(&core(3, 2)).unwrap();
    native.report.coefficient_sha256 = "0".repeat(64);
    assert_eq!(
        certify_kaiser(&native, BankLimits::default()),
        Err(CertificationError::IdentityMismatch)
    );
    let mut designed = big();
    designed.report.coefficient_sha256 = "0".repeat(64);
    assert_eq!(
        certify_kaiser_big(&designed, BankLimits::default()),
        Err(CertificationError::IdentityMismatch)
    );
    designed.spec.core.ratio = RateRatio::from_fraction(2, 3).unwrap();
    assert_eq!(
        certify_kaiser_big(&designed, BankLimits::default()),
        Err(CertificationError::BankLayout)
    );
}

#[test]
fn pre_rounding_signal_error_uses_peak_bound_without_float() {
    let certificate = certify_kaiser_big(&big(), BankLimits::default()).unwrap();
    assert_eq!(
        certificate.signal_error_bound(&Rational::from(0)).unwrap(),
        0
    );
    assert_eq!(
        certificate
            .signal_error_bound(&Rational::from((7, 3)))
            .unwrap(),
        certificate.max_phase_l1().clone() * Rational::from((7, 3))
    );
    assert_eq!(
        certificate.signal_error_bound(&Rational::from(-1)),
        Err(CertificationError::NegativeInputBound)
    );
    assert!(
        certificate
            .signal_error_bound(&Rational::from(Integer::from(1) << 65536))
            .is_err()
    );
}

#[test]
fn invalid_limits_and_oversized_targets_fail_before_work() {
    let designed = big();
    for limits in [
        BankLimits {
            max_phases: 0,
            ..BankLimits::default()
        },
        BankLimits {
            max_total_series_terms: 0,
            ..BankLimits::default()
        },
        BankLimits {
            max_precision_bits: 16,
            ..BankLimits::default()
        },
        BankLimits {
            max_precision_bits: 1_048_577,
            ..BankLimits::default()
        },
        BankLimits {
            target_max_phase_l1: Some(Rational::from(-1)),
            ..BankLimits::default()
        },
    ] {
        assert_eq!(
            certify_kaiser_big(&designed, limits),
            Err(CertificationError::InvalidLimits)
        );
    }
    assert_eq!(
        certify_kaiser_big(
            &designed,
            BankLimits {
                target_max_phase_l1: Some(binary_bound(65536)),
                ..BankLimits::default()
            }
        ),
        Err(CertificationError::Enclosure(Error::InputTooWide))
    );
}
