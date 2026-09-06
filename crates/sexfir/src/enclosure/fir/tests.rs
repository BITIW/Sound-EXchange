use super::*;
use crate::{BigKaiserSpec, Fraction, design_kaiser, design_kaiser_big};
use sexq::RoundingMode;
use sexrate::RateRatio;

fn spec(up: u64, down: u64, taps: usize, beta: u64) -> KaiserSpec {
    KaiserSpec {
        ratio: RateRatio::from_fraction(up, down).unwrap(),
        taps_per_phase: taps,
        rolloff: Fraction::new(1, 2).unwrap(),
        beta: Fraction::new(beta, 1).unwrap(),
        working_precision_bits: 256,
        quantization_rounding: RoundingMode::NearestTiesToEven,
    }
}

fn encloses(value: &Enclosure, lo: &Rational, hi: &Rational) {
    assert!(
        value.lower.to_rational().unwrap() <= *lo,
        "lower excluded exact oracle"
    );
    assert!(
        value.upper.to_rational().unwrap() >= *hi,
        "upper excluded exact oracle"
    );
}

// Independent rational Machin identity with alternating-series remainders.
// No MPFR constant or trigonometric function participates in this oracle.
fn pi_bracket() -> (Rational, Rational) {
    fn atan_inverse(n: u32) -> (Rational, Rational) {
        let x = Rational::from((1, n));
        let squared = Rational::from(&x * &x);
        let mut power = x;
        let mut sum = Rational::from(0);
        for k in 0..320 {
            let term = power.clone() / (2 * k + 1);
            if k % 2 == 0 {
                sum += term;
            } else {
                sum -= term;
            }
            power *= &squared;
        }
        let hi = sum.clone() + power / 641;
        (sum, hi)
    }
    let (a, b) = atan_inverse(5);
    let (c, d) = atan_inverse(239);
    (a * 16 - d * 4, b * 16 - c * 4)
}

#[test]
fn exact_rational_quadrants_do_not_lose_large_integer_offsets() {
    for p in [16, 64, 192, 512, 4096] {
        let limits = Limits {
            precision_bits: p,
            ..Limits::default()
        };
        for (x, y) in [(0, 0), (1, 0), (2, 0), (-1, 0)] {
            let actual = sin_pi_rational(&Rational::from(x), limits).unwrap();
            assert_eq!(actual.width(), 0);
            assert_eq!(actual.lower, y);
        }
        for (n, d, expected) in [
            (1, 2, Rational::from(1)),
            (3, 2, Rational::from(-1)),
            (1, 6, Rational::from((1, 2))),
            (5, 6, Rational::from((1, 2))),
            (7, 6, Rational::from((-1, 2))),
        ] {
            for sign in [-1, 1] {
                let value = Rational::from((n * sign, d));
                let want = expected.clone() * sign;
                encloses(&sin_pi_rational(&value, limits).unwrap(), &want, &want);
            }
        }
        let huge: Integer = Integer::from(1) << 1000;
        for offset in [0, 1] {
            let x = Rational::from(huge.clone() + offset) + Rational::from((1, 6));
            let y = Rational::from((if offset == 0 { 1 } else { -1 }, 2));
            encloses(&sin_pi_rational(&x, limits).unwrap(), &y, &y);
        }
    }
}

#[test]
fn signed_interval_operations_enclose_all_rational_corners() {
    let pairs = [(-7, -2), (-3, 5), (2, 9), (0, 0)];
    for (a, b) in pairs {
        for (c, d) in pairs {
            let left = checked(Float::with_val(32, a), Float::with_val(32, b)).unwrap();
            let right = checked(Float::with_val(32, c), Float::with_val(32, d)).unwrap();
            let product = multiply(&left, &right, 32).unwrap();
            for x in [a, b] {
                for y in [c, d] {
                    let exact = Rational::from(x * y);
                    encloses(&product, &exact, &exact);
                }
            }
            if c <= 0 && d >= 0 {
                assert_eq!(divide(&left, &right, 32), Err(Error::UnresolvedPhaseGain));
            } else {
                let quotient = divide(&left, &right, 32).unwrap();
                for x in [a, b] {
                    for y in [c, d] {
                        let exact = Rational::from((x, y));
                        encloses(&quotient, &exact, &exact);
                    }
                }
            }
        }
    }
}

#[test]
fn irrational_sines_and_tiny_coordinates_have_independent_bounds() {
    let (pi_lo, pi_hi) = pi_bracket();
    let tiny = Rational::from((Integer::from(1), Integer::from(1) << 1000));
    let y_lo = pi_lo * &tiny;
    let y_hi = pi_hi * &tiny;
    let sine_lo = y_lo - y_hi.clone() * &y_hi * &y_hi / 6;
    for p in [16, 64, 256, 1152] {
        let limits = Limits {
            precision_bits: p,
            ..Limits::default()
        };
        for (x, square) in [
            (Rational::from((1, 4)), Rational::from((1, 2))),
            (Rational::from((1, 3)), Rational::from((3, 4))),
        ] {
            let value = sin_pi_rational(&x, limits).unwrap();
            let lo = value.lower.to_rational().unwrap();
            let hi = value.upper.to_rational().unwrap();
            assert!(lo >= 0);
            assert!(Rational::from(&lo * &lo) <= square);
            assert!(Rational::from(&hi * &hi) >= square);
        }
        let actual = sin_pi_rational(&tiny, limits).unwrap();
        // The rational pi bracket is narrower than every tested precision;
        // the cubic Taylor remainder also lies below their endpoint widths.
        encloses(&actual, &sine_lo, &y_hi);
    }
}

