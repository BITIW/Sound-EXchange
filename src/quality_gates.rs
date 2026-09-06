//! One gate configuration for planning, analysis, and the bank used by conversion.
use super::{CertificateLimits, HarmonicLimits, PrecisionPlan, parse_nonzero_u32, parse_u64};

/// CLI work policy, deliberately separate from the library's generic limits.
/// An explicit field is never increased, including during certified redesign.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct RefinementBudget {
    pub max_attempts: Option<u32>,
    pub max_total_coefficients: Option<u64>,
    pub max_response_terms: Option<u64>,
    pub max_precision_bits: Option<u32>,
}
impl From<sexfir::refinement::Limits> for RefinementBudget {
    fn from(limits: sexfir::refinement::Limits) -> Self {
        Self {
            max_attempts: Some(limits.max_attempts),
            max_total_coefficients: Some(limits.max_total_coefficients),
            max_response_terms: Some(limits.max_response_terms),
            max_precision_bits: Some(limits.max_precision_bits),
        }
    }
}
impl RefinementBudget {
    /// Resolve once against the initial execution geometry, not each retry.
    /// Four initial-bank equivalents reserve bounded feedback work, not four
    /// guaranteed candidates: refined banks can be larger than the initial one.
    pub fn resolve(
        self,
        initial: &PrecisionPlan,
        grid: u32,
    ) -> Result<sexfir::refinement::Limits, String> {
        let mut defaults = sexfir::refinement::Limits::default();
        if initial.preset == sexplan::QualityPreset::Until40k {
            if self.max_total_coefficients.is_none() {
                defaults.max_total_coefficients = defaults.max_total_coefficients.max(
                    initial
                        .coefficient_count
                        .checked_mul(4)
                        .and_then(|v| u64::try_from(v).ok())
                        .ok_or("Until-40k default coefficient work budget overflow")?,
                );
            }
            if self.max_response_terms.is_none() {
                defaults.max_response_terms = defaults.max_response_terms.max(
                    initial
                        .coefficient_count
                        .checked_mul(8 * u128::from(grid))
                        .and_then(|v| u64::try_from(v).ok())
                        .ok_or("Until-40k default response work budget overflow")?,
                );
            }
        }
        Ok(sexfir::refinement::Limits {
            max_attempts: self.max_attempts.unwrap_or(defaults.max_attempts),
            max_total_coefficients: self
                .max_total_coefficients
                .unwrap_or(defaults.max_total_coefficients),
            max_response_terms: self
                .max_response_terms
                .unwrap_or(defaults.max_response_terms),
            max_precision_bits: self
                .max_precision_bits
                .unwrap_or(defaults.max_precision_bits),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Gates {
    pub grid: u32,
    pub certificate: Option<CertificateLimits>,
    pub harmonics: Option<HarmonicLimits>,
}
impl Default for Gates {
    fn default() -> Self {
        Self {
            grid: 65,
            certificate: None,
            harmonics: None,
        }
    }
}
impl Gates {
    pub fn has_supplemental(self) -> bool {
        self.certificate.is_some() || self.harmonics.is_some()
    }

    /// Does not materialize a bank or claim response compliance. The refiner
    /// also repeats its per-candidate guards after any shape/precision change.
    pub fn preflight(
        self,
        plan: &PrecisionPlan,
    ) -> Result<Option<u64>, Box<dyn std::error::Error>> {
        if let Some(limits) = self.certificate
            && plan.taps_per_phase > limits.max_taps as u64
        {
            return Err(format!(
                "continuous certificate: Inconclusive(Taps); no coefficients designed or loaded: {} taps/phase exceed limit {}; no quality pass is claimed",
                plan.taps_per_phase, limits.max_taps,
            ).into());
        }
        self.harmonics
            .map(|limits| {
                sexfir::harmonics::preflight(
                    sexfir::harmonics::Spec {
                        ratio: plan.ratio,
                        rolloff: sexfir::Fraction::new(
                            plan.rolloff.numerator(),
                            plan.rolloff.denominator(),
                        )?,
                        taps_per_phase: plan.taps_per_phase,
                        fractional_bits: match plan.backend {
                            sexplan::BackendRequirement::NativeI128 => 62,
                            _ => plan.coefficient_fractional_bits,
                        },
                        input_delay_frames: (plan.taps_per_phase - 1) / 2,
                        precision_bits: plan.working_precision_bits,
                    },
                    self.grid,
                    limits,
                )
                .map_err(Into::into)
            })
            .transpose()
    }
}

#[derive(Default)]
pub(super) struct Arguments {
    grid: Option<u32>,
    certificate_enabled: bool,
    certificate: CertificateLimits,
    harmonic_enabled: bool,
    harmonics: HarmonicLimits,
    seen: Vec<String>,
}
impl Arguments {
    pub fn consume(&mut self, args: &[String], index: &mut usize) -> Result<bool, String> {
        let option = args[*index].as_str();
        if !matches!(option, "--grid" | "--certify" | "--harmonics")
            && !option.starts_with("--certificate-")
            && !option.starts_with("--harmonic-")
        {
            return Ok(false);
        }
        if self.seen.iter().any(|seen| seen == option) {
            return Err(format!("{option} may be specified only once"));
        }
        self.seen.push(option.to_owned());
        if option == "--certify" || option == "--harmonics" {
            if option == "--certify" {
                self.certificate_enabled = true;
            } else {
                self.harmonic_enabled = true;
            }
            *index += 1;
            return Ok(true);
        }
        let raw = args
            .get(*index + 1)
            .ok_or_else(|| format!("{option} requires a value"))?;
        match option {
            "--grid" => {
                let value = parse_nonzero_u32(raw, "response grid size")?;
                if value < 2 {
                    return Err("response grid size must be at least 2".to_owned());
                }
                self.grid = Some(value);
            }
            "--certificate-work" => {
                self.certificate.max_work = positive(raw, "certificate work budget")?
            }
            "--certificate-taps" => {
                self.certificate.max_taps =
                    usize::try_from(parse_nonzero_u32(raw, "certificate tap limit")?)
                        .map_err(|_| "certificate tap limit exceeds platform size")?;
            }
            "--certificate-depth" => {
                self.certificate.max_depth =
                    raw.parse().map_err(|_| "invalid certificate depth")?;
                if self.certificate.max_depth > 64 {
                    return Err("certificate depth must be 0..64".to_owned());
                }
            }
            "--certificate-endpoint-bits" => {
                self.certificate.endpoint_bits =
                    parse_nonzero_u32(raw, "certificate endpoint bits")?;
                if !(8..=128).contains(&self.certificate.endpoint_bits) {
                    return Err("certificate endpoint bits must be 8..128".to_owned());
                }
            }
            "--certificate-integer-bits" => {
                self.certificate.max_integer_bits = positive(raw, "certificate integer bits")?
            }
            "--harmonic-work" => self.harmonics.max_work = positive(raw, "harmonic work budget")?,
            "--harmonic-coefficients" => {
                self.harmonics.max_coefficients = positive(raw, "harmonic coefficient limit")?
            }
            "--harmonic-phases" => {
                self.harmonics.max_phases = positive(raw, "harmonic phase limit")?
            }
            "--harmonic-storage-bytes" => {
                self.harmonics.max_storage_bytes = positive(raw, "harmonic storage limit")?
            }
            "--harmonic-precision-limit" => {
                self.harmonics.max_precision_bits =
                    u32::try_from(positive(raw, "harmonic precision limit")?)
                        .map_err(|_| "harmonic precision limit exceeds u32")?;
            }
            _ => return Err(format!("unknown quality gate option: {option}")),
        }
        *index += 2;
        Ok(true)
    }

    pub fn finish(self) -> Result<Gates, String> {
        if !self.certificate_enabled && self.seen.iter().any(|v| v.starts_with("--certificate-")) {
            return Err("--certificate-* options require --certify".to_owned());
        }
        if !self.harmonic_enabled && self.seen.iter().any(|v| v.starts_with("--harmonic-")) {
            return Err("--harmonic-* options require --harmonics".to_owned());
        }
        Ok(Gates {
            grid: self.grid.unwrap_or(65),
            certificate: self.certificate_enabled.then_some(self.certificate),
            harmonics: self.harmonic_enabled.then_some(self.harmonics),
        })
    }
}

fn positive(raw: &str, name: &str) -> Result<u64, String> {
    let value = parse_u64(raw, name)?;
    if value == 0 {
        return Err(format!("{name} must be positive"));
    }
    Ok(value)
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use sexfir::refinement::{self, Limits, Request};
    use sexplan::{PlanRequest, QualityPreset, plan_precision};

    fn initial(preset: QualityPreset) -> PrecisionPlan {
        plan_precision(PlanRequest {
            ratio: sexrate::RateRatio::from_fraction(160, 147).unwrap(),
            preset,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap()
    }

    #[test]
    fn until_common_ratio_reserves_four_initial_bank_equivalents() {
        let plan = initial(QualityPreset::Until40k);
        assert_eq!(plan.coefficient_count, 10_698_720);
        let limits = RefinementBudget::default().resolve(&plan, 65).unwrap();
        assert_eq!(
            limits,
            Limits {
                max_total_coefficients: 42_794_880,
                max_response_terms: 5_563_334_400,
                ..Limits::default()
            }
        );
        assert_eq!(
            RefinementBudget::default()
                .resolve(&plan, 129)
                .unwrap()
                .max_response_terms,
            42_794_880 * 258
        );
    }

    #[test]
    fn ordinary_presets_keep_generic_limits() {
        for preset in [
            QualityPreset::Fast,
            QualityPreset::Sane,
            QualityPreset::High,
            QualityPreset::Absurd,
            QualityPreset::Pointless,
        ] {
            assert_eq!(
                RefinementBudget::default()
                    .resolve(&initial(preset), 65)
                    .unwrap(),
                Limits::default()
            );
        }
    }

    #[test]
    fn each_explicit_limit_wins_independently() {
        let plan = initial(QualityPreset::Until40k);
        let limits = RefinementBudget {
            max_attempts: Some(1),
            max_total_coefficients: Some(7),
            max_precision_bits: Some(512),
            ..RefinementBudget::default()
        }
        .resolve(&plan, 65)
        .unwrap();
        assert_eq!(limits.max_attempts, 1);
        assert_eq!(limits.max_total_coefficients, 7);
        assert_eq!(limits.max_precision_bits, 512);
        assert_eq!(limits.max_response_terms, 5_563_334_400);
        let limits = RefinementBudget {
            max_response_terms: Some(9),
            ..RefinementBudget::default()
        }
        .resolve(&plan, 65)
        .unwrap();
        assert_eq!(limits.max_response_terms, 9);
        assert_eq!(limits.max_total_coefficients, 42_794_880);
    }

    #[test]
    fn redesign_cannot_refill_resolved_or_exhausted_budgets() {
        let mut plan = initial(QualityPreset::Until40k);
        let mut limits = RefinementBudget::default().resolve(&plan, 65).unwrap();
        limits.max_total_coefficients -= plan.coefficient_count as u64;
        limits.max_response_terms = 0;
        limits.max_attempts = 0;
        plan.coefficient_count *= 2;
        assert_eq!(
            RefinementBudget::from(limits).resolve(&plan, 129).unwrap(),
            limits
        );
    }

    #[test]
    fn automatic_budget_overflow_fails_closed_but_explicit_limits_are_not_replaced() {
        let mut plan = initial(QualityPreset::Until40k);
        plan.coefficient_count = u128::MAX;
        assert!(RefinementBudget::default().resolve(&plan, 65).is_err());
        assert!(
            RefinementBudget {
                max_total_coefficients: Some(1),
                ..RefinementBudget::default()
            }
            .resolve(&plan, 65)
            .is_err()
        );
        let limits = Limits::default();
        assert_eq!(
            RefinementBudget::from(limits).resolve(&plan, 65).unwrap(),
            limits
        );
    }

    #[test]
    fn real_refiner_admits_initial_until_geometry_without_allocating_the_bank() {
        let plan = initial(QualityPreset::Until40k);
        let resolved = RefinementBudget::default().resolve(&plan, 65).unwrap();
        for (limits, should_materialize) in [
            (resolved, true),
            (
                Limits {
                    max_total_coefficients: 4_000_000,
                    ..resolved
                },
                false,
            ),
            (
                Limits {
                    max_response_terms: 200_000_000,
                    ..resolved
                },
                false,
            ),
            (
                Limits {
                    max_precision_bits: 512,
                    ..resolved
                },
                false,
            ),
        ] {
            let mut called = false;
            let failure = refinement::qualify(
                plan.clone(),
                Request {
                    limits,
                    ..Request::default()
                },
                Ok,
                |_| -> Result<(), Box<dyn std::error::Error>> {
                    called = true;
                    Err("test stops before allocating coefficients".into())
                },
                |_, _, _| unreachable!("no actual response assessment in this preflight test"),
            )
            .unwrap_err();
            assert_eq!(called, should_materialize, "{failure}");
            assert!(failure.attempts.is_empty());
        }
    }
}
