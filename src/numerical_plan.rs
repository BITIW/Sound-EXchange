use super::*;
use sexplan::numerical::{ExactRatio, NumericalBudget, NumericalStage, evaluate_numerical_budget};
use sexplan::numerical::{FirL1Bound, evaluate_numerical_budget_with_fir_l1};
use sexplan::raise_coefficient_precision;

#[derive(Debug)]
pub(super) struct ExecutionPlan {
    pub filter: PrecisionPlan,
    pub signal: SignalPrecisionPlan,
    pub numerical: NumericalBudget,
    pub refinements: u32,
    pub certified_fir_reference: bool,
    pub fir_l1: Option<FirL1Bound>,
}

fn exact(numerator: i128, denominator: u64) -> ExactRatio {
    ExactRatio {
        numerator,
        denominator,
    }
}

fn stages(
    quality: &QualityOptions,
    filter: &PrecisionPlan,
    resampling: bool,
) -> Vec<NumericalStage> {
    let mut result = Vec::new();
    if let Some(g) = quality.gain {
        result.push(NumericalStage::Linear {
            name: "gain".to_owned(),
            rows: vec![vec![exact(i128::from(g.numerator), g.denominator)]],
        });
    }
    if let Some(r) = quality.dc_block_radius {
        result.push(NumericalStage::DcBlock {
            radius: exact(i128::from(r.0.numerator), r.0.denominator),
        });
    }
    if let Some(matrix) = &quality.channel_mix {
        result.push(NumericalStage::Linear {
            name: "channel mix".to_owned(),
            rows: matrix
                .coefficients
                .chunks(usize::from(matrix.input_channels))
                .map(|row| {
                    row.iter()
                        .map(|c| exact(i128::from(c.numerator), c.denominator))
                        .collect()
                })
                .collect(),
        });
    }
    if let Some(convolution) = &quality.convolution {
        result.push(NumericalStage::Linear {
            name: "convolution".to_owned(),
            rows: vec![
                convolution
                    .taps
                    .iter()
                    .map(|c| exact(i128::from(c.numerator), c.denominator))
                    .collect(),
            ],
        });
    }
    if resampling {
        result.push(NumericalStage::DesignedFir {
            taps_per_phase: filter.taps_per_phase,
            coefficient_fractional_bits: filter.coefficient_fractional_bits.max(62),
        });
    }
    if quality.clip_policy == ClipPolicy::Normalize {
        result.push(NumericalStage::Normalize);
    }
    result
}

pub(super) fn plan_execution(
    ratio: RateRatio,
    quality: &QualityOptions,
    resampling: bool,
) -> Result<ExecutionPlan, Box<dyn Error>> {
    let filter = plan_precision(PlanRequest {
        ratio,
        preset: quality.preset,
        error_floor: quality.error_floor,
        working_precision_bits: quality.working_precision_bits,
    })?;
    plan_from_filter(filter, quality, resampling)
}

