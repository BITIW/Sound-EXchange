//! Sampled harmonic-transfer analysis of the complete quantized rate converter.
//! Unlike per-phase magnitudes, this includes every periodically time-varying
//! image and its output-frequency location. This is not a continuous certificate.
use crate::{DesignedBigFilter, DesignedFilter, Fraction, amplitude_db, amplitude_meets_floor};
use rug::{Float, Integer, Rational};
use sexplan::ErrorFloor;
use sexrate::RateRatio;
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_work: u64,
    pub max_coefficients: u64,
    pub max_phases: u64,
    pub max_precision_bits: u32,
    /// Conservative storage estimate for retained MPFR values, not a hard RSS cap.
    pub max_storage_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_work: 100_000_000,
            max_coefficients: 1_000_000,
            max_phases: 1024,
            max_precision_bits: 16384,
            max_storage_bytes: 128 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Spec {
    pub ratio: RateRatio,
    pub rolloff: Fraction,
    pub taps_per_phase: u64,
    pub fractional_bits: u32,
    pub input_delay_frames: u64,
    pub precision_bits: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnalysisError {
    InvalidConfiguration,
    CoefficientCount,
    CoefficientNotExactlyRepresentable,
    ResourceLimit(&'static str),
}
impl fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "harmonic-transfer analysis: {self:?}; no harmonic quality pass is claimed"
        )
    }
}
impl std::error::Error for AnalysisError {}

