use super::*;
use crate::{CausalResamplerQ63, CausalResamplerWideQ63, output_frames_for_input};
use rug::Integer;
use sexq::{Q1_63, Q2_62, WideQ63, round_shift_integer};

const ROUND: RoundingMode = RoundingMode::NearestTiesToEven;
const ERROR: OverflowPolicy = OverflowPolicy::Error;

#[test]
fn automatic_signed_width_is_minimal_and_does_not_round_intermediate_products() {
    for fractional in [0, 31, 63, 4096] {
        let format = BigQFormat::new(1, fractional).unwrap();
        let cf = BigQFormat::new(2, fractional).unwrap();
        let coefficients = vec![BigQ::one(cf).unwrap(), BigQ::zero(cf)];
        let spec = BigFirSpec {
            phase_count: 1,
            taps_per_phase: 2,
            input_format: format,
            output_format: format,
            coefficient_format: cf,
            accumulator_bits: None,
        };
        let bank = PolyphaseFirBig::new(spec, coefficients.clone()).unwrap();
        let bits = 1 + 2 * fractional;
        assert_eq!(bank.required_accumulator_bits(), bits);
        assert_eq!(bank.accumulator_bits(), bits);
        for raw in [format.minimum_raw(), format.maximum_raw()] {
            let sample = BigQ::from_raw(raw, format).unwrap();
            assert_eq!(
                bank.convolve(0, &[sample.clone(), sample.clone()], ROUND, ERROR)
                    .unwrap()
                    .value,
                sample
            );
        }
        if bits > 1 {
            assert!(
                matches!(PolyphaseFirBig::new(BigFirSpec { accumulator_bits: Some(bits - 1), ..spec }, coefficients),
                Err(PolyphaseError::BigAccumulatorTooNarrow { required_bits, .. }) if required_bits == bits)
            );
        }
        let two_phases = PolyphaseFirBig::new(
            BigFirSpec {
                phase_count: 2,
                ..spec
            },
            vec![
                BigQ::one(cf).unwrap(),
                BigQ::zero(cf),
                BigQ::from_raw(-(Integer::from(1) << fractional), cf).unwrap(),
                BigQ::zero(cf),
            ],
        )
        .unwrap();
        assert_eq!(two_phases.required_accumulator_bits(), bits + 1);
    }
}

fn identity(format: BigQFormat) -> Arc<PolyphaseFirBig> {
    let c = BigQFormat::new(2, 257).unwrap();
    Arc::new(
        PolyphaseFirBig::new(
            BigFirSpec {
                phase_count: 1,
                taps_per_phase: 3,
                input_format: format,
                output_format: format,
                coefficient_format: c,
                accumulator_bits: None,
            },
            vec![BigQ::zero(c), BigQ::one(c).unwrap(), BigQ::zero(c)],
        )
        .unwrap(),
    )
}