pub(super) fn plan_from_filter(
    mut filter: PrecisionPlan,
    quality: &QualityOptions,
    resampling: bool,
) -> Result<ExecutionPlan, Box<dyn Error>> {
    // Non-Kaiser CLI banks use the arbitrary-width path, with an explicit C>=64
    // floor. The native Q2.62 library designers remain available separately.
    if quality.window != refinement::Designer::Kaiser {
        filter = raise_coefficient_precision(&filter, 64, quality.working_precision_bits)?;
    }
    let mut signal = processing::initial_signal_plan(&filter, quality, resampling)?;
    let target_bits = filter.amplitude_error_bits + 2;
    for refinements in 0..128 {
        if quality.window != refinement::Designer::Kaiser {
            filter = sexplan::reserve_full_range_fir_accumulator(
                &filter,
                quality.working_precision_bits,
            )?;
        }
        let numerical = evaluate_numerical_budget(
            &stages(quality, &filter, resampling),
            signal.fractional_bits,
            signal.effect_coefficient_fractional_bits,
        )?;
        signal.integer_bits = signal.integer_bits.max(numerical.minimum_integer_bits());
        BigQFormat::new(signal.integer_bits, signal.fractional_bits)?;
        if numerical.meets_target_bits(target_bits) {
            return Ok(ExecutionPlan {
                filter,
                signal,
                numerical,
                refinements,
                certified_fir_reference: false,
                fir_l1: None,
            });
        }
        let effect_and_rounding = numerical.final_bound.rounding_error_raw.clone()
            + &numerical.final_bound.effect_coefficient_error_raw;
        let signal_increment = numerical.additional_bits(&effect_and_rounding, target_bits);
        let filter_increment = numerical.additional_bits(
            &numerical.final_bound.fir_coefficient_error_raw,
            target_bits,
        );
        if filter_increment != 0 {
            let required = filter
                .coefficient_fractional_bits
                .max(62)
                .checked_add(filter_increment)
                .ok_or("filter precision increment overflow")?;
            filter =
                raise_coefficient_precision(&filter, required, quality.working_precision_bits)?;
        }
        let required = signal
            .fractional_bits
            .checked_add(signal_increment)
            .ok_or("signal precision increment overflow")?;
        signal = processing::initial_signal_plan(&filter, quality, resampling)?;
        signal.fractional_bits = signal.fractional_bits.max(required);
        if let Some(requested) = quality.signal_fractional_bits
            && requested < signal.fractional_bits
        {
            return Err(sexplan::PlanError::SignalPrecisionBelowMinimum {
                requested,
                minimum: signal.fractional_bits,
            }
            .into());
        }
        signal.effect_coefficient_fractional_bits = if signal.fractional_bits == 63 {
            62
        } else {
            signal.fractional_bits
        };
        BigQFormat::new(signal.integer_bits, signal.fractional_bits)?;
    }
    Err("numerical precision refinement did not converge within 128 checked candidates".into())
}

pub(super) fn format_bound(budget: &NumericalBudget, raw: &Integer) -> String {
    budget
        .binary_ceiling(raw)
        .map_or_else(|| "0".to_owned(), |e| format!("2^{e} FS"))
}

pub(super) fn fir_l1_report(l1: &FirL1Bound) -> String {
    format!(
        "numerical FIR gain: exact max phase L1 = {} * 2^-{}; actual quantized bank",
        l1.maximum_raw(),
        l1.coefficient_fractional_bits()
    )
}

/// Bind the tighter gain to the bank actually accepted by the shared gates.
/// Keep selected precision unchanged: this refines the proof, not audio math.
pub(super) fn bind_fir_l1(
    execution: &mut ExecutionPlan,
    quality: &QualityOptions,
    design: &PlannedDesign,
) -> Result<(), Box<dyn Error>> {
    if execution.filter.ratio.up() == execution.filter.ratio.down() {
        return Ok(());
    }
    let bound = match design {
        PlannedDesign::Native(design) => FirL1Bound::from_native(design.bank()),
        PlannedDesign::Big(design) => FirL1Bound::from_big(design.bank())?,
        PlannedDesign::Windowed(design) => FirL1Bound::from_big(design.bank())?,
        PlannedDesign::Optimized(design) => FirL1Bound::from_big(design.bank())?,
    };
    let numerical = evaluate_numerical_budget_with_fir_l1(
        &stages(quality, &execution.filter, true),
        execution.signal.fractional_bits,
        execution.signal.effect_coefficient_fractional_bits,
        Some(&bound),
    )?;
    if !numerical.meets_target_bits(execution.filter.amplitude_error_bits + 2)
        || numerical.maximum_signal_peak_raw > execution.numerical.maximum_signal_peak_raw
        || numerical.final_bound.total_error_raw()
            > execution.numerical.final_bound.total_error_raw()
    {
        return Err("actual FIR L1 bound exceeds the generic numerical plan".into());
    }
    execution.numerical = numerical;
    execution.fir_l1 = Some(bound);
    Ok(())
}

