use super::*;
use rug::Rational;
use sexq::{BigQ, BigQFormat, Q2_62};

#[test]
fn actual_fir_l1_uses_maximum_phase_and_tightens_without_changing_error_allowance() {
    let bank = PolyphaseFirQ63::new(
        2,
        3,
        [1_i64 << 62, 0, 0, -(1_i64 << 61), 1_i64 << 62, 1_i64 << 61]
            .map(Q2_62::from_raw)
            .into(),
    )
    .unwrap();
    let l1 = FirL1Bound::from_native(&bank);
    assert_eq!(l1.maximum_raw(), &(Integer::from(2) << 62));
    for certified in [false, true] {
        let fir = if certified {
            NumericalStage::CertifiedFir {
                taps_per_phase: 3,
                coefficient_fractional_bits: 62,
                error_numerator: Integer::from(1),
                error_denominator: Integer::from(3),
            }
        } else {
            NumericalStage::DesignedFir {
                taps_per_phase: 3,
                coefficient_fractional_bits: 62,
            }
        };
        let stages = [linear("gain", &[(1, 2)]), fir, NumericalStage::Normalize];
        let generic = evaluate_numerical_budget(&stages, 63, 62).unwrap();
        let actual = evaluate_numerical_budget_with_fir_l1(&stages, 63, 62, Some(&l1)).unwrap();
        assert!(actual.maximum_signal_peak_raw < generic.maximum_signal_peak_raw);
        assert!(actual.final_bound.rounding_error_raw < generic.final_bound.rounding_error_raw);
        assert_eq!(
            actual.final_bound.fir_coefficient_error_raw,
            generic.final_bound.fir_coefficient_error_raw
        );
        let epsilon = Integer::from(1) << (actual.bound_fractional_bits - 64);
        // One gain rounding propagated by L1=2, FIR rounding, normalization rounding.
        assert_eq!(actual.final_bound.rounding_error_raw, epsilon * 4);
        let scale = Integer::from(1) << actual.bound_fractional_bits;
        let expected_delta = if certified {
            Rational::from((1, 3))
        } else {
            Rational::from((Integer::from(2), Integer::from(1) << 62))
        };
        let ideal_peak = Rational::from((actual.final_bound.ideal_peak_raw, scale));
        let expected = (Rational::from(2) + expected_delta) / 2;
        assert!(ideal_peak >= expected);
        assert!(ideal_peak - expected < Rational::from((1, Integer::from(1) << 100)));
    }
}

#[test]
fn actual_fir_l1_supports_big_precision_and_rejects_wrong_or_ambiguous_geometry() {
    for bits in [62, 96, 4096] {
        let format = BigQFormat::new(2, bits).unwrap();
        let one = Integer::from(1) << bits;
        let coefficients = [
            one.clone(),
            Integer::new(),
            Integer::new(),
            -one.clone(),
            one.clone(),
            one.clone(),
        ]
        .map(|raw| BigQ::from_raw(raw, format).unwrap())
        .into();
        let bank = PolyphaseFirBigQ63::new(2, 3, format, bits + 70, coefficients).unwrap();
        let l1 = FirL1Bound::from_big(&bank).unwrap();
        assert_eq!(l1.maximum_raw(), &(one * 3));
        let stage = NumericalStage::DesignedFir {
            taps_per_phase: 3,
            coefficient_fractional_bits: bits,
        };
        let actual = evaluate_numerical_budget_with_fir_l1(
            std::slice::from_ref(&stage),
            bits,
            bits,
            Some(&l1),
        )
        .unwrap();
        let generic = evaluate_numerical_budget(std::slice::from_ref(&stage), bits, bits).unwrap();
        assert!(actual.maximum_signal_peak_raw < generic.maximum_signal_peak_raw);
        for stages in [
            vec![],
            vec![stage.clone(), stage.clone()],
            vec![NumericalStage::DesignedFir {
                taps_per_phase: 2,
                coefficient_fractional_bits: bits,
            }],
            vec![NumericalStage::DesignedFir {
                taps_per_phase: 3,
                coefficient_fractional_bits: bits + 1,
            }],
        ] {
            assert_eq!(
                evaluate_numerical_budget_with_fir_l1(&stages, bits, bits, Some(&l1)),
                Err(BudgetError::InvalidFir)
            );
        }
    }
}

