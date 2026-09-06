//! Explicit complete-bank Kaiser coefficient certification; separate from response gates.
use super::{PlannedDesign, PrecisionPlan, QualityOptions, refinement};
use rug::{Float, Integer, Rational, float::Round};
use sexfir::enclosure::{BankLimits, certify_kaiser, certify_kaiser_big};
use std::error::Error;
use std::fmt::Write as _;

/// Conservative proposal, never a proof: the next bank must pass response and
/// enclosure checks again. No change to ratio, filter shape or requested bands.
pub(super) fn redesign_plan(
    plan: &PrecisionPlan,
    upper: &Rational,
    target: &Rational,
    explicit_working: Option<u32>,
) -> Result<PrecisionPlan, Box<dyn Error>> {
    if target <= &0 || upper <= target {
        return Err("invalid certified redesign error ratio".into());
    }
    let ratio = upper.clone() / target;
    let mut increment = ratio
        .numer()
        .significant_bits()
        .saturating_sub(ratio.denom().significant_bits());
    if ratio.numer() > &(ratio.denom().clone() << increment) {
        increment += 1;
    }
    increment = increment
        .checked_add(8)
        .ok_or("certified redesign precision overflow")?;
    let bits = plan
        .coefficient_fractional_bits
        .max(62)
        .checked_add(increment)
        .ok_or("certified redesign coefficient width overflow")?;
    let mut base = plan.clone();
    if explicit_working.is_none() {
        base.working_precision_bits = base
            .working_precision_bits
            .checked_add(increment)
            .ok_or("certified redesign working width overflow")?;
        if base.working_precision_bits > 1_048_576 {
            return Err("certified redesign working width exceeds safety limit".into());
        }
    }
    Ok(sexplan::raise_coefficient_precision(
        &base,
        bits,
        explicit_working,
    )?)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Controls {
    limits: BankLimits,
    target_bits: Option<u32>,
}
impl Controls {
    pub fn work_limit(&self) -> u64 {
        self.limits.max_total_series_terms
    }
    fn resolved(&self, plan: &PrecisionPlan) -> BankLimits {
        let mut limits = self.limits.clone();
        let bits = self.target_bits.unwrap_or(plan.amplitude_error_bits + 3);
        limits.target_max_phase_l1 =
            Some(Rational::from((Integer::from(1), Integer::from(1) << bits)));
        limits
    }
    pub fn preflight(
        &self,
        plan: &PrecisionPlan,
        quality: &QualityOptions,
    ) -> Result<(), Box<dyn Error>> {
        if quality.window != refinement::Designer::Kaiser {
            return Err("--certify-design currently requires the Kaiser windowed-sinc designer; no design-error certificate is claimed".into());
        }
        if plan.ratio.up() > self.limits.max_phases
            || plan.taps_per_phase > self.limits.phase.max_taps as u64
            || plan.coefficient_count > u128::from(self.limits.max_coefficients)
        {
            return Err("design certificate preflight: planned bank exceeds phase/tap/coefficient limits; no certificate is claimed".into());
        }
        let bits = self.target_bits.unwrap_or(plan.amplitude_error_bits + 3);
        if bits >= self.limits.phase.arithmetic.max_input_bits {
            return Err("design certificate target exceeds rational input-bit limit".into());
        }
        Ok(())
    }
    pub fn plan_description(&self, plan: &PrecisionPlan) -> String {
        format!(
            "design certificate requested: all phases, joint coefficient L1 <= 2^-{}; endpoint bits {}..{}; total I0 terms <= {}; not executed by plan",
            self.target_bits.unwrap_or(plan.amplitude_error_bits + 3),
            self.limits.phase.arithmetic.precision_bits,
            self.limits.max_precision_bits,
            self.limits.max_total_series_terms
        )
    }
    pub fn certify(
        &self,
        plan: &PrecisionPlan,
        design: &PlannedDesign,
        chain_target: Option<&Rational>,
        remaining_work: u64,
    ) -> Result<(String, sexfir::enclosure::BankCertificate), Box<dyn Error>> {
        let mut limits = self.resolved(plan);
        if remaining_work == 0 {
            return Err(sexfir::enclosure::CertificationError::TotalSeriesBudget.into());
        }
        limits.max_total_series_terms = limits.max_total_series_terms.min(remaining_work);
        if let Some(target) = chain_target {
            limits.target_max_phase_l1 = Some(
                limits
                    .target_max_phase_l1
                    .take()
                    .unwrap()
                    .min(target.clone()),
            );
        }
        let effective_target = limits.target_max_phase_l1.clone().unwrap();
        let certificate = match design {
            PlannedDesign::Native(bank) => certify_kaiser(bank, limits),
            PlannedDesign::Big(bank) => certify_kaiser_big(bank, limits),
            _ => return Err("design-error certificate is unavailable for this designer".into()),
        }?;
        let p = certificate.precision_bits();
        let decimal = |value: &Rational| {
            Float::with_val_round(p, value, Round::Up)
                .0
                .to_string_radix_round(10, Some(20), Round::Up)
        };
        let mut text = String::new();
        writeln!(
            text,
            "design certificate: passed; all {} phases; actual coefficient sha256 {}",
            certificate.phase_errors().len(),
            certificate.coefficient_sha256()
        )?;
        writeln!(
            text,
            "design certificate arithmetic: {} endpoint bits, {} pass(es), {} total I0 terms; actual coefficient fractional bits {}",
            p,
            certificate.passes(),
            certificate.total_series_terms(),
            certificate.coefficient_fractional_bits()
        )?;
        writeln!(
            text,
            "design certificate target: joint phase-L1 <= 2^-{}; actual upper bound {}",
            self.target_bits.unwrap_or(plan.amplitude_error_bits + 3),
            decimal(certificate.max_phase_l1())
        )?;
        writeln!(
            text,
            "design certificate maximum coefficient error upper bound: {}",
            decimal(certificate.max_absolute())
        )?;
        writeln!(
            text,
            "design certificate exact maximum phase-L1 bound: {}",
            certificate.max_phase_l1()
        )?;
        writeln!(
            text,
            "design certificate effective coefficient target: {effective_target}; includes the available whole-chain allowance"
        )?;
        writeln!(
            text,
            "design certificate scope: mathematical normalized Kaiser versus actual final integer bank; includes MPFR design error, coefficient quantization and DC correction jointly"
        )?;
        writeln!(
            text,
            "design certificate signal bound: pre-rounding FIR error <= input peak * maximum phase-L1; whole-chain numerical report separately propagates effects and signal rounding; excludes ideal-filter approximation and PCM/dither/clipping"
        )?;
        Ok((text, certificate))
    }
}

#[derive(Default)]
pub(super) struct Arguments {
    enabled: bool,
    controls: Controls,
    seen: Vec<String>,
}
impl Arguments {
    pub fn consume(&mut self, args: &[String], index: &mut usize) -> Result<bool, String> {
        let option = args[*index].as_str();
        if option != "--certify-design" && !option.starts_with("--design-certificate-") {
            return Ok(false);
        }
        if self.seen.iter().any(|old| old == option) {
            return Err(format!("{option} may be specified only once"));
        }
        self.seen.push(option.to_owned());
        if option == "--certify-design" {
            self.enabled = true;
            *index += 1;
            return Ok(true);
        }
        let raw = args
            .get(*index + 1)
            .ok_or_else(|| format!("{option} requires a value"))?;
        let value: u64 = raw
            .parse()
            .map_err(|_| format!("invalid design certificate limit: {raw}"))?;
        let maximum = match option {
            "--design-certificate-work" | "--design-certificate-phase-work" => 1_000_000_000,
            "--design-certificate-coefficients" => 1_073_741_824,
            "--design-certificate-error-bits" => 1_048_575,
            _ => 1_048_576,
        };
        if value == 0 || value > maximum {
            return Err(format!("{option} must be 1..{maximum}"));
        }
        let limits = &mut self.controls.limits;
        match option {
            "--design-certificate-bits" => limits.phase.arithmetic.precision_bits = value as u32,
            "--design-certificate-max-bits" => limits.max_precision_bits = value as u32,
            "--design-certificate-error-bits" => self.controls.target_bits = Some(value as u32),
            "--design-certificate-work" => limits.max_total_series_terms = value,
            "--design-certificate-phase-work" => limits.phase.max_total_series_terms = value,
            "--design-certificate-series" => {
                limits.phase.arithmetic.max_series_terms = value as u32
            }
            "--design-certificate-input-bits" => {
                limits.phase.arithmetic.max_input_bits = value as u32
            }
            "--design-certificate-taps" => limits.phase.max_taps = value as usize,
            "--design-certificate-phases" => limits.max_phases = value,
            "--design-certificate-coefficients" => limits.max_coefficients = value,
            _ => return Err(format!("unknown design certificate option: {option}")),
        }
        *index += 2;
        Ok(true)
    }
    pub fn finish(self) -> Result<Option<Controls>, String> {
        if !self.enabled {
            return if self.seen.is_empty() {
                Ok(None)
            } else {
                Err("--design-certificate-* options require --certify-design".to_owned())
            };
        }
        let limits = &self.controls.limits;
        if limits.phase.arithmetic.precision_bits < 16
            || limits.max_precision_bits < limits.phase.arithmetic.precision_bits
        {
            return Err(
                "design certificate endpoint bits must satisfy 16 <= bits <= max-bits".to_owned(),
            );
        }
        if self
            .controls
            .target_bits
            .is_some_and(|bits| bits >= limits.phase.arithmetic.max_input_bits)
        {
            return Err("design certificate error-bits must be smaller than input-bits".to_owned());
        }
        Ok(Some(self.controls))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Command, parse_args};

    #[test]
    fn redesign_proposal_preserves_filter_shape_and_explicit_precision() {
        let plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio: sexrate::RateRatio::from_fraction(3, 2).unwrap(),
            preset: sexplan::QualityPreset::Fast,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        let next = redesign_plan(
            &plan,
            &Rational::from((1, 1024)),
            &Rational::from((Integer::from(1), Integer::from(1) << 200)),
            None,
        )
        .unwrap();
        assert_eq!(next.coefficient_fractional_bits, 260);
        assert!(next.working_precision_bits >= plan.working_precision_bits + 198);
        assert_eq!(next.taps_per_phase, plan.taps_per_phase);
        assert_eq!(next.ratio, plan.ratio);
        assert_eq!(next.rolloff, plan.rolloff);
        assert_eq!(next.kaiser_beta, plan.kaiser_beta);
        assert_eq!(next.target_error_floor, plan.target_error_floor);
        assert!(
            redesign_plan(
                &plan,
                &Rational::from(1),
                &Rational::from((Integer::from(1), Integer::from(1) << 200)),
                Some(128)
            )
            .is_err()
        );
        assert!(redesign_plan(&plan, &Rational::from(1), &Rational::from(0), None).is_err());
    }

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }
    #[test]
    fn all_frontends_accept_explicit_design_certificate_controls() {
        for mut args in [
            vec!["sex", "in.wav", "out.wav", "-r", "48000"],
            vec!["sex", "analyze", "in.wav", "-r", "48000"],
            vec!["sex", "plan", "in.wav", "-r", "48000"],
        ] {
            args.extend([
                "--design-certificate-bits",
                "32",
                "--design-certificate-max-bits",
                "100",
                "--design-certificate-error-bits",
                "75",
                "--certify-design",
            ]);
            let quality = match parse(&args).unwrap() {
                Command::Convert { quality, .. }
                | Command::Analyze { quality, .. }
                | Command::Plan { quality, .. } => quality,
                _ => panic!("unexpected frontend"),
            };
            let controls = quality.design_certificate.unwrap();
            assert_eq!(controls.limits.phase.arithmetic.precision_bits, 32);
            assert_eq!(controls.limits.max_precision_bits, 100);
            assert_eq!(controls.target_bits, Some(75));
        }
    }
    #[test]
    fn limits_require_enablement_and_consistent_precision() {
        for suffix in [
            vec!["--design-certificate-bits", "32"],
            vec!["--certify-design", "--certify-design"],
            vec!["--certify-design", "--design-certificate-work", "0"],
            vec![
                "--certify-design",
                "--design-certificate-work",
                "1000000001",
            ],
            vec!["--certify-design", "--design-certificate-bits", "15"],
            vec!["--certify-design", "--design-certificate-max-bits", "16"],
            vec![
                "--certify-design",
                "--design-certificate-error-bits",
                "65536",
            ],
            vec!["--certify-design", "--design-certificate-unrecognized", "1"],
            vec![
                "--certify-design",
                "--design-certificate-series",
                "1",
                "--design-certificate-series",
                "1",
            ],
            vec!["--certify-design", "--design-certificate-taps"],
        ] {
            let mut args = vec!["sex", "analyze", "in.wav", "-r", "48000"];
            args.extend(suffix);
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
    }
    #[test]
    fn default_target_uses_planner_coefficient_allowance() {
        let plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio: sexrate::RateRatio::from_fraction(3, 2).unwrap(),
            preset: sexplan::QualityPreset::High,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        assert_eq!(
            Controls::default()
                .resolved(&plan)
                .target_max_phase_l1
                .unwrap(),
            Rational::from((
                Integer::from(1),
                Integer::from(1) << (plan.amplitude_error_bits + 3)
            ))
        );
    }
}
