use rug::{Integer, Rational};
use sexdsp::arbitrary::{BigPipeline, BigStage};
use sexplan::numerical::{ExactRatio, NumericalStage, evaluate_numerical_budget};
use sexq::{BigQ, BigQFormat, OverflowPolicy, RoundingMode, round_div_integer};
use sexrate::{
    BigFirSpec, FrameCountPolicy, InterleavedResamplerBig, PolyphaseFirBig, RateRatio,
    output_frames_for_input,
};
use std::sync::Arc;

const ROUND: RoundingMode = RoundingMode::NearestTiesToEven;
const ERROR: OverflowPolicy = OverflowPolicy::Error;
fn exact(n: i128, d: u64) -> ExactRatio {
    ExactRatio {
        numerator: n,
        denominator: d,
    }
}
fn linear(name: &str, values: &[(i128, u64)]) -> NumericalStage {
    NumericalStage::Linear {
        name: name.into(),
        rows: vec![values.iter().map(|(n, d)| exact(*n, *d)).collect()],
    }
}
fn coefficient(n: i128, d: u64, format: BigQFormat) -> BigQ {
    let raw = round_div_integer(
        &(Integer::from(n) << format.fractional_bits()),
        &Integer::from(d),
        ROUND,
    )
    .unwrap();
    BigQ::from_raw(raw, format).unwrap()
}