#[test]
fn normalized_three_tap_filter_contains_analytic_pi_oracle() {
    let (pi_lo, pi_hi) = pi_bracket();
    let edge_lo = Rational::from(2) / (pi_hi.clone() + 4);
    let edge_hi = Rational::from(2) / (pi_lo.clone() + 4);
    let center_lo = pi_lo.clone() / (pi_lo + 4);
    let center_hi = pi_hi.clone() / (pi_hi + 4);
    for p in [16, 64, 192, 512] {
        let phase = kaiser_phase(
            &spec(1, 1, 3, 0),
            0,
            PhaseLimits {
                arithmetic: Limits {
                    precision_bits: p,
                    ..Limits::default()
                },
                ..PhaseLimits::default()
            },
        )
        .unwrap();
        encloses(&phase.coefficients[0], &edge_lo, &edge_hi);
        encloses(&phase.coefficients[1], &center_lo, &center_hi);
        assert_eq!(phase.coefficients[0], phase.coefficients[2]);
        assert_eq!(phase.series_terms(), 0);
    }
}

#[test]
fn nonzero_kaiser_beta_phase_contains_independent_series_pi_oracle() {
    let (pi_lo, pi_hi) = pi_bracket();
    // For three taps, beta=8, cutoff=1/4: endpoint weights are 1/I0(8).
    // Thus normalized edges = 2/(4 + pi*I0(8)), center = 1 - 2*edge.
    let mut term = Rational::from(1);
    let mut i0_lo = term.clone();
    for k in 1..=256 {
        term *= 16;
        term /= k * k;
        i0_lo += &term;
    }
    let next = term * Rational::from((16, 257 * 257));
    let tail = next / (Rational::from(1) - Rational::from((16, 258 * 258)));
    let i0_hi = i0_lo.clone() + tail;
    let edge_lo: Rational = Rational::from(2) / (pi_hi * i0_hi + 4);
    let edge_hi: Rational = Rational::from(2) / (pi_lo * i0_lo + 4);
    let center_lo = Rational::from(1) - edge_hi.clone() * 2;
    let center_hi = Rational::from(1) - edge_lo.clone() * 2;
    for p in [32, 64, 192, 512] {
        let phase = kaiser_phase(
            &spec(1, 1, 3, 8),
            0,
            PhaseLimits {
                arithmetic: Limits {
                    precision_bits: p,
                    ..Limits::default()
                },
                ..PhaseLimits::default()
            },
        )
        .unwrap();
        encloses(&phase.coefficients[0], &edge_lo, &edge_hi);
        encloses(&phase.coefficients[1], &center_lo, &center_hi);
        assert_eq!(phase.coefficients[0], phase.coefficients[2]);
    }
}

#[test]
fn exact_geometry_matches_integer_and_fractional_phase_support() {
    let mut integer = spec(1, 1, 3, 0);
    integer.rolloff = Fraction::new(1, 1).unwrap();
    let identity = kaiser_phase(&integer, 0, PhaseLimits::default()).unwrap();
    for (value, exact) in identity.coefficients.iter().zip([0, 1, 0]) {
        assert_eq!(value.lower, exact);
        assert_eq!(value.width(), 0);
    }
    integer.ratio = RateRatio::from_fraction(2, 1).unwrap();
    let half = kaiser_phase(&integer, 1, PhaseLimits::default()).unwrap();
    for (value, exact) in half.coefficients.iter().zip([
        Rational::from((1, 2)),
        Rational::from((1, 2)),
        Rational::from(0),
    ]) {
        encloses(value, &exact, &exact);
    }
    assert_eq!(half.coefficients[2].width(), 0);
}

#[test]
fn actual_native_and_bigint_banks_include_dc_correction_in_error_bound() {
    for (up, down) in [(1, 3), (3, 2), (7, 11)] {
        let core = spec(up, down, 17, 8);
        let native = design_kaiser(&core).unwrap();
        let big = design_kaiser_big(&BigKaiserSpec {
            core: core.clone(),
            coefficient_fractional_bits: 96,
            accumulator_bits: 256,
        })
        .unwrap();
        for phase in 0..up {
            let enclosed = kaiser_phase(&core, phase, PhaseLimits::default()).unwrap();
            let raw: Vec<_> = native
                .bank()
                .phase(phase)
                .unwrap()
                .iter()
                .map(|q| Integer::from(q.raw()))
                .collect();
            let native_error = enclosed.compare_quantized(&raw, 62).unwrap();
            let big_error = enclosed
                .compare_quantized(big.bank().phase(phase).unwrap().iter().map(|q| q.raw()), 96)
                .unwrap();
            assert!(native_error.phase_l1 > 0);
            assert!(native_error.phase_l1 < Rational::from((1, 1_u64 << 50)));
            assert!(big_error.phase_l1 < native_error.phase_l1);
            assert!(
                big_error.phase_l1 < Rational::from((Integer::from(1), Integer::from(1) << 80))
            );
            assert!(enclosed.series_terms() <= PhaseLimits::default().max_total_series_terms);
        }
    }
}

