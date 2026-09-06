//! Arbitrary-width windowed-sinc design. MPFR never leaves the designer.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spec {
    pub core: WindowedSincSpec,
    pub coefficient_fractional_bits: u32,
    pub accumulator_bits: u32,
}

impl Spec {
    pub fn from_precision_plan(
        plan: &PrecisionPlan,
        window: WindowFunction,
    ) -> Result<Self, DesignError> {
        Ok(Self {
            core: WindowedSincSpec {
                ratio: plan.ratio,
                taps_per_phase: usize::try_from(plan.taps_per_phase)
                    .map_err(|_| DesignError::PlannedTapCountTooLarge(plan.taps_per_phase))?,
                rolloff: Fraction::new(plan.rolloff.numerator(), plan.rolloff.denominator())?,
                window,
                working_precision_bits: plan.working_precision_bits,
                quantization_rounding: RoundingMode::NearestTiesToEven,
            },
            coefficient_fractional_bits: plan.coefficient_fractional_bits,
            accumulator_bits: plan.planned_accumulator_bits,
        })
    }

    /// Static cost checks run before allocating coefficients, including on cache hits.
    /// Dolph's direct continuous-window series costs L*T*T terms.
    pub fn preflight(&self) -> Result<usize, DesignError> {
        self.core.validate()?;
        BigQFormat::new(2, self.coefficient_fractional_bits)?;
        if self.accumulator_bits == 0 {
            return Err(DesignError::ZeroAccumulatorWidth);
        }
        let minimum = self.coefficient_fractional_bits.checked_add(64).ok_or(
            DesignError::WorkingPrecisionTooHigh(self.core.working_precision_bits),
        )?;
        if self.core.working_precision_bits < minimum {
            return Err(DesignError::CoefficientWorkingPrecisionTooLow {
                requested: self.core.working_precision_bits,
                minimum,
            });
        }
        let phases = usize::try_from(self.core.ratio.up())
            .map_err(|_| DesignError::PhaseCountTooLarge(self.core.ratio.up()))?;
        let count = phases
            .checked_mul(self.core.taps_per_phase)
            .ok_or(DesignError::CoefficientCountOverflow)?;
        if count > MAX_BIG_COEFFICIENTS {
            return Err(DesignError::BigCoefficientBudgetExceeded {
                requested: count,
                maximum: MAX_BIG_COEFFICIENTS,
            });
        }
        if matches!(self.core.window, WindowFunction::DolphChebyshev { .. }) {
            let requested = count
                .checked_mul(self.core.taps_per_phase)
                .ok_or(DesignError::CoefficientCountOverflow)?;
            if requested > MAX_DOLPH_SERIES_TERMS {
                return Err(DesignError::DolphSeriesBudgetExceeded {
                    requested,
                    maximum: MAX_DOLPH_SERIES_TERMS,
                });
            }
        }
        Ok(count)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Designed {
    pub(super) spec: Spec,
    pub(super) input_delay_frames: u64,
    pub(super) bank: PolyphaseFirBigQ63,
    pub(super) report: BigQuantizationReport,
}

impl Designed {
    pub const fn spec(&self) -> &Spec {
        &self.spec
    }
    pub const fn ratio(&self) -> RateRatio {
        self.spec.core.ratio
    }
    pub const fn input_delay_frames(&self) -> u64 {
        self.input_delay_frames
    }
    pub const fn bank(&self) -> &PolyphaseFirBigQ63 {
        &self.bank
    }
    pub const fn report(&self) -> &BigQuantizationReport {
        &self.report
    }
    pub fn into_bank(self) -> PolyphaseFirBigQ63 {
        self.bank
    }
}

pub(super) fn algorithm(window: WindowFunction) -> &'static str {
    match window {
        WindowFunction::Rectangular => "windowed-sinc-rectangular-big-v1",
        WindowFunction::Hann => "windowed-sinc-hann-big-v1",
        WindowFunction::Blackman => "windowed-sinc-blackman-big-v1",
        WindowFunction::DolphChebyshev { .. } => "windowed-sinc-dolph-chebyshev-big-v2",
    }
}

pub(super) fn spec_hash(spec: &Spec, domain: &[u8]) -> Sha256 {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(algorithm(spec.core.window).as_bytes());
    if let WindowFunction::DolphChebyshev { attenuation_db } = spec.core.window {
        hash.update(attenuation_db.to_le_bytes());
    }
    hash.update(spec.core.ratio.up().to_le_bytes());
    hash.update(spec.core.ratio.down().to_le_bytes());
    hash.update((spec.core.taps_per_phase as u64).to_le_bytes());
    hash.update(spec.core.rolloff.numerator.to_le_bytes());
    hash.update(spec.core.rolloff.denominator.to_le_bytes());
    hash.update(spec.core.working_precision_bits.to_le_bytes());
    hash.update(spec.coefficient_fractional_bits.to_le_bytes());
    hash.update(spec.accumulator_bits.to_le_bytes());
    hash.update(b"nearest-ties-to-even;dc-last-largest-v1\0");
    hash
}

pub(super) fn identity(spec: &Spec, delay: u64, coefficients: &[BigQ]) -> String {
    let mut hash = spec_hash(spec, b"sex/sexfir/windowed-big-coefficients-v1\0");
    hash.update(delay.to_le_bytes());
    for c in coefficients {
        let raw = c.raw().to_string_radix(16);
        hash.update((raw.len() as u64).to_le_bytes());
        hash.update(raw.as_bytes());
    }
    encode_sha256(hash)
}

pub fn design(spec: &Spec) -> Result<Designed, DesignError> {
    let coefficient_count = spec.preflight()?;
    let core = &spec.core;
    let p = core.working_precision_bits;
    let delay = (core.taps_per_phase as u64 - 1) / 2;
    let format = BigQFormat::new(2, spec.coefficient_fractional_bits)?;
    let cutoff = cutoff_for_windowed(core, p);
    let pi = Float::with_val(p, Constant::Pi);
    let dolph = match core.window {
        WindowFunction::DolphChebyshev { attenuation_db } => {
            Some(DolphWindow::new(core.taps_per_phase, attenuation_db, p)?)
        }
        _ => None,
    };
    let mut coefficients = Vec::with_capacity(coefficient_count);
    let mut max_dc = Integer::new();
    let mut max_error = Float::with_val(p, 0);
    let mut max_l1 = Float::with_val(p, 0);
    let unity = Integer::from(1) << spec.coefficient_fractional_bits;
    for phase in 0..core.ratio.up() as usize {
        let mut ideal = (0..core.taps_per_phase)
            .map(|tap| design_windowed_tap(core, phase, tap, delay, &cutoff, &pi, dolph.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut sum = Float::with_val(p, 0);
        for c in &ideal {
            sum += c;
        }
        if sum.is_zero() {
            return Err(DesignError::ZeroPhaseGain(phase));
        }
        for c in &mut ideal {
            *c /= &sum;
        }
        let start = coefficients.len();
        let mut raw_sum = Integer::new();
        for (tap, c) in ideal.iter().enumerate() {
            let raw = quantize_big(c, spec.coefficient_fractional_bits, phase, tap)?;
            raw_sum += &raw;
            coefficients.push(BigQ::from_raw(raw, format)?);
        }
        let residual = &unity - raw_sum;
        // Match the native window designer's last-largest tie rule, not Kaiser's first-largest.
        let correction = coefficients[start..]
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| c.raw().clone().abs())
            .unwrap()
            .0
            + start;
        let corrected = Integer::from(coefficients[correction].raw() + &residual);
        coefficients[correction] = BigQ::from_raw(corrected, format)?;
        max_dc = max_dc.max(residual.abs());
        let mut l1 = Float::with_val(p, 0);
        for (tap, c) in ideal.iter().enumerate() {
            let mut error = Float::with_val(p, coefficients[start + tap].raw());
            error >>= spec.coefficient_fractional_bits;
            error -= c;
            error.abs_mut();
            if error > max_error {
                max_error = error.clone();
            }
            l1 += error;
        }
        if l1 > max_l1 {
            max_l1 = l1;
        }
    }
    let sha = identity(spec, delay, &coefficients);
    let bank = PolyphaseFirBigQ63::for_ratio(
        core.ratio,
        core.taps_per_phase,
        format,
        spec.accumulator_bits,
        coefficients,
    )?;
    let report = BigQuantizationReport {
        algorithm: algorithm(core.window),
        working_precision_bits: p,
        coefficient_fractional_bits: spec.coefficient_fractional_bits,
        coefficient_count,
        max_dc_correction_raw: max_dc.to_string(),
        max_abs_coefficient_error_decimal: max_error.to_string_radix(10, Some(20)),
        max_abs_coefficient_error_db: amplitude_db(&max_error, p),
        max_phase_l1_error_decimal: max_l1.to_string_radix(10, Some(20)),
        max_phase_l1_error_db: amplitude_db(&max_l1, p),
        configured_accumulator_bits: spec.accumulator_bits,
        required_accumulator_bits: bank.required_accumulator_bits(),
        coefficient_sha256: sha,
    };
    Ok(Designed {
        spec: spec.clone(),
        input_delay_frames: delay,
        bank,
        report,
    })
}

pub fn analyze_against(
    filter: &Designed,
    grid: u32,
    target: ErrorFloor,
) -> Result<(ResponseAnalysis, ResponseCompliance), DesignError> {
    let spec = filter.spec();
    let (response, check) = analyze_response_internal(
        ResponseContext {
            ratio: filter.ratio(),
            rolloff: spec.core.rolloff,
            precision: spec.core.working_precision_bits,
            phase_count: filter.bank.phase_count(),
            coefficient_quantization_error_decimal: &filter.report.max_phase_l1_error_decimal,
            grid_points_per_band: grid,
            target: Some(target),
        },
        |phase| {
            Ok(filter
                .bank
                .phase(phase as u64)?
                .iter()
                .map(|c| {
                    let mut value = Float::with_val(spec.core.working_precision_bits, c.raw());
                    value >>= spec.coefficient_fractional_bits;
                    value
                })
                .collect())
        },
    )?;
    Ok((response, check.expect("target provided")))
}

pub fn certify(
    filter: &Designed,
    target: ErrorFloor,
    limits: certificate::Limits,
) -> Result<certificate::Certificate, certificate::CertificateError> {
    certificate::certify_bank(
        filter.ratio(),
        filter.spec.core.rolloff,
        filter.bank.phase_count(),
        filter.spec.coefficient_fractional_bits,
        target,
        limits,
        |phase| {
            filter
                .bank
                .phase(phase as u64)
                .map(|c| c.iter().map(|c| c.raw().clone()).collect())
                .map_err(|_| certificate::CertificateError::InvalidCoefficients)
        },
    )
}

pub fn analyze_harmonics(
    filter: &Designed,
    grid: u32,
    target: ErrorFloor,
    limits: harmonics::Limits,
) -> Result<harmonics::Report, harmonics::AnalysisError> {
    harmonics::analyze_bank(
        harmonics::Spec {
            ratio: filter.ratio(),
            rolloff: filter.spec.core.rolloff,
            taps_per_phase: filter.spec.core.taps_per_phase as u64,
            fractional_bits: filter.spec.coefficient_fractional_bits,
            input_delay_frames: filter.input_delay_frames,
            precision_bits: filter.spec.core.working_precision_bits,
        },
        grid,
        target,
        limits,
        |phase| {
            filter
                .bank
                .phase(phase as u64)
                .map(|c| c.iter().map(|c| c.raw().clone()).collect())
                .map_err(|_| harmonics::AnalysisError::CoefficientCount)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refinement::{self, Designer, Request};

    fn spec(window: WindowFunction, bits: u32) -> Spec {
        Spec {
            core: WindowedSincSpec {
                ratio: RateRatio::from_fraction(3, 2).unwrap(),
                taps_per_phase: 17,
                rolloff: Fraction::new(9, 10).unwrap(),
                window,
                working_precision_bits: (bits + 64).max(128),
                quantization_rounding: RoundingMode::NearestTiesToEven,
            },
            coefficient_fractional_bits: bits,
            accumulator_bits: (bits + 80).max(128),
        }
    }

    #[test]
    fn arbitrary_windows_match_native_q62_coefficients_and_measurements() {
        for window in [
            WindowFunction::Rectangular,
            WindowFunction::Hann,
            WindowFunction::Blackman,
            WindowFunction::DolphChebyshev {
                attenuation_db: 100,
            },
        ] {
            let spec = spec(window, 62);
            let wide = design(&spec).unwrap();
            let native = design_windowed_sinc(&spec.core).unwrap();
            for phase in 0..3 {
                for (a, b) in wide
                    .bank
                    .phase(phase)
                    .unwrap()
                    .iter()
                    .zip(native.bank().phase(phase).unwrap())
                {
                    assert_eq!(*a.raw(), Integer::from(b.raw()), "{window:?}/{phase}");
                }
            }
            assert_eq!(
                wide.report.max_phase_l1_error_decimal,
                native.report().max_phase_l1_error_decimal
            );
            let floor = ErrorFloor::new(80).unwrap();
            assert_eq!(
                analyze_against(&wide, 9, floor).unwrap(),
                analyze_windowed_quantized_response_against(&native, 9, floor).unwrap()
            );
        }
    }

    #[test]
    fn arbitrary_windows_have_exact_dc_at_4096_bits_with_8192_bit_mac() {
        for window in [
            WindowFunction::Hann,
            WindowFunction::Blackman,
            WindowFunction::DolphChebyshev {
                attenuation_db: 300,
            },
        ] {
            let mut spec = spec(window, 4096);
            spec.accumulator_bits = 8192;
            let filter = design(&spec).unwrap();
            for phase in 0..3 {
                let sum = Integer::from(Integer::sum(
                    filter.bank.phase(phase).unwrap().iter().map(BigQ::raw),
                ));
                assert_eq!(sum, Integer::from(1) << 4096);
            }
            assert!(filter.report.required_accumulator_bits > 4096);
            assert!(filter.report.required_accumulator_bits <= 8192);
            assert_eq!(filter, design(&spec).unwrap());
        }
    }

    #[test]
    fn window_preflight_rejects_insufficient_precision_and_expensive_dolph() {
        let mut request = spec(WindowFunction::Hann, 96);
        request.core.working_precision_bits = 128;
        assert!(matches!(
            design(&request),
            Err(DesignError::CoefficientWorkingPrecisionTooLow {
                requested: 128,
                minimum: 160
            })
        ));
        assert_eq!(
            design(&request).unwrap_err().to_string(),
            "MPFR coefficient design requires at least 160 working bits (C+64), got 128"
        );
        request = spec(
            WindowFunction::DolphChebyshev {
                attenuation_db: 100,
            },
            96,
        );
        request.core.taps_per_phase = 4097;
        assert!(matches!(
            request.preflight(),
            Err(DesignError::DolphSeriesBudgetExceeded { .. })
        ));
        request.core.window = WindowFunction::Hann;
        assert!(request.preflight().is_ok());
        request.core.window = WindowFunction::DolphChebyshev { attenuation_db: 0 };
        assert!(matches!(
            request.preflight(),
            Err(DesignError::InvalidDolphAttenuation(0))
        ));
    }

    #[test]
    fn window_feedback_preserves_method_and_checks_all_requested_gates() {
        let initial = sexplan::plan_precision(sexplan::PlanRequest {
            ratio: RateRatio::from_fraction(3, 2).unwrap(),
            preset: sexplan::QualityPreset::Fast,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        let initial = sexplan::raise_coefficient_precision(&initial, 64, None).unwrap();
        let request = Request {
            designer: Designer::Hann,
            grid: 9,
            certificate: Some(certificate::Limits {
                max_taps: 4097,
                ..Default::default()
            }),
            harmonics: Some(harmonics::Limits::default()),
            ..Default::default()
        };
        let result = refinement::qualify(
            initial.clone(),
            request,
            Ok,
            |p| {
                Ok(design(&Spec::from_precision_plan(
                    p,
                    request.designer.window(p).unwrap(),
                )?)?)
            },
            refinement::assess_windowed,
        )
        .unwrap();
        assert!(result.attempts.len() > 1);
        assert_eq!(result.plan.target_error_floor, initial.target_error_floor);
        assert_eq!(
            result.plan.design_attenuation_db,
            initial.design_attenuation_db
        );
        assert_eq!(result.design.spec.core.window, WindowFunction::Hann);
        assert_eq!(
            result.plan.taps_per_phase - 1,
            (initial.taps_per_phase - 1) << (result.attempts.len() - 1)
        );
        let assessment = &result.attempts.last().unwrap().assessment;
        assert!(assessment.response_passed());
        assert_eq!(
            assessment.continuous.as_ref().unwrap().outcome,
            certificate::Outcome::Certified
        );
        assert!(assessment.harmonic.as_ref().unwrap().meets_target());
        assert!(
            refinement::assess_windowed(
                &result.design,
                &result.plan,
                Request {
                    designer: Designer::Blackman,
                    ..request
                }
            )
            .is_err()
        );
    }

    #[test]
    fn window_coefficient_refinement_does_not_change_the_window_or_length() {
        let initial = sexplan::plan_precision(sexplan::PlanRequest {
            ratio: RateRatio::from_fraction(1, 2).unwrap(),
            preset: sexplan::QualityPreset::Fast,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        let mut initial = sexplan::double_filter_length(&initial, None).unwrap();
        initial.coefficient_fractional_bits = 8;
        let request = Request {
            designer: Designer::Blackman,
            grid: 9,
            ..Default::default()
        };
        let result = refinement::qualify(
            initial.clone(),
            request,
            Ok,
            |p| {
                Ok(design(&Spec::from_precision_plan(
                    p,
                    request.designer.window(p).unwrap(),
                )?)?)
            },
            refinement::assess_windowed,
        )
        .unwrap();
        assert_eq!(
            result.attempts[0].next_change,
            Some(refinement::Change::CoefficientPrecision)
        );
        assert_eq!(
            result.attempts[1].plan.taps_per_phase,
            initial.taps_per_phase
        );
        assert_eq!(
            result.attempts[1]
                .assessment
                .actual_coefficient_fractional_bits,
            24
        );
        assert_eq!(result.design.spec.core.window, WindowFunction::Blackman);
    }

    #[test]
    fn fractional_dolph_uses_centered_signed_dft_bins() {
        let p = 192;
        let dolph = DolphWindow::new(3, 80, p).unwrap();
        // For N=3 the real inverse transform is (P0 + 2*P1*cos(2*pi*x/3))/sum(P).
        // At x=1/2, cos(pi/3)=1/2, so this is (P0+P1)/sum(P), exactly.
        let mut expected = Float::with_val(p, &dolph.spectrum[0]);
        expected += &dolph.spectrum[1];
        expected /= &dolph.center_normalization;
        let actual = dolph.at_distance(&Float::with_val(p, 0.5));
        let error = Float::with_val(p, actual - expected).abs();
        assert!(error < Float::with_val(p, Float::u_exp(1, -180)), "{error}");
    }
}
