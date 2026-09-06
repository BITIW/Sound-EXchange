//! Arbitrary-width least-squares and Parks–McClellan coefficient design.
//! MPFR solves the design problem; only quantized integers reach the engine.
use super::*;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Method {
    /// Type-I global prototype. The legacy native `design_least_squares` API
    /// instead fits independent phases and is intentionally a different method.
    GlobalLeastSquares(LeastSquaresSpec),
    Equiripple(EquirippleSpec),
}

impl Method {
    pub fn ratio(&self) -> RateRatio {
        match self {
            Self::GlobalLeastSquares(s) => s.ratio,
            Self::Equiripple(s) => s.ratio,
        }
    }
    pub fn taps(&self) -> usize {
        match self {
            Self::GlobalLeastSquares(s) => s.taps_per_phase,
            Self::Equiripple(s) => s.taps_per_phase,
        }
    }
    pub fn rolloff(&self) -> Fraction {
        match self {
            Self::GlobalLeastSquares(s) => s.rolloff,
            Self::Equiripple(s) => s.rolloff,
        }
    }
    pub fn precision(&self) -> u32 {
        match self {
            Self::GlobalLeastSquares(s) => s.working_precision_bits,
            Self::Equiripple(s) => s.working_precision_bits,
        }
    }
    pub fn algorithm(&self) -> &'static str {
        match self {
            Self::GlobalLeastSquares(spec) if spec.ratio.up() == spec.ratio.down() => {
                "least-squares-global-lowpass-big-unity-v2"
            }
            Self::Equiripple(spec) if spec.ratio.up() == spec.ratio.down() => {
                "parks-mcclellan-lowpass-big-unity-v3"
            }
            Self::GlobalLeastSquares(_) => "least-squares-global-lowpass-big-v1",
            Self::Equiripple(_) => "parks-mcclellan-lowpass-big-v2",
        }
    }
    fn validate(&self) -> Result<(), DesignError> {
        match self {
            Self::GlobalLeastSquares(s) => s.validate(),
            Self::Equiripple(s) => s.validate(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spec {
    pub method: Method,
    pub coefficient_fractional_bits: u32,
    pub accumulator_bits: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_coefficients: u64,
    pub max_grid_points: u64,
    pub max_work: u64,
    pub max_storage_bytes: u64,
    pub max_precision_bits: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_coefficients: 1_000_000,
            max_grid_points: 1_000_000,
            max_work: 67_108_864,
            max_storage_bytes: 134_217_728,
            max_precision_bits: 16384,
        }
    }
}

/// Conservative structural work and storage estimates, not wall-clock bounds.
/// MPFR transcendental cost also depends on precision and operand values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Work {
    pub coefficient_count: u64,
    pub grid_points: u64,
    pub solver_dimension: u64,
    pub global_prototype_length: Option<u64>,
    /// Explicit candidate cap used before allocating Remez selector tables.
    pub extrema_candidate_limit: Option<u64>,
    pub terms: u64,
    pub storage_bytes: u64,
    pub cosine_cache_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Design(DesignError),
    InvalidLimits,
    Limit {
        resource: &'static str,
        required: u128,
        maximum: u64,
    },
    WorkOverflow,
}
impl From<DesignError> for Error {
    fn from(e: DesignError) -> Self {
        Self::Design(e)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Design(e) => e.fmt(f),
            Self::InvalidLimits => f.write_str("invalid optimized-designer limits"),
            Self::Limit {
                resource,
                required,
                maximum,
            } => write!(
                f,
                "optimized FIR {resource} requires {required}; limit {maximum}"
            ),
            Self::WorkOverflow => f.write_str("optimized FIR work/storage estimate overflow"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Design(error) => Some(error),
            _ => None,
        }
    }
}

fn capped(resource: &'static str, required: u128, maximum: u64) -> Result<u64, Error> {
    if required > u128::from(maximum) {
        return Err(Error::Limit {
            resource,
            required,
            maximum,
        });
    }
    Ok(required as u64)
}

