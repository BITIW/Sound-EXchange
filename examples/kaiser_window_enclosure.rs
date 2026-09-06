//! Diagnostic for the window-only rigorous design enclosures, not FIR quality.
use rug::{Float, Rational, float::Round};
use sexfir::enclosure::{KaiserWindow, Limits};
use sexplan::{PlanRequest, QualityPreset, plan_precision};
use sexrate::RateRatio;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "preset\tendpoint_bits\tradius\tx\ti0_beta_terms\tabsolute_window_interval_width_upper"
    );
    for preset in QualityPreset::ALL {
        let plan = plan_precision(PlanRequest {
            ratio: RateRatio::from_fraction(160, 147)?,
            preset,
            error_floor: None,
            working_precision_bits: None,
        })?;
        let beta = Rational::from((plan.kaiser_beta.numerator(), plan.kaiser_beta.denominator()));
        let radius = plan.taps_per_phase / 2;
        let x = Rational::from((radius, 2));
        for precision_bits in [64, 192, 512, 1024, 4096] {
            let window = KaiserWindow::new(
                radius,
                &beta,
                Limits {
                    precision_bits,
                    ..Limits::default()
                },
            )?;
            let interval = window.at(&x)?;
            let width_upper = Float::with_val_round(precision_bits, interval.width(), Round::Up).0;
            println!(
                "{preset}\t{precision_bits}\t{radius}\t{x}\t{}\t{}",
                window.denominator().series_terms,
                width_upper.to_string_radix_round(10, Some(20), Round::Up)
            );
        }
    }
    Ok(())
}