fn ratio(numerator: i128, denominator: u64) -> ExactRatio {
    ExactRatio {
        numerator,
        denominator,
    }
}

fn certified(n: i32, d: i32) -> NumericalStage {
    NumericalStage::CertifiedFir {
        taps_per_phase: 3,
        coefficient_fractional_bits: 2,
        error_numerator: Integer::from(n),
        error_denominator: Integer::from(d),
    }
}

#[test]
fn certified_fir_replaces_generic_quantization_allowance() {
    let exact = evaluate_numerical_budget(&[certified(0, 1)], 63, 62).unwrap();
    assert_eq!(exact.final_bound.fir_coefficient_error_raw, 0);
    let supplied = evaluate_numerical_budget(&[certified(1, 2)], 63, 62).unwrap();
    let generic = evaluate_numerical_budget(
        &[NumericalStage::DesignedFir {
            taps_per_phase: 3,
            coefficient_fractional_bits: 2,
        }],
        63,
        62,
    )
    .unwrap();
    assert_eq!(
        supplied.final_bound.total_error_raw(),
        generic.final_bound.total_error_raw()
    );
    assert_eq!(
        supplied.final_bound.ideal_peak_raw,
        generic.final_bound.ideal_peak_raw
    );
}

#[test]
fn certified_error_propagates_input_peak_and_subsequent_amplification() {
    let budget = evaluate_numerical_budget(
        &[
            linear("amplify", &[(1_i128 << 60, 1)]),
            certified(1, 3),
            linear("later gain", &[(3, 1)]),
        ],
        128,
        128,
    )
    .unwrap();
    let scale = Integer::from(1) << budget.bound_fractional_bits;
    let bound = Rational::from((budget.final_bound.fir_coefficient_error_raw, scale));
    let exact = Rational::from(Integer::from(1) << 60);
    assert!(bound >= exact);
    assert!(bound - exact < Rational::from((1, 1_u64 << 60)));
}

#[test]
fn certified_chain_contains_independent_rational_sample_oracle() {
    // Explicitly known references: actual [1/4,1/2,1/4], ideal [1/3;3],
    // both exact DC. Their joint L1 difference is exactly 1/3.
    let budget = evaluate_numerical_budget(
        &[
            linear("third", &[(1, 3)]),
            certified(1, 3),
            NumericalStage::Normalize,
        ],
        16,
        7,
    )
    .unwrap();
    let bound = Rational::from((
        budget.final_bound.total_error_raw(),
        Integer::from(1) << budget.bound_fractional_bits,
    ));
    let mut actual_history = vec![Integer::from(0); 3];
    let mut ideal_history = vec![Rational::from(0); 3];
    for index in 0..200 {
        let input = [65535, -65536, 17, 0, -1, 32768, -49152][index % 7];
        let amplified = round_div_integer(
            &(Integer::from(input) * 43),
            &Integer::from(128),
            RoundingMode::NearestTiesToEven,
        )
        .unwrap();
        actual_history.rotate_right(1);
        actual_history[0] = amplified;
        ideal_history.rotate_right(1);
        ideal_history[0] = Rational::from((input, 3 * 65536));
        let dot = actual_history[0].clone() + &actual_history[1] * 2 + &actual_history[2];
        let fir =
            round_div_integer(&dot, &Integer::from(4), RoundingMode::NearestTiesToEven).unwrap();
        let normalized =
            round_div_integer(&fir, &Integer::from(2), RoundingMode::NearestTiesToEven).unwrap();
        let reference: Rational =
            (ideal_history[0].clone() + &ideal_history[1] + &ideal_history[2]) / 6;
        assert!((Rational::from((normalized, 65536)) - reference).abs() <= bound);
    }
}