impl Spec {
    pub fn from_precision_plan(
        plan: &PrecisionPlan,
        equiripple: bool,
    ) -> Result<Self, DesignError> {
        let taps = usize::try_from(plan.taps_per_phase)
            .map_err(|_| DesignError::CoefficientCountOverflow)?;
        let global_length = u128::from(plan.taps_per_phase)
            .checked_sub(1)
            .and_then(|span| span.checked_mul(u128::from(plan.ratio.up())))
            .and_then(|span| span.checked_add(1))
            .ok_or(DesignError::CoefficientCountOverflow)?;
        let grid = u32::try_from(
            global_length
                .div_ceil(2)
                .checked_mul(2)
                .ok_or(DesignError::CoefficientCountOverflow)?
                .max(256),
        )
        .map_err(|_| DesignError::CoefficientCountOverflow)?;
        let rolloff = Fraction::new(plan.rolloff.numerator(), plan.rolloff.denominator())?;
        let pass = Fraction::new(1, 1)?;
        let stop = Fraction::new(10, 1)?;
        let method = if equiripple {
            Method::Equiripple(EquirippleSpec {
                ratio: plan.ratio,
                taps_per_phase: taps,
                rolloff,
                passband_weight: pass,
                stopband_weight: stop,
                grid_density: 16,
                max_iterations: 64,
                working_precision_bits: plan.working_precision_bits,
                quantization_rounding: RoundingMode::NearestTiesToEven,
            })
        } else {
            Method::GlobalLeastSquares(LeastSquaresSpec {
                ratio: plan.ratio,
                taps_per_phase: taps,
                rolloff,
                passband_weight: pass,
                stopband_weight: stop,
                grid_points_per_band: grid,
                working_precision_bits: plan.working_precision_bits,
                quantization_rounding: RoundingMode::NearestTiesToEven,
            })
        };
        Ok(Self {
            method,
            coefficient_fractional_bits: plan.coefficient_fractional_bits,
            accumulator_bits: plan.planned_accumulator_bits,
        })
    }

