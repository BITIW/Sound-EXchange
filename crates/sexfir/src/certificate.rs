//! Continuous-frequency certificates for the actual quantized FIR bank.
//!
//! |H(f)|² is an exact rational polynomial in x = cos(2πf). Bernstein convex
//! hull bounds and exact midpoint subdivision cover intervals, not a grid.
//! MPFR is used only with directed rounding for band endpoints and dB targets.
use crate::{DesignedBigFilter, DesignedFilter, Fraction};
use rug::ops::{DivRounding, MulAssignRound, Pow, SubAssignRound};
use rug::{Float, Integer, Rational, float::Round};
use sexplan::ErrorFloor;
use sexrate::RateRatio;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub endpoint_bits: u32,
    pub max_taps: usize,
    pub max_depth: u32,
    pub max_work: u64,
    /// Upper bound on each exact integer, including subdivision growth.
    pub max_integer_bits: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            endpoint_bits: 32,
            max_taps: 1025,
            max_depth: 32,
            max_work: 1_000_000_000,
            max_integer_bits: 262_144,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitReason {
    Taps,
    Work,
    Depth,
    IntegerWidth,
    EndpointResolution,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Certified,
    Violated,
    Inconclusive(LimitReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Band {
    Passband,
    Stopband,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Witness {
    pub phase: usize,
    pub band: Band,
    /// Exact coordinate x = cos(2π f), proven inside the requested band.
    pub cosine_coordinate: Rational,
    pub power: Rational,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Certificate {
    pub outcome: Outcome,
    pub target: ErrorFloor,
    pub expected_phases: usize,
    pub certified_phases: usize,
    pub visited_cells: u64,
    pub charged_work: u64,
    /// Present only when the complete bank has been certified.
    pub passband_power_bounds: Option<(Rational, Rational)>,
    pub stopband_power_upper: Option<Rational>,
    pub witness: Option<Witness>,
}

impl Certificate {
    /// Human-readable upper enclosures, including directed decimal formatting.
    /// These are not available for partial or failed certificates.
    pub fn upper_enclosures(&self) -> Option<(String, String)> {
        if self.outcome != Outcome::Certified || self.certified_phases != self.expected_phases {
            return None;
        }
        let (lo, hi) = self.passband_power_bounds.as_ref()?;
        let stop = self.stopband_power_upper.as_ref()?;
        let precision = self.target.attenuation_db() / 6 + 128;
        let mut lower = Float::with_val_round(precision, lo, Round::Down).0;
        lower.sqrt_round(Round::Down);
        let mut upper = Float::with_val_round(precision, hi, Round::Up).0;
        upper.sqrt_round(Round::Up);
        let mut a = Float::with_val(precision, 1);
        a.sub_assign_round(&lower, Round::Up);
        upper.sub_assign_round(1, Round::Up);
        let deviation = a.max(&upper).max(&Float::with_val(precision, 0));
        let mut stop_db = Float::with_val_round(precision, stop, Round::Up).0;
        stop_db.log10_round(Round::Up);
        stop_db.mul_assign_round(10, Round::Up);
        Some((
            deviation.to_string_radix_round(10, Some(16), Round::Up),
            stop_db.to_string_radix_round(10, Some(16), Round::Up),
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CertificateError {
    InvalidLimits,
    InvalidBand,
    InvalidCoefficients,
}
impl fmt::Display for CertificateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "continuous FIR certificate: {self:?}")
    }
}
impl std::error::Error for CertificateError {}

struct Budget {
    limits: Limits,
    work: u64,
    cells: u64,
}
impl Budget {
    fn charge(&mut self, count: u64) -> Result<(), LimitReason> {
        let next = self.work.checked_add(count).ok_or(LimitReason::Work)?;
        if next > self.limits.max_work {
            return Err(LimitReason::Work);
        }
        self.work = next;
        Ok(())
    }
}

/// Does not certify the unquantized designer, effects, final PCM, or the entire
/// expanded anti-imaging band. Upsampling's per-phase stopband is one endpoint.
pub fn certify(
    filter: &DesignedFilter,
    target: ErrorFloor,
    limits: Limits,
) -> Result<Certificate, CertificateError> {
    certify_bank(
        filter.spec().ratio,
        filter.spec().rolloff,
        filter.bank().phase_count(),
        62,
        target,
        limits,
        |phase| {
            filter
                .bank()
                .phase(phase as u64)
                .map(|values| values.iter().map(|c| Integer::from(c.raw())).collect())
                .map_err(|_| CertificateError::InvalidCoefficients)
        },
    )
}

pub fn certify_big(
    filter: &DesignedBigFilter,
    target: ErrorFloor,
    limits: Limits,
) -> Result<Certificate, CertificateError> {
    let spec = filter.spec();
    certify_bank(
        spec.core.ratio,
        spec.core.rolloff,
        filter.bank().phase_count(),
        spec.coefficient_fractional_bits,
        target,
        limits,
        |phase| {
            filter
                .bank()
                .phase(phase as u64)
                .map(|values| values.iter().map(|c| c.raw().clone()).collect())
                .map_err(|_| CertificateError::InvalidCoefficients)
        },
    )
}

/// Low-level entry point for any real, uniformly quantized coefficient bank.
/// Each callback returns a single phase's signed integer coefficient codes.
pub fn certify_bank<F>(
    ratio: RateRatio,
    rolloff: Fraction,
    phase_count: usize,
    fractional_bits: u32,
    target: ErrorFloor,
    limits: Limits,
    mut coefficients: F,
) -> Result<Certificate, CertificateError>
where
    F: FnMut(usize) -> Result<Vec<Integer>, CertificateError>,
{
    if !(8..=128).contains(&limits.endpoint_bits)
        || limits.max_taps == 0
        || limits.max_depth > 64
        || limits.max_work == 0
        || limits.max_integer_bits == 0
        || phase_count == 0
    {
        return Err(CertificateError::InvalidLimits);
    }
    if u64::try_from(phase_count).ok() != Some(ratio.up()) {
        return Err(CertificateError::InvalidCoefficients);
    }
    if rolloff.numerator() > rolloff.denominator()
        || u128::from(rolloff.numerator()) * 2 < u128::from(rolloff.denominator())
    {
        return Err(CertificateError::InvalidBand);
    }
    let bands = band_coordinates(ratio, rolloff, limits.endpoint_bits);
    let mut budget = Budget {
        limits,
        work: 0,
        cells: 0,
    };
    let mut result = Certificate {
        outcome: Outcome::Certified,
        target,
        expected_phases: phase_count,
        certified_phases: 0,
        visited_cells: 0,
        charged_work: 0,
        passband_power_bounds: None,
        stopband_power_upper: None,
        witness: None,
    };
    // Includes target comparisons, whose exact dyadic denominators grow with A.
    let target_width_bound = 2 * (u64::from(target.attenuation_db()).div_ceil(6) + 256);
    if target_width_bound > limits.max_integer_bits {
        result.outcome = Outcome::Inconclusive(LimitReason::IntegerWidth);
        return Ok(result);
    }
    let thresholds = thresholds(target);
    let mut pass_bounds: Option<(Rational, Rational)> = None;
    let mut stop_upper: Option<Rational> = None;
    'phases: for phase in 0..phase_count {
        let raw = coefficients(phase)?;
        if raw.is_empty() {
            return Err(CertificateError::InvalidCoefficients);
        }
        let degree = raw.len() - 1;
        let raw_bits = raw.iter().map(Integer::significant_bits).max().unwrap_or(0);
        let width_bound = (degree as u64)
            .saturating_mul(
                u64::from(limits.endpoint_bits + limits.max_depth + 4)
                    + u64::from((degree.max(1)).ilog2() + 1),
            )
            .saturating_add(2 * u64::from(raw_bits.max(fractional_bits)))
            .saturating_add(64)
            .saturating_mul(2)
            .saturating_add(target_width_bound);
        let preparation = if raw.len() > limits.max_taps {
            Err(LimitReason::Taps)
        } else if width_bound > limits.max_integer_bits {
            Err(LimitReason::IntegerWidth)
        } else {
            power_polynomial(&raw, &mut budget)
        };
        let polynomial = match preparation {
            Ok(value) => value,
            Err(reason) => {
                result.outcome = Outcome::Inconclusive(reason);
                break;
            }
        };
        for (band, coordinates) in [(Band::Passband, &bands.0), (Band::Stopband, &bands.1)] {
            match certify_interval(
                &polynomial,
                fractional_bits,
                coordinates,
                band,
                &thresholds,
                &mut budget,
            ) {
                BandResult::Certified(lo, hi) => match band {
                    Band::Passband => merge_bounds(&mut pass_bounds, lo, hi),
                    Band::Stopband => {
                        if stop_upper.as_ref().is_none_or(|old| hi > *old) {
                            stop_upper = Some(hi);
                        }
                    }
                },
                BandResult::Violated(x, power) => {
                    result.outcome = Outcome::Violated;
                    result.witness = Some(Witness {
                        phase,
                        band,
                        cosine_coordinate: x,
                        power,
                    });
                    break 'phases;
                }
                BandResult::Inconclusive(reason) => {
                    result.outcome = Outcome::Inconclusive(reason);
                    break 'phases;
                }
            }
        }
        result.certified_phases += 1;
    }
    result.visited_cells = budget.cells;
    result.charged_work = budget.work;
    if result.outcome == Outcome::Certified && result.certified_phases == phase_count {
        result.passband_power_bounds = pass_bounds;
        result.stopband_power_upper = stop_upper;
    }
    Ok(result)
}

fn merge_bounds(bounds: &mut Option<(Rational, Rational)>, lo: Rational, hi: Rational) {
    if let Some((a, b)) = bounds {
        if lo < *a {
            *a = lo;
        }
        if hi > *b {
            *b = hi;
        }
    } else {
        *bounds = Some((lo, hi));
    }
}

/// Coefficients of sum r_i² + 2 sum_(k>0) sum_i r_i r_(i+k) T_k(x).
fn power_polynomial(raw: &[Integer], budget: &mut Budget) -> Result<Vec<Integer>, LimitReason> {
    let n = raw.len();
    budget.charge((n as u64).saturating_mul(n as u64).saturating_mul(3))?;
    let mut autocorrelation = vec![Integer::new(); n];
    for lag in 0..n {
        for (a, b) in raw[..n - lag].iter().zip(&raw[lag..]) {
            autocorrelation[lag] += Integer::from(a * b);
        }
        if lag != 0 {
            autocorrelation[lag] <<= 1;
        }
    }
    let mut polynomial = vec![Integer::new(); n];
    let mut previous = Vec::<Integer>::new();
    let mut current = vec![Integer::from(1)];
    for (order, weight) in autocorrelation.iter().enumerate() {
        for (power, coefficient) in current.iter().enumerate() {
            polynomial[power] += Integer::from(coefficient * weight);
        }
        if order + 1 == n {
            break;
        }
        let mut next = vec![Integer::new(); current.len() + 1];
        if order == 0 {
            next[1] = Integer::from(1);
        } else {
            for (i, coefficient) in current.iter().enumerate() {
                next[i + 1] = Integer::from(coefficient << 1);
            }
            for (i, coefficient) in previous.iter().enumerate() {
                next[i] -= coefficient;
            }
        }
        previous = current;
        current = next;
    }
    while polynomial.len() > 1 && polynomial.last().is_some_and(Integer::is_zero) {
        polynomial.pop();
    }
    Ok(polynomial)
}

struct Coordinates {
    outer_lo: Rational,
    outer_hi: Rational,
    inner_lo: Rational,
    inner_hi: Rational,
}

fn cosine_enclosure(twice_frequency: &Rational, bits: u32) -> (Rational, Rational) {
    // cos(pi*t) decreases on [0,1]. Round its argument in the opposite direction.
    let mut lower = Float::with_val_round(bits + 16, twice_frequency, Round::Up).0;
    let mut upper = Float::with_val_round(bits + 16, twice_frequency, Round::Down).0;
    lower.cos_pi_round(Round::Down);
    upper.cos_pi_round(Round::Up);
    lower <<= bits;
    upper <<= bits;
    let denominator = Integer::from(1) << bits;
    (
        Rational::from((
            lower.to_integer_round(Round::Down).unwrap().0,
            denominator.clone(),
        )),
        Rational::from((upper.to_integer_round(Round::Up).unwrap().0, denominator)),
    )
}

fn band_coordinates(ratio: RateRatio, rolloff: Fraction, bits: u32) -> (Coordinates, Coordinates) {
    let lower_nyquist_twice = if ratio.up() < ratio.down() {
        Rational::from((ratio.up(), ratio.down()))
    } else {
        Rational::from(1)
    };
    let mut pass_end = Rational::from((
        Integer::from(rolloff.numerator()) * 2 - rolloff.denominator(),
        Integer::from(rolloff.denominator()),
    ));
    pass_end *= &lower_nyquist_twice;
    let (p_lo, p_hi) = cosine_enclosure(&pass_end, bits);
    let (s_lo, s_hi) = cosine_enclosure(&lower_nyquist_twice, bits);
    (
        Coordinates {
            outer_lo: p_lo,
            inner_lo: p_hi,
            outer_hi: Rational::from(1),
            inner_hi: Rational::from(1),
        },
        Coordinates {
            outer_lo: Rational::from(-1),
            inner_lo: Rational::from(-1),
            outer_hi: s_hi,
            inner_hi: s_lo,
        },
    )
}

struct Thresholds {
    pass_lo: Rational,
    pass_hi: Rational,
    stop_hi: Rational,
    witness_pass_lo: Rational,
    witness_pass_hi: Rational,
    witness_stop_hi: Rational,
}

fn thresholds(target: ErrorFloor) -> Thresholds {
    let exponent = Rational::from((-i64::from(target.attenuation_db()), 20));
    let mut low = Float::with_val_round(128, &exponent, Round::Down).0;
    let mut high = Float::with_val_round(128, &exponent, Round::Up).0;
    low.exp10_round(Round::Down);
    high.exp10_round(Round::Up);
    let (low, high) = if target.attenuation_db().is_multiple_of(20) {
        let exact = Rational::from((
            Integer::from(1),
            Integer::from(10).pow(target.attenuation_db() / 20),
        ));
        (exact.clone(), exact)
    } else {
        (low.to_rational().unwrap(), high.to_rational().unwrap())
    };
    let square = |value: Rational| Rational::from(&value * &value);
    let (stop_hi, witness_stop_hi) = if target.attenuation_db().is_multiple_of(10) {
        let exact = Rational::from((
            Integer::from(1),
            Integer::from(10).pow(target.attenuation_db() / 10),
        ));
        (exact.clone(), exact)
    } else {
        (square(low.clone()), square(high.clone()))
    };
    Thresholds {
        pass_lo: square(Rational::from(1) - &low),
        pass_hi: square(Rational::from(1) + &low),
        stop_hi,
        witness_pass_lo: square(Rational::from(1) - &high),
        witness_pass_hi: square(Rational::from(1) + &high),
        witness_stop_hi,
    }
}

#[derive(Clone)]
struct Node {
    values: Vec<Integer>,
    denominator: Integer,
    lo: Rational,
    hi: Rational,
    depth: u32,
}

fn bernstein(
    polynomial: &[Integer],
    fractional_bits: u32,
    coordinates: &Coordinates,
    budget: &mut Budget,
) -> Result<Node, LimitReason> {
    let n = polynomial.len() - 1;
    budget.charge(
        (n as u64 + 1)
            .saturating_mul(n as u64 + 1)
            .saturating_mul(3),
    )?;
    // The coordinate denominators are powers of two. Work with one common S.
    let scale = coordinates
        .outer_lo
        .denom()
        .clone()
        .max(coordinates.outer_hi.denom().clone());
    let a = coordinates.outer_lo.numer() * Integer::from(&scale / coordinates.outer_lo.denom());
    let b = coordinates.outer_hi.numer() * Integer::from(&scale / coordinates.outer_hi.denom());
    let mut values = vec![polynomial[n].clone()];
    let mut denominator = Integer::from(1);
    // Exact Horner in Bernstein form: multiply a degree-d polynomial by
    // a(1-t)+bt, elevate to d+1, then add the next power-basis coefficient.
    for (degree, constant) in polynomial[..n].iter().rev().enumerate() {
        let new_degree = degree + 1;
        denominator *= &scale;
        denominator *= new_degree;
        let offset = Integer::from(constant * &denominator);
        let mut next = vec![offset; new_degree + 1];
        for i in 0..=new_degree {
            if i < values.len() {
                let mut term = Integer::from(&values[i] * &a);
                term *= new_degree - i;
                next[i] += term;
            }
            if i > 0 {
                let mut term = Integer::from(&values[i - 1] * &b);
                term *= i;
                next[i] += term;
            }
        }
        values = next;
    }
    denominator <<= fractional_bits;
    denominator <<= fractional_bits;
    Ok(Node {
        values,
        denominator,
        lo: coordinates.outer_lo.clone(),
        hi: coordinates.outer_hi.clone(),
        depth: 0,
    })
}

fn subdivide(node: Node, budget: &mut Budget) -> Result<(Node, Node), LimitReason> {
    let n = node.values.len() - 1;
    budget.charge((n as u64 + 1).saturating_mul(n as u64 + 1))?;
    let mut row = node.values;
    let mut left = Vec::with_capacity(n + 1);
    let mut right = Vec::with_capacity(n + 1);
    for level in 0..=n {
        left.push(Integer::from(&row[0] << (n - level)));
        right.push(Integer::from(&row[n - level] << (n - level)));
        for i in 0..n - level {
            let following = row[i + 1].clone();
            row[i] += following;
        }
    }
    right.reverse();
    let denominator = node.denominator << n;
    let mut middle = Rational::from(&node.lo + &node.hi);
    middle /= 2;
    Ok((
        Node {
            values: left,
            denominator: denominator.clone(),
            lo: node.lo,
            hi: middle.clone(),
            depth: node.depth + 1,
        },
        Node {
            values: right,
            denominator,
            lo: middle,
            hi: node.hi,
            depth: node.depth + 1,
        },
    ))
}

enum BandResult {
    Certified(Rational, Rational),
    Violated(Rational, Rational),
    Inconclusive(LimitReason),
}

fn scaled_floor(value: &Rational, denominator: &Integer) -> Integer {
    let numerator = Integer::from(value.numer() * denominator);
    numerator.div_floor(value.denom())
}
fn scaled_ceil(value: &Rational, denominator: &Integer) -> Integer {
    let numerator = Integer::from(value.numer() * denominator);
    numerator.div_ceil(value.denom())
}

fn certify_interval(
    polynomial: &[Integer],
    fractional_bits: u32,
    coordinates: &Coordinates,
    band: Band,
    thresholds: &Thresholds,
    budget: &mut Budget,
) -> BandResult {
    let initial = match bernstein(polynomial, fractional_bits, coordinates, budget) {
        Ok(v) => v,
        Err(e) => return BandResult::Inconclusive(e),
    };
    let mut stack = vec![initial];
    let mut bounds = None;
    while let Some(node) = stack.pop() {
        if let Err(reason) = budget.charge(node.values.len() as u64) {
            return BandResult::Inconclusive(reason);
        }
        budget.cells += 1;
        let allowed_lo = scaled_ceil(&thresholds.pass_lo, &node.denominator);
        let allowed_hi = scaled_floor(
            if band == Band::Passband {
                &thresholds.pass_hi
            } else {
                &thresholds.stop_hi
            },
            &node.denominator,
        );
        let min = node.values.iter().min().unwrap();
        let max = node.values.iter().max().unwrap();
        if *max <= allowed_hi && (band == Band::Stopband || *min >= allowed_lo) {
            merge_bounds(
                &mut bounds,
                Rational::from((min.clone().max(Integer::from(0)), node.denominator.clone())),
                Rational::from((max.clone(), node.denominator)),
            );
            continue;
        }
        for (coordinate, value) in [
            (&node.lo, node.values.first().unwrap()),
            (&node.hi, node.values.last().unwrap()),
        ] {
            if *coordinate < coordinates.inner_lo || *coordinate > coordinates.inner_hi {
                continue;
            }
            let power = Rational::from((value.clone(), node.denominator.clone()));
            let violates = if band == Band::Passband {
                power < thresholds.witness_pass_lo || power > thresholds.witness_pass_hi
            } else {
                power > thresholds.witness_stop_hi
            };
            if violates {
                return BandResult::Violated(coordinate.clone(), power);
            }
        }
        if node.depth == budget.limits.max_depth {
            return BandResult::Inconclusive(LimitReason::Depth);
        }
        if node.lo == node.hi {
            return BandResult::Inconclusive(LimitReason::EndpointResolution);
        }
        match subdivide(node, budget) {
            Ok((left, right)) => {
                stack.push(right);
                stack.push(left);
            }
            Err(reason) => return BandResult::Inconclusive(reason),
        }
    }
    let (lo, hi) = bounds.expect("nonempty interval was covered");
    BandResult::Certified(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn budget() -> Budget {
        Budget {
            limits: Limits::default(),
            work: 0,
            cells: 0,
        }
    }
    fn evaluate(polynomial: &[Integer], x: &Rational) -> Rational {
        let mut value = Rational::from(0);
        for coefficient in polynomial.iter().rev() {
            value *= x;
            value += coefficient;
        }
        value
    }
    #[test]
    fn exact_power_matches_direct_complex_sum_at_quarter_cycle() {
        for raw in [vec![1, 2, 3], vec![-17, 3, 11, -4, 23], vec![7]] {
            let raw: Vec<_> = raw.into_iter().map(Integer::from).collect();
            let p = power_polynomial(&raw, &mut budget()).unwrap();
            let mut re = Integer::new();
            let mut im = Integer::new();
            for (i, r) in raw.iter().enumerate() {
                match i % 4 {
                    0 => re += r,
                    1 => im -= r,
                    2 => re -= r,
                    _ => im += r,
                }
            }
            let expected = Integer::from(&re * &re) + Integer::from(&im * &im);
            assert_eq!(evaluate(&p, &Rational::from(0)), expected);
            let sum: Integer = raw.iter().sum();
            assert_eq!(evaluate(&p, &Rational::from(1)), Integer::from(&sum * &sum));
        }
    }

    #[test]
    fn exact_power_matches_independent_rational_complex_oracle() {
        // No autocorrelation, Chebyshev recurrence, or polynomial conversion
        // in this oracle: accumulate r_i * z^i with exact complex arithmetic.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for length in 1..=24 {
            let raw: Vec<_> = (0..length)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    Integer::from((state % 2001) as i32 - 1000)
                })
                .collect();
            let p = power_polynomial(&raw, &mut budget()).unwrap();
            for (x, y) in [(1, 0), (-1, 0), (0, 1), (3, 4), (-3, 4)] {
                let denominator = if y == 4 { 5 } else { 1 };
                let x = Rational::from((x, denominator));
                let y = Rational::from((y, denominator));
                let (mut re, mut im) = (Rational::from(0), Rational::from(0));
                let (mut zr, mut zi) = (Rational::from(1), Rational::from(0));
                for r in &raw {
                    re += Rational::from(&zr * r);
                    im += Rational::from(&zi * r);
                    let next_r = Rational::from(&zr * &x) - Rational::from(&zi * &y);
                    zi = Rational::from(&zr * &y) + Rational::from(&zi * &x);
                    zr = next_r;
                }
                assert_eq!(
                    evaluate(&p, &x),
                    Rational::from(&re * &re) + Rational::from(&im * &im),
                    "length={length}, x={x}"
                );
            }
        }
    }
    #[test]
    fn bernstein_subdivision_endpoints_match_exact_horner() {
        let p = vec![
            Integer::from(7),
            Integer::from(-13),
            Integer::from(19),
            Integer::from(-3),
        ];
        let c = Coordinates {
            outer_lo: Rational::from((-3, 4)),
            outer_hi: Rational::from((7, 8)),
            inner_lo: Rational::from((-3, 4)),
            inner_hi: Rational::from((7, 8)),
        };
        let mut b = budget();
        let root = bernstein(&p, 0, &c, &mut b).unwrap();
        let mut nodes = vec![root];
        for _ in 0..5 {
            let mut next = Vec::new();
            for node in nodes {
                assert_eq!(
                    Rational::from((node.values[0].clone(), node.denominator.clone())),
                    evaluate(&p, &node.lo)
                );
                assert_eq!(
                    Rational::from((
                        node.values.last().unwrap().clone(),
                        node.denominator.clone()
                    )),
                    evaluate(&p, &node.hi)
                );
                let min = Rational::from((
                    node.values.iter().min().unwrap().clone(),
                    node.denominator.clone(),
                ));
                let max = Rational::from((
                    node.values.iter().max().unwrap().clone(),
                    node.denominator.clone(),
                ));
                for step in 0..=13 {
                    let mut x = Rational::from(&node.hi - &node.lo);
                    x *= Rational::from((step, 13));
                    x += &node.lo;
                    let value = evaluate(&p, &x);
                    assert!(min <= value && value <= max);
                }
                let (a, b_node) = subdivide(node, &mut b).unwrap();
                next.push(a);
                next.push(b_node);
            }
            nodes = next;
        }
    }
    #[test]
    fn endpoints_and_targets_enclose_exact_known_values() {
        assert_eq!(
            cosine_enclosure(&Rational::from(0), 32),
            (Rational::from(1), Rational::from(1))
        );
        assert_eq!(
            cosine_enclosure(&Rational::from(1), 32),
            (Rational::from(-1), Rational::from(-1))
        );
        let (a, b) = cosine_enclosure(&Rational::from((1, 3)), 32);
        assert!(a <= Rational::from((1, 2)) && b >= Rational::from((1, 2)));
        let t = thresholds(ErrorFloor::new(80).unwrap());
        let exact = Rational::from((1, 100_000_000));
        assert_eq!(t.stop_hi, exact);
        assert_eq!(t.witness_stop_hi, exact);
        assert_eq!(t.pass_lo, Rational::from((99_980_001, 100_000_000)));
        for attenuation in [3, 63, 109, 599, 39_999] {
            let t = thresholds(ErrorFloor::new(attenuation).unwrap());
            let mut epsilon = Float::with_val(1024, -i64::from(attenuation));
            epsilon /= 20;
            epsilon.exp10_mut();
            let mut stop = epsilon.clone();
            stop.square_mut();
            assert!(Float::with_val(1024, &t.stop_hi) <= stop);
            assert!(stop <= Float::with_val(1024, &t.witness_stop_hi));
            let lo = Float::with_val(1024, 1) - &epsilon;
            let hi = Float::with_val(1024, 1) + &epsilon;
            assert!(Float::with_val(1024, &t.witness_pass_lo) <= lo.clone().square());
            assert!(lo.square() <= Float::with_val(1024, &t.pass_lo));
            assert!(Float::with_val(1024, &t.pass_hi) <= hi.clone().square());
            assert!(hi.square() <= Float::with_val(1024, &t.witness_pass_hi));
        }
    }

    fn half_average(fractional_bits: u32, limits: Limits) -> Certificate {
        certify_bank(
            RateRatio::from_rates(2, 1).unwrap(),
            Fraction::new(3, 4).unwrap(),
            1,
            fractional_bits,
            ErrorFloor::new(3).unwrap(),
            limits,
            |_| Ok(vec![Integer::from(1) << (fractional_bits - 1); 2]),
        )
        .unwrap()
    }

    #[test]
    fn whole_bank_certification_is_exact_at_63_and_4096_fractional_bits() {
        let a = half_average(63, Limits::default());
        let b = half_average(4096, Limits::default());
        assert_eq!(a.outcome, Outcome::Certified);
        assert_eq!(a, b);
        assert_eq!(a.stopband_power_upper, Some(Rational::from((1, 2))));
        let (pass_decimal, stop_decimal) = a.upper_enclosures().unwrap();
        let pass_decimal = Float::with_val(1024, Float::parse(pass_decimal).unwrap());
        let stop_decimal = Float::with_val(1024, Float::parse(stop_decimal).unwrap());
        let (lo, hi) = a.passband_power_bounds.as_ref().unwrap();
        let below = Float::with_val(1024, 1) - Float::with_val(1024, lo).sqrt();
        let above = Float::with_val(1024, hi).sqrt() - 1;
        assert!(pass_decimal >= below && pass_decimal >= above);
        let actual_stop_db = Float::with_val(1024, Rational::from((1, 2))).log10() * 10;
        assert!(stop_decimal >= actual_stop_db);
        // Formatting must not replace a tight bound by a vacuous upper bound.
        assert!(stop_decimal - actual_stop_db < Float::with_val(1024, 1e-14));
    }

    #[test]
    fn resource_limits_never_return_a_partial_quality_pass() {
        for (limits, reason) in [
            (
                Limits {
                    max_work: 1,
                    ..Limits::default()
                },
                LimitReason::Work,
            ),
            (
                Limits {
                    max_taps: 1,
                    ..Limits::default()
                },
                LimitReason::Taps,
            ),
            (
                Limits {
                    max_integer_bits: 1024,
                    ..Limits::default()
                },
                LimitReason::IntegerWidth,
            ),
        ] {
            let proof = half_average(4096, limits);
            assert_eq!(proof.outcome, Outcome::Inconclusive(reason));
            assert_eq!(proof.certified_phases, 0);
            assert!(proof.passband_power_bounds.is_none());
            assert!(proof.stopband_power_upper.is_none());
            assert!(proof.witness.is_none());
            assert!(proof.upper_enclosures().is_none());
        }
    }

    #[test]
    fn invalid_configuration_is_rejected_before_loading_coefficients() {
        for (phases, rolloff, limits, expected) in [
            (
                2,
                Fraction::new(3, 4).unwrap(),
                Limits::default(),
                CertificateError::InvalidCoefficients,
            ),
            (
                1,
                Fraction::new(1, 3).unwrap(),
                Limits::default(),
                CertificateError::InvalidBand,
            ),
            (
                1,
                Fraction::new(3, 4).unwrap(),
                Limits {
                    max_work: 0,
                    ..Limits::default()
                },
                CertificateError::InvalidLimits,
            ),
        ] {
            let proof = certify_bank(
                RateRatio::from_rates(2, 1).unwrap(),
                rolloff,
                phases,
                63,
                ErrorFloor::new(3).unwrap(),
                limits,
                |_| panic!("invalid configuration loaded coefficients"),
            );
            assert_eq!(proof.unwrap_err(), expected);
        }
    }

    #[test]
    fn failed_later_phase_does_not_publish_partial_bounds() {
        let proof = certify_bank(
            RateRatio::from_rates(1, 2).unwrap(),
            Fraction::new(3, 4).unwrap(),
            2,
            1,
            ErrorFloor::new(3).unwrap(),
            Limits::default(),
            |phase| {
                Ok(if phase == 0 {
                    vec![Integer::from(1); 2]
                } else {
                    vec![Integer::from(2)]
                })
            },
        )
        .unwrap();
        assert_eq!(proof.outcome, Outcome::Violated);
        assert_eq!(proof.certified_phases, 1);
        assert_eq!(proof.witness.as_ref().unwrap().phase, 1);
        assert!(proof.passband_power_bounds.is_none());
        assert!(proof.stopband_power_upper.is_none());
        assert!(proof.upper_enclosures().is_none());
    }

    #[test]
    fn target_uncertainty_at_a_single_point_is_inconclusive() {
        let c = Coordinates {
            outer_lo: Rational::from(0),
            outer_hi: Rational::from(0),
            inner_lo: Rational::from(0),
            inner_hi: Rational::from(0),
        };
        let t = Thresholds {
            pass_lo: Rational::from(0),
            pass_hi: Rational::from(1),
            stop_hi: Rational::from(0),
            witness_pass_lo: Rational::from(0),
            witness_pass_hi: Rational::from(2),
            witness_stop_hi: Rational::from(2),
        };
        assert!(matches!(
            certify_interval(
                &[Integer::from(1)],
                0,
                &c,
                Band::Stopband,
                &t,
                &mut budget()
            ),
            BandResult::Inconclusive(LimitReason::EndpointResolution)
        ));
    }
    #[test]
    fn witness_is_not_in_an_expanded_endpoint_sliver() {
        let coordinates = Coordinates {
            outer_lo: Rational::from(0),
            outer_hi: Rational::from(1),
            inner_lo: Rational::from((1, 2)),
            inner_hi: Rational::from(1),
        };
        let polynomial = vec![Integer::from(4), Integer::from(-4)];
        let mut b = budget();
        b.limits.max_depth = 0;
        let t = Thresholds {
            pass_lo: Rational::from(0),
            pass_hi: Rational::from(2),
            stop_hi: Rational::from(2),
            witness_pass_lo: Rational::from(0),
            witness_pass_hi: Rational::from(2),
            witness_stop_hi: Rational::from(2),
        };
        assert!(matches!(
            certify_interval(&polynomial, 0, &coordinates, Band::Stopband, &t, &mut b),
            BandResult::Inconclusive(_)
        ));
    }

    #[test]
    fn hidden_interior_peak_is_a_proved_violation_not_a_grid_pass() {
        // |1 - exp(-2iω)|² = 4 - 4 x² vanishes at both x endpoints.
        let p = vec![Integer::from(4), Integer::from(0), Integer::from(-4)];
        let c = Coordinates {
            outer_lo: Rational::from(-1),
            inner_lo: Rational::from(-1),
            outer_hi: Rational::from(1),
            inner_hi: Rational::from(1),
        };
        let t = Thresholds {
            pass_lo: Rational::from(0),
            pass_hi: Rational::from(1),
            stop_hi: Rational::from(1),
            witness_pass_lo: Rational::from(0),
            witness_pass_hi: Rational::from(1),
            witness_stop_hi: Rational::from(1),
        };
        let mut depth_limited = budget();
        depth_limited.limits.max_depth = 0;
        assert!(matches!(
            certify_interval(&p, 0, &c, Band::Stopband, &t, &mut depth_limited),
            BandResult::Inconclusive(LimitReason::Depth)
        ));
        match certify_interval(&p, 0, &c, Band::Stopband, &t, &mut budget()) {
            BandResult::Violated(x, power) => {
                assert_eq!(x, 0);
                assert_eq!(power, 4);
            }
            _ => panic!("interior peak was not exposed"),
        }
    }

    #[test]
    fn corrected_sane_bank_has_a_continuous_certificate() {
        let filter = crate::design_kaiser(&crate::KaiserSpec::native_candidate(
            RateRatio::from_rates(48000, 16000).unwrap(),
        ))
        .unwrap();
        let result = certify(&filter, ErrorFloor::new(110).unwrap(), Limits::default()).unwrap();
        eprintln!(
            "outcome {:?}, phases {}/{}, cells {}, work {}",
            result.outcome,
            result.certified_phases,
            result.expected_phases,
            result.visited_cells,
            result.charged_work
        );
        assert_eq!(result.outcome, Outcome::Certified);
        assert_eq!(result.certified_phases, 1);
        assert!(result.upper_enclosures().is_some());
    }

    #[test]
    fn high_correction_is_certified_without_weakening_its_target() {
        let target = ErrorFloor::new(160).unwrap();
        let plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio: RateRatio::from_rates(48000, 24000).unwrap(),
            preset: sexplan::QualityPreset::High,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        let spec = crate::BigKaiserSpec::from_precision_plan(&plan).unwrap();
        let mut old_spec = spec.clone();
        old_spec.core.taps_per_phase = 1025;
        old_spec.core.beta = Fraction::new(16, 1).unwrap();
        let old = crate::design_kaiser_big(&old_spec).unwrap();
        let failure = certify_big(&old, target, Limits::default()).unwrap();
        assert_eq!(failure.outcome, Outcome::Violated);
        let witness = failure.witness.unwrap();
        assert_eq!(witness.band, Band::Passband);
        assert_eq!(
            witness.cosine_coordinate,
            Rational::from((12_762_330_023_u64, 137_438_953_472_u64))
        );
        let corrected = crate::design_kaiser_big(&spec).unwrap();
        let limits = Limits {
            max_taps: 2049,
            ..Limits::default()
        };
        let proof = certify_big(&corrected, target, limits).unwrap();
        assert_eq!(proof.outcome, Outcome::Certified);
        assert_eq!(proof.certified_phases, 1);
        assert!(proof.upper_enclosures().is_some());
    }

    #[test]
    fn legacy_sane_bank_has_an_exact_violation_witness() {
        let mut spec =
            crate::KaiserSpec::native_candidate(RateRatio::from_rates(48000, 16000).unwrap());
        spec.taps_per_phase = 769;
        spec.beta = Fraction::new(10, 1).unwrap();
        let filter = crate::design_kaiser(&spec).unwrap();
        let result = certify(&filter, ErrorFloor::new(110).unwrap(), Limits::default()).unwrap();
        assert_eq!(result.outcome, Outcome::Violated);
        assert!(result.upper_enclosures().is_none());
        let witness = result.witness.unwrap();
        let raw: Vec<_> = filter
            .bank()
            .phase(0)
            .unwrap()
            .iter()
            .map(|c| Integer::from(c.raw()))
            .collect();
        // Independent scalar Chebyshev oracle, without power-basis conversion
        // or Bernstein evaluation. Evaluates the exact frequency in the witness.
        let x = &witness.cosine_coordinate;
        let mut sum = Rational::from(0);
        let (mut previous, mut current) = (Rational::from(1), x.clone());
        for lag in 0..raw.len() {
            let weight: Integer = raw[..raw.len() - lag]
                .iter()
                .zip(&raw[lag..])
                .map(|(a, b)| Integer::from(a * b))
                .sum();
            if lag == 0 {
                sum += weight;
            } else {
                sum += Rational::from(&current * weight) * 2;
                let next = Rational::from(&current * x) * 2 - &previous;
                previous = current;
                current = next;
            }
        }
        sum /= Integer::from(1) << 124;
        assert_eq!(sum, witness.power);
        let t = thresholds(ErrorFloor::new(110).unwrap());
        match witness.band {
            Band::Passband => assert!(sum < t.witness_pass_lo || sum > t.witness_pass_hi),
            Band::Stopband => assert!(sum > t.witness_stop_hi),
        }
    }
}
