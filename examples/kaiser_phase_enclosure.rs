//! Small-bank arithmetic qualification, not a preset response certificate.
use rug::{Float, Integer, Rational, float::Round};
use sexfir::enclosure::{Limits, PhaseLimits, kaiser_phase};
use sexfir::{BigKaiserSpec, Fraction, KaiserSpec, design_kaiser, design_kaiser_big};
use sexq::RoundingMode;
use sexrate::RateRatio;

fn upper(value: &Rational, p: u32) -> String {
    Float::with_val_round(p, value, Round::Up)
        .0
        .to_string_radix_round(10, Some(20), Round::Up)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "ratio\tphase\ttaps\tbeta\tdesign_bits\tenclosure_bits\tcoefficient_bits\tseries_terms\tmax_coefficient_error_upper\tphase_l1_error_upper"
    );
    for (up, down) in [(1, 3), (3, 2), (7, 11)] {
        for (bits, design_bits, enclosure_bits) in [
            (62, 192, 256),
            (96, 256, 384),
            (256, 512, 640),
            (512, 1024, 1152),
        ] {
            let core = KaiserSpec {
                ratio: RateRatio::from_fraction(up, down)?,
                taps_per_phase: 17,
                rolloff: Fraction::new(1, 2)?,
                beta: Fraction::new(8, 1)?,
                working_precision_bits: design_bits,
                quantization_rounding: RoundingMode::NearestTiesToEven,
            };
            let raw: Vec<Vec<Integer>> = if bits == 62 {
                let designed = design_kaiser(&core)?;
                (0..up)
                    .map(|phase| {
                        Ok(designed
                            .bank()
                            .phase(phase)?
                            .iter()
                            .map(|q| Integer::from(q.raw()))
                            .collect())
                    })
                    .collect::<Result<_, sexrate::PolyphaseError>>()?
            } else {
                let designed = design_kaiser_big(&BigKaiserSpec {
                    core: core.clone(),
                    coefficient_fractional_bits: bits,
                    accumulator_bits: bits + 128,
                })?;
                (0..up)
                    .map(|phase| {
                        Ok(designed
                            .bank()
                            .phase(phase)?
                            .iter()
                            .map(|q| q.raw().clone())
                            .collect())
                    })
                    .collect::<Result<_, sexrate::PolyphaseError>>()?
            };
            for (phase, coefficients) in raw.iter().enumerate() {
                let enclosed = kaiser_phase(
                    &core,
                    phase as u64,
                    PhaseLimits {
                        arithmetic: Limits {
                            precision_bits: enclosure_bits,
                            ..Limits::default()
                        },
                        ..PhaseLimits::default()
                    },
                )?;
                let error = enclosed.compare_quantized(coefficients, bits)?;
                println!(
                    "{up}/{down}\t{phase}\t17\t8\t{design_bits}\t{enclosure_bits}\t{bits}\t{}\t{}\t{}",
                    enclosed.series_terms(),
                    upper(&error.max_absolute, enclosure_bits),
                    upper(&error.phase_l1, enclosure_bits)
                );
            }
        }
    }
    Ok(())
}