    /// All validation happens before the first grid, matrix or coefficient allocation.
    pub fn preflight(&self, limits: Limits) -> Result<Work, Error> {
        if limits.max_coefficients == 0
            || limits.max_grid_points == 0
            || limits.max_work == 0
            || limits.max_storage_bytes == 0
            || limits.max_precision_bits == 0
        {
            return Err(Error::InvalidLimits);
        }
        self.method.validate()?;
        BigQFormat::new(2, self.coefficient_fractional_bits).map_err(DesignError::from)?;
        if self.accumulator_bits == 0 {
            return Err(DesignError::ZeroAccumulatorWidth.into());
        }
        let p = self.method.precision();
        let minimum = self
            .coefficient_fractional_bits
            .checked_add(64)
            .ok_or(Error::WorkOverflow)?;
        if p < minimum {
            return Err(DesignError::CoefficientWorkingPrecisionTooLow {
                requested: p,
                minimum,
            }
            .into());
        }
        capped(
            "MPFR bits",
            u128::from(p),
            u64::from(limits.max_precision_bits),
        )?;
        let l = u128::from(self.method.ratio().up());
        let t = self.method.taps() as u128;
        let count = capped(
            "coefficients",
            l.checked_mul(t).ok_or(Error::WorkOverflow)?,
            limits.max_coefficients.min(MAX_BIG_COEFFICIENTS as u64),
        )?;
        // Once L*T is capped at 2^24, every following term fits u128 except
        // explicit iteration/grid factors; those are checked as well.
        let (grid, dim, prototype, extrema_limit, terms, slots, bookkeeping) = match &self.method {
            Method::GlobalLeastSquares(s) => {
                let n = (t - 1) * l + 1;
                let a = n.div_ceil(2);
                let gp = u128::from(s.grid_points_per_band);
                let g = prototype_grid_points(s.ratio, gp).ok_or(Error::WorkOverflow)?;
                // Type-I cosine moments + RHS + one global solve/residual + quantization.
                let work = a * a * a + (2 * a - 1) * g + a * gp + 3 * a * a + 4 * l * t;
                (
                    g,
                    a,
                    Some(n as u64),
                    None,
                    work,
                    a * a + 12 * a + 3 * g + n + 4 * t + 64,
                    0,
                )
            }
            Method::Equiripple(s) => {
                let n = (t - 1) * l + 1;
                let e = n.div_ceil(2) + 1;
                let g = prototype_grid_points(s.ratio, u128::from(s.grid_density) * e)
                    .ok_or(Error::WorkOverflow)?;
                let k = 2 * e + 4;
                let cells = (e + 1) * k;
                // The selector examines every eligible predecessor. Preserve
                // MPFR sum/tie ordering instead of replacing it with a prefix
                // maximum that could change a rounded equal-score decision.
                let work = remez_work(e, g, s.max_iterations)
                    .and_then(|v| v.checked_add(4 * l * t))
                    .ok_or(Error::WorkOverflow)?;
                (
                    g,
                    e,
                    Some(n as u64),
                    Some(k as u64),
                    work,
                    e * e + 8 * e + 8 * g + n + 4 * t + 64 + cells,
                    // Option<usize> predecessors, row vectors and candidates;
                    // conservatively reserve 64-bit layout on both targets.
                    16 * cells + 128 * e + 16 * k,
                )
            }
        };
        let grid_points = capped(
            "grid points",
            grid,
            limits.max_grid_points.min(u64::from(u32::MAX)),
        )?;
        let terms = capped("structural work", terms, limits.max_work)?;
        // Include MPFR/GMP payload plus conservative per-value bookkeeping.
        // Both methods solve one global prototype, then quantize one phase at a time.
        let float_bytes = 64 + u128::from(p.div_ceil(64)) * 8;
        let integer_bytes =
            64 + u128::from((self.coefficient_fractional_bits + 2).div_ceil(64)) * 8;
        let storage = slots
            .checked_mul(float_bytes)
            .and_then(|n| n.checked_add((u128::from(count) + t + 64) * integer_bytes))
            .and_then(|n| n.checked_add(bookkeeping))
            .ok_or(Error::WorkOverflow)?;
        let storage_bytes = capped("estimated storage bytes", storage, limits.max_storage_bytes)?;
        let cosine_cache_bytes = if matches!(self.method, Method::Equiripple(_)) {
            remez_cosine_cache_bytes(grid_points, dim as u64 - 1, p)
                .filter(|bytes| *bytes <= limits.max_storage_bytes - storage_bytes)
                .unwrap_or(0)
        } else {
            0
        };
        Ok(Work {
            coefficient_count: count,
            grid_points,
            solver_dimension: dim as u64,
            global_prototype_length: prototype,
            extrema_candidate_limit: extrema_limit,
            terms,
            storage_bytes,
            cosine_cache_bytes,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Solution {
    /// Maximum |R*a-b| for global cosine amplitudes before DC normalization. An MPFR measurement,
    /// not a condition-number estimate or proof of exact least-squares optimality.
    LeastSquares { max_normal_residual_decimal: String },
    /// Grid-converged global prototype before per-phase DC normalization and quantization.
    Equiripple {
        iterations: u32,
        extremal_frequencies_decimal: Vec<String>,
        prototype_weighted_error_decimal: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub quantization: BigQuantizationReport,
    pub work: Work,
    pub solution: Solution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Designed {
    pub(super) spec: Spec,
    pub(super) input_delay_frames: u64,
    pub(super) bank: PolyphaseFirBigQ63,
    pub(super) report: Report,
}
impl Designed {
    pub fn spec(&self) -> &Spec {
        &self.spec
    }
    pub fn ratio(&self) -> RateRatio {
        self.spec.method.ratio()
    }
    pub const fn input_delay_frames(&self) -> u64 {
        self.input_delay_frames
    }
    pub fn bank(&self) -> &PolyphaseFirBigQ63 {
        &self.bank
    }
    pub fn report(&self) -> &Report {
        &self.report
    }
    pub fn into_bank(self) -> PolyphaseFirBigQ63 {
        self.bank
    }
}

struct Quantizer {
    precision: u32,
    format: BigQFormat,
    coefficients: Vec<BigQ>,
    max_dc: Integer,
    max_error: Float,
    max_l1: Float,
}
impl Quantizer {
    fn new(spec: &Spec, count: usize) -> Result<Self, Error> {
        let p = spec.method.precision();
        Ok(Self {
            precision: p,
            format: BigQFormat::new(2, spec.coefficient_fractional_bits)
                .map_err(DesignError::from)?,
            coefficients: Vec::with_capacity(count),
            max_dc: Integer::new(),
            max_error: Float::with_val(p, 0),
            max_l1: Float::with_val(p, 0),
        })
    }

    fn phase(&mut self, phase: usize, mut ideal: Vec<Float>, bits: u32) -> Result<(), Error> {
        let p = self.precision;
        let mut sum = Float::with_val(p, 0);
        for c in &ideal {
            sum += c;
        }
        if sum.is_zero() {
            return Err(DesignError::ZeroPhaseGain(phase).into());
        }
        for c in &mut ideal {
            *c /= &sum;
        }
        let start = self.coefficients.len();
        let mut raw_sum = Integer::new();
        for (tap, c) in ideal.iter().enumerate() {
            let raw = quantize_big(c, bits, phase, tap)?;
            raw_sum += &raw;
            self.coefficients
                .push(BigQ::from_raw(raw, self.format).map_err(DesignError::from)?);
        }
        let residual = (Integer::from(1) << bits) - raw_sum;
        let index = self.coefficients[start..]
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| c.raw().clone().abs())
            .expect("validated nonempty phase")
            .0
            + start;
        let raw = Integer::from(self.coefficients[index].raw() + &residual);
        self.coefficients[index] = BigQ::from_raw(raw, self.format).map_err(DesignError::from)?;
        self.max_dc = self.max_dc.clone().max(residual.abs());
        let mut l1 = Float::with_val(p, 0);
        for (tap, c) in ideal.iter().enumerate() {
            let mut error = Float::with_val(p, self.coefficients[start + tap].raw());
            error >>= bits;
            error -= c;
            error.abs_mut();
            if error > self.max_error {
                self.max_error = error.clone();
            }
            l1 += error;
        }
        if l1 > self.max_l1 {
            self.max_l1 = l1;
        }
        Ok(())
    }
}

pub(super) fn spec_hash(spec: &Spec, domain: &[u8]) -> Sha256 {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(spec.method.algorithm().as_bytes());
    hash.update(spec.method.ratio().up().to_le_bytes());
    hash.update(spec.method.ratio().down().to_le_bytes());
    hash.update((spec.method.taps() as u64).to_le_bytes());
    hash.update(spec.method.rolloff().numerator().to_le_bytes());
    hash.update(spec.method.rolloff().denominator().to_le_bytes());
    let (pass, stop) = match &spec.method {
        Method::GlobalLeastSquares(s) => {
            hash.update(s.grid_points_per_band.to_le_bytes());
            (s.passband_weight, s.stopband_weight)
        }
        Method::Equiripple(s) => {
            hash.update(s.grid_density.to_le_bytes());
            hash.update(s.max_iterations.to_le_bytes());
            (s.passband_weight, s.stopband_weight)
        }
    };
    for n in [
        pass.numerator(),
        pass.denominator(),
        stop.numerator(),
        stop.denominator(),
    ] {
        hash.update(n.to_le_bytes());
    }
    hash.update(spec.method.precision().to_le_bytes());
    hash.update(spec.coefficient_fractional_bits.to_le_bytes());
    hash.update(spec.accumulator_bits.to_le_bytes());
    hash.update(b"nearest-ties-to-even;dc-last-largest-v1\0");
    hash
}

pub(super) fn identity(spec: &Spec, delay: u64, coefficients: &[BigQ]) -> String {
    let mut hash = spec_hash(spec, b"sex/sexfir/optimized-big-v1\0");
    hash.update(delay.to_le_bytes());
    for c in coefficients {
        let raw = c.raw().to_string_radix(16);
        hash.update((raw.len() as u64).to_le_bytes());
        hash.update(raw.as_bytes());
    }
    encode_sha256(hash)
}

pub fn design(spec: &Spec, limits: Limits) -> Result<Designed, Error> {
    let work = spec.preflight(limits)?;
    let p = spec.method.precision();
    let phases = spec.method.ratio().up() as usize;
    let taps = spec.method.taps();
    let delay = (taps as u64 - 1) / 2;
    let mut q = Quantizer::new(spec, work.coefficient_count as usize)?;
    let length = work
        .global_prototype_length
        .expect("global prototype length") as usize;
    let amplitude_count = length.div_ceil(2);
    let (amplitudes, solution) = match &spec.method {
        Method::GlobalLeastSquares(s) => {
            let grid = build_prototype_grid(
                s.ratio,
                s.rolloff,
                (s.passband_weight, s.stopband_weight),
                s.grid_points_per_band as usize,
                p,
            );
            let mut moments = Vec::with_capacity(2 * amplitude_count - 1);
            for lag in 0..(2 * amplitude_count - 1) {
                let offset = Float::with_val(p, lag);
                let mut sum = Float::with_val(p, 0);
                for point in &grid {
                    let mut term = cosine_frequency_offset(&point.frequency, &offset, p);
                    term *= &point.weight;
                    sum += term;
                }
                moments.push(sum);
            }
            // cos(jw) cos(kw) = (cos((j-k)w) + cos((j+k)w))/2.
            let element = |row: usize, column: usize| {
                let mut value = Float::with_val(p, &moments[row.abs_diff(column)]);
                value += &moments[row + column];
                value /= 2;
                value
            };
            let factor = cholesky_matrix(amplitude_count, p, element)?;
            let mut rhs = Vec::with_capacity(amplitude_count);
            for lag in 0..amplitude_count {
                let offset = Float::with_val(p, lag);
                let mut sum = Float::with_val(p, 0);
                for point in grid.iter().filter(|point| point.band == 0) {
                    let mut term = cosine_frequency_offset(&point.frequency, &offset, p);
                    term *= &point.weight;
                    term *= &point.desired;
                    sum += term;
                }
                rhs.push(sum);
            }
            let amplitudes = cholesky_solve(&factor, &rhs, p);
            let mut max_residual = Float::with_val(p, 0);
            for (row, b) in rhs.iter().enumerate() {
                let mut residual = Float::with_val(p, 0);
                for (column, amplitude) in amplitudes.iter().enumerate() {
                    let mut term = element(row, column);
                    term *= amplitude;
                    residual += term;
                }
                residual -= b;
                residual.abs_mut();
                if residual > max_residual {
                    max_residual = residual;
                }
            }
            (
                amplitudes,
                Solution::LeastSquares {
                    max_normal_residual_decimal: max_residual.to_string_radix(10, Some(20)),
                },
            )
        }
        Method::Equiripple(s) => {
            let grid = build_remez_grid(
                s,
                s.grid_density as usize * work.solver_dimension as usize,
                p,
            );
            let exchange =
                remez_exchange(s, amplitude_count, &grid, p, work.cosine_cache_bytes != 0)?;
            let solution = Solution::Equiripple {
                iterations: exchange.iterations,
                extremal_frequencies_decimal: exchange
                    .extrema
                    .iter()
                    .map(|i| grid[*i].frequency.to_string_radix(10, Some(20)))
                    .collect(),
                prototype_weighted_error_decimal: exchange
                    .max_weighted_error
                    .to_string_radix(10, Some(20)),
            };
            (exchange.amplitudes, solution)
        }
    };
    let center = (length - 1) / 2;
    let mut prototype = vec![Float::with_val(p, 0); length];
    prototype[center] = amplitudes[0].clone();
    for offset in 1..amplitude_count {
        let mut value = amplitudes[offset].clone();
        value /= 2;
        prototype[center - offset] = value.clone();
        prototype[center + offset] = value;
    }
    for phase in 0..phases {
        let ideal = (0..taps)
            .map(|tap| {
                prototype
                    .get(tap * phases + phase)
                    .cloned()
                    .unwrap_or_else(|| Float::with_val(p, 0))
            })
            .collect();
        q.phase(phase, ideal, spec.coefficient_fractional_bits)?;
    }
    let sha = identity(spec, delay, &q.coefficients);
    let bank = PolyphaseFirBigQ63::for_ratio(
        spec.method.ratio(),
        taps,
        q.format,
        spec.accumulator_bits,
        q.coefficients,
    )
    .map_err(DesignError::from)?;
    let quantization = BigQuantizationReport {
        algorithm: spec.method.algorithm(),
        working_precision_bits: p,
        coefficient_fractional_bits: spec.coefficient_fractional_bits,
        coefficient_count: work.coefficient_count as usize,
        max_dc_correction_raw: q.max_dc.to_string(),
        max_abs_coefficient_error_decimal: q.max_error.to_string_radix(10, Some(20)),
        max_abs_coefficient_error_db: amplitude_db(&q.max_error, p),
        max_phase_l1_error_decimal: q.max_l1.to_string_radix(10, Some(20)),
        max_phase_l1_error_db: amplitude_db(&q.max_l1, p),
        configured_accumulator_bits: spec.accumulator_bits,
        required_accumulator_bits: bank.required_accumulator_bits(),
        coefficient_sha256: sha,
    };
    Ok(Designed {
        spec: spec.clone(),
        input_delay_frames: delay,
        bank,
        report: Report {
            quantization,
            work,
            solution,
        },
    })
}

pub fn analyze_against(
    filter: &Designed,
    grid: u32,
    target: ErrorFloor,
) -> Result<(ResponseAnalysis, ResponseCompliance), DesignError> {
    let spec = filter.spec();
    let p = spec.method.precision();
    let (response, check) = analyze_response_internal(
        ResponseContext {
            ratio: filter.ratio(),
            rolloff: spec.method.rolloff(),
            precision: p,
            phase_count: filter.bank.phase_count(),
            coefficient_quantization_error_decimal: &filter
                .report
                .quantization
                .max_phase_l1_error_decimal,
            grid_points_per_band: grid,
            target: Some(target),
        },
        |phase| {
            Ok(filter
                .bank
                .phase(phase as u64)?
                .iter()
                .map(|c| {
                    let mut value = Float::with_val(p, c.raw());
                    value >>= spec.coefficient_fractional_bits;
                    value
                })
                .collect())
        },
    )?;
    Ok((response, check.expect("provided target")))
}

pub fn certify(
    filter: &Designed,
    target: ErrorFloor,
    limits: certificate::Limits,
) -> Result<certificate::Certificate, certificate::CertificateError> {
    certificate::certify_bank(
        filter.ratio(),
        filter.spec.method.rolloff(),
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
            rolloff: filter.spec.method.rolloff(),
            taps_per_phase: filter.spec.method.taps() as u64,
            fractional_bits: filter.spec.coefficient_fractional_bits,
            input_delay_frames: filter.input_delay_frames,
            precision_bits: filter.spec.method.precision(),
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

/// Exact DC-corrected coefficient-error gate, plus the measured MPFR-reference
/// L1 gate. Passing says nothing about the unquantized filter's spectral quality.
pub fn coefficient_budget_passed(
    filter: &Designed,
    target: ErrorFloor,
) -> Result<bool, DesignError> {
    let exact = crate::refinement::coefficient_budget_for(
        target,
        filter.spec.method.taps(),
        filter.spec.coefficient_fractional_bits,
    );
    let parsed = Float::parse(&filter.report.quantization.max_phase_l1_error_decimal)
        .map_err(|_| DesignError::InvalidInternalMeasurement)?;
    let error = Float::with_val(filter.spec.method.precision(), parsed);
    if !error.is_finite() || error < 0 {
        return Err(DesignError::InvalidInternalMeasurement);
    }
    Ok(exact && amplitude_meets_floor(&error, target, filter.spec.method.precision()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchLimits {
    pub design: Limits,
    pub max_attempts: u32,
    pub max_total_work: u64,
    pub max_total_coefficients: u64,
}
impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            design: Limits::default(),
            max_attempts: 8,
            max_total_work: 200_000_000,
            max_total_coefficients: 4_000_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrecisionAttempt {
    pub spec: Spec,
    pub coefficient_budget_passed: bool,
    pub measured_phase_l1_error_decimal: String,
    pub coefficient_sha256: String,
    pub work: Work,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrecisionSearch {
    pub design: Designed,
    pub attempts: Vec<PrecisionAttempt>,
    pub charged_work: u64,
    pub charged_coefficients: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchError {
    pub reason: String,
    pub attempts: Vec<PrecisionAttempt>,
}
impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "optimized coefficient precision search failed after {} assessed candidates: {}",
            self.attempts.len(),
            self.reason
        )
    }
}
impl std::error::Error for SearchError {}

/// Re-solve and re-quantize at C+16 on an insufficient coefficient budget.
/// Method, length, bands, weights, grid and iteration cap never change.
/// `auto_working_precision` permits raising MPFR to at least C+64 in 32-bit steps.
/// This is coefficient precision selection, NOT whole-filter quality qualification.
pub fn refine_quantization(
    initial: &Spec,
    target: ErrorFloor,
    auto_working_precision: bool,
    limits: SearchLimits,
) -> Result<PrecisionSearch, SearchError> {
    refine_quantization_with(
        initial,
        target,
        auto_working_precision,
        limits,
        |spec, limits| design(spec, limits).map_err(|error| error.to_string()),
    )
}

pub(super) fn refine_quantization_with(
    initial: &Spec,
    target: ErrorFloor,
    auto_working_precision: bool,
    limits: SearchLimits,
    mut materialize: impl FnMut(&Spec, Limits) -> Result<Designed, String>,
) -> Result<PrecisionSearch, SearchError> {
    let mut spec = initial.clone();
    let mut attempts = Vec::new();
    let mut charged_work = 0_u64;
    let mut charged_coefficients = 0_u64;
    let result = (|| -> Result<Designed, String> {
        if limits.max_attempts == 0
            || limits.max_attempts > 128
            || limits.max_total_work == 0
            || limits.max_total_coefficients == 0
        {
            return Err("invalid precision-search limits".into());
        }
        for _ in 0..limits.max_attempts {
            let work = spec.preflight(limits.design).map_err(|e| e.to_string())?;
            charged_work = charged_work
                .checked_add(work.terms)
                .ok_or("cumulative work overflow")?;
            charged_coefficients = charged_coefficients
                .checked_add(work.coefficient_count)
                .ok_or("cumulative coefficient count overflow")?;
            if charged_work > limits.max_total_work
                || charged_coefficients > limits.max_total_coefficients
            {
                return Err("cumulative precision-search resource limit".into());
            }
            let filter = materialize(&spec, limits.design)?;
            if filter.spec != spec || filter.report.work != work {
                return Err(
                    "optimized materializer returned a different request or work estimate".into(),
                );
            }
            let passed = coefficient_budget_passed(&filter, target).map_err(|e| e.to_string())?;
            let report = &filter.report.quantization;
            attempts.push(PrecisionAttempt {
                spec: spec.clone(),
                coefficient_budget_passed: passed,
                measured_phase_l1_error_decimal: report.max_phase_l1_error_decimal.clone(),
                coefficient_sha256: report.coefficient_sha256.clone(),
                work,
            });
            if passed {
                return Ok(filter);
            }
            let bits = spec
                .coefficient_fractional_bits
                .checked_add(16)
                .ok_or("coefficient width overflow")?;
            let old_bits = spec.coefficient_fractional_bits;
            spec.coefficient_fractional_bits = bits;
            // Preserve the same normalized accumulator headroom when C grows.
            spec.accumulator_bits = spec
                .accumulator_bits
                .checked_add(bits - old_bits)
                .ok_or("accumulator width overflow")?;
            if auto_working_precision {
                let minimum = bits
                    .checked_add(64)
                    .ok_or("working precision overflow")?
                    .max(spec.method.precision());
                let precision = minimum
                    .checked_add(31)
                    .ok_or("working precision overflow")?
                    / 32
                    * 32;
                match &mut spec.method {
                    Method::GlobalLeastSquares(s) => s.working_precision_bits = precision,
                    Method::Equiripple(s) => s.working_precision_bits = precision,
                }
            }
        }
        Err("precision-search candidate limit exhausted".into())
    })();
    match result {
        Ok(design) => Ok(PrecisionSearch {
            design,
            attempts,
            charged_work,
            charged_coefficients,
        }),
        Err(reason) => Err(SearchError { reason, attempts }),
    }
}