#[test]
fn identity_retains_arbitrary_fractional_and_integer_bits_in_history() {
    for (integer, fractional) in [(1, 256), (65, 4096), (4096, 4096), (8, 0)] {
        let format = BigQFormat::new(integer, fractional).unwrap();
        let bank = identity(format);
        let input = vec![
            BigQ::from_raw(format.minimum_raw(), format).unwrap(),
            BigQ::from_i64(1, format).unwrap(),
            BigQ::zero(format),
            BigQ::from_raw(format.maximum_raw(), format).unwrap(),
            BigQ::from_i64(-1, format).unwrap(),
        ];
        for block in [1, 3, 5] {
            let mut stream = CausalResamplerBig::new_with_input_delay(
                RateRatio::from_fraction(1, 1).unwrap(),
                bank.clone(),
                1,
                ROUND,
                ERROR,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in input.chunks(block) {
                stream
                    .push_finite_into(chunk, FrameCountPolicy::NearestTiesToEven, &mut output)
                    .unwrap();
            }
            let stats = stream.finish_exact_frames(5, &mut output).unwrap();
            assert_eq!(output, input, "Q{integer}.{fractional} block={block}");
            assert_eq!(
                stats,
                StreamStats {
                    input_frames: 5,
                    output_frames: 5,
                    saturated_outputs: 0
                }
            );
        }
    }
}

#[test]
fn sub_q63_lsb_products_round_once_for_all_modes_and_signs() {
    let format = BigQFormat::new(3, 256).unwrap();
    let coefficient_format = BigQFormat::new(2, 4096).unwrap();
    let half = BigQ::one(coefficient_format)
        .unwrap()
        .shift_value(-1, ROUND, ERROR)
        .unwrap()
        .value;
    let bank = PolyphaseFirBig::new(
        BigFirSpec {
            phase_count: 1,
            taps_per_phase: 3,
            input_format: format,
            output_format: format,
            coefficient_format,
            accumulator_bits: None,
        },
        vec![half; 3],
    )
    .unwrap();
    for (rounding, positive, negative) in [
        (RoundingMode::TowardZero, 1, -1),
        (RoundingMode::Floor, 1, -2),
        (RoundingMode::Ceiling, 2, -1),
        (ROUND, 2, -2),
        (RoundingMode::NearestTiesAwayFromZero, 2, -2),
    ] {
        for (raw, expected) in [(1, positive), (-1, negative)] {
            let input = vec![BigQ::from_i64(raw, format).unwrap(); 3];
            let result = bank.convolve(0, &input, rounding, ERROR).unwrap();
            assert_eq!(result.value.raw_i64(), Some(expected));
            assert!(!result.saturated);
        }
    }
}

#[test]
fn eight_thousand_bit_accumulator_cancels_before_final_signal_rounding() {
    let format = BigQFormat::new(3, 2000).unwrap();
    let coefficient_format = BigQFormat::new(2, 4096).unwrap();
    let one = BigQ::one(coefficient_format).unwrap();
    let minus_one = BigQ::from_raw(-one.raw().clone(), coefficient_format).unwrap();
    let spec = BigFirSpec {
        phase_count: 1,
        taps_per_phase: 3,
        input_format: format,
        output_format: format,
        coefficient_format,
        accumulator_bits: Some(8192),
    };
    let coefficients = vec![one.clone(), minus_one, one];
    let bank = PolyphaseFirBig::new(spec, coefficients.clone()).unwrap();
    assert!(bank.required_accumulator_bits() > 6000);
    assert_eq!(bank.accumulator_bits(), 8192);
    let maximum = BigQ::from_raw(format.maximum_raw(), format).unwrap();
    let low_bit = BigQ::from_i64(1, format).unwrap();
    assert_eq!(
        bank.convolve(
            0,
            &[maximum.clone(), maximum, low_bit.clone()],
            ROUND,
            ERROR
        )
        .unwrap()
        .value,
        low_bit
    );
    assert!(matches!(
        PolyphaseFirBig::new(
            BigFirSpec {
                accumulator_bits: Some(128),
                ..spec
            },
            coefficients
        ),
        Err(PolyphaseError::BigAccumulatorTooNarrow { .. })
    ));
}

#[test]
fn automatic_fir_width_exceeds_8192_and_retains_one_output_lsb_after_cancellation() {
    let format = BigQFormat::new(3, 4096).unwrap();
    let cf = BigQFormat::new(2, 4096).unwrap();
    let coefficient = BigQ::from_raw((Integer::from(1) << 4094) - 1, cf).unwrap();
    let negative = BigQ::from_raw(-coefficient.raw().clone(), cf).unwrap();
    let half = BigQ::from_raw(Integer::from(1) << 4095, cf).unwrap();
    let coefficients = vec![coefficient, negative, half.clone(), half, BigQ::zero(cf)];
    let spec = BigFirSpec {
        phase_count: 1,
        taps_per_phase: 5,
        input_format: format,
        output_format: format,
        coefficient_format: cf,
        accumulator_bits: None,
    };
    let bank = PolyphaseFirBig::new(spec, coefficients.clone()).unwrap();
    // Exact L1 = (3*2^4095 - 2); full-input peak = 2^4098.
    // Their product needs 8196 signed bits under the conservative L1 contract.
    assert_eq!(bank.required_accumulator_bits(), 8196);
    assert_eq!(bank.accumulator_bits(), 8196);
    assert!(matches!(
        PolyphaseFirBig::new(
            BigFirSpec {
                accumulator_bits: Some(8195),
                ..spec
            },
            coefficients
        ),
        Err(PolyphaseError::BigAccumulatorTooNarrow {
            phase: 0,
            required_bits: 8196,
            configured_bits: 8195
        })
    ));
    let large = BigQ::from_raw((Integer::from(1) << 4097) - 1, format).unwrap();
    let tiny = BigQ::from_i64(1, format).unwrap();
    let samples = [
        large.clone(),
        large,
        tiny.clone(),
        tiny.clone(),
        BigQ::zero(format),
    ];
    let result = bank.convolve(0, &samples, ROUND, ERROR).unwrap();
    assert_eq!(result.value, tiny);
    assert!(!result.saturated);
}

// Independent finite-file oracle: absolute u128 coordinates, no PhaseClock,
// Timeline, streaming history, or BigMac. Products are summed as raw integers.
fn reference(
    bank: &PolyphaseFirBig,
    input: &[BigQ],
    channels: usize,
    ratio: RateRatio,
    delay: u64,
    policy: FrameCountPolicy,
) -> Vec<BigQ> {
    let count = output_frames_for_input((input.len() / channels) as u64, ratio, policy).unwrap();
    let mut output = Vec::new();
    for frame in 0..count {
        let coordinate = u128::from(frame) * u128::from(ratio.down());
        let anchor = u128::from(delay) + coordinate / u128::from(ratio.up());
        let phase = (coordinate % u128::from(ratio.up())) as u64;
        for channel in 0..channels {
            let mut sum = BigQ::zero(bank.spec.input_format).raw().clone();
            for (tap, coefficient) in bank.phase(phase).unwrap().iter().enumerate() {
                if let Some(index) = anchor.checked_sub(tap as u128)
                    && index < (input.len() / channels) as u128
                {
                    sum += input[index as usize * channels + channel].raw().clone()
                        * coefficient.raw();
                }
            }
            let raw =
                round_shift_integer(&sum, bank.spec.coefficient_format.fractional_bits(), ROUND);
            output.push(BigQ::from_raw(raw, bank.spec.output_format).unwrap());
        }
    }
    output
}

#[test]
fn arbitrary_signal_matches_independent_oracle_for_ratios_channels_and_chunks() {
    let format = BigQFormat::new(65, 257).unwrap();
    for (up, down) in [(1, 5), (5, 4), (160, 147), (147, 160), (997, 1009)] {
        let ratio = RateRatio::from_fraction(up, down).unwrap();
        let coefficients = (0..ratio.up())
            .flat_map(|phase| {
                [
                    Q2_62::from_raw((1_i64 << 59) + phase as i64),
                    Q2_62::from_raw(1_i64 << 61),
                    Q2_62::from_raw(-(1_i64 << 60)),
                ]
            })
            .collect();
        let native = PolyphaseFirQ63::for_ratio(ratio, 3, coefficients).unwrap();
        let bank =
            Arc::new(PolyphaseFirBig::from_native_bank(&native, format, format, None).unwrap());
        for channels in [1, 3] {
            let input = (0..41 * channels)
                .map(|index| {
                    let raw = i64::from(index * 101 - 4000);
                    let mut value = BigQ::from_i64(raw, format).unwrap().raw().clone() << 193_u32;
                    value += index % 17 - 8; // deliberately nonzero below the Q*.63 grid
                    BigQ::from_raw(value, format).unwrap()
                })
                .collect::<Vec<_>>();
            for policy in [
                FrameCountPolicy::Floor,
                FrameCountPolicy::Ceiling,
                FrameCountPolicy::NearestTiesToEven,
            ] {
                let expected = reference(&bank, &input, channels as usize, ratio, 1, policy);
                for block in [1, 7, 41] {
                    let mut stream = InterleavedResamplerBig::new_with_input_delay(
                        channels as u16,
                        ratio,
                        bank.clone(),
                        1,
                        ROUND,
                        ERROR,
                    )
                    .unwrap();
                    let mut output = Vec::new();
                    for chunk in input.chunks(block * channels as usize) {
                        stream
                            .push_interleaved_finite_into(chunk, policy, &mut output)
                            .unwrap();
                    }
                    let target = output_frames_for_input(41, ratio, policy).unwrap();
                    let stats = stream.finish_exact_frames(target, &mut output).unwrap();
                    assert_eq!(
                        output, expected,
                        "{up}/{down}, channels={channels}, block={block}, policy={policy:?}"
                    );
                    assert_eq!(stats.input_frames, 41);
                    assert_eq!(stats.output_frames, target);
                }
            }
        }
    }
}

#[test]
fn matching_binary_points_agree_with_both_legacy_signal_domains() {
    let ratio = RateRatio::from_fraction(5, 4).unwrap();
    let native = Arc::new(
        PolyphaseFirQ63::for_ratio(ratio, 3, vec![Q2_62::from_raw(1_i64 << 60); 15]).unwrap(),
    );
    for wide in [false, true] {
        let format = BigQFormat::new(65, 63).unwrap();
        let bank =
            Arc::new(PolyphaseFirBig::from_native_bank(&native, format, format, None).unwrap());
        let input = (-20..20)
            .map(|n| WideQ63::from_raw(i128::from(n) << if wide { 75 } else { 55 }))
            .collect::<Vec<_>>();
        let big_input = input
            .iter()
            .map(|s| BigQ::from_i128(s.raw(), format).unwrap())
            .collect::<Vec<_>>();
        let mut big =
            CausalResamplerBig::new_with_input_delay(ratio, bank, 1, ROUND, ERROR).unwrap();
        let mut big_output = Vec::new();
        for chunk in big_input.chunks(7) {
            big.push_finite_into(chunk, FrameCountPolicy::NearestTiesToEven, &mut big_output)
                .unwrap();
        }
        let target =
            output_frames_for_input(40, ratio, FrameCountPolicy::NearestTiesToEven).unwrap();
        big.finish_exact_frames(target, &mut big_output).unwrap();
        let mut legacy_output = Vec::new();
        if wide {
            let mut stream = CausalResamplerWideQ63::new_with_input_delay(
                ratio,
                native.clone(),
                1,
                ROUND,
                ERROR,
            )
            .unwrap();
            stream
                .push_finite_wide_into(
                    &input,
                    FrameCountPolicy::NearestTiesToEven,
                    &mut legacy_output,
                )
                .unwrap();
            stream
                .finish_wide_exact_frames(target, &mut legacy_output)
                .unwrap();
        } else {
            let mut stream =
                CausalResamplerQ63::new_with_input_delay(ratio, native.clone(), 1, ROUND, ERROR)
                    .unwrap();
            let input = input
                .iter()
                .map(|s| Q1_63::from_raw(s.raw() as i64))
                .collect::<Vec<_>>();
            stream
                .push_finite_wide_into(
                    &input,
                    FrameCountPolicy::NearestTiesToEven,
                    &mut legacy_output,
                )
                .unwrap();
            stream
                .finish_wide_exact_frames(target, &mut legacy_output)
                .unwrap();
        }
        assert_eq!(
            big_output
                .iter()
                .map(|s| s.raw_i128().unwrap())
                .collect::<Vec<_>>(),
            legacy_output.iter().map(|s| s.raw()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn format_errors_poison_before_committing_interleaved_output() {
    let format = BigQFormat::new(5, 256).unwrap();
    let ratio = RateRatio::from_fraction(1, 1).unwrap();
    let mut stream =
        InterleavedResamplerBig::new_with_input_delay(2, ratio, identity(format), 1, ROUND, ERROR)
            .unwrap();
    let sentinel = BigQ::from_i64(7, format).unwrap();
    let mut output = vec![sentinel.clone()];
    let wrong = BigQ::zero(BigQFormat::new(5, 255).unwrap());
    assert!(matches!(
        stream.push_interleaved_into(&[sentinel.clone(), wrong], &mut output),
        Err(StreamError::Polyphase(PolyphaseError::BigArithmetic(
            BigQError::FormatMismatch { .. }
        )))
    ));
    assert_eq!(output, [sentinel]);
    assert_eq!(
        stream.push_interleaved_into(&[], &mut output),
        Err(StreamError::Poisoned)
    );
}

#[test]
fn arbitrary_history_remains_bounded_for_long_downsample_streams() {
    let format = BigQFormat::new(65, 4096).unwrap();
    let ratio = RateRatio::from_fraction(1, 1009).unwrap();
    let mut stream =
        CausalResamplerBig::new_with_input_delay(ratio, identity(format), 1, ROUND, ERROR).unwrap();
    let chunk = vec![BigQ::from_i64(1, format).unwrap(); 137];
    let mut output = Vec::new();
    for _ in 0..100 {
        stream
            .push_finite_into(&chunk, FrameCountPolicy::NearestTiesToEven, &mut output)
            .unwrap();
        assert!(stream.retained_input_frames() <= 1012);
        output.clear();
    }
    let target =
        output_frames_for_input(13700, ratio, FrameCountPolicy::NearestTiesToEven).unwrap();
    assert_eq!(
        stream
            .finish_exact_frames(target, &mut output)
            .unwrap()
            .input_frames,
        13700
    );
}

#[test]
fn designer_bank_is_shared_and_destination_precision_is_explicit() {
    let coefficient_format = BigQFormat::new(2, 96).unwrap();
    let coefficients = vec![BigQ::one(coefficient_format).unwrap()];
    let original = PolyphaseFirBigQ63::new(1, 1, coefficient_format, 192, coefficients).unwrap();
    let input_format = BigQFormat::new(3, 256).unwrap();
    let output_format = BigQFormat::new(3, 200).unwrap();
    let bank =
        PolyphaseFirBig::from_q63_bank(&original, input_format, output_format, None).unwrap();
    assert!(Arc::ptr_eq(&original.coefficients, &bank.coefficients));
    drop(original);
    let raw = (BigQ::from_i64(3, input_format).unwrap().raw().clone() << 55_u32) + 1;
    let sample = BigQ::from_raw(raw, input_format).unwrap();
    let result = bank.convolve(0, &[sample], ROUND, ERROR).unwrap();
    assert_eq!(result.value.format(), output_format);
    assert_eq!(result.value.raw_i64(), Some(2));
    assert!(bank.phase(1).is_err());
    assert!(bank.convolve(0, &[], ROUND, ERROR).is_err());
    let wrong_coefficient = BigQ::zero(input_format);
    assert!(matches!(
        PolyphaseFirBig::new(bank.spec(), vec![wrong_coefficient]),
        Err(PolyphaseError::BigArithmetic(
            BigQError::FormatMismatch { .. }
        ))
    ));
}

#[test]
fn arbitrary_tail_and_empty_finite_streams_use_the_same_duration_contract() {
    let format = BigQFormat::new(5, 256).unwrap();
    let ratio = RateRatio::from_fraction(1, 1).unwrap();
    let mut stream =
        CausalResamplerBig::new_with_input_delay(ratio, identity(format), 0, ROUND, ERROR).unwrap();
    let low_bit = BigQ::from_i64(1, format).unwrap();
    let mut output = Vec::new();
    stream
        .push_into(std::slice::from_ref(&low_bit), &mut output)
        .unwrap();
    let stats = stream.finish_into(&mut output).unwrap();
    assert_eq!(output, [BigQ::zero(format), low_bit, BigQ::zero(format)]);
    assert_eq!(stats.input_frames, 1);
    assert_eq!(stats.output_frames, 3);
    let mut empty =
        CausalResamplerBig::new_with_input_delay(ratio, identity(format), 1, ROUND, ERROR).unwrap();
    let mut output = Vec::new();
    empty
        .push_finite_into(&[], FrameCountPolicy::NearestTiesToEven, &mut output)
        .unwrap();
    assert_eq!(
        empty.finish_exact_frames(0, &mut output).unwrap(),
        StreamStats::default()
    );
    assert!(output.is_empty());
}

#[test]
fn arbitrary_output_overflow_is_explicit_and_errors_poison() {
    let format = BigQFormat::new(1, 256).unwrap();
    let coefficients = BigQFormat::new(3, 96).unwrap();
    let two = BigQ::one(coefficients)
        .unwrap()
        .shift_value(1, ROUND, ERROR)
        .unwrap()
        .value;
    let bank = Arc::new(
        PolyphaseFirBig::new(
            BigFirSpec {
                phase_count: 1,
                taps_per_phase: 1,
                input_format: format,
                output_format: format,
                coefficient_format: coefficients,
                accumulator_bits: None,
            },
            vec![two],
        )
        .unwrap(),
    );
    let maximum = BigQ::from_raw(format.maximum_raw(), format).unwrap();
    let ratio = RateRatio::from_fraction(1, 1).unwrap();
    let mut stream =
        CausalResamplerBig::new_with_input_delay(ratio, bank.clone(), 0, ROUND, ERROR).unwrap();
    let mut output = Vec::new();
    assert!(
        stream
            .push_into(std::slice::from_ref(&maximum), &mut output)
            .is_err()
    );
    assert!(output.is_empty());
    assert_eq!(
        stream.push_into(&[], &mut output),
        Err(StreamError::Poisoned)
    );
    let mut saturating = InterleavedResamplerBig::new_with_input_delay(
        2,
        ratio,
        bank,
        0,
        ROUND,
        OverflowPolicy::Saturate,
    )
    .unwrap();
    saturating
        .push_interleaved_finite_into(
            &[maximum.clone(), maximum.clone()],
            FrameCountPolicy::NearestTiesToEven,
            &mut output,
        )
        .unwrap();
    assert_eq!(
        saturating
            .finish_exact_frames(1, &mut output)
            .unwrap()
            .saturated_samples,
        2
    );
    assert_eq!(output, [maximum.clone(), maximum]);
}
