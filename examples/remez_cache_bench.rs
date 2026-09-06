use sexfir::{optimized, refinement};
use sexplan::{PlanRequest, QualityPreset, plan_precision, reserve_full_range_fir_accumulator};
use sexrate::RateRatio;
use std::error::Error;
use std::time::Instant;

fn main() -> Result<(), Box<dyn Error>> {
    let taps = std::env::args()
        .nth(1)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(129);
    let mut plan = plan_precision(PlanRequest {
        ratio: RateRatio::from_fraction(2, 3)?,
        preset: QualityPreset::Fast,
        error_floor: None,
        working_precision_bits: None,
    })?;
    plan.taps_per_phase = taps;
    plan.coefficient_count = u128::from(taps) * 2;
    plan.coefficient_fractional_bits = 96;
    plan.working_precision_bits = 160;
    let plan = reserve_full_range_fir_accumulator(&plan, None)?;
    let spec = refinement::Designer::Equiripple
        .optimized_spec(&plan)?
        .unwrap();
    let cached_limits = optimized::Limits {
        max_work: 4_000_000_000,
        ..Default::default()
    };
    let cached_work = spec.preflight(cached_limits)?;
    if cached_work.cosine_cache_bytes == 0 {
        return Err("requested bank does not fit a full cosine cache".into());
    }
    let direct_limits = optimized::Limits {
        max_storage_bytes: cached_work.storage_bytes,
        ..cached_limits
    };
    let start = Instant::now();
    let direct = optimized::design(&spec, direct_limits)?;
    let direct_seconds = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let cached = optimized::design(&spec, cached_limits)?;
    let cached_seconds = start.elapsed().as_secs_f64();
    assert_eq!(direct.bank(), cached.bank());
    assert_eq!(direct.report().quantization, cached.report().quantization);
    assert_eq!(direct.report().solution, cached.report().solution);
    println!("taps/phase={taps}, C=96, P=160, ratio=2/3");
    println!(
        "direct_seconds={direct_seconds:.6}, cached_seconds={cached_seconds:.6}, speedup={:.3}",
        direct_seconds / cached_seconds
    );
    println!(
        "core_storage_bytes={}, cosine_cache_bytes={}",
        cached_work.storage_bytes, cached_work.cosine_cache_bytes
    );
    println!(
        "identical coefficients and complete numerical/solver reports; coefficient sha256={}",
        cached.report().quantization.coefficient_sha256
    );
    Ok(())
}
