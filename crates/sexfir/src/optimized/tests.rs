use super::*;
use sexq::{OverflowPolicy, Q1_63};
use sexrate::{FrameCountPolicy, InterleavedResamplerBig, PolyphaseFirBig};
use std::sync::Arc;

fn spec(equiripple: bool, bits: u32) -> Spec {
    let ratio = RateRatio::from_fraction(2, 3).unwrap();
    let p = (bits + 64).max(128);
    let method = if equiripple {
        Method::Equiripple(EquirippleSpec {
            ratio,
            taps_per_phase: 7,
            rolloff: Fraction::new(9, 10).unwrap(),
            passband_weight: Fraction::new(1, 1).unwrap(),
            stopband_weight: Fraction::new(10, 1).unwrap(),
            grid_density: 16,
            max_iterations: 64,
            working_precision_bits: p,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        })
    } else {
        Method::GlobalLeastSquares(LeastSquaresSpec {
            ratio,
            taps_per_phase: 7,
            rolloff: Fraction::new(9, 10).unwrap(),
            passband_weight: Fraction::new(1, 1).unwrap(),
            stopband_weight: Fraction::new(10, 1).unwrap(),
            grid_points_per_band: 32,
            working_precision_bits: p,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        })
    };
    Spec {
        method,
        coefficient_fractional_bits: bits,
        accumulator_bits: (bits + 80).max(128),
    }
}

#[test]
fn remez_cosine_cache_preserves_exact_values_errors_and_solver_results() {
    for unity in [false, true] {
        for bits in [62, 96, 4096] {
            let mut request = spec(true, bits);
            let Method::Equiripple(core) = &mut request.method else {
                unreachable!()
            };
            if unity {
                core.ratio = RateRatio::from_fraction(1, 1).unwrap();
            }
            let cached_work = request.preflight(Limits::default()).unwrap();
            assert!(cached_work.cosine_cache_bytes > 0);
            assert!(
                cached_work.storage_bytes + cached_work.cosine_cache_bytes
                    <= Limits::default().max_storage_bytes
            );
            let direct_limits = Limits {
                max_storage_bytes: cached_work.storage_bytes,
                ..Limits::default()
            };
            let direct_work = request.preflight(direct_limits).unwrap();
            assert_eq!(direct_work.cosine_cache_bytes, 0);
            let total_bytes = cached_work.storage_bytes + cached_work.cosine_cache_bytes;
            assert_eq!(
                request
                    .preflight(Limits {
                        max_storage_bytes: total_bytes,
                        ..Limits::default()
                    })
                    .unwrap()
                    .cosine_cache_bytes,
                cached_work.cosine_cache_bytes
            );
            assert_eq!(
                request
                    .preflight(Limits {
                        max_storage_bytes: total_bytes - 1,
                        ..Limits::default()
                    })
                    .unwrap()
                    .cosine_cache_bytes,
                0
            );
            assert_eq!(direct_work.terms, cached_work.terms);
            let cached = design(&request, Limits::default()).unwrap();
            let mut direct = design(&request, direct_limits).unwrap();
            assert_eq!(cached.bank, direct.bank);
            assert_eq!(cached.report.solution, direct.report.solution);
            assert_eq!(cached.report.quantization, direct.report.quantization);
            direct.report.work.cosine_cache_bytes = cached_work.cosine_cache_bytes;
            assert_eq!(direct, cached);
            let Method::Equiripple(core) = &request.method else {
                unreachable!()
            };
            let amplitudes = cached_work.solver_dimension as usize - 1;
            let grid = build_remez_grid(
                core,
                core.grid_density as usize * (amplitudes + 1),
                core.working_precision_bits,
            );
            let cached = RemezCosines::new(&grid, amplitudes, core.working_precision_bits, true);
            let direct = RemezCosines::new(&grid, amplitudes, core.working_precision_bits, false);
            for point in 0..grid.len() {
                for harmonic in 0..amplitudes {
                    assert_eq!(cached.value(point, harmonic), direct.value(point, harmonic));
                }
            }
            let coefficients: Vec<_> = (0..amplitudes)
                .map(|index| Float::with_val(core.working_precision_bits, index + 1))
                .collect();
            assert_eq!(
                evaluate_remez_errors(&cached, &coefficients, core.working_precision_bits),
                evaluate_remez_errors(&direct, &coefficients, core.working_precision_bits)
            );
        }
    }
    assert_eq!(remez_cosine_cache_bytes(u64::MAX, u64::MAX, u32::MAX), None);
}

