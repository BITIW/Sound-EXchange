use super::*;
use sexq::{OverflowPolicy, WideQ63};

const MODES: [DitherMode; 6] = [
    DitherMode::None,
    DitherMode::Tpdf,
    DitherMode::HighPassTpdf,
    DitherMode::NoiseShaped { order: 1 },
    DitherMode::NoiseShaped { order: 5 },
    DitherMode::NoiseShaped { order: 9 },
];
fn config(mode: DitherMode, bits: u32) -> DitherConfig {
    DitherConfig {
        target_bits: bits,
        mode,
        seed: 42,
        rounding: RoundingMode::NearestTiesToEven,
        overflow: sexq::OverflowPolicy::Saturate,
    }
}

#[test]
fn q63_big_dither_matches_native_for_all_modes_widths_and_rounding() {
    let format = BigQFormat::new(65, 63).unwrap();
    for mode in MODES {
        for bits in 1..=64 {
            for rounding in [
                RoundingMode::TowardZero,
                RoundingMode::Floor,
                RoundingMode::Ceiling,
                RoundingMode::NearestTiesToEven,
                RoundingMode::NearestTiesAwayFromZero,
            ] {
                let config = DitherConfig {
                    rounding,
                    ..config(mode, bits)
                };
                let mut native = DitherQuantizer::new(config).unwrap();
                let mut big = BigDitherQuantizer::new(format, config).unwrap();
                let mut random = SplitMix64::new(17);
                for index in 0..257 {
                    let raw = if index % 17 == 0 {
                        i128::from(i64::MAX) * 2
                    } else {
                        i128::from(random.next() as i64)
                    };
                    let sample = BigQ::from_i128(raw, format).unwrap();
                    assert_eq!(
                        big.quantize(&sample).unwrap(),
                        native.quantize_wide(WideQ63::from_raw(raw)).unwrap(),
                        "{mode:?} bits={bits} rounding={rounding:?}"
                    );
                }
                assert_eq!(big.stats(), native.stats());
                assert_eq!(
                    BigDitherQuantizer::maximum_excursion_raw(format, config).unwrap(),
                    Integer::from(DitherQuantizer::maximum_excursion_raw(config).unwrap())
                );
            }
        }
    }
}

#[test]
fn coarse_signal_grids_promote_exactly_without_inventing_dither_noise() {
    for fractional in [0, 7, 15] {
        let format = BigQFormat::new(3, fractional).unwrap();
        for bits in [16, 24, 64] {
            for mode in MODES {
                let config = config(mode, bits);
                let mut dither = BigDitherQuantizer::new(format, config).unwrap();
                assert_eq!(
                    BigDitherQuantizer::maximum_excursion_raw(format, config).unwrap(),
                    0
                );
                for raw in [0, 1, -1, 2, -2, 0] {
                    let sample = BigQ::from_i64(raw, format).unwrap();
                    assert_eq!(
                        dither.quantize(&sample).unwrap(),
                        sample
                            .to_signed_pcm(bits, config.rounding, config.overflow)
                            .unwrap()
                            .value,
                        "F={fractional}, B={bits}, mode={mode:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn arbitrary_dither_is_multichannel_seeded_chunk_invariant_and_bounded() {
    for fractional in [64, 127, 256, 4096] {
        let format = BigQFormat::new(65, fractional).unwrap();
        let input = (0..303)
            .map(|n| BigQ::from_i64(n - 151, format).unwrap())
            .collect::<Vec<_>>();
        for mode in MODES {
            let config = config(mode, 24);
            let run = |block: usize| {
                let mut dither = BigInterleavedDither::new(3, format, config).unwrap();
                let mut output = Vec::new();
                for chunk in input.chunks(block * 3) {
                    dither
                        .quantize_interleaved_into(chunk, &mut output)
                        .unwrap();
                }
                (output, dither.stats().unwrap())
            };
            assert_eq!(run(1), run(7));
            assert_eq!(run(1), run(4096));
            let mut quantizer = BigDitherQuantizer::new(format, config).unwrap();
            let bound = BigDitherQuantizer::maximum_excursion_raw(format, config).unwrap();
            let step = Integer::from(1) << (fractional - 23);
            for sample in &input {
                let code = quantizer.quantize(sample).unwrap();
                let difference = ((Integer::from(code) << (fractional - 23)) - sample.raw()).abs();
                assert!(difference <= bound.clone() + (step.clone() >> 1));
            }
        }
        let a = |seed| {
            let mut dither = BigInterleavedDither::new(
                3,
                format,
                DitherConfig {
                    seed,
                    ..config(DitherMode::Tpdf, 24)
                },
            )
            .unwrap();
            let mut output = Vec::new();
            dither
                .quantize_interleaved_into(&input, &mut output)
                .unwrap();
            output
        };
        assert_ne!(a(1), a(2));
    }
}

#[test]
fn arbitrary_no_dither_keeps_pcm_tie_breaking_low_bits() {
    let format = BigQFormat::new(5, 256).unwrap();
    let raw = (Integer::from(65_533) << 240_u32) + 1;
    let sample = BigQ::from_raw(raw, format).unwrap();
    let mut q = BigDitherQuantizer::new(format, config(DitherMode::None, 16)).unwrap();
    assert_eq!(q.quantize(&sample).unwrap(), 32767);
}

#[test]
fn interleaved_big_failure_rolls_back_all_states_and_poison_is_global() {
    let format = BigQFormat::new(65, 256).unwrap();
    let cfg = DitherConfig {
        overflow: OverflowPolicy::Error,
        ..config(DitherMode::NoiseShaped { order: 9 }, 16)
    };
    let mut dither = BigInterleavedDither::new(2, format, cfg).unwrap();
    let before = dither.quantizers.clone();
    let mut output = vec![17];
    let large = BigQ::one(format)
        .unwrap()
        .shift_value(3, cfg.rounding, OverflowPolicy::Error)
        .unwrap()
        .value;
    assert!(
        dither
            .quantize_interleaved_into(&[BigQ::zero(format), large], &mut output)
            .is_err()
    );
    assert_eq!(output, [17]);
    assert_eq!(dither.quantizers, before);
    assert_eq!(
        dither.quantize_interleaved_into(&[], &mut output),
        Err(DspError::Poisoned.into())
    );
}