#[test]
fn complete_big_pipeline_fits_independent_rational_error_bound() {
    let source = (0..79)
        .map(|n| [(n * 71 % 256) - 128, 127 - (n * 37 % 256)])
        .collect::<Vec<_>>();
    // Independently computed exact-effect reference, including convolution tail.
    let mut previous_input = [Rational::new(), Rational::new()];
    let mut previous_output = [Rational::new(), Rational::new()];
    let mut mixed: Vec<Rational> = Vec::new();
    for frame in &source {
        let mut dc = [Rational::new(), Rational::new()];
        for channel in 0..2 {
            let gain = Rational::from((frame[channel], 128)) * Rational::from((3, 5));
            dc[channel] = gain.clone() - &previous_input[channel]
                + previous_output[channel].clone() * Rational::from((10, 11));
            previous_input[channel] = gain;
            previous_output[channel] = dc[channel].clone();
        }
        mixed.push(dc[0].clone() / 3 - dc[1].clone() * Rational::from((2, 7)));
    }
    let convolution = [
        Rational::from(1),
        Rational::from((-1, 5)),
        Rational::from((2, 9)),
    ];
    let convolved = (0..source.len() + 2)
        .map(|n| {
            let mut value = Rational::new();
            for (k, h) in convolution.iter().enumerate() {
                if let Some(x) = n.checked_sub(k).and_then(|i| mixed.get(i)) {
                    value += x.clone() * h;
                }
            }
            value
        })
        .collect::<Vec<_>>();
    let ideal_phases = [
        vec![(-1, 7), (5, 7), (3, 7)],
        vec![(1, 11), (4, 11), (6, 11)],
        vec![(2, 13), (6, 13), (5, 13)],
    ];
    let ratio = RateRatio::from_fraction(3, 2).unwrap();
    let target = output_frames_for_input(
        convolved.len() as u64,
        ratio,
        FrameCountPolicy::NearestTiesToEven,
    )
    .unwrap();
    let ideal_output = (0..target as usize)
        .map(|t| {
            let coordinate = t * 2;
            let anchor = coordinate / 3;
            let phase = coordinate % 3;
            let mut y = Rational::new();
            for (k, (n, d)) in ideal_phases[phase].iter().enumerate() {
                if let Some(x) = anchor.checked_sub(k).and_then(|i| convolved.get(i)) {
                    y += x.clone() * Rational::from((*n, *d));
                }
            }
            y * Rational::from((17, 31))
        })
        .collect::<Vec<_>>();
    for (fractional, coefficient_bits) in [(16, 7), (63, 32), (127, 96), (4096, 4096)] {
        let format = BigQFormat::new(65, fractional).unwrap();
        let cf = BigQFormat::new(2, coefficient_bits).unwrap();
        let coefficients = |row: &[(i128, u64)]| {
            row.iter()
                .map(|(n, d)| coefficient(*n, *d, cf))
                .collect::<Vec<_>>()
        };
        let effects = vec![
            BigStage::gain(2, format, coefficient(3, 5, cf), ROUND).unwrap(),
            BigStage::dc_block(2, format, coefficient(10, 11, cf), ROUND).unwrap(),
            BigStage::mix(2, 1, format, coefficients(&[(1, 3), (-2, 7)]), ROUND).unwrap(),
            BigStage::convolution(1, format, coefficients(&[(1, 1), (-1, 5), (2, 9)]), ROUND)
                .unwrap(),
        ];
        let mut bank_coefficients = Vec::new();
        for phase in &ideal_phases {
            let mut row = coefficients(
                &phase
                    .iter()
                    .map(|(n, d)| (i128::from(*n), *d as u64))
                    .collect::<Vec<_>>(),
            );
            let largest = row
                .iter()
                .enumerate()
                .max_by_key(|(_, c)| c.raw().clone().abs())
                .unwrap()
                .0;
            let sum = row.iter().fold(Integer::new(), |sum, c| sum + c.raw());
            let corrected =
                row[largest].raw().clone() + (Integer::from(1) << coefficient_bits) - sum;
            row[largest] = BigQ::from_raw(corrected, cf).unwrap();
            bank_coefficients.extend(row);
        }
        let bank = Arc::new(
            PolyphaseFirBig::new(
                BigFirSpec {
                    phase_count: 3,
                    taps_per_phase: 3,
                    input_format: format,
                    output_format: format,
                    coefficient_format: cf,
                    accumulator_bits: None,
                },
                bank_coefficients,
            )
            .unwrap(),
        );
        let budget = evaluate_numerical_budget(
            &[
                linear("gain", &[(3, 5)]),
                NumericalStage::DcBlock {
                    radius: exact(10, 11),
                },
                linear("mix", &[(1, 3), (-2, 7)]),
                linear("convolution", &[(1, 1), (-1, 5), (2, 9)]),
                NumericalStage::DesignedFir {
                    taps_per_phase: 3,
                    coefficient_fractional_bits: coefficient_bits,
                },
                NumericalStage::Normalize,
            ],
            fractional,
            coefficient_bits,
        )
        .unwrap();
        let error_bound = Rational::from((
            budget.final_bound.total_error_raw(),
            Integer::from(1) << budget.bound_fractional_bits,
        ));
        let input = source
            .iter()
            .flatten()
            .map(|code| {
                BigQ::from_signed_pcm(i64::from(*code), 8, format, ROUND, ERROR)
                    .unwrap()
                    .value
            })
            .collect::<Vec<_>>();
        let mut previous = None;
        for block in [1, 7, 4096] {
            let mut pipeline = BigPipeline::new(2, format, effects.clone()).unwrap();
            let mut stream = InterleavedResamplerBig::new_with_input_delay(
                1,
                ratio,
                bank.clone(),
                0,
                ROUND,
                ERROR,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in input.chunks(block * 2) {
                let mut intermediate = Vec::new();
                pipeline.process_chunk(chunk, &mut intermediate).unwrap();
                stream
                    .push_interleaved_finite_into(
                        &intermediate,
                        FrameCountPolicy::NearestTiesToEven,
                        &mut output,
                    )
                    .unwrap();
            }
            let mut tail = Vec::new();
            pipeline.finish(&mut tail).unwrap();
            stream
                .push_interleaved_finite_into(
                    &tail,
                    FrameCountPolicy::NearestTiesToEven,
                    &mut output,
                )
                .unwrap();
            stream.finish_exact_frames(target, &mut output).unwrap();
            let output = output
                .into_iter()
                .map(|s| {
                    s.scale_ratio(&Integer::from(17), &Integer::from(31), ROUND, ERROR)
                        .unwrap()
                        .value
                })
                .collect::<Vec<_>>();
            assert_eq!(output.len(), ideal_output.len());
            for (t, (actual, ideal)) in output.iter().zip(&ideal_output).enumerate() {
                let actual = Rational::from((actual.raw().clone(), Integer::from(1) << fractional));
                assert!(
                    (actual - ideal).abs() <= error_bound,
                    "F={fractional}, C={coefficient_bits}, t={t}, block={block}"
                );
            }
            if let Some(previous) = &previous {
                assert_eq!(&output, previous);
            }
            previous = Some(output);
        }
    }
}
