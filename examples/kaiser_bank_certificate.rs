//! Complete-bank joint coefficient error certificates, not stopband claims.
use rug::{Float, Integer, Rational, float::Round};
use sexfir::enclosure::{
    BankCertificate, BankLimits, Limits, PhaseLimits, certify_kaiser, certify_kaiser_big,
};
use sexfir::{BigKaiserSpec, Fraction, KaiserSpec, design_kaiser, design_kaiser_big};
use sexq::RoundingMode;
use sexrate::RateRatio;

fn print(label: &str, certificate: &BankCertificate) {
    let error = Float::with_val_round(
        certificate.precision_bits(),
        certificate.max_phase_l1(),
        Round::Up,
    )
    .0;
    println!(
        "{label}\t{}/{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        certificate.spec().ratio.up(),
        certificate.spec().ratio.down(),
        certificate.phase_errors().len(),
        certificate.spec().taps_per_phase,
        certificate.coefficient_fractional_bits(),
        certificate.precision_bits(),
        certificate.passes(),
        certificate.total_series_terms(),
        error.to_string_radix_round(10, Some(20), Round::Up),
        certificate.coefficient_sha256()
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "bank\tratio\tphases_certified\ttaps_per_phase\tcoefficient_bits\tenclosure_bits\tpasses\tall_pass_series_terms\tmax_phase_l1_error_upper\tcoefficient_sha256"
    );
    for (up, down) in [(1, 3), (3, 2), (7, 11)] {
        for bits in [62_u32, 96, 256, 512] {
            let core = KaiserSpec {
                ratio: RateRatio::from_fraction(up, down)?,
                taps_per_phase: 17,
                rolloff: Fraction::new(1, 2)?,
                beta: Fraction::new(8, 1)?,
                working_precision_bits: (bits * 2).max(192),
                quantization_rounding: RoundingMode::NearestTiesToEven,
            };
            let limits = BankLimits {
                phase: PhaseLimits {
                    arithmetic: Limits {
                        precision_bits: 32,
                        ..Limits::default()
                    },
                    ..PhaseLimits::default()
                },
                max_precision_bits: bits + 128,
                target_max_phase_l1: Some(Rational::from((
                    Integer::from(1),
                    Integer::from(1) << (bits - 12),
                ))),
                ..BankLimits::default()
            };
            let certified = if bits == 62 {
                certify_kaiser(&design_kaiser(&core)?, limits)?
            } else {
                certify_kaiser_big(
                    &design_kaiser_big(&BigKaiserSpec {
                        core,
                        coefficient_fractional_bits: bits,
                        accumulator_bits: bits + 128,
                    })?,
                    limits,
                )?
            };
            print("small-arithmetic-bank", &certified);
        }
    }
    let core = KaiserSpec::native_candidate(RateRatio::from_fraction(160, 147)?);
    let designed = design_kaiser(&core)?;
    let certified = certify_kaiser(&designed, BankLimits::default())?;
    print("sane-candidate-not-response-certificate", &certified);
    Ok(())
}