/// Remaining joint FIR allowance after all other propagated errors. Reserve
/// the exact worst-case cost of upward conversion/multiplication on the bound
/// grid; the resulting rational need not be a power of two.
pub(super) fn certificate_target(
    execution: &ExecutionPlan,
) -> Result<Option<rug::Rational>, Box<dyn Error>> {
    let budget = &execution.numerical;
    let Some(index) = budget
        .stages
        .iter()
        .position(|stage| stage.name == "polyphase FIR")
    else {
        return Ok(None);
    };
    if budget.stages[index + 1..]
        .iter()
        .any(|stage| stage.name != "normalization (same selected scale)")
    {
        return Err("certified FIR target propagation only supports same-scale normalization after the resampler".into());
    }
    let scale = Integer::from(1) << budget.bound_fractional_bits;
    let peak = if index == 0 {
        scale.clone()
    } else {
        budget.stages[index - 1].ideal_peak_raw.clone()
    };
    if peak == 0 {
        return Ok(None);
    }
    let target_bits = execution.filter.amplitude_error_bits + 2;
    if target_bits > budget.bound_fractional_bits {
        return Err("certified chain target is finer than its bound grid".into());
    }
    let available = (Integer::from(1) << (budget.bound_fractional_bits - target_bits))
        - &budget.final_bound.rounding_error_raw
        - &budget.final_bound.effect_coefficient_error_raw;
    let (integer_peak, remainder) = peak.clone().div_rem(scale);
    let reserved = integer_peak + u32::from(remainder != 0) + 1;
    let numerator = available - reserved;
    if numerator <= 0 {
        return Err("no certified FIR error allowance remains after signal/effect errors and outward rounding".into());
    }
    Ok(Some(rug::Rational::from((numerator, peak))))
}

pub(super) fn merge_design_certificate(
    execution: &mut ExecutionPlan,
    quality: &QualityOptions,
    certificate: &sexfir::enclosure::BankCertificate,
) -> Result<(), Box<dyn Error>> {
    let filter = &execution.filter;
    let expected_spec = match filter.backend {
        BackendRequirement::NativeI128 => KaiserSpec::from_precision_plan(filter)?,
        _ => BigKaiserSpec::from_precision_plan(filter)?.core,
    };
    if certificate.spec() != &expected_spec
        || certificate.coefficient_fractional_bits() != filter.coefficient_fractional_bits.max(62)
    {
        return Err("certified bank does not match numerical execution plan".into());
    }
    if filter.ratio.up() == filter.ratio.down() {
        return Ok(());
    }
    let mut chain = stages(quality, filter, true);
    let mut replacements = 0;
    for stage in &mut chain {
        if let NumericalStage::DesignedFir {
            taps_per_phase,
            coefficient_fractional_bits,
        } = stage
        {
            *stage = NumericalStage::CertifiedFir {
                taps_per_phase: *taps_per_phase,
                coefficient_fractional_bits: *coefficient_fractional_bits,
                error_numerator: certificate.max_phase_l1().numer().clone(),
                error_denominator: certificate.max_phase_l1().denom().clone(),
            };
            replacements += 1;
        }
    }
    if replacements != 1 {
        return Err("certified chain requires exactly one actual FIR stage".into());
    }
    let numerical = evaluate_numerical_budget_with_fir_l1(
        &chain,
        execution.signal.fractional_bits,
        execution.signal.effect_coefficient_fractional_bits,
        execution.fir_l1.as_ref(),
    )?;
    if !numerical.meets_target_bits(filter.amplitude_error_bits + 2) {
        return Err("certified whole-chain numerical budget exceeded; no output or whole-chain compliance is claimed".into());
    }
    execution.signal.integer_bits = execution
        .signal
        .integer_bits
        .max(numerical.minimum_integer_bits());
    BigQFormat::new(
        execution.signal.integer_bits,
        execution.signal.fractional_bits,
    )?;
    execution.numerical = numerical;
    execution.certified_fir_reference = true;
    Ok(())
}

pub(super) fn reference(certified: bool) -> &'static str {
    if certified {
        "numerical reference: exact effects + mathematical normalized Kaiser FIR + same normalization scale; includes certified MPFR design/quantization/DC-correction error; excludes ideal-filter approximation and final PCM/dither/clipping"
    } else {
        "numerical reference: exact effects + exact-DC MPFR FIR + same normalization scale; excludes ideal-filter approximation, MPFR design error, and final PCM/dither"
    }
}

