//! Shared window selection, arbitrary-width materialization, and cache routing.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Controls {
    pub limits: sexfir::optimized::Limits,
    pub total_work: u64,
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            limits: sexfir::optimized::Limits {
                max_work: 4_000_000_000,
                ..Default::default()
            },
            total_work: 8_000_000_000,
        }
    }
}

#[derive(Default)]
pub(super) struct Arguments {
    window: Option<refinement::Designer>,
    method: Option<refinement::Designer>,
    pub controls: Controls,
    seen: Vec<String>,
}
impl Arguments {
    pub fn consume(&mut self, args: &[String], index: &mut usize) -> Result<bool, String> {
        let option = &args[*index];
        if option != "--window"
            && option != "--designer"
            && !matches!(
                option.as_str(),
                "--designer-work"
                    | "--designer-total-work"
                    | "--designer-storage"
                    | "--designer-grid-limit"
                    | "--designer-coefficients"
                    | "--designer-precision-limit"
            )
        {
            return Ok(false);
        }
        let value = args
            .get(*index + 1)
            .ok_or_else(|| format!("{option} requires a value"))?;
        if self.seen.contains(option) {
            return Err(format!("{option} may be specified only once"));
        }
        self.seen.push(option.clone());
        match option.as_str() {
            "--window" => {
                let window = value.parse()?;
                if matches!(
                    window,
                    refinement::Designer::GlobalLeastSquares | refinement::Designer::Equiripple
                ) {
                    return Err("unknown FIR window; use --designer global-ls|remez".to_owned());
                }
                self.window = Some(window);
            }
            "--designer" => {
                self.method = Some(match value.as_str() {
                    "windowed-sinc" => refinement::Designer::Kaiser,
                    "global-ls" => refinement::Designer::GlobalLeastSquares,
                    "remez" => refinement::Designer::Equiripple,
                    _ => {
                        return Err(format!(
                            "unknown FIR designer: {value}; expected windowed-sinc|global-ls|remez"
                        ));
                    }
                })
            }
            _ => {
                let number = value
                    .parse::<u64>()
                    .map_err(|_| format!("{option} requires a positive integer"))?;
                if number == 0 {
                    return Err(format!("{option} must be positive"));
                }
                match option.as_str() {
                    "--designer-work" => self.controls.limits.max_work = number,
                    "--designer-total-work" => self.controls.total_work = number,
                    "--designer-storage" => self.controls.limits.max_storage_bytes = number,
                    "--designer-grid-limit" => self.controls.limits.max_grid_points = number,
                    "--designer-coefficients" => self.controls.limits.max_coefficients = number,
                    "--designer-precision-limit" => {
                        self.controls.limits.max_precision_bits = u32::try_from(number)
                            .map_err(|_| "designer precision limit exceeds u32")?
                    }
                    _ => unreachable!(),
                }
            }
        }
        *index += 2;
        Ok(true)
    }
    pub fn finish(&self) -> Result<refinement::Designer, String> {
        let method = self.method.unwrap_or_default();
        if method != refinement::Designer::Kaiser {
            if self.window.is_some() {
                return Err("--window cannot be combined with global-ls or remez".to_owned());
            }
            Ok(method)
        } else {
            if self
                .seen
                .iter()
                .any(|option| option.starts_with("--designer-"))
            {
                return Err(
                    "designer resource controls require --designer global-ls|remez".to_owned(),
                );
            }
            Ok(self.window.unwrap_or_default())
        }
    }
}

pub(super) fn preflight(
    plan: &PrecisionPlan,
    designer: refinement::Designer,
    controls: Controls,
) -> Result<(), Box<dyn Error>> {
    designer.preflight(plan)?;
    if let Some(spec) = designer.optimized_spec(plan)? {
        let work = spec.preflight(controls.limits)?;
        if work.terms > controls.total_work {
            return Err(format!(
                "initial designer work {} exceeds cumulative limit {}",
                work.terms, controls.total_work
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn plan_and_design(
    plan: PrecisionPlan,
    designer: refinement::Designer,
    controls: Controls,
) -> Result<(PrecisionPlan, PlannedDesign, Option<CacheStatus>), Box<dyn Error>> {
    if let Some(spec) = designer.optimized_spec(&plan)? {
        spec.preflight(controls.limits)?;
        let explicit = env::var_os("SEX_COEFFICIENT_CACHE").is_some();
        let (design, status) = if let Some(directory) = coefficient_cache_directory() {
            match sexfir::cache::design_optimized_big_cached(&directory, &spec, controls.limits) {
                Ok((design, status)) => (design, Some(status)),
                Err(CacheError::Io(error)) if !explicit => {
                    eprintln!(
                        "sex: default coefficient cache is unavailable ({error}); designing without cache"
                    );
                    (sexfir::optimized::design(&spec, controls.limits)?, None)
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            (sexfir::optimized::design(&spec, controls.limits)?, None)
        };
        return Ok((plan, PlannedDesign::Optimized(design), status));
    }
    let window = designer
        .window(&plan)
        .ok_or("non-windowed designer in window route")?;
    let spec = sexfir::windowed::Spec::from_precision_plan(&plan, window)?;
    spec.preflight()?;
    let explicit = env::var_os("SEX_COEFFICIENT_CACHE").is_some();
    let (design, status) = if let Some(dir) = coefficient_cache_directory() {
        match sexfir::cache::design_windowed_big_cached(&dir, &spec) {
            Ok((design, status)) => (design, Some(status)),
            Err(CacheError::Io(error)) if !explicit => {
                eprintln!(
                    "sex: default coefficient cache is unavailable ({error}); designing without cache"
                );
                (sexfir::windowed::design(&spec)?, None)
            }
            Err(error) => return Err(error.into()),
        }
    } else {
        (sexfir::windowed::design(&spec)?, None)
    };
    Ok((plan, PlannedDesign::Windowed(design), status))
}