#[test]
fn invalid_certified_error_bounds_fail_closed() {
    for (n, d) in [(-1, 1), (1, 0), (1, -1)] {
        assert_eq!(
            evaluate_numerical_budget(&[certified(n, d)], 63, 62),
            Err(BudgetError::InvalidFir)
        );
    }
    let wide = NumericalStage::CertifiedFir {
        taps_per_phase: 3,
        coefficient_fractional_bits: 62,
        error_numerator: Integer::from(1) << (MAX_BITS + 64),
        error_denominator: Integer::from(1),
    };
    assert_eq!(
        evaluate_numerical_budget(&[wide], 63, 62),
        Err(BudgetError::InvalidFir)
    );
}
fn linear(name: &str, row: &[(i128, u64)]) -> NumericalStage {
    NumericalStage::Linear {
        name: name.to_owned(),
        rows: vec![row.iter().map(|(n, d)| ratio(*n, *d)).collect()],
    }
}

#[test]
fn coefficient_intervals_enclose_exact_rational_errors() {
    let grid = Grid { bits: 101 };
    for fractional in [0, 1, 7, 63, 96] {
        for numerator in [-8193, -17, -1, 0, 1, 17, 8193] {
            for denominator in [1, 3, 11, u64::MAX] {
                let (ideal, quantized, error, raw) = grid
                    .coefficient(ratio(numerator, denominator), fractional)
                    .unwrap();
                let exact = Rational::from((numerator, denominator));
                let quantized_exact = Rational::from((raw, Integer::from(1) << fractional));
                let bound = |x: Integer| Rational::from((x, grid.one()));
                assert!(bound(ideal) >= exact.clone().abs());
                assert_eq!(bound(quantized), quantized_exact.clone().abs());
                assert!(bound(error) >= (quantized_exact - exact).abs());
            }
        }
    }
    assert_eq!(
        grid.coefficient(ratio(1, 0), 3),
        Err(BudgetError::InvalidRatio)
    );
}

#[test]
fn linear_bounds_include_coefficient_error_and_later_amplification() {
    let stages = [
        linear("third", &[(1, 3)]),
        linear("amplify", &[(1_i128 << 40, 1)]),
    ];
    let low = evaluate_numerical_budget(&stages, 63, 62).unwrap();
    let high = evaluate_numerical_budget(&stages, 96, 96).unwrap();
    assert!(!low.meets_target_bits(50));
    assert!(high.meets_target_bits(50));
    assert!(low.final_bound.effect_coefficient_error_raw > 0);
    assert_eq!(low.final_bound.fir_coefficient_error_raw, 0);
    assert!(low.minimum_integer_bits() >= 41);
    let dyadic = evaluate_numerical_budget(&[linear("exact", &[(1, 2), (-3, 4)])], 63, 62).unwrap();
    assert_eq!(dyadic.final_bound.effect_coefficient_error_raw, 0);
}

