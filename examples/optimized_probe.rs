//! Deterministic designer + integer-stream cross-architecture regression corpus.
//! This is a library probe, not a new CLI designer selection or a PCM claim.
use rug::Integer;
use sexfir::{
    EquirippleSpec, Fraction, LeastSquaresSpec, cache, certificate, harmonics, optimized,
};
use sexplan::ErrorFloor;
use sexq::{BigQ, BigQFormat, OverflowPolicy, RoundingMode};
use sexrate::{FrameCountPolicy, InterleavedResamplerBig, PolyphaseFirBig, RateRatio};
use std::error::Error;
use std::sync::Arc;

fn spec(equiripple: bool, bits: u32, qualified: bool) -> optimized::Spec {
    let ratio = RateRatio::from_fraction(2, 3).unwrap();
    let taps = if qualified { 31 } else { 7 };
    let rolloff = if qualified {
        Fraction::new(3, 4)
    } else {
        Fraction::new(9, 10)
    }
    .unwrap();
    let precision = (bits + 64).max(128);
    let pass = Fraction::new(1, 1).unwrap();
    let stop = Fraction::new(10, 1).unwrap();
    let method = if equiripple {
        optimized::Method::Equiripple(EquirippleSpec {
            ratio,
            taps_per_phase: taps,
            rolloff,
            passband_weight: pass,
            stopband_weight: stop,
            grid_density: 16,
            max_iterations: 64,
            working_precision_bits: precision,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        })
    } else {
        optimized::Method::GlobalLeastSquares(LeastSquaresSpec {
            ratio,
            taps_per_phase: taps,
            rolloff,
            passband_weight: pass,
            stopband_weight: stop,
            grid_points_per_band: if qualified { 256 } else { 32 },
            working_precision_bits: precision,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        })
    };
    optimized::Spec {
        method,
        coefficient_fractional_bits: bits,
        accumulator_bits: if bits == 4096 {
            8192
        } else {
            (bits + 80).max(128)
        },
    }
}

