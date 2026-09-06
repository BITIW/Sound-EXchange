use super::*;
use rug::Integer;

// Independent exact-rational arithmetic, fixed term count, no MPFR or adaptive
// sum-stagnation rule. The geometric remainder provides both inner witnesses
// used to challenge the returned outward interval.
fn exact_series(s: &Rational, terms: u64) -> (Rational, Rational) {
    let mut factor = s.clone();
    factor /= 4;
    let mut sum = Rational::from(1);
    let mut term = Rational::from(1);
    for k in 1..=terms {
        term *= &factor;
        term /= k * k;
        sum += &term;
    }
    let mut next = term * &factor;
    next /= (terms + 1).pow(2);
    let mut ratio = factor;
    ratio /= (terms + 2).pow(2);
    assert!(ratio < 1);
    let tail = next / (Rational::from(1) - ratio);
    let upper = sum.clone() + tail;
    (sum, upper)
}

fn encloses(interval: &Enclosure, lower: &Rational, upper: &Rational) {
    assert!(
        interval.lower.to_rational().unwrap() <= *lower,
        "lower endpoint excluded exact oracle"
    );
    assert!(
        interval.upper.to_rational().unwrap() >= *upper,
        "upper endpoint excluded exact oracle"
    );
    assert!(interval.lower <= interval.upper);
}

#[test]
fn directed_i0_bounds_contain_exact_rational_series_and_its_tail() {
    let values = [
        Rational::from(0),
        Rational::from((1, 9)),
        Rational::from(1),
        Rational::from((225, 4)),
        Rational::from(4225),
        Rational::from((Integer::from(1), Integer::from(1) << 2000)),
    ];
    for s in values {
        // Four terms already bound the tiny 2^-2000 case far beyond every
        // tested precision; avoid constructing needless million-bit oracles.
        let terms = if s.denom().significant_bits() > 1000 {
            4
        } else {
            1024
        };
        let (lo, hi) = exact_series(&s, terms);
        for p in [16, 32, 64, 192, 512, 1024, 4096] {
            let result = i0_squared(
                &s,
                Limits {
                    precision_bits: p,
                    ..Limits::default()
                },
            )
            .unwrap();
            encloses(&result.value, &lo, &hi);
            assert!(result.tail_upper_bound >= 0);
            assert!(result.series_terms <= 16384);
            if s == 0 {
                assert_eq!(result.series_terms, 0);
                assert_eq!(result.value.width(), 0);
            } else {
                // Tightness sanity check, not the API's guaranteed width.
                let relative_width = result.value.width() / &lo;
                let loose_limit = Rational::from((Integer::from(1), Integer::from(1) << (p - 12)));
                assert!(relative_width < loose_limit, "s={s}, p={p}");
            }
        }
    }
}

#[test]
fn window_bounds_include_exact_numerator_denominator_series_ratios() {
    let beta = Rational::from((649, 10));
    let beta_squared = Rational::from(&beta * &beta);
    let (den_lo, den_hi) = exact_series(&beta_squared, 512);
    let radius = 17;
    for x in [
        Rational::from(0),
        Rational::from((1, 147)),
        Rational::from((31, 7)),
        Rational::from((16999, 1000)),
        Rational::from(17),
    ] {
        let s = (Rational::from(1) - Rational::from(&x * &x) / (radius * radius)) * &beta_squared;
        let (num_lo, num_hi) = exact_series(&s, 512);
        let lower = num_lo / &den_hi;
        let upper = num_hi / &den_lo;
        for p in [32, 128, 512] {
            let window = KaiserWindow::new(
                radius,
                &beta,
                Limits {
                    precision_bits: p,
                    ..Limits::default()
                },
            )
            .unwrap();
            let actual = window.at(&x).unwrap();
            if x == 0 {
                assert_eq!(actual.lower, 1);
                assert_eq!(actual.upper, 1);
            } else {
                encloses(&actual, &lower, &upper);
            }
            assert_eq!(actual, window.at(&(-x.clone())).unwrap());
            assert!(actual.lower >= 0 && actual.upper <= 1);
        }
    }
}