#[test]
fn recursive_dc_bound_contains_independent_exact_rational_oracle() {
    for (numerator, denominator) in [(0, 1), (1, 2), (10, 11), (127, 128)] {
        let stages = [NumericalStage::DcBlock {
            radius: ratio(numerator, denominator),
        }];
        let budget = evaluate_numerical_budget(&stages, 16, 7).unwrap();
        let bound = Rational::from((
            budget.final_bound.total_error_raw(),
            Integer::from(1) << budget.bound_fractional_bits,
        ));
        let radius = Rational::from((numerator, denominator));
        let raw_radius = round_div_integer(
            &(Integer::from(numerator) << 7),
            &Integer::from(denominator),
            RoundingMode::NearestTiesToEven,
        )
        .unwrap();
        let mut last_input = 0;
        let mut last_actual = Integer::new();
        let mut last_ideal = Rational::new();
        for n in 0..400 {
            let input = match n % 7 {
                0 => 65535,
                1 => -65536,
                2 => 17,
                3 => -31,
                _ => 0,
            };
            let actual = round_div_integer(
                &((Integer::from(input - last_input) << 7) + last_actual * &raw_radius),
                &(Integer::from(1) << 7),
                RoundingMode::NearestTiesToEven,
            )
            .unwrap();
            let ideal = Rational::from((input - last_input, 65536)) + last_ideal * &radius;
            let error = (Rational::from((actual.clone(), 65536)) - &ideal).abs();
            assert!(error <= bound, "R={radius}, n={n}");
            last_input = input;
            last_actual = actual;
            last_ideal = ideal;
        }
    }
    assert_eq!(
        evaluate_numerical_budget(
            &[NumericalStage::DcBlock {
                radius: ratio(1023, 1024)
            }],
            16,
            7
        ),
        Err(BudgetError::UnstableDcPole)
    );
}

#[test]
fn fir_precision_is_a_separate_error_source_and_scales_with_input_peak() {
    let stages = |bits| {
        [
            linear("gain", &[(1_i128 << 60, 1)]),
            NumericalStage::DesignedFir {
                taps_per_phase: 257,
                coefficient_fractional_bits: bits,
            },
        ]
    };
    let insufficient = evaluate_numerical_budget(&stages(62), 128, 128).unwrap();
    assert!(!insufficient.meets_target_bits(21));
    assert!(
        insufficient.final_bound.fir_coefficient_error_raw
            > insufficient.final_bound.rounding_error_raw
    );
    let sufficient = evaluate_numerical_budget(&stages(96), 128, 128).unwrap();
    assert!(sufficient.meets_target_bits(21));
    let filter = crate::plan_precision(crate::PlanRequest {
        ratio: sexrate::RateRatio::from_fraction(160, 147).unwrap(),
        preset: crate::QualityPreset::Sane,
        error_floor: None,
        working_precision_bits: None,
    })
    .unwrap();
    let raised = crate::raise_coefficient_precision(&filter, 200, None).unwrap();
    assert_eq!(raised.taps_per_phase, filter.taps_per_phase);
    assert_eq!(raised.target_error_floor, filter.target_error_floor);
    assert_eq!(raised.coefficient_fractional_bits, 200);
    assert!(raised.working_precision_bits >= 264);
    assert!(matches!(
        raised.backend,
        crate::BackendRequirement::BigInt { .. }
    ));
    assert!(crate::raise_coefficient_precision(&filter, 200, Some(192)).is_err());
}

#[test]
fn bounds_have_explicit_zero_shape_and_resource_semantics() {
    let empty = evaluate_numerical_budget(&[], 4096, 4096).unwrap();
    assert_eq!(empty.binary_ceiling(&Integer::new()), None);
    assert!(empty.meets_target_bits(u32::MAX));
    assert_eq!(
        evaluate_numerical_budget(&[], u32::MAX, 1),
        Err(BudgetError::InvalidPrecision)
    );
    assert_eq!(
        evaluate_numerical_budget(
            &[NumericalStage::Linear {
                name: "bad".into(),
                rows: vec![]
            }],
            63,
            62
        ),
        Err(BudgetError::EmptyLinearStage)
    );
    assert_eq!(
        evaluate_numerical_budget(
            &[NumericalStage::DesignedFir {
                taps_per_phase: 0,
                coefficient_fractional_bits: 62
            }],
            63,
            62
        ),
        Err(BudgetError::InvalidFir)
    );
    let identity = evaluate_numerical_budget(
        &[linear("identity", &[(1, 1)]), NumericalStage::Normalize],
        63,
        62,
    )
    .unwrap();
    assert_eq!(
        identity.binary_ceiling(&identity.final_bound.total_error_raw()),
        Some(-63)
    );
}