/// Deterministic preflight, usable before designing or loading a bank.
/// Work units bound weighted complex Horner operations, not CPU instructions.
pub fn preflight(spec: Spec, grid: u32, limits: Limits) -> Result<u64, AnalysisError> {
    if spec.taps_per_phase == 0
        || grid < 2
        || spec.precision_bits < 96
        || spec
            .fractional_bits
            .checked_add(2)
            .is_none_or(|bits| bits > spec.precision_bits)
        || spec.rolloff.numerator() > spec.rolloff.denominator()
        || u128::from(spec.rolloff.numerator()) * 2 < u128::from(spec.rolloff.denominator())
        || limits.max_work == 0
        || limits.max_coefficients == 0
        || limits.max_phases == 0
        || limits.max_precision_bits == 0
        || limits.max_storage_bytes == 0
    {
        return Err(AnalysisError::InvalidConfiguration);
    }
    let phases = u128::from(spec.ratio.up());
    let coefficients = phases * u128::from(spec.taps_per_phase);
    if phases > u128::from(limits.max_phases) {
        return Err(AnalysisError::ResourceLimit("phases"));
    }
    if coefficients > u128::from(limits.max_coefficients) || coefficients > usize::MAX as u128 {
        return Err(AnalysisError::ResourceLimit("coefficients"));
    }
    if spec.precision_bits > limits.max_precision_bits {
        return Err(AnalysisError::ResourceLimit("precision"));
    }
    let bytes_per_float =
        u128::from(spec.precision_bits.div_ceil(8)) + std::mem::size_of::<Float>() as u128 + 32;
    if (coefficients + 16 * phases) * bytes_per_float > u128::from(limits.max_storage_bytes) {
        return Err(AnalysisError::ResourceLimit("storage estimate"));
    }
    // Two nondegenerate bands when upsampling, three when downsampling.
    let points = if spec.ratio.up() >= spec.ratio.down() {
        2 * u128::from(grid) - 1
    } else {
        3 * u128::from(grid) - 2
    };
    let work = phases
        .saturating_mul(phases)
        .saturating_add(coefficients)
        .saturating_add(4 * phases)
        .saturating_mul(9)
        .saturating_mul(points);
    if work > u128::from(limits.max_work) {
        return Err(AnalysisError::ResourceLimit("work"));
    }
    Ok(work as u64)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Complex {
    pub re: Float,
    pub im: Float,
}
impl Complex {
    fn zero(precision: u32) -> Self {
        Self {
            re: Float::with_val(precision, 0),
            im: Float::with_val(precision, 0),
        }
    }
    fn phasor(cycles: &Rational, precision: u32) -> Self {
        // Exact modulo reduction keeps even huge delay coordinates small.
        let mut cycles = cycles.clone();
        cycles -= cycles.clone().floor();
        cycles *= 2;
        let angle = Float::with_val(precision, &cycles);
        Self {
            re: angle.clone().cos_pi(),
            im: angle.sin_pi(),
        }
    }
    fn multiply(&self, other: &Self) -> Self {
        let p = self.re.prec();
        Self {
            re: Float::with_val(p, &self.re * &other.re) - Float::with_val(p, &self.im * &other.im),
            im: Float::with_val(p, &self.re * &other.im) + Float::with_val(p, &self.im * &other.re),
        }
    }
    fn norm_squared(&self) -> Float {
        Float::with_val(self.re.prec(), &self.re * &self.re)
            + Float::with_val(self.im.prec(), &self.im * &self.im)
    }
    fn magnitude(&self) -> Float {
        self.norm_squared().sqrt()
    }
}

/// Signed cycles/output sample in [-1/2, 1/2), before taking absolute value
/// for a real-signal spectrum. Input-frequency and image labels remain exact.
pub fn output_frequency(ratio: RateRatio, input: &Rational, image: u64) -> Rational {
    let mut output = input.clone();
    output += image;
    output *= ratio.down();
    output /= ratio.up();
    let mut offset = output.clone();
    offset += Rational::from((1, 2));
    output -= offset.floor();
    output
}

/// Immutable analysis view, with exact conversion of every stored coefficient.
pub struct Bank {
    spec: Spec,
    coefficients: Vec<Vec<Float>>,
    roots: Vec<Complex>,
}
impl Bank {
    pub fn new(
        spec: Spec,
        grid: u32,
        limits: Limits,
        mut phase: impl FnMut(usize) -> Result<Vec<Integer>, AnalysisError>,
    ) -> Result<Self, AnalysisError> {
        preflight(spec, grid, limits)?;
        let mut coefficients = Vec::new();
        let mut roots = Vec::new();
        for p in 0..spec.ratio.up() {
            let raw = phase(p as usize)?;
            if raw.len() as u64 != spec.taps_per_phase {
                return Err(AnalysisError::CoefficientCount);
            }
            if raw
                .iter()
                .any(|c| c.significant_bits() > spec.precision_bits)
            {
                return Err(AnalysisError::CoefficientNotExactlyRepresentable);
            }
            coefficients.push(
                raw.iter()
                    .map(|r| {
                        let mut value = Float::with_val(spec.precision_bits, r);
                        value >>= spec.fractional_bits;
                        value
                    })
                    .collect(),
            );
            roots.push(Complex::phasor(
                &(-Rational::from((p, spec.ratio.up()))),
                spec.precision_bits,
            ));
        }
        Ok(Self {
            spec,
            coefficients,
            roots,
        })
    }

    /// C_r(f) = (1/L) sum_p exp(2 pi i f (D-p/L)) H_p(f)
    ///                         exp(-2 pi i r p/L).
    /// y[n] = sum_r C_r(f) exp(2 pi i (f+r) M n/L) for a unit complex input tone.
    pub fn at(&self, input_frequency: &Rational) -> Result<Vec<Complex>, AnalysisError> {
        if *input_frequency < 0 || *input_frequency > Rational::from((1, 2)) {
            return Err(AnalysisError::InvalidConfiguration);
        }
        let precision = self.spec.precision_bits;
        let z = Complex::phasor(&(-input_frequency.clone()), precision);
        let mut phases = Vec::with_capacity(self.coefficients.len());
        for (p, coefficients) in self.coefficients.iter().enumerate() {
            let mut response = Complex::zero(precision);
            for coefficient in coefficients.iter().rev() {
                response = response.multiply(&z);
                response.re += coefficient;
            }
            let coordinate = (Rational::from(self.spec.input_delay_frames)
                - Rational::from((p, self.spec.ratio.up())))
                * input_frequency;
            phases.push(response.multiply(&Complex::phasor(&coordinate, precision)));
        }
        let mut harmonics = Vec::with_capacity(phases.len());
        for root in &self.roots {
            let mut response = Complex::zero(precision);
            for phase in phases.iter().rev() {
                response = response.multiply(root);
                response.re += &phase.re;
                response.im += &phase.im;
            }
            response.re /= self.spec.ratio.up();
            response.im /= self.spec.ratio.up();
            harmonics.push(response);
        }
        Ok(harmonics)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Peak {
    pub amplitude_db: String,
    pub input_frequency: Rational,
    pub image: u64,
    pub output_frequency: Rational,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub frequency_points: usize,
    pub harmonics_per_point: u64,
    pub precision_bits: u32,
    pub planned_work: u64,
    pub passband_end: Rational,
    pub stopband_start: Rational,
    pub main_passband_ripple_db: String,
    pub main_complex_error_db: String,
    pub main_phase_error_radians: String,
    pub image_peak: Option<Peak>,
    /// sqrt(sum_(r>0) |C_r|^2), maximized over all sampled input frequencies.
    pub image_l2_peak_db: Option<String>,
    pub image_l2_peak_input_frequency: Option<Rational>,
    pub stopband_peak: Peak,
    pub target: ErrorFloor,
    pub main_complex_error_meets_target: bool,
    pub image_peak_meets_target: bool,
    pub image_l2_meets_target: bool,
    pub stopband_meets_target: bool,
}
impl Report {
    pub const fn meets_target(&self) -> bool {
        self.main_complex_error_meets_target
            && self.image_peak_meets_target
            && self.image_l2_meets_target
            && self.stopband_meets_target
    }
}

fn band_edges(spec: Spec) -> (Rational, Rational) {
    let lower = Rational::from((spec.ratio.up(), spec.ratio.down())).min(Rational::from(1)) / 2;
    let pass =
        (Rational::from((spec.rolloff.numerator(), spec.rolloff.denominator())) * 2 - 1) * &lower;
    (pass, lower)
}

fn frequencies(pass: &Rational, stop: &Rational, grid: u32) -> Vec<Rational> {
    let mut points = BTreeSet::new();
    let edges = [
        Rational::from(0),
        pass.clone(),
        stop.clone(),
        Rational::from((1, 2)),
    ];
    for edges in edges.windows(2) {
        for point in 0..grid {
            let mut f = Rational::from(&edges[1] - &edges[0]);
            f *= Rational::from((point, grid - 1));
            f += &edges[0];
            points.insert(f);
        }
    }
    points.into_iter().collect()
}

pub fn analyze_bank(
    spec: Spec,
    grid: u32,
    target: ErrorFloor,
    limits: Limits,
    phase: impl FnMut(usize) -> Result<Vec<Integer>, AnalysisError>,
) -> Result<Report, AnalysisError> {
    let work = preflight(spec, grid, limits)?;
    let bank = Bank::new(spec, grid, limits, phase)?;
    let p = spec.precision_bits;
    let (pass_end, stop_start) = band_edges(spec);
    let points = frequencies(&pass_end, &stop_start, grid);
    let mut min_main = Float::with_val(p, rug::float::Special::Infinity);
    let mut max_main = Float::with_val(p, 0);
    let mut complex_error = Float::with_val(p, 0);
    let mut phase_error = Float::with_val(p, 0);
    let mut image_max = Float::with_val(p, 0);
    let mut image_l2 = Float::with_val(p, 0);
    let mut image_peak = None;
    let mut image_l2_frequency = None;
    let mut stop_max = Float::with_val(p, 0);
    let mut stop_peak = None;
    for f in &points {
        let harmonics = bank.at(f)?;
        if *f <= pass_end {
            let magnitude = harmonics[0].magnitude();
            if magnitude < min_main {
                min_main = magnitude.clone();
            }
            if magnitude > max_main {
                max_main = magnitude;
            }
            let mut error = harmonics[0].clone();
            error.re -= 1;
            complex_error = complex_error.max(&error.magnitude());
            phase_error = phase_error.max(&harmonics[0].im.clone().atan2(&harmonics[0].re).abs());
        }
        let mut image_power = Float::with_val(p, 0);
        for (image, value) in harmonics.iter().enumerate() {
            let magnitude = value.magnitude();
            let peak = || Peak {
                amplitude_db: amplitude_db(&magnitude, p),
                input_frequency: f.clone(),
                image: image as u64,
                output_frequency: output_frequency(spec.ratio, f, image as u64),
            };
            if image != 0 {
                image_power += value.norm_squared();
                if image_peak.is_none() || magnitude > image_max {
                    image_peak = Some(peak());
                    image_max = magnitude.clone();
                }
            }
            if *f >= stop_start && (stop_peak.is_none() || magnitude > stop_max) {
                stop_peak = Some(peak());
                stop_max = magnitude;
            }
        }
        if harmonics.len() > 1 {
            let magnitude = image_power.sqrt();
            if image_l2_frequency.is_none() || magnitude > image_l2 {
                image_l2 = magnitude;
                image_l2_frequency = Some(f.clone());
            }
        }
    }
    Ok(Report {
        frequency_points: points.len(),
        harmonics_per_point: spec.ratio.up(),
        precision_bits: p,
        planned_work: work,
        passband_end: pass_end,
        stopband_start: stop_start,
        main_passband_ripple_db: if min_main.is_zero() {
            "undefined (passband includes zero)".to_owned()
        } else {
            amplitude_db(&(max_main / &min_main), p)
        },
        main_complex_error_db: amplitude_db(&complex_error, p),
        main_phase_error_radians: if min_main.is_zero() {
            "undefined (passband includes zero)".to_owned()
        } else {
            phase_error.to_string_radix(10, Some(16))
        },
        image_peak,
        image_l2_peak_db: image_l2_frequency
            .as_ref()
            .map(|_| amplitude_db(&image_l2, p)),
        image_l2_peak_input_frequency: image_l2_frequency,
        stopband_peak: stop_peak.expect("nonempty grid includes the stopband endpoint"),
        target,
        main_complex_error_meets_target: amplitude_meets_floor(&complex_error, target, p),
        image_peak_meets_target: amplitude_meets_floor(&image_max, target, p),
        image_l2_meets_target: amplitude_meets_floor(&image_l2, target, p),
        stopband_meets_target: amplitude_meets_floor(&stop_max, target, p),
    })
}

pub fn analyze(
    filter: &DesignedFilter,
    grid: u32,
    target: ErrorFloor,
    limits: Limits,
) -> Result<Report, AnalysisError> {
    analyze_bank(
        Spec {
            ratio: filter.ratio(),
            rolloff: filter.spec().rolloff,
            taps_per_phase: filter.bank().taps_per_phase() as u64,
            fractional_bits: 62,
            input_delay_frames: filter.input_delay_frames(),
            precision_bits: filter.spec().working_precision_bits,
        },
        grid,
        target,
        limits,
        |phase| {
            filter
                .bank()
                .phase(phase as u64)
                .map(|c| c.iter().map(|c| Integer::from(c.raw())).collect())
                .map_err(|_| AnalysisError::CoefficientCount)
        },
    )
}

pub fn analyze_big(
    filter: &DesignedBigFilter,
    grid: u32,
    target: ErrorFloor,
    limits: Limits,
) -> Result<Report, AnalysisError> {
    analyze_bank(
        Spec {
            ratio: filter.ratio(),
            rolloff: filter.spec().core.rolloff,
            taps_per_phase: filter.bank().taps_per_phase() as u64,
            fractional_bits: filter.spec().coefficient_fractional_bits,
            input_delay_frames: filter.input_delay_frames(),
            precision_bits: filter.spec().core.working_precision_bits,
        },
        grid,
        target,
        limits,
        |phase| {
            filter
                .bank()
                .phase(phase as u64)
                .map(|c| c.iter().map(|c| c.raw().clone()).collect())
                .map_err(|_| AnalysisError::CoefficientCount)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sexq::{OverflowPolicy, Q1_63, Q2_62, RoundingMode};
    use sexrate::{CausalResamplerQ63, PolyphaseFirQ63};
    use std::sync::Arc;

    fn spec(up: u64, down: u64) -> Spec {
        Spec {
            ratio: RateRatio::from_fraction(up, down).unwrap(),
            rolloff: Fraction::new(9, 10).unwrap(),
            taps_per_phase: 3,
            fractional_bits: 8,
            input_delay_frames: 1,
            precision_bits: 192,
        }
    }
    fn raw(phase: usize) -> Vec<Integer> {
        // Deliberately asymmetric, phase-dependent and not exactly DC-normalized.
        [31 + phase as i32, 123 - 2 * phase as i32, -17]
            .into_iter()
            .map(Integer::from)
            .collect()
    }
    fn close(actual: &Complex, expected: &Complex, bits: i32) {
        let mut tolerance = Float::with_val(actual.re.prec(), 1);
        tolerance >>= bits;
        assert!(
            Float::with_val(actual.re.prec(), &actual.re - &expected.re).abs() < tolerance,
            "real: {:?} != {:?}",
            actual,
            expected
        );
        assert!(
            Float::with_val(actual.im.prec(), &actual.im - &expected.im).abs() < tolerance,
            "imaginary: {:?} != {:?}",
            actual,
            expected
        );
    }

    #[test]
    fn folded_output_frequencies_keep_exact_signed_alias_coordinates() {
        let ratio = RateRatio::from_fraction(160, 147).unwrap();
        let f = Rational::from((1, 4));
        let locations: BTreeSet<_> = (0..160).map(|r| output_frequency(ratio, &f, r)).collect();
        assert_eq!(locations.len(), 160);
        assert_eq!(output_frequency(ratio, &f, 0), Rational::from((147, 640)));
        assert!(
            locations
                .iter()
                .all(|x| *x >= Rational::from((-1, 2)) && *x < Rational::from((1, 2)))
        );
        assert_eq!(
            output_frequency(RateRatio::from_fraction(1, 3).unwrap(), &f, 0),
            Rational::from((-1, 4))
        );
        assert_eq!(
            output_frequency(
                RateRatio::from_fraction(1, 1).unwrap(),
                &Rational::from((1, 2)),
                0
            ),
            Rational::from((-1, 2))
        );
    }

    #[test]
    fn every_harmonic_matches_independent_global_prototype_sum() {
        for (up, down) in [(1, 3), (2, 1), (3, 2), (5, 7), (7, 4)] {
            for delay in [1, u64::MAX] {
                let spec = Spec {
                    input_delay_frames: delay,
                    ..spec(up, down)
                };
                let bank = Bank::new(spec, 3, Limits::default(), |p| Ok(raw(p))).unwrap();
                for f in [
                    Rational::from(0),
                    Rational::from((7, 32)),
                    Rational::from((1, 2)),
                ] {
                    let harmonics = bank.at(&f).unwrap();
                    for (r, actual) in harmonics.iter().enumerate() {
                        let mut sum = Complex::zero(192);
                        for phase in 0..up {
                            for (tap, coefficient) in raw(phase as usize).iter().enumerate() {
                                let global_index = up * tap as u64 + phase;
                                let cycles =
                                    -(f.clone() + r as u64) * global_index / up + f.clone() * delay;
                                let rotation = Complex::phasor(&cycles, 192);
                                let coefficient = Float::with_val(192, coefficient) / (256 * up);
                                sum.re += Float::with_val(192, rotation.re * &coefficient);
                                sum.im += Float::with_val(192, rotation.im * &coefficient);
                            }
                        }
                        close(actual, &sum, 165);
                    }
                }
            }
        }
    }

    #[test]
    fn harmonic_reconstruction_matches_real_fixed_point_streams_and_chunks() {
        for (up, down) in [(1, 3), (2, 1), (3, 2), (5, 7), (7, 4)] {
            let spec = spec(up, down);
            let analysis = Bank::new(spec, 3, Limits::default(), |p| Ok(raw(p))).unwrap();
            let f = Rational::from((1, 4));
            let responses = analysis.at(&f).unwrap();
            let coefficients = (0..up)
                .flat_map(|p| raw(p as usize))
                .map(|r| Q2_62::from_raw(r.to_i64().unwrap() << 54))
                .collect();
            let fir = Arc::new(PolyphaseFirQ63::for_ratio(spec.ratio, 3, coefficients).unwrap());
            for block in [1, 7, 4096] {
                let mut signals = Vec::new();
                for quadrature in [false, true] {
                    let input: Vec<_> = (0..128)
                        .map(|n| {
                            let code = if quadrature {
                                [0, 1, 0, -1][n % 4]
                            } else {
                                [1, 0, -1, 0][n % 4]
                            };
                            Q1_63::from_raw(code * (1_i64 << 61))
                        })
                        .collect();
                    let mut stream = CausalResamplerQ63::new_with_input_delay(
                        spec.ratio,
                        fir.clone(),
                        1,
                        RoundingMode::NearestTiesToEven,
                        OverflowPolicy::Error,
                    )
                    .unwrap();
                    let mut output = Vec::new();
                    for chunk in input.chunks(block) {
                        stream.push_into(chunk, &mut output).unwrap();
                    }
                    signals.push(output);
                }
                for (n, (real, imaginary)) in signals[0]
                    .iter()
                    .zip(&signals[1])
                    .enumerate()
                    .skip(up as usize)
                {
                    let mut expected = Complex::zero(192);
                    for (image, response) in responses.iter().enumerate() {
                        let frequency = output_frequency(spec.ratio, &f, image as u64);
                        let rotation = Complex::phasor(&(frequency * n), 192);
                        let term = response.multiply(&rotation);
                        expected.re += term.re;
                        expected.im += term.im;
                    }
                    expected.re /= 4;
                    expected.im /= 4;
                    let actual = Complex {
                        re: Float::with_val(192, real.raw()) >> 63,
                        im: Float::with_val(192, imaginary.raw()) >> 63,
                    };
                    // All chosen PCM/coefficient products are exactly representable;
                    // differences here are analysis rounding, not signal-path noise.
                    close(&actual, &expected, 160);
                }
            }
        }
    }

    #[test]
    fn zero_order_hold_exposes_images_that_phase_magnitudes_cannot_see() {
        let spec = Spec {
            taps_per_phase: 1,
            fractional_bits: 1,
            input_delay_frames: 0,
            ..spec(2, 1)
        };
        let bank = Bank::new(spec, 5, Limits::default(), |_| Ok(vec![Integer::from(2)])).unwrap();
        let components = bank.at(&Rational::from((1, 2))).unwrap();
        // Both individual phase filters have magnitude 1 everywhere. The actual
        // 2x hold has wanted/image powers 1/2 each at input Nyquist.
        for component in components {
            assert!(
                (component.norm_squared() - Float::with_val(192, 0.5)).abs()
                    < Float::with_val(192, 1) >> 170
            );
        }
        let report = analyze_bank(
            spec,
            5,
            ErrorFloor::new(80).unwrap(),
            Limits::default(),
            |_| Ok(vec![Integer::from(2)]),
        )
        .unwrap();
        assert!(!report.meets_target());
        assert!(!report.image_peak_meets_target && !report.image_l2_meets_target);
        assert!(report.image_peak.is_some());
        assert_eq!(report.frequency_points, 9);
    }

    #[test]
    fn image_power_agrees_with_parseval_across_the_actual_phase_cycle() {
        let spec = spec(7, 4);
        let bank = Bank::new(spec, 3, Limits::default(), |p| Ok(raw(p))).unwrap();
        let f = Rational::from((7, 32));
        let components = bank.at(&f).unwrap();
        let mut harmonic_power = Float::with_val(192, 0);
        for c in &components {
            harmonic_power += c.norm_squared();
        }
        let mut phase_power = Float::with_val(192, 0);
        for p in 0..7 {
            let mut value = Complex::zero(192);
            for (tap, coefficient) in raw(p).iter().enumerate() {
                let factor = Complex::phasor(&(-f.clone() * tap), 192);
                value.re += factor.re * Float::with_val(192, coefficient) / 256;
                value.im += factor.im * Float::with_val(192, coefficient) / 256;
            }
            phase_power += value.norm_squared();
        }
        phase_power /= 7;
        assert!((harmonic_power - phase_power).abs() < Float::with_val(192, 1) >> 165);
    }

    #[test]
    fn resource_preflight_and_shape_errors_are_explicit_before_loading() {
        for limits in [
            Limits {
                max_work: 1,
                ..Limits::default()
            },
            Limits {
                max_coefficients: 1,
                ..Limits::default()
            },
            Limits {
                max_phases: 1,
                ..Limits::default()
            },
            Limits {
                max_precision_bits: 96,
                ..Limits::default()
            },
            Limits {
                max_storage_bytes: 1,
                ..Limits::default()
            },
        ] {
            assert!(matches!(
                Bank::new(spec(3, 2), 5, limits, |_| panic!(
                    "preflight loaded coefficients"
                )),
                Err(AnalysisError::ResourceLimit(_))
            ));
        }
        assert!(matches!(
            Bank::new(spec(3, 2), 1, Limits::default(), |_| panic!()),
            Err(AnalysisError::InvalidConfiguration)
        ));
        assert!(matches!(
            Bank::new(spec(3, 2), 5, Limits::default(), |_| Ok(vec![])),
            Err(AnalysisError::CoefficientCount)
        ));
        assert!(matches!(
            Bank::new(spec(3, 2), 5, Limits::default(), |_| Ok(vec![
                Integer::from(
                    1
                ) << 512;
                3
            ])),
            Err(AnalysisError::CoefficientNotExactlyRepresentable)
        ));
    }

    #[test]
    fn frequency_grid_covers_transition_and_stopband_including_exact_edges() {
        for ratio in [(3, 2), (2, 3)] {
            let (pass, stop) = band_edges(spec(ratio.0, ratio.1));
            let points = frequencies(&pass, &stop, 5);
            assert_eq!(points.first(), Some(&Rational::from(0)));
            assert_eq!(points.last(), Some(&Rational::from((1, 2))));
            assert!(points.contains(&pass) && points.contains(&stop));
            assert!(points.windows(2).all(|pair| pair[0] < pair[1]));
            assert_eq!(points.len(), if ratio.0 > ratio.1 { 9 } else { 13 });
        }
    }

    #[test]
    fn single_phase_and_zero_bank_do_not_invent_image_or_phase_measurements() {
        let report = analyze_bank(
            spec(1, 3),
            5,
            ErrorFloor::new(80).unwrap(),
            Limits::default(),
            |_| Ok(vec![Integer::new(); 3]),
        )
        .unwrap();
        assert!(report.image_peak.is_none() && report.image_l2_peak_db.is_none());
        assert!(!report.meets_target());
        assert!(report.main_phase_error_radians.contains("undefined"));
        assert!(report.main_passband_ripple_db.contains("undefined"));
    }

    #[test]
    fn arbitrary_fractional_coefficients_are_not_promoted_from_native_q() {
        let spec = Spec {
            fractional_bits: 4096,
            precision_bits: 4224,
            taps_per_phase: 1,
            input_delay_frames: 0,
            ..spec(1, 1)
        };
        let bank = Bank::new(spec, 2, Limits::default(), |_| {
            Ok(vec![Integer::from(1) << 100])
        })
        .unwrap();
        let value = bank.at(&Rational::from((1, 4))).unwrap();
        assert_eq!(value[0].re, Float::with_val(4224, 1) >> 3996);
        assert_eq!(value[0].im, 0);
    }

    #[test]
    fn extreme_preflight_arithmetic_fails_closed_without_overflow_or_allocation() {
        let huge = Spec {
            ratio: RateRatio::from_fraction(u64::MAX, 1).unwrap(),
            taps_per_phase: 1,
            ..spec(1, 1)
        };
        let limits = Limits {
            max_work: u64::MAX,
            max_phases: u64::MAX,
            max_coefficients: u64::MAX,
            max_storage_bytes: u64::MAX,
            ..Limits::default()
        };
        assert!(matches!(
            preflight(huge, u32::MAX, limits),
            Err(AnalysisError::ResourceLimit(_))
        ));
        assert!(matches!(
            preflight(spec(3, 2), u32::MAX, Limits::default()),
            Err(AnalysisError::ResourceLimit("work"))
        ));
    }

    #[test]
    fn exact_per_phase_dc_sums_cancel_every_non_main_dc_harmonic() {
        let s = spec(7, 4);
        let bank = Bank::new(s, 3, Limits::default(), |p| {
            Ok(vec![
                Integer::from(p + 1),
                Integer::from(254 - 2 * p),
                Integer::from(p + 1),
            ])
        })
        .unwrap();
        let dc = bank.at(&Rational::from(0)).unwrap();
        assert_eq!(dc[0].re, 1);
        assert_eq!(dc[0].im, 0);
        for image in &dc[1..] {
            assert!(image.magnitude() < Float::with_val(192, 1) >> 165);
        }
    }
}