#[test]
fn unity_prototype_grid_has_one_stop_endpoint_and_distinct_remez_references() {
    let ratio = RateRatio::from_fraction(1, 1).unwrap();
    let grid = build_prototype_grid(
        ratio,
        Fraction::new(3, 4).unwrap(),
        (Fraction::new(1, 1).unwrap(), Fraction::new(10, 1).unwrap()),
        32,
        160,
    );
    assert_eq!(grid.len(), 33);
    assert_eq!(grid.iter().filter(|point| point.band == 1).count(), 1);
    assert_eq!(grid.last().unwrap().frequency, Float::with_val(160, 0.5));
    assert_eq!(grid.last().unwrap().weight, 10);
    assert!(
        grid.windows(2)
            .all(|pair| pair[0].frequency < pair[1].frequency)
    );
    let references = initial_remez_extrema(&grid, 9, 160);
    assert_eq!(references.len(), 9);
    assert_eq!(references[0], 0);
    assert_eq!(references[7], 31);
    assert_eq!(references[8], 32);
    assert!(references.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(prototype_grid_points(ratio, u128::MAX), None);
    assert_eq!(
        prototype_grid_points(RateRatio::from_fraction(2, 3).unwrap(), 32),
        Some(64)
    );
}

#[test]
fn unity_remez_matches_an_exact_three_tap_minimax_solution_and_native_bank() {
    let mut request = spec(true, 62);
    let Method::Equiripple(core) = &mut request.method else {
        unreachable!()
    };
    core.ratio = RateRatio::from_fraction(1, 1).unwrap();
    core.taps_per_phase = 3;
    core.rolloff = Fraction::new(3, 4).unwrap();
    core.stopband_weight = Fraction::new(1, 1).unwrap();
    let native = design_equiripple(core).unwrap();
    let wide = design(&request, Limits::default()).unwrap();
    assert_eq!(
        wide.report.work.grid_points,
        native.report().dense_grid_points as u64
    );
    assert_eq!(wide.report.work.grid_points, u64::from(16_u32 * 3 + 1));
    let side: Integer = ((Integer::from(1) << 62_u32) + 2) / 5;
    let middle = (Integer::from(1) << 62_u32) - Integer::from(&side * 2);
    let expected = [side.clone(), middle, side];
    for ((coefficient, native), expected) in wide
        .bank
        .phase(0)
        .unwrap()
        .iter()
        .zip(native.bank().phase(0).unwrap())
        .zip(expected)
    {
        assert_eq!(*coefficient.raw(), expected);
        assert_eq!(*coefficient.raw(), native.raw());
    }
    assert_eq!(
        wide.report.quantization.algorithm,
        "parks-mcclellan-lowpass-big-unity-v3"
    );
    assert_eq!(
        native.report().quantization.algorithm,
        "parks-mcclellan-lowpass-unity-v3"
    );
}

#[test]
fn unity_optimized_preflight_matches_built_grid_and_rejects_tighter_limits() {
    for equiripple in [false, true] {
        let mut request = spec(equiripple, 96);
        let expected_grid = match &mut request.method {
            Method::GlobalLeastSquares(core) => {
                core.ratio = RateRatio::from_fraction(1, 1).unwrap();
                u64::from(core.grid_points_per_band) + 1
            }
            Method::Equiripple(core) => {
                core.ratio = RateRatio::from_fraction(1, 1).unwrap();
                u64::from(core.grid_density) * 5 + 1
            }
        };
        let work = request.preflight(Limits::default()).unwrap();
        assert_eq!(work.grid_points, expected_grid);
        let limits = Limits {
            max_grid_points: expected_grid,
            ..Limits::default()
        };
        let bank = design(&request, limits).unwrap();
        assert_eq!(bank.report.work, work);
        assert!(bank.report.quantization.algorithm.contains("unity"));
        assert!(matches!(
            request.preflight(Limits {
                max_grid_points: expected_grid - 1,
                ..limits
            }),
            Err(Error::Limit {
                resource: "grid points",
                ..
            })
        ));
    }
}

#[test]
fn unity_global_ls_counts_the_stop_endpoint_weight_once() {
    for bits in [62, 96] {
        let mut request = spec(false, bits);
        let Method::GlobalLeastSquares(core) = &mut request.method else {
            unreachable!()
        };
        core.ratio = RateRatio::from_fraction(1, 1).unwrap();
        core.taps_per_phase = 3;
        core.rolloff = Fraction::new(3, 4).unwrap();
        core.stopband_weight = Fraction::new(1, 1).unwrap();
        core.grid_points_per_band = 2;
        let bank = design(&request, Limits::default()).unwrap();
        assert_eq!(bank.report.work.grid_points, 3);
        let side: Integer = ((Integer::from(3) << bits) + 7) / 14;
        let middle = (Integer::from(1) << bits) - Integer::from(&side * 2);
        for (coefficient, expected) in
            bank.bank
                .phase(0)
                .unwrap()
                .iter()
                .zip([side.clone(), middle, side])
        {
            assert_eq!(*coefficient.raw(), expected);
        }
        assert_eq!(
            bank.report.quantization.algorithm,
            "least-squares-global-lowpass-big-unity-v2"
        );
    }
}

#[test]
fn optimized_equiripple_q62_matches_native_coefficients_reports_and_response() {
    let request = spec(true, 62);
    let wide = design(&request, Limits::default()).unwrap();
    let Method::Equiripple(core) = &request.method else {
        unreachable!()
    };
    let native = design_equiripple(core).unwrap();
    let floor = ErrorFloor::new(80).unwrap();
    let original = native.report();
    let Solution::Equiripple {
        iterations,
        extremal_frequencies_decimal,
        prototype_weighted_error_decimal,
    } = &wide.report.solution
    else {
        unreachable!()
    };
    assert_eq!(*iterations, original.exchange_iterations);
    assert_eq!(
        *extremal_frequencies_decimal,
        original.extremal_frequencies_decimal
    );
    assert_eq!(
        *prototype_weighted_error_decimal,
        original.prototype_weighted_error_decimal
    );
    for phase in 0..2 {
        for (a, b) in wide
            .bank
            .phase(phase)
            .unwrap()
            .iter()
            .zip(native.bank().phase(phase).unwrap())
        {
            assert_eq!(*a.raw(), Integer::from(b.raw()));
        }
    }
    assert_eq!(
        wide.report.quantization.max_phase_l1_error_decimal,
        original.quantization.max_phase_l1_error_decimal
    );
    assert_eq!(
        wide.report.quantization.max_abs_coefficient_error_decimal,
        original.quantization.max_abs_coefficient_error_decimal
    );
    assert_eq!(
        analyze_against(&wide, 17, floor).unwrap(),
        analyze_equiripple_quantized_response_against(&native, 17, floor).unwrap()
    );
    assert_eq!(design(&request, Limits::default()).unwrap(), wide);
}

#[test]
fn optimized_global_ls_matches_an_independent_exact_normal_equation_solution() {
    // N=3, L/M=1/2, r=1, two equally weighted points per band:
    // f={0,1/4,1/4,1/2}, desired={1,1,0,0}, cosine basis {1, cos(2*pi*f)}.
    // R=diag(4,2), b=(2,1), a=(1/2,1/2), h=(1/4,1/2,1/4), already exact DC.
    for bits in [31, 62, 96, 4096] {
        let mut request = spec(false, bits);
        let Method::GlobalLeastSquares(s) = &mut request.method else {
            unreachable!()
        };
        s.ratio = RateRatio::from_fraction(1, 2).unwrap();
        s.taps_per_phase = 3;
        s.rolloff = Fraction::new(1, 1).unwrap();
        s.stopband_weight = Fraction::new(1, 1).unwrap();
        s.grid_points_per_band = 2;
        let designed = design(&request, Limits::default()).unwrap();
        let expected = [
            Integer::from(1) << (bits - 2),
            Integer::from(1) << (bits - 1),
            Integer::from(1) << (bits - 2),
        ];
        for (coefficient, expected) in designed.bank.phase(0).unwrap().iter().zip(expected) {
            assert_eq!(*coefficient.raw(), expected);
        }
    }
}

#[test]
fn optimized_coefficients_have_real_precision_beyond_q62() {
    for equiripple in [false, true] {
        let wide_spec = spec(equiripple, 96);
        let mut low_spec = wide_spec.clone();
        low_spec.coefficient_fractional_bits = 62;
        let low = design(&low_spec, Limits::default()).unwrap();
        let wide = design(&wide_spec, Limits::default()).unwrap();
        assert!(
            wide.bank
                .phase(0)
                .unwrap()
                .iter()
                .any(|c| !c.raw().is_divisible_2pow(34))
        );
        let mut low_error = Float::with_val(
            160,
            Float::parse(&low.report.quantization.max_phase_l1_error_decimal).unwrap(),
        );
        low_error >>= 20;
        let high_error = Float::with_val(
            160,
            Float::parse(&wide.report.quantization.max_phase_l1_error_decimal).unwrap(),
        );
        assert!(high_error < low_error);
        assert_ne!(
            low.report.quantization.coefficient_sha256,
            wide.report.quantization.coefficient_sha256
        );
    }
}

fn rounded_saturated_q63(sum: Integer, bits: u32) -> i64 {
    let negative = sum < 0;
    let denominator = Integer::from(1) << bits;
    let magnitude = sum.abs();
    let (mut quotient, remainder) = magnitude.div_rem(denominator.clone());
    let twice = remainder * 2;
    if twice > denominator || (twice == denominator && quotient.is_odd()) {
        quotient += 1;
    }
    if negative {
        quotient = -quotient;
    }
    if quotient < i64::MIN {
        i64::MIN
    } else if quotient > i64::MAX {
        i64::MAX
    } else {
        quotient.to_i64().unwrap()
    }
}

#[test]
fn optimized_4096_bit_coefficients_execute_one_round_8192_bit_mac() {
    for equiripple in [false, true] {
        let mut request = spec(equiripple, 4096);
        request.accumulator_bits = 8192;
        let filter = design(&request, Limits::default()).unwrap();
        assert!(filter.bank.required_accumulator_bits() > 4096);
        assert!(filter.bank.required_accumulator_bits() < 8192);
        assert_eq!(filter.bank.accumulator_bits(), 8192);
        assert!(
            filter
                .bank
                .phase(0)
                .unwrap()
                .iter()
                .any(|c| !c.raw().is_divisible_2pow(4034))
        );
        for phase in 0..2 {
            let coefficients = filter.bank.phase(phase).unwrap();
            let dc = Integer::from(Integer::sum(coefficients.iter().map(BigQ::raw)));
            assert_eq!(dc, Integer::from(1) << 4096);
            for raw in [
                vec![0; 7],
                vec![i64::MIN; 7],
                vec![i64::MAX; 7],
                vec![
                    i64::MIN,
                    i64::MAX,
                    i64::MIN,
                    i64::MAX,
                    i64::MIN,
                    i64::MAX,
                    i64::MIN,
                ],
                vec![0, 0, 0, 1_i64 << 62, 0, 0, 0],
                vec![1, -1, 3, -7, 31, -511, 4095],
            ] {
                let mut sum = Integer::new();
                for (sample, coefficient) in raw.iter().zip(coefficients) {
                    sum += Integer::from(coefficient.raw() * *sample);
                }
                let expected = rounded_saturated_q63(sum, 4096);
                let input = raw.into_iter().map(Q1_63::from_raw).collect::<Vec<_>>();
                let actual = filter
                    .bank
                    .convolve(
                        phase,
                        &input,
                        RoundingMode::NearestTiesToEven,
                        OverflowPolicy::Saturate,
                    )
                    .unwrap();
                assert_eq!(actual.value.raw(), expected);
            }
        }
    }
}

#[test]
fn optimized_arbitrary_signal_streaming_preserves_chunks_and_sub_q63_codes() {
    for equiripple in [false, true] {
        let filter = design(&spec(equiripple, 96), Limits::default()).unwrap();
        let format = BigQFormat::new(65, 160).unwrap();
        let bank =
            Arc::new(PolyphaseFirBig::from_q63_bank(filter.bank(), format, format, None).unwrap());
        let input = [
            1, -1, 17, -31, 251, -127, 1234567, -567890, 3, -5, 0, 0, 9, -11,
        ]
        .into_iter()
        .map(|x| BigQ::from_i64(x, format).unwrap())
        .collect::<Vec<_>>();
        let run = |block: usize| {
            let mut stream = InterleavedResamplerBig::new_with_input_delay(
                2,
                filter.ratio(),
                Arc::clone(&bank),
                filter.input_delay_frames(),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in input.chunks(block * 2) {
                stream
                    .push_interleaved_finite_into(
                        chunk,
                        FrameCountPolicy::NearestTiesToEven,
                        &mut output,
                    )
                    .unwrap();
            }
            let target = sexrate::output_frames_for_input(
                7,
                filter.ratio(),
                FrameCountPolicy::NearestTiesToEven,
            )
            .unwrap();
            stream.finish_exact_frames(target, &mut output).unwrap();
            output
        };
        let expected = run(1);
        assert_eq!(expected, run(3));
        assert_eq!(expected, run(4096));
        assert!(expected.iter().any(|x| x.raw() != &0));
        assert!(expected.iter().all(|x| x.format() == format));
    }
}

#[test]
fn optimized_preflight_counts_global_solves_and_bounds_precision_memory_and_work() {
    for equiripple in [false, true] {
        let request = spec(equiripple, 96);
        let work = request.preflight(Limits::default()).unwrap();
        assert_eq!(work.coefficient_count, 14);
        for limits in [
            Limits {
                max_coefficients: 13,
                ..Default::default()
            },
            Limits {
                max_grid_points: 1,
                ..Default::default()
            },
            Limits {
                max_work: work.terms - 1,
                ..Default::default()
            },
            Limits {
                max_storage_bytes: work.storage_bytes - 1,
                ..Default::default()
            },
            Limits {
                max_precision_bits: 128,
                ..Default::default()
            },
        ] {
            assert!(matches!(design(&request, limits), Err(Error::Limit { .. })));
        }
        let mut precision = request.clone();
        precision.coefficient_fractional_bits = 160;
        assert!(matches!(
            design(&precision, Limits::default()),
            Err(Error::Design(
                DesignError::CoefficientWorkingPrecisionTooLow { .. }
            ))
        ));
        let mut huge = request;
        match &mut huge.method {
            Method::GlobalLeastSquares(s) => s.taps_per_phase = usize::MAX,
            Method::Equiripple(s) => s.taps_per_phase = usize::MAX,
        }
        assert!(matches!(
            huge.preflight(Limits::default()),
            Err(Error::Limit {
                resource: "coefficients",
                ..
            })
        ));
    }
    let mut many = spec(false, 96);
    if let Method::GlobalLeastSquares(s) = &mut many.method {
        s.ratio = RateRatio::from_fraction(17, 18).unwrap();
    }
    let work = many.preflight(Limits::default()).unwrap();
    assert_eq!(
        work.terms,
        52 * 52 * 52 + 103 * 64 + 52 * 32 + 3 * 52 * 52 + 4 * 17 * 7
    );
    assert_eq!(work.solver_dimension, 52);
    assert_eq!(work.global_prototype_length, Some(103));
    assert!(
        design(
            &many,
            Limits {
                max_work: work.terms - 1,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn optimized_precision_feedback_is_bounded_and_method_preserving() {
    let target = ErrorFloor::new(80).unwrap();
    for equiripple in [false, true] {
        let initial = spec(equiripple, 8);
        let result = refine_quantization(&initial, target, false, SearchLimits::default()).unwrap();
        assert_eq!(result.attempts.len(), 2);
        assert!(!result.attempts[0].coefficient_budget_passed);
        assert!(result.attempts[1].coefficient_budget_passed);
        assert_eq!(result.design.spec.method, initial.method);
        assert_eq!(result.design.spec.coefficient_fractional_bits, 24);
        assert_eq!(result.charged_coefficients, 28);
        assert_eq!(result.charged_work, 2 * result.attempts[0].work.terms);
        // Fine coefficients cannot repair a deliberately short filter.
        let (_, response) = analyze_against(&result.design, 65, target).unwrap();
        assert!(!response.passband_deviation_meets_target || !response.stopband_meets_target);
        let error = refine_quantization(
            &initial,
            target,
            false,
            SearchLimits {
                max_attempts: 1,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.attempts.len(), 1);
        let error = refine_quantization(
            &initial,
            target,
            false,
            SearchLimits {
                max_total_coefficients: 14,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.attempts.len(), 1);
    }
}

#[test]
fn optimized_precision_feedback_respects_explicit_mpfr_width() {
    for equiripple in [false, true] {
        let initial = spec(equiripple, 64);
        let target = ErrorFloor::new(500).unwrap();
        let error =
            refine_quantization(&initial, target, false, SearchLimits::default()).unwrap_err();
        assert_eq!(error.attempts.len(), 1);
        assert!(
            error
                .reason
                .contains("at least 144 working bits (C+64), got 128")
        );
        let auto = refine_quantization(&initial, target, true, SearchLimits::default()).unwrap();
        assert!(auto.design.spec.coefficient_fractional_bits > 64);
        assert!(
            auto.design.spec.method.precision()
                >= auto.design.spec.coefficient_fractional_bits + 64
        );
        assert!(coefficient_budget_passed(&auto.design, target).unwrap());
    }
}

#[test]
fn optimized_reports_and_proofs_bind_the_actual_quantized_bank() {
    for equiripple in [false, true] {
        let mut request = spec(equiripple, 96);
        match &mut request.method {
            Method::GlobalLeastSquares(s) => {
                s.taps_per_phase = 31;
                s.rolloff = Fraction::new(3, 4).unwrap();
                s.grid_points_per_band = 256;
            }
            Method::Equiripple(s) => {
                s.taps_per_phase = 31;
                s.rolloff = Fraction::new(3, 4).unwrap();
            }
        }
        let filter = design(&request, Limits::default()).unwrap();
        let target = ErrorFloor::new(40).unwrap();
        let (_, response) = analyze_against(&filter, 129, target).unwrap();
        assert!(response.passband_deviation_meets_target && response.stopband_meets_target);
        assert!(coefficient_budget_passed(&filter, target).unwrap());
        let proof = certify(&filter, target, certificate::Limits::default()).unwrap();
        assert_eq!(proof.outcome, certificate::Outcome::Certified);
        let images = analyze_harmonics(&filter, 33, target, harmonics::Limits::default()).unwrap();
        assert!(images.meets_target(), "{images:?}");
        let budget = certify(
            &filter,
            target,
            certificate::Limits {
                max_work: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(matches!(
            budget.outcome,
            certificate::Outcome::Inconclusive(_)
        ));
    }
}

#[test]
fn optimized_global_ls_repairs_the_legacy_transition_band_image() {
    let mut request = spec(false, 96);
    let Method::GlobalLeastSquares(core) = &mut request.method else {
        unreachable!()
    };
    core.taps_per_phase = 31;
    core.rolloff = Fraction::new(3, 4).unwrap();
    core.grid_points_per_band = 256;
    let legacy = design_least_squares(core).unwrap();
    let target = ErrorFloor::new(40).unwrap();
    let proof = certificate::certify_bank(
        core.ratio,
        core.rolloff,
        legacy.bank().phase_count(),
        62,
        target,
        certificate::Limits::default(),
        |phase| {
            Ok(legacy
                .bank()
                .phase(phase as u64)
                .unwrap()
                .iter()
                .map(|c| Integer::from(c.raw()))
                .collect())
        },
    )
    .unwrap();
    assert_eq!(proof.outcome, certificate::Outcome::Certified);
    let legacy_images = harmonics::analyze_bank(
        harmonics::Spec {
            ratio: core.ratio,
            rolloff: core.rolloff,
            taps_per_phase: 31,
            fractional_bits: 62,
            input_delay_frames: 15,
            precision_bits: core.working_precision_bits,
        },
        33,
        target,
        harmonics::Limits::default(),
        |phase| {
            Ok(legacy
                .bank()
                .phase(phase as u64)
                .unwrap()
                .iter()
                .map(|c| Integer::from(c.raw()))
                .collect())
        },
    )
    .unwrap();
    assert!(legacy_images.main_complex_error_meets_target);
    assert!(legacy_images.stopband_meets_target);
    assert!(!legacy_images.image_peak_meets_target);
    assert_eq!(
        legacy_images.image_peak.as_ref().unwrap().input_frequency,
        rug::Rational::from((47, 192))
    );
    let global = design(&request, Limits::default()).unwrap();
    assert!(
        analyze_harmonics(&global, 33, target, harmonics::Limits::default())
            .unwrap()
            .meets_target()
    );
    assert!(matches!(global.spec.method, Method::GlobalLeastSquares(_)));
}

#[test]
fn optimized_global_ls_meets_fast_band_geometry_and_80_db_target() {
    let mut request = spec(false, 96);
    let Method::GlobalLeastSquares(s) = &mut request.method else {
        unreachable!()
    };
    s.taps_per_phase = 129;
    s.grid_points_per_band = 256;
    let filter = design(&request, Limits::default()).unwrap();
    let target = ErrorFloor::new(80).unwrap();
    let (response, compliance) = analyze_against(&filter, 257, target).unwrap();
    assert!(
        compliance.passband_deviation_meets_target && compliance.stopband_meets_target,
        "{response:?}"
    );
    assert_eq!(
        certify(&filter, target, certificate::Limits::default())
            .unwrap()
            .outcome,
        certificate::Outcome::Certified
    );
    let images = analyze_harmonics(&filter, 65, target, harmonics::Limits::default()).unwrap();
    assert!(images.meets_target(), "{images:?}");
}

#[test]
fn optimized_remez_selector_preserves_single_point_sign_lobes() {
    let grid = (0..5)
        .map(|index| RemezGridPoint {
            frequency: Float::with_val(160, index) / 8,
            desired: Float::with_val(160, 0),
            weight: Float::with_val(160, 1),
            band: 0,
        })
        .collect::<Vec<_>>();
    let errors = [1, -2, 3, -4, 5].map(|error| Float::with_val(160, error));
    assert_eq!(
        select_remez_extrema(&grid, &errors, 5).unwrap(),
        vec![0, 1, 2, 3, 4]
    );
}

#[test]
fn optimized_remez_seed_tracks_band_width_not_grid_density() {
    let mut request = spec(true, 96);
    let Method::Equiripple(core) = &mut request.method else {
        unreachable!()
    };
    core.taps_per_phase = 129;
    let required = 130;
    let grid = build_remez_grid(core, required * core.grid_density as usize, 160);
    let extrema = initial_remez_extrema(&grid, required, 160);
    assert_eq!(extrema.len(), required);
    assert_eq!(extrema[0], 0);
    assert_eq!(extrema[required - 1], grid.len() - 1);
    assert!(extrema.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        extrema
            .iter()
            .filter(|index| grid[**index].band == 0)
            .count(),
        37
    );
    assert_eq!(remez_work(u128::MAX, 1, 1), None);
    assert_eq!(remez_work(2, u128::MAX, 1), None);
}

#[test]
fn optimized_remez_native_preflight_charges_the_same_exchange_work() {
    let mut request = spec(true, 96);
    let Method::Equiripple(core) = &mut request.method else {
        unreachable!()
    };
    core.taps_per_phase = 65;
    let expected = 115_772_704;
    assert_eq!(
        design_equiripple(core),
        Err(DesignError::RemezWorkBudgetExceeded {
            requested: expected,
            maximum: MAX_REMEZ_WORK,
        })
    );
    assert!(
        matches!(request.preflight(Limits::default()), Err(Error::Limit {
        resource: "structural work", required, maximum
    }) if required == expected as u128 && maximum == MAX_REMEZ_WORK as u64)
    );
}

#[test]
fn optimized_remez_convergence_requires_alternation_and_matching_ripple() {
    let mut errors = [1, 0, -1, 0, 1].map(|error| Float::with_val(160, error));
    let extrema = [0, 2, 4];
    let ripple = Float::with_val(160, 1);
    assert!(remez_grid_converged(&errors, &extrema, &ripple, 160));
    errors[1] = Float::with_val(160, 2);
    assert!(!remez_grid_converged(&errors, &extrema, &ripple, 160));
    errors[1] = Float::with_val(160, 0);
    errors[2] /= 2;
    assert!(!remez_grid_converged(&errors, &extrema, &ripple, 160));
    errors[2] = Float::with_val(160, 1);
    assert!(!remez_grid_converged(&errors, &extrema, &ripple, 160));
    errors[2] = Float::with_val(160, -1);
    assert!(!remez_grid_converged(
        &errors,
        &extrema,
        &Float::with_val(160, 0),
        160
    ));
}

#[test]
fn optimized_remez_large_exchange_meets_unchanged_80_db_target() {
    let mut request = spec(true, 96);
    let Method::Equiripple(s) = &mut request.method else {
        unreachable!()
    };
    s.taps_per_phase = 129;
    s.working_precision_bits = 160;
    let limits = Limits {
        max_work: 1_000_000_000,
        ..Default::default()
    };
    let filter = design(&request, limits).unwrap();
    let target = ErrorFloor::new(80).unwrap();
    let (response, compliance) = analyze_against(&filter, 257, target).unwrap();
    assert!(
        compliance.passband_deviation_meets_target && compliance.stopband_meets_target,
        "{response:?}"
    );
    assert_eq!(
        certify(&filter, target, certificate::Limits::default())
            .unwrap()
            .outcome,
        certificate::Outcome::Certified
    );
    let images = analyze_harmonics(&filter, 65, target, harmonics::Limits::default()).unwrap();
    assert!(images.meets_target(), "{images:?}");
    assert!(coefficient_budget_passed(&filter, target).unwrap());
    let Method::Equiripple(core) = &mut request.method else {
        unreachable!()
    };
    core.working_precision_bits = 512;
    let higher_precision = design(&request, limits).unwrap();
    for phase in 0..filter.ratio().up() {
        assert_eq!(
            filter.bank().phase(phase).unwrap(),
            higher_precision.bank().phase(phase).unwrap()
        );
    }
    assert!(coefficient_budget_passed(&higher_precision, target).unwrap());
}

#[test]
fn optimized_remez_selector_budget_includes_tables_and_rejects_plateaus() {
    let request = spec(true, 96);
    let work = request.preflight(Limits::default()).unwrap();
    let e = work.solver_dimension;
    let k = 2 * e + 4;
    let g = work.grid_points;
    assert_eq!(work.extrema_candidate_limit, Some(k));
    assert_eq!(
        work.terms,
        64 * (e * e * e + e * e + 2 * g * e + 8 * g + 8 * e + e * k * k) + 4 * g + 12 * e + 4 * 14
    );
    let float_bytes = 64 + 160_u64.div_ceil(64) * 8;
    let slots = e * e + 8 * e + 8 * g + 13 + 4 * 7 + 64 + (e + 1) * k;
    assert_eq!(
        work.storage_bytes,
        slots * float_bytes + (14 + 7 + 64) * 80 + 16 * (e + 1) * k + 128 * e + 16 * k
    );
    assert!(matches!(
        request.preflight(Limits {
            max_work: work.terms - 1,
            ..Default::default()
        }),
        Err(Error::Limit {
            resource: "structural work",
            ..
        })
    ));
    assert!(matches!(
        request.preflight(Limits {
            max_storage_bytes: work.storage_bytes - 1,
            ..Default::default()
        }),
        Err(Error::Limit {
            resource: "estimated storage bytes",
            ..
        })
    ));
    let grid = build_remez_grid(
        match &request.method {
            Method::Equiripple(s) => s,
            _ => unreachable!(),
        },
        32,
        160,
    );
    let errors = vec![Float::with_val(160, 1); grid.len()];
    assert_eq!(
        select_remez_extrema(&grid, &errors, e as usize),
        Err(DesignError::RemezExtremaBudgetExceeded {
            found: k as usize + 1,
            maximum: k as usize
        })
    );
}