#[test]
fn center_support_boundary_and_zero_beta_are_exact() {
    for beta in [Rational::from(0), Rational::from(1), Rational::from(65)] {
        let window = KaiserWindow::new(3, &beta, Limits::default()).unwrap();
        assert_eq!(window.at(&Rational::from(0)).unwrap().width(), 0);
        assert_eq!(window.at(&Rational::from(0)).unwrap().lower, 1);
        let outside = window.at(&Rational::from((30001, 10000))).unwrap();
        assert_eq!(outside.lower, 0);
        assert_eq!(outside.upper, 0);
        let boundary = window.at(&Rational::from(3)).unwrap();
        if beta == 0 {
            assert_eq!(boundary.lower, 1);
            assert_eq!(boundary.width(), 0);
        } else {
            assert!(boundary.lower > 0 && boundary.upper < 1);
        }
    }
}

#[test]
fn current_window_approximations_have_exact_absolute_error_bounds() {
    for preset in sexplan::QualityPreset::ALL {
        let plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio: sexrate::RateRatio::from_fraction(160, 147).unwrap(),
            preset,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        let beta = Rational::from((plan.kaiser_beta.numerator(), plan.kaiser_beta.denominator()));
        let radius = plan.taps_per_phase / 2;
        let p = plan.working_precision_bits;
        let window = KaiserWindow::new(
            radius,
            &beta,
            Limits {
                precision_bits: p,
                ..Limits::default()
            },
        )
        .unwrap();
        let rounded_beta = Float::with_val(p, &beta);
        let old_denominator = crate::bessel_i0(&rounded_beta, p).unwrap();
        for x in [
            Rational::from(0),
            Rational::from((1, 147)),
            Rational::from((radius * 147 - 1, 147)),
            Rational::from(radius),
        ] {
            let approximation = crate::kaiser_window(
                &Float::with_val(p, &x),
                radius,
                &rounded_beta,
                &old_denominator,
                p,
            )
            .unwrap();
            let enclosure = window.at(&x).unwrap();
            let error = enclosure.absolute_error_bound(&approximation).unwrap();
            let rational_approximation = approximation.to_rational().unwrap();
            assert!(
                rational_approximation.clone() - &error <= enclosure.lower.to_rational().unwrap()
            );
            assert!(rational_approximation + &error >= enclosure.upper.to_rational().unwrap());
            let limit = Rational::from((Integer::from(1), Integer::from(1) << (p - 20)));
            assert!(error < limit, "preset {preset}, x={x}, error={error}");
        }
    }
}

#[test]
fn limits_domains_and_nonfinite_approximations_fail_closed() {
    let limits = Limits::default();
    assert_eq!(
        i0_squared(&Rational::from(-1), limits),
        Err(Error::NegativeArgument)
    );
    for bad in [
        Limits {
            precision_bits: 15,
            ..limits
        },
        Limits {
            max_series_terms: 0,
            ..limits
        },
        Limits {
            max_input_bits: 0,
            ..limits
        },
        Limits {
            precision_bits: u32::MAX,
            ..limits
        },
    ] {
        assert_eq!(
            i0_squared(&Rational::from(0), bad),
            Err(Error::InvalidLimits)
        );
    }
    let wide = Rational::from(Integer::from(1) << 17);
    assert_eq!(
        i0_squared(
            &wide,
            Limits {
                max_input_bits: 16,
                ..limits
            }
        ),
        Err(Error::InputTooWide)
    );
    assert_eq!(
        i0_squared(
            &Rational::from(4225),
            Limits {
                max_series_terms: 1,
                ..limits
            }
        ),
        Err(Error::SeriesBudget { maximum: 1 })
    );
    assert!(matches!(
        KaiserWindow::new(0, &Rational::from(1), limits),
        Err(Error::ZeroRadius)
    ));
    assert!(matches!(
        KaiserWindow::new(1, &Rational::from(-1), limits),
        Err(Error::NegativeArgument)
    ));
    let one = Enclosure::exact(64, 1);
    for bad in [
        rug::float::Special::Nan,
        rug::float::Special::Infinity,
        rug::float::Special::NegInfinity,
    ] {
        assert_eq!(
            one.absolute_error_bound(&Float::with_val(64, bad)),
            Err(Error::NonFinite)
        );
    }
}