pub(super) fn report(budget: &NumericalBudget, target_bits: u32, refinements: u32) -> String {
    let final_bound = &budget.final_bound;
    format!(
        "numerical error bound: <= {} (budget 2^-{} FS); rounding {}, effect coefficients {}, FIR coefficients {}; {} precision refinement(s)",
        format_bound(budget, &final_bound.total_error_raw()),
        target_bits,
        format_bound(budget, &final_bound.rounding_error_raw),
        format_bound(budget, &final_bound.effect_coefficient_error_raw),
        format_bound(budget, &final_bound.fir_coefficient_error_raw),
        refinements,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_fir_l1_binding_preserves_precision_and_survives_design_certificate_merge() {
        let quality = QualityOptions {
            preset: QualityPreset::Fast,
            gain: Some(LinearGain::parse("1/3").unwrap()),
            clip_policy: ClipPolicy::Normalize,
            ..QualityOptions::default()
        };
        let ratio = RateRatio::from_fraction(3, 2).unwrap();
        let mut execution = plan_execution(ratio, &quality, true).unwrap();
        let mut generic = plan_execution(ratio, &quality, true).unwrap();
        let spec = KaiserSpec::from_precision_plan(&execution.filter).unwrap();
        let design = design_kaiser(&spec).unwrap();
        let proof =
            sexfir::enclosure::certify_kaiser(&design, sexfir::enclosure::BankLimits::default())
                .unwrap();
        bind_fir_l1(&mut execution, &quality, &PlannedDesign::Native(design)).unwrap();
        assert_eq!(execution.filter, generic.filter);
        assert_eq!(execution.signal, generic.signal);
        assert!(
            execution.numerical.maximum_signal_peak_raw < generic.numerical.maximum_signal_peak_raw
        );
        assert!(
            execution.numerical.final_bound.total_error_raw()
                < generic.numerical.final_bound.total_error_raw()
        );
        merge_design_certificate(&mut execution, &quality, &proof).unwrap();
        merge_design_certificate(&mut generic, &quality, &proof).unwrap();
        assert!(execution.certified_fir_reference);
        assert!(execution.fir_l1.is_some());
        assert_eq!(execution.filter, generic.filter);
        assert_eq!(execution.signal, generic.signal);
        assert!(
            execution.numerical.maximum_signal_peak_raw < generic.numerical.maximum_signal_peak_raw
        );
        assert!(
            execution.numerical.final_bound.total_error_raw()
                < generic.numerical.final_bound.total_error_raw()
        );
    }

    #[test]
    fn mathematical_reference_requires_matching_full_fir_specification() {
        let quality = QualityOptions {
            preset: QualityPreset::Fast,
            ..QualityOptions::default()
        };
        let mut execution =
            plan_execution(RateRatio::from_fraction(3, 2).unwrap(), &quality, true).unwrap();
        let spec = KaiserSpec::from_precision_plan(&execution.filter).unwrap();
        let mut wrong = spec.clone();
        wrong.beta = sexfir::Fraction::new(1, 1).unwrap();
        let proof = sexfir::enclosure::certify_kaiser(
            &design_kaiser(&wrong).unwrap(),
            sexfir::enclosure::BankLimits::default(),
        )
        .unwrap();
        assert!(merge_design_certificate(&mut execution, &quality, &proof).is_err());
        assert!(!execution.certified_fir_reference);
        let correct = sexfir::enclosure::certify_kaiser(
            &design_kaiser(&spec).unwrap(),
            sexfir::enclosure::BankLimits::default(),
        )
        .unwrap();
        merge_design_certificate(&mut execution, &quality, &correct).unwrap();
        assert!(execution.certified_fir_reference);
        assert!(
            execution
                .numerical
                .meets_target_bits(execution.filter.amplitude_error_bits + 2)
        );
    }

    #[test]
    fn joint_coefficient_target_tightens_with_peak_at_resampler() {
        let ratio = RateRatio::from_fraction(3, 2).unwrap();
        let simple = QualityOptions {
            preset: QualityPreset::Fast,
            ..QualityOptions::default()
        };
        let base = plan_execution(ratio, &simple, true).unwrap();
        let amplified_quality = QualityOptions {
            gain: Some(LinearGain::parse("1152921504606846976").unwrap()),
            clip_policy: ClipPolicy::Normalize,
            ..simple.clone()
        };
        let amplified = plan_execution(ratio, &amplified_quality, true).unwrap();
        let target = certificate_target(&amplified).unwrap().unwrap();
        assert!(target < certificate_target(&base).unwrap().unwrap() / (Integer::from(1) << 59));
        let restored = plan_execution(
            ratio,
            &QualityOptions {
                convolution: Some(ConvolutionSpec::parse("1/1152921504606846976").unwrap()),
                ..amplified_quality
            },
            true,
        )
        .unwrap();
        assert!(
            certificate_target(&restored).unwrap().unwrap() > target * (Integer::from(1) << 59)
        );
        let silent = plan_execution(
            ratio,
            &QualityOptions {
                gain: Some(LinearGain::parse("0").unwrap()),
                ..simple
            },
            true,
        )
        .unwrap();
        assert_eq!(certificate_target(&silent).unwrap(), None);
    }

    #[test]
    fn supplied_chain_target_leaves_outward_rounding_room() {
        let quality = QualityOptions {
            preset: QualityPreset::Fast,
            gain: Some(LinearGain::parse("1152921504606846976").unwrap()),
            convolution: Some(ConvolutionSpec::parse("1/3,1/7").unwrap()),
            clip_policy: ClipPolicy::Normalize,
            ..QualityOptions::default()
        };
        let execution =
            plan_execution(RateRatio::from_fraction(3, 2).unwrap(), &quality, true).unwrap();
        let target = certificate_target(&execution).unwrap().unwrap();
        let mut chain = stages(&quality, &execution.filter, true);
        for stage in &mut chain {
            if let NumericalStage::DesignedFir {
                taps_per_phase,
                coefficient_fractional_bits,
            } = stage
            {
                *stage = NumericalStage::CertifiedFir {
                    taps_per_phase: *taps_per_phase,
                    coefficient_fractional_bits: *coefficient_fractional_bits,
                    error_numerator: target.numer().clone(),
                    error_denominator: target.denom().clone(),
                };
            }
        }
        let budget = evaluate_numerical_budget(
            &chain,
            execution.signal.fractional_bits,
            execution.signal.effect_coefficient_fractional_bits,
        )
        .unwrap();
        assert!(budget.meets_target_bits(execution.filter.amplitude_error_bits + 2));
    }

    #[test]
    fn precision_refinement_follows_peak_at_the_fir_not_just_gain_presence() {
        let ratio = RateRatio::from_fraction(160, 147).unwrap();
        let quality = QualityOptions {
            preset: QualityPreset::Fast,
            gain: Some(LinearGain::parse("1152921504606846976").unwrap()),
            clip_policy: ClipPolicy::Normalize,
            ..QualityOptions::default()
        };
        let amplified = plan_execution(ratio, &quality, true).unwrap();
        assert_eq!(amplified.filter.coefficient_fractional_bits, 85);
        assert!(amplified.refinements > 0);
        assert!(
            amplified
                .numerical
                .meets_target_bits(amplified.filter.amplitude_error_bits + 2)
        );
        let restored = plan_execution(
            ratio,
            &QualityOptions {
                convolution: Some(ConvolutionSpec::parse("1/1152921504606846976").unwrap()),
                ..quality
            },
            true,
        )
        .unwrap();
        assert_eq!(restored.filter.coefficient_fractional_bits, 40);
        assert_eq!(restored.refinements, 0);
        assert!(
            restored
                .numerical
                .meets_target_bits(restored.filter.amplitude_error_bits + 2)
        );
    }

    #[test]
    fn coupled_plan_cannot_undercut_explicit_mpfr_precision() {
        let quality = QualityOptions {
            preset: QualityPreset::Fast,
            gain: Some(LinearGain::parse("18446744073709551615").unwrap()),
            error_floor: Some(ErrorFloor::new(400).unwrap()),
            working_precision_bits: Some(192),
            ..QualityOptions::default()
        };
        let ratio = RateRatio::from_fraction(160, 147).unwrap();
        assert!(
            plan_execution(ratio, &quality, true)
                .unwrap_err()
                .to_string()
                .contains("below the calculated minimum")
        );
        let automatic = plan_execution(
            ratio,
            &QualityOptions {
                working_precision_bits: None,
                ..quality
            },
            true,
        )
        .unwrap();
        assert!(automatic.filter.working_precision_bits > 192);
        assert!(
            automatic
                .numerical
                .meets_target_bits(automatic.filter.amplitude_error_bits + 2)
        );
    }
}