fn stream(filter: &optimized::Designed, block: usize) -> Result<Vec<BigQ>, Box<dyn Error>> {
    let format = BigQFormat::new(65, 160)?;
    let bank = Arc::new(PolyphaseFirBig::from_q63_bank(
        filter.bank(),
        format,
        format,
        None,
    )?);
    let mut stream = InterleavedResamplerBig::new_with_input_delay(
        2,
        filter.ratio(),
        bank,
        filter.input_delay_frames(),
        RoundingMode::NearestTiesToEven,
        OverflowPolicy::Error,
    )?;
    let input = (0..34_i64)
        .map(|index| {
            BigQ::from_raw(
                (Integer::from(index * 37 - 500) << 148) + (index % 5 + 1),
                format,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut output = Vec::new();
    for chunk in input.chunks(block * 2) {
        stream.push_interleaved_finite_into(
            chunk,
            FrameCountPolicy::NearestTiesToEven,
            &mut output,
        )?;
    }
    let frames =
        sexrate::output_frames_for_input(17, filter.ratio(), FrameCountPolicy::NearestTiesToEven)?;
    let stats = stream.finish_exact_frames(frames, &mut output)?;
    if stats.output_frames != frames {
        return Err("incorrect finite duration".into());
    }
    Ok(output)
}

fn inspect(
    name: &str,
    filter: &optimized::Designed,
    target: Option<ErrorFloor>,
) -> Result<(), Box<dyn Error>> {
    println!("case: {name}");
    println!("spec: {:?}", filter.spec());
    println!("report: {:?}", filter.report());
    let unity = Integer::from(1) << filter.spec().coefficient_fractional_bits;
    for phase in 0..filter.ratio().up() {
        let sum = Integer::from(Integer::sum(
            filter.bank().phase(phase)?.iter().map(BigQ::raw),
        ));
        if sum != unity {
            return Err("non-unit phase DC".into());
        }
    }
    let result = stream(filter, 1)?;
    if result != stream(filter, 3)? || result != stream(filter, 4096)? {
        return Err("chunk-dependent arbitrary signal output".into());
    }
    if !result
        .iter()
        .any(|sample| !sample.raw().is_divisible_2pow(97))
    {
        return Err("lost sub-Q1.63 signal information".into());
    }
    println!(
        "Q65.160 stereo stream (hex raw; chunks 1/3/4096 agree): {:?}",
        result
            .iter()
            .map(|sample| sample.raw().to_string_radix(16))
            .collect::<Vec<_>>()
    );
    if let Some(target) = target {
        let (phase_grid, image_grid) = if target.attenuation_db() >= 80 {
            (257, 65)
        } else {
            (129, 33)
        };
        let (response, compliance) = optimized::analyze_against(filter, phase_grid, target)?;
        let proof = optimized::certify(filter, target, certificate::Limits::default())?;
        let images =
            optimized::analyze_harmonics(filter, image_grid, target, harmonics::Limits::default())?;
        if !compliance.passband_deviation_meets_target
            || !compliance.stopband_meets_target
            || !optimized::coefficient_budget_passed(filter, target)?
            || proof.outcome != certificate::Outcome::Certified
            || !images.meets_target()
        {
            return Err("requested bank qualification failed".into());
        }
        println!("phase grid: {response:?}; compliance: {compliance:?}");
        println!(
            "continuous per-phase amplitude: {:?}; upper enclosures {:?}; phases {}/{}; cells {}; work {}",
            proof.outcome,
            proof.upper_enclosures(),
            proof.certified_phases,
            proof.expected_phases,
            proof.visited_cells,
            proof.charged_work
        );
        println!("all-image sampled transfer: {images:?}");
    } else {
        println!("spectral quality: not qualified (short arithmetic/precision fixture)");
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let cache_directory = match arguments.as_slice() {
        [_] => None,
        [_, option, directory] if option == "--cache-directory" => {
            Some(std::path::PathBuf::from(directory))
        }
        _ => return Err("usage: optimized_probe [--cache-directory PATH]".into()),
    };
    let design =
        |request: &optimized::Spec, limits| -> Result<optimized::Designed, Box<dyn Error>> {
            match &cache_directory {
                Some(directory) => {
                    Ok(cache::design_optimized_big_cached(directory, request, limits)?.0)
                }
                None => Ok(optimized::design(request, limits)?),
            }
        };
    for equiripple in [false, true] {
        let method = if equiripple {
            "equiripple"
        } else {
            "global-ls"
        };
        for bits in [62, 96, 4096] {
            let bank = design(&spec(equiripple, bits, false), optimized::Limits::default())?;
            inspect(&format!("{method}-C{bits}"), &bank, None)?;
        }
        let bank = design(&spec(equiripple, 96, true), optimized::Limits::default())?;
        inspect(
            &format!("{method}-qualified-C96"),
            &bank,
            Some(ErrorFloor::new(40)?),
        )?;
        let result = if let Some(directory) = &cache_directory {
            cache::refine_optimized_quantization_cached(
                directory,
                &spec(equiripple, 8, false),
                ErrorFloor::new(80)?,
                false,
                optimized::SearchLimits::default(),
            )?
        } else {
            optimized::refine_quantization(
                &spec(equiripple, 8, false),
                ErrorFloor::new(80)?,
                false,
                optimized::SearchLimits::default(),
            )?
        };
        println!(
            "precision search: {method}; attempts {:?}; work {}; coefficients {}",
            result.attempts, result.charged_work, result.charged_coefficients
        );
        inspect(
            &format!(
                "{method}-refined-C{}",
                result.design.spec().coefficient_fractional_bits
            ),
            &result.design,
            None,
        )?;
    }
    let mut fast = spec(false, 96, false);
    let optimized::Method::GlobalLeastSquares(s) = &mut fast.method else {
        unreachable!()
    };
    s.taps_per_phase = 129;
    s.grid_points_per_band = 256;
    let bank = design(&fast, optimized::Limits::default())?;
    inspect(
        "global-ls-fast-geometry-80dB-C96",
        &bank,
        Some(ErrorFloor::new(80)?),
    )?;

    for precision in [160, 512] {
        let mut request = spec(true, 96, false);
        let optimized::Method::Equiripple(core) = &mut request.method else {
            unreachable!()
        };
        core.taps_per_phase = 129;
        core.working_precision_bits = precision;
        let limits = optimized::Limits {
            max_work: 1_000_000_000,
            ..Default::default()
        };
        let bank = design(&request, limits)?;
        inspect(
            &format!("equiripple-fast-geometry-80dB-C96-P{precision}"),
            &bank,
            Some(ErrorFloor::new(80)?),
        )?;
    }
    println!(
        "scope: deterministic library design and integer stream; image grids are not continuous proofs; no CLI/PCM qualification is claimed"
    );
    Ok(())
}
