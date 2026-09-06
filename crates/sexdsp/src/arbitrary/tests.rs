use super::*;

const ROUND: RoundingMode = RoundingMode::NearestTiesToEven;
const ERROR: OverflowPolicy = OverflowPolicy::Error;

fn integer_coefficient(value: i64) -> BigQ {
    let format = BigQFormat::new(4, 4096).unwrap();
    let raw = BigQ::one(format).unwrap().raw().clone() * value;
    BigQ::from_raw(raw, format).unwrap()
}

#[test]
fn low_fractional_bits_survive_complete_stateful_chain_and_tail() {
    let format = BigQFormat::new(65, 4096).unwrap();
    let fraction = integer_coefficient(1)
        .shift_value(-3, ROUND, ERROR)
        .unwrap()
        .value;
    let make = || {
        BigPipeline::new(
            2,
            format,
            vec![
                BigStage::gain(2, format, integer_coefficient(4), ROUND).unwrap(),
                BigStage::dc_block(2, format, integer_coefficient(0), ROUND).unwrap(),
                BigStage::mix(2, 1, format, vec![integer_coefficient(1); 2], ROUND).unwrap(),
                BigStage::convolution(1, format, vec![fraction.clone(); 2], ROUND).unwrap(),
            ],
        )
        .unwrap()
    };
    let input = [1, 1, 3, 3, 0, 0].map(|raw| BigQ::from_i64(raw, format).unwrap());
    for block in [2, 4, 6] {
        let mut pipeline = make();
        assert!(pipeline.maximum_accumulator_bits() > 8192);
        let mut output = Vec::new();
        for chunk in input.chunks(block) {
            pipeline.process_chunk(chunk, &mut output).unwrap();
        }
        let stats = pipeline.finish(&mut output).unwrap();
        assert_eq!(
            output
                .iter()
                .map(|sample| sample.raw_i64().unwrap())
                .collect::<Vec<_>>(),
            [1, 3, -1, -3]
        );
        assert!(output.iter().all(|sample| sample.format() == format));
        assert_eq!(
            stats,
            StageStats {
                input_samples: 6,
                output_samples: 4,
                saturated_samples: 0
            }
        );
    }
}

#[test]
fn arbitrary_overflow_rolls_back_history_and_poisons_the_pipeline() {
    let format = BigQFormat::new(3, 256).unwrap();
    let mut pipeline = BigPipeline::new(
        1,
        format,
        vec![BigStage::convolution(1, format, vec![integer_coefficient(1); 2], ROUND).unwrap()],
    )
    .unwrap();
    let input = vec![BigQ::from_raw(format.maximum_raw(), format).unwrap(); 2];
    let sentinel = BigQ::from_i64(7, format).unwrap();
    let mut output = vec![sentinel.clone()];
    assert!(pipeline.process_chunk(&input, &mut output).is_err());
    assert_eq!(output, [sentinel]);
    assert_eq!(pipeline.stats(), StageStats::default());
    match &pipeline.stages[0].kind {
        Kind::Convolution {
            history, saw_input, ..
        } => {
            assert!(history[0].is_empty());
            assert!(!saw_input);
        }
        _ => unreachable!(),
    }
    assert_eq!(
        pipeline.process_chunk(&[], &mut output),
        Err(DspError::Poisoned.into())
    );
}

#[test]
fn format_boundaries_are_explicit_and_checked_even_without_effects() {
    let format = BigQFormat::new(5, 256).unwrap();
    let other = BigQFormat::new(5, 255).unwrap();
    let stage = BigStage::gain(1, other, integer_coefficient(1), ROUND).unwrap();
    assert!(matches!(
        BigPipeline::new(1, format, vec![stage]),
        Err(BigDspError::Arithmetic(BigQError::FormatMismatch { .. }))
    ));
    let mut pipeline = BigPipeline::new(1, format, vec![]).unwrap();
    let mut output = Vec::new();
    assert!(matches!(
        pipeline.process_chunk(&[BigQ::zero(other)], &mut output),
        Err(BigDspError::Arithmetic(BigQError::FormatMismatch { .. }))
    ));
    assert!(output.is_empty());
    assert_eq!(
        pipeline.process_chunk(&[], &mut output),
        Err(DspError::Poisoned.into())
    );
}

#[test]
fn dc_radius_may_use_q1_and_retains_fractional_feedback_state() {
    let format = BigQFormat::new(4, 1024).unwrap();
    let coefficient_format = BigQFormat::new(1, 1024).unwrap();
    let half = BigQ::from_raw(coefficient_format.minimum_raw() / -2, coefficient_format).unwrap();
    let mut pipeline = BigPipeline::new(
        1,
        format,
        vec![BigStage::dc_block(1, format, half, ROUND).unwrap()],
    )
    .unwrap();
    let input = [8, 0, 0, 0, 0].map(|raw| BigQ::from_i64(raw, format).unwrap());
    let mut output = Vec::new();
    pipeline.process_chunk(&input, &mut output).unwrap();
    assert_eq!(
        output
            .iter()
            .map(|sample| sample.raw_i64().unwrap())
            .collect::<Vec<_>>(),
        [8, -4, -2, -1, 0]
    );
}

#[test]
fn arbitrary_integer_headroom_and_empty_tail_are_supported() {
    let format = BigQFormat::new(4096, 4096).unwrap();
    let make = || {
        BigPipeline::new(
            2,
            format,
            vec![
                BigStage::mix(
                    2,
                    1,
                    format,
                    vec![integer_coefficient(1), integer_coefficient(-1)],
                    ROUND,
                )
                .unwrap(),
            ],
        )
        .unwrap()
    };
    let maximum = BigQ::from_raw(format.maximum_raw(), format).unwrap();
    let mut pipeline = make();
    let mut output = Vec::new();
    pipeline
        .process_chunk(&[maximum.clone(), maximum], &mut output)
        .unwrap();
    pipeline.finish(&mut output).unwrap();
    assert_eq!(output, [BigQ::zero(format)]);
    let mut empty = Vec::new();
    make().finish(&mut empty).unwrap();
    assert!(empty.is_empty());
}