#[test]
fn final_integer_error_bound_cannot_hide_an_arbitrary_dc_adjustment() {
    let mut core = spec(1, 1, 3, 0);
    core.rolloff = Fraction::new(1, 1).unwrap();
    let phase = kaiser_phase(&core, 0, PhaseLimits::default()).unwrap();
    let exact: Vec<_> = [0, 16, 0].into_iter().map(Integer::from).collect();
    assert_eq!(phase.compare_quantized(&exact, 4).unwrap().phase_l1, 0);
    let modified: Vec<_> = [-1, 19, -2].into_iter().map(Integer::from).collect();
    let error = phase.compare_quantized(&modified, 4).unwrap();
    assert_eq!(error.max_absolute, Rational::from((3, 16)));
    assert_eq!(error.phase_l1, Rational::from((6, 16)));
    assert_eq!(error.phase_l1_lower, error.phase_l1);
}

#[test]
fn wide_coefficients_do_not_conceal_low_precision_design_error() {
    let mut low_core = spec(3, 2, 17, 8);
    low_core.working_precision_bits = 96;
    let mut high_core = low_core.clone();
    high_core.working_precision_bits = 512;
    let bank = |core| {
        design_kaiser_big(&BigKaiserSpec {
            core,
            coefficient_fractional_bits: 256,
            accumulator_bits: 512,
        })
        .unwrap()
    };
    let low = bank(low_core.clone());
    let high = bank(high_core);
    for phase in 0..3 {
        let enclosed = kaiser_phase(
            &low_core,
            phase,
            PhaseLimits {
                arithmetic: Limits {
                    precision_bits: 384,
                    ..Limits::default()
                },
                ..PhaseLimits::default()
            },
        )
        .unwrap();
        let low_error = enclosed
            .compare_quantized(
                low.bank().phase(phase).unwrap().iter().map(|q| q.raw()),
                256,
            )
            .unwrap();
        let high_error = enclosed
            .compare_quantized(
                high.bank().phase(phase).unwrap().iter().map(|q| q.raw()),
                256,
            )
            .unwrap();
        assert!(low_error.phase_l1 > Rational::from((Integer::from(1), Integer::from(1) << 120)));
        assert!(high_error.phase_l1 < Rational::from((Integer::from(1), Integer::from(1) << 240)));
    }
}

#[test]
fn phase_and_coefficient_resource_limits_fail_closed() {
    let core = spec(3, 2, 17, 8);
    let defaults = PhaseLimits::default();
    assert_eq!(
        kaiser_phase(&core, 3, defaults),
        Err(Error::PhaseOutOfRange)
    );
    assert_eq!(
        kaiser_phase(
            &core,
            0,
            PhaseLimits {
                max_taps: 3,
                ..defaults
            }
        ),
        Err(Error::TapBudget)
    );
    assert_eq!(
        kaiser_phase(
            &core,
            0,
            PhaseLimits {
                max_total_series_terms: 0,
                ..defaults
            }
        ),
        Err(Error::InvalidLimits)
    );
    assert!(matches!(
        kaiser_phase(
            &core,
            0,
            PhaseLimits {
                max_total_series_terms: 1,
                ..defaults
            }
        ),
        Err(Error::SeriesBudget { .. })
    ));
    let full = kaiser_phase(&core, 0, defaults).unwrap();
    let work = full.series_terms();
    assert_eq!(
        kaiser_phase(
            &core,
            0,
            PhaseLimits {
                max_total_series_terms: work,
                ..defaults
            }
        )
        .unwrap(),
        PhaseEnclosure {
            limits: PhaseLimits {
                max_total_series_terms: work,
                ..defaults
            },
            ..full.clone()
        }
    );
    assert!(
        kaiser_phase(
            &core,
            0,
            PhaseLimits {
                max_total_series_terms: work - 1,
                ..defaults
            }
        )
        .is_err()
    );
    let zeros = vec![Integer::from(0); 17];
    assert_eq!(
        full.compare_quantized(&zeros[..16], 62),
        Err(Error::CoefficientLayout)
    );
    assert_eq!(
        full.compare_quantized(zeros.iter().chain(zeros.iter().take(1)), 62),
        Err(Error::CoefficientLayout)
    );
    assert_eq!(
        full.compare_quantized(&zeros, 65537),
        Err(Error::InputTooWide)
    );
    let enormous = vec![Integer::from(1) << 65536; 17];
    assert_eq!(
        full.compare_quantized(&enormous, 62),
        Err(Error::InputTooWide)
    );
    let mut invalid = core;
    invalid.taps_per_phase = 4;
    assert!(matches!(
        kaiser_phase(&invalid, 0, defaults),
        Err(Error::InvalidSpecification(_))
    ));
}
