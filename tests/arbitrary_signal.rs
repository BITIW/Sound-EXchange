use sexdsp::arbitrary::{BigPipeline, BigStage};
use sexfir::{BigKaiserSpec, Fraction, KaiserSpec, design_kaiser_big};
use sexq::{BigQ, BigQFormat, OverflowPolicy, RoundingMode};
use sexrate::{
    FrameCountPolicy, InterleavedResamplerBig, PolyphaseFirBig, RateRatio, output_frames_for_input,
};
use std::sync::Arc;

#[test]
fn real_pcm_survives_two_hundred_bits_of_attenuation_through_designed_fir() {
    let round = RoundingMode::NearestTiesToEven;
    let overflow = OverflowPolicy::Error;
    let signal_format = BigQFormat::new(65, 512).unwrap();
    let coefficient_format = BigQFormat::new(202, 256).unwrap();
    let one = BigQ::one(coefficient_format).unwrap();
    let attenuation = one.shift_value(-200, round, overflow).unwrap().value;
    let restoration = one.shift_value(200, round, overflow).unwrap().value;
    let ratio = RateRatio::from_fraction(160, 147).unwrap();
    let designed = design_kaiser_big(&BigKaiserSpec {
        core: KaiserSpec {
            ratio,
            taps_per_phase: 17,
            rolloff: Fraction::new(9, 10).unwrap(),
            beta: Fraction::new(5, 1).unwrap(),
            working_precision_bits: 192,
            quantization_rounding: round,
        },
        coefficient_fractional_bits: 96,
        accumulator_bits: 192,
    })
    .unwrap();
    let bank = Arc::new(
        PolyphaseFirBig::from_q63_bank(designed.bank(), signal_format, signal_format, None)
            .unwrap(),
    );
    let input = (0..101)
        .map(|frame| {
            BigQ::from_signed_pcm(
                (frame * 65_537 + 17) % 4_194_304 - 2_097_152,
                24,
                signal_format,
                round,
                overflow,
            )
            .unwrap()
            .value
        })
        .collect::<Vec<_>>();
    let target = output_frames_for_input(
        input.len() as u64,
        ratio,
        FrameCountPolicy::NearestTiesToEven,
    )
    .unwrap();
    let run = |attenuated: bool, block: usize| {
        let mut before = BigPipeline::new(
            1,
            signal_format,
            if attenuated {
                vec![BigStage::gain(1, signal_format, attenuation.clone(), round).unwrap()]
            } else {
                vec![]
            },
        )
        .unwrap();
        let mut after = BigPipeline::new(
            1,
            signal_format,
            if attenuated {
                vec![BigStage::gain(1, signal_format, restoration.clone(), round).unwrap()]
            } else {
                vec![]
            },
        )
        .unwrap();
        let mut stream = InterleavedResamplerBig::new_with_input_delay(
            1,
            ratio,
            bank.clone(),
            designed.input_delay_frames(),
            round,
            overflow,
        )
        .unwrap();
        let mut filtered = Vec::new();
        let mut prepared = Vec::new();
        let mut restored = Vec::new();
        let mut pcm = Vec::new();
        for chunk in input.chunks(block) {
            prepared.clear();
            before.process_chunk(chunk, &mut prepared).unwrap();
            if attenuated {
                assert!(prepared.iter().all(|sample| {
                    sample
                        .rescale(BigQFormat::new(65, 63).unwrap(), round, overflow)
                        .unwrap()
                        .value
                        .raw_i128()
                        == Some(0)
                }));
            }
            filtered.clear();
            stream
                .push_interleaved_finite_into(
                    &prepared,
                    FrameCountPolicy::NearestTiesToEven,
                    &mut filtered,
                )
                .unwrap();
            restored.clear();
            after.process_chunk(&filtered, &mut restored).unwrap();
            pcm.extend(
                restored
                    .iter()
                    .map(|sample| sample.to_signed_pcm(24, round, overflow).unwrap().value),
            );
        }
        prepared.clear();
        before.finish(&mut prepared).unwrap();
        assert!(prepared.is_empty()); // gain has no tail
        filtered.clear();
        stream.finish_exact_frames(target, &mut filtered).unwrap();
        restored.clear();
        after.process_chunk(&filtered, &mut restored).unwrap();
        after.finish(&mut restored).unwrap();
        pcm.extend(
            restored
                .iter()
                .map(|sample| sample.to_signed_pcm(24, round, overflow).unwrap().value),
        );
        assert_eq!(pcm.len() as u64, target);
        pcm
    };
    let reference = run(false, 4096);
    assert!(reference.iter().any(|sample| sample.abs() > 1_000_000));
    for block in [1, 7, 4096] {
        assert_eq!(run(true, block), reference, "block={block}");
    }
}
