//! Exact dot-product dispatch for the native FIR backend.
//!
//! SIMD is allowed to change how raw products are formed, never their width,
//! order, overflow checks, or the final rounding boundary.

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use super::signed_from_magnitude;
use super::{ArithmeticError, Operation, Q1_63, Q2_62};
use core::fmt;

/// Concrete implementation used to form exact Q1.63 × Q2.62 products.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExactDotBackend {
    Scalar,
    Avx2,
    Avx512,
}

impl ExactDotBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Avx2 => "avx2-exact",
            Self::Avx512 => "avx512-exact",
        }
    }
}

impl fmt::Display for ExactDotBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Contract failure for an exact native dot product.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExactDotError {
    LengthMismatch { samples: usize, coefficients: usize },
    BackendUnavailable(ExactDotBackend),
    Arithmetic(ArithmeticError),
}

impl fmt::Display for ExactDotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::LengthMismatch {
                samples,
                coefficients,
            } => write!(
                formatter,
                "dot product has {samples} samples and {coefficients} coefficients"
            ),
            Self::BackendUnavailable(backend) => {
                write!(
                    formatter,
                    "exact dot-product backend {backend} is unavailable"
                )
            }
            Self::Arithmetic(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ExactDotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Arithmetic(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ArithmeticError> for ExactDotError {
    fn from(value: ArithmeticError) -> Self {
        Self::Arithmetic(value)
    }
}

/// Return whether this process can execute a concrete backend.
pub fn exact_dot_backend_available(backend: ExactDotBackend) -> bool {
    match backend {
        ExactDotBackend::Scalar => true,
        ExactDotBackend::Avx2 => avx2_available(),
        ExactDotBackend::Avx512 => avx512_available(),
    }
}

/// Select the backend that has passed both bit-exactness and speed gates.
///
/// AVX2 exact-product qualification is available explicitly, but 64×64→128
/// decomposition is not yet faster than the scalar instruction sequence on
/// measured targets. It is deliberately excluded from automatic dispatch
/// until a benchmark demonstrates a win.
pub const fn automatic_exact_dot_backend() -> ExactDotBackend {
    ExactDotBackend::Scalar
}

/// Form and accumulate every Q1.63 × Q2.62 product without rounding.
///
/// Products are added in slice order with a checked `i128` addition. Runtime
/// dispatch changes only the multiplication implementation.
pub fn exact_dot_q1_63_q2_62(
    samples: &[Q1_63],
    coefficients: &[Q2_62],
) -> Result<i128, ExactDotError> {
    exact_dot_q1_63_q2_62_with_backend(samples, coefficients, automatic_exact_dot_backend())
}

/// Execute a caller-selected exact backend, primarily for differential tests
/// and controlled benchmarking.
pub fn exact_dot_q1_63_q2_62_with_backend(
    samples: &[Q1_63],
    coefficients: &[Q2_62],
    backend: ExactDotBackend,
) -> Result<i128, ExactDotError> {
    if samples.len() != coefficients.len() {
        return Err(ExactDotError::LengthMismatch {
            samples: samples.len(),
            coefficients: coefficients.len(),
        });
    }
    if !exact_dot_backend_available(backend) {
        return Err(ExactDotError::BackendUnavailable(backend));
    }
    match backend {
        ExactDotBackend::Scalar => scalar_dot(samples, coefficients),
        ExactDotBackend::Avx2 => avx2_dot(samples, coefficients),
        ExactDotBackend::Avx512 => avx512_dot(samples, coefficients),
    }
}

fn checked_accumulate(accumulator: &mut i128, product: i128) -> Result<(), ExactDotError> {
    *accumulator = accumulator
        .checked_add(product)
        .ok_or(ArithmeticError::Overflow(Operation::Accumulate))?;
    Ok(())
}

fn scalar_dot(samples: &[Q1_63], coefficients: &[Q2_62]) -> Result<i128, ExactDotError> {
    let mut accumulator = 0_i128;
    for (&sample, &coefficient) in samples.iter().zip(coefficients) {
        checked_accumulate(&mut accumulator, coefficient.widening_mul_sample(sample))?;
    }
    Ok(accumulator)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn avx2_available() -> bool {
    std::arch::is_x86_feature_detected!("avx2")
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
const fn avx2_available() -> bool {
    false
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn avx512_available() -> bool {
    std::arch::is_x86_feature_detected!("avx512f")
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
const fn avx512_available() -> bool {
    false
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn avx2_dot(samples: &[Q1_63], coefficients: &[Q2_62]) -> Result<i128, ExactDotError> {
    // The availability check in the public dispatcher is the safety gate for
    // the target-feature implementation.
    #[allow(unsafe_code)]
    unsafe {
        avx2_dot_inner(samples, coefficients)
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn avx2_dot(_samples: &[Q1_63], _coefficients: &[Q2_62]) -> Result<i128, ExactDotError> {
    Err(ExactDotError::BackendUnavailable(ExactDotBackend::Avx2))
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn avx512_dot(samples: &[Q1_63], coefficients: &[Q2_62]) -> Result<i128, ExactDotError> {
    // The availability check in the public dispatcher is the safety gate for
    // the target-feature implementation.
    #[allow(unsafe_code)]
    unsafe {
        avx512_dot_inner(samples, coefficients)
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn avx512_dot(_samples: &[Q1_63], _coefficients: &[Q2_62]) -> Result<i128, ExactDotError> {
    Err(ExactDotError::BackendUnavailable(ExactDotBackend::Avx512))
}

#[cfg(target_arch = "x86")]
use std::arch::x86::{
    __m256i, __m512i, _mm256_add_epi64, _mm256_and_si256, _mm256_cmpgt_epi64, _mm256_loadu_si256,
    _mm256_mul_epu32, _mm256_or_si256, _mm256_set1_epi64x, _mm256_setzero_si256, _mm256_slli_epi64,
    _mm256_srli_epi64, _mm256_storeu_si256, _mm256_sub_epi64, _mm256_xor_si256, _mm512_abs_epi64,
    _mm512_add_epi64, _mm512_and_si512, _mm512_loadu_si512, _mm512_mul_epu32, _mm512_or_si512,
    _mm512_set1_epi64, _mm512_slli_epi64, _mm512_srli_epi64, _mm512_storeu_si512,
};
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    __m256i, __m512i, _mm256_add_epi64, _mm256_and_si256, _mm256_cmpgt_epi64, _mm256_loadu_si256,
    _mm256_mul_epu32, _mm256_or_si256, _mm256_set1_epi64x, _mm256_setzero_si256, _mm256_slli_epi64,
    _mm256_srli_epi64, _mm256_storeu_si256, _mm256_sub_epi64, _mm256_xor_si256, _mm512_abs_epi64,
    _mm512_add_epi64, _mm512_and_si512, _mm512_loadu_si512, _mm512_mul_epu32, _mm512_or_si512,
    _mm512_set1_epi64, _mm512_slli_epi64, _mm512_srli_epi64, _mm512_storeu_si512,
};

/// Multiply four signed 64-bit lanes as exact 128-bit scalar products.
///
/// AVX2 has no 64×64→128 instruction. Each absolute magnitude is decomposed
/// into unsigned 32-bit limbs, four lane-wise partial products are formed with
/// `_mm256_mul_epu32`, and each 128-bit magnitude is reconstructed exactly.
/// Signs and checked accumulation are then applied in original tap order.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
#[allow(unsafe_code)]
unsafe fn avx2_dot_inner(samples: &[Q1_63], coefficients: &[Q2_62]) -> Result<i128, ExactDotError> {
    const LANES: usize = 4;

    let mut accumulator = 0_i128;
    let mut position = 0;
    while position + LANES <= samples.len() {
        let mut low = [0_u64; LANES];
        let mut high = [0_u64; LANES];

        // Q1_63 and Q2_62 are repr(transparent) over i64. Unaligned vector
        // loads therefore read exactly four consecutive raw values.
        #[allow(unsafe_code)]
        unsafe {
            let sample_vector =
                _mm256_loadu_si256(samples.as_ptr().add(position).cast::<__m256i>());
            let coefficient_vector =
                _mm256_loadu_si256(coefficients.as_ptr().add(position).cast::<__m256i>());
            let zero = _mm256_setzero_si256();
            let sample_sign = _mm256_cmpgt_epi64(zero, sample_vector);
            let coefficient_sign = _mm256_cmpgt_epi64(zero, coefficient_vector);
            let sample_magnitude =
                _mm256_sub_epi64(_mm256_xor_si256(sample_vector, sample_sign), sample_sign);
            let coefficient_magnitude = _mm256_sub_epi64(
                _mm256_xor_si256(coefficient_vector, coefficient_sign),
                coefficient_sign,
            );
            let sample_high = _mm256_srli_epi64::<32>(sample_magnitude);
            let coefficient_high = _mm256_srli_epi64::<32>(coefficient_magnitude);
            let p00 = _mm256_mul_epu32(sample_magnitude, coefficient_magnitude);
            let p01 = _mm256_mul_epu32(sample_magnitude, coefficient_high);
            let p10 = _mm256_mul_epu32(sample_high, coefficient_magnitude);
            let p11 = _mm256_mul_epu32(sample_high, coefficient_high);
            let low_mask = _mm256_set1_epi64x(i64::from(u32::MAX));
            let middle = _mm256_add_epi64(
                _mm256_add_epi64(
                    _mm256_srli_epi64::<32>(p00),
                    _mm256_and_si256(p01, low_mask),
                ),
                _mm256_and_si256(p10, low_mask),
            );
            let low_vector = _mm256_or_si256(
                _mm256_and_si256(p00, low_mask),
                _mm256_slli_epi64::<32>(middle),
            );
            let high_vector = _mm256_add_epi64(
                _mm256_add_epi64(
                    _mm256_add_epi64(p11, _mm256_srli_epi64::<32>(p01)),
                    _mm256_srli_epi64::<32>(p10),
                ),
                _mm256_srli_epi64::<32>(middle),
            );
            _mm256_storeu_si256(low.as_mut_ptr().cast::<__m256i>(), low_vector);
            _mm256_storeu_si256(high.as_mut_ptr().cast::<__m256i>(), high_vector);
        }

        for lane in 0..LANES {
            let magnitude = (u128::from(high[lane]) << 64) | u128::from(low[lane]);
            let negative = samples[position + lane].raw().is_negative()
                ^ coefficients[position + lane].raw().is_negative();
            let product = signed_from_magnitude(magnitude, negative, Operation::Accumulate)?;
            checked_accumulate(&mut accumulator, product)?;
        }
        position += LANES;
    }

    for (&sample, &coefficient) in samples[position..].iter().zip(&coefficients[position..]) {
        checked_accumulate(&mut accumulator, coefficient.widening_mul_sample(sample))?;
    }
    Ok(accumulator)
}

/// Eight-lane form of the same exact 32-bit-limb decomposition used by AVX2.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
#[allow(unsafe_code)]
unsafe fn avx512_dot_inner(
    samples: &[Q1_63],
    coefficients: &[Q2_62],
) -> Result<i128, ExactDotError> {
    const LANES: usize = 8;

    let mut accumulator = 0_i128;
    let mut position = 0;
    while position + LANES <= samples.len() {
        let mut low = [0_u64; LANES];
        let mut high = [0_u64; LANES];

        #[allow(unsafe_code)]
        unsafe {
            let sample_vector =
                _mm512_loadu_si512(samples.as_ptr().add(position).cast::<__m512i>());
            let coefficient_vector =
                _mm512_loadu_si512(coefficients.as_ptr().add(position).cast::<__m512i>());
            let sample_magnitude = _mm512_abs_epi64(sample_vector);
            let coefficient_magnitude = _mm512_abs_epi64(coefficient_vector);
            let sample_high = _mm512_srli_epi64::<32>(sample_magnitude);
            let coefficient_high = _mm512_srli_epi64::<32>(coefficient_magnitude);
            let p00 = _mm512_mul_epu32(sample_magnitude, coefficient_magnitude);
            let p01 = _mm512_mul_epu32(sample_magnitude, coefficient_high);
            let p10 = _mm512_mul_epu32(sample_high, coefficient_magnitude);
            let p11 = _mm512_mul_epu32(sample_high, coefficient_high);
            let low_mask = _mm512_set1_epi64(i64::from(u32::MAX));
            let middle = _mm512_add_epi64(
                _mm512_add_epi64(
                    _mm512_srli_epi64::<32>(p00),
                    _mm512_and_si512(p01, low_mask),
                ),
                _mm512_and_si512(p10, low_mask),
            );
            let low_vector = _mm512_or_si512(
                _mm512_and_si512(p00, low_mask),
                _mm512_slli_epi64::<32>(middle),
            );
            let high_vector = _mm512_add_epi64(
                _mm512_add_epi64(
                    _mm512_add_epi64(p11, _mm512_srli_epi64::<32>(p01)),
                    _mm512_srli_epi64::<32>(p10),
                ),
                _mm512_srli_epi64::<32>(middle),
            );
            _mm512_storeu_si512(low.as_mut_ptr().cast::<__m512i>(), low_vector);
            _mm512_storeu_si512(high.as_mut_ptr().cast::<__m512i>(), high_vector);
        }

        for lane in 0..LANES {
            let magnitude = (u128::from(high[lane]) << 64) | u128::from(low[lane]);
            let negative = samples[position + lane].raw().is_negative()
                ^ coefficients[position + lane].raw().is_negative();
            let product = signed_from_magnitude(magnitude, negative, Operation::Accumulate)?;
            checked_accumulate(&mut accumulator, product)?;
        }
        position += LANES;
    }

    for (&sample, &coefficient) in samples[position..].iter().zip(&coefficients[position..]) {
        checked_accumulate(&mut accumulator, coefficient.widening_mul_sample(sample))?;
    }
    Ok(accumulator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_dot_checks_shape_and_accumulator_overflow() {
        assert_eq!(
            exact_dot_q1_63_q2_62_with_backend(&[Q1_63::ZERO], &[], ExactDotBackend::Scalar,),
            Err(ExactDotError::LengthMismatch {
                samples: 1,
                coefficients: 0,
            })
        );
        assert_eq!(
            exact_dot_q1_63_q2_62_with_backend(
                &[Q1_63::MIN, Q1_63::MIN],
                &[Q2_62::MIN, Q2_62::MIN],
                ExactDotBackend::Scalar,
            ),
            Err(ExactDotError::Arithmetic(ArithmeticError::Overflow(
                Operation::Accumulate,
            )))
        );
    }

    #[test]
    fn avx2_is_bit_exact_for_edges_lengths_and_random_values() {
        if !exact_dot_backend_available(ExactDotBackend::Avx2) {
            return;
        }
        let edge_samples = [
            i64::MIN,
            i64::MAX,
            -1,
            0,
            1,
            1_i64 << 62,
            -(1_i64 << 62),
            0x0123_4567_89ab_cdef,
        ];
        let edge_coefficients = [
            i64::MAX,
            i64::MIN,
            1,
            -1,
            0,
            1_i64 << 62,
            -(1_i64 << 61),
            -0x0123_4567_89ab_cdef,
        ];
        let samples = edge_samples.map(Q1_63::from_raw);
        let coefficients = edge_coefficients.map(Q2_62::from_raw);
        for length in 0..=samples.len() {
            assert_eq!(
                exact_dot_q1_63_q2_62_with_backend(
                    &samples[..length],
                    &coefficients[..length],
                    ExactDotBackend::Avx2,
                ),
                exact_dot_q1_63_q2_62_with_backend(
                    &samples[..length],
                    &coefficients[..length],
                    ExactDotBackend::Scalar,
                )
            );
        }

        let mut state = 0x243f_6a88_85a3_08d3_u64;
        for length in 0..=257 {
            let mut samples = Vec::with_capacity(length);
            let mut coefficients = Vec::with_capacity(length);
            for _ in 0..length {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                samples.push(Q1_63::from_raw((state as i64) >> 5));
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                coefficients.push(Q2_62::from_raw((state as i64) >> 9));
            }
            let scalar = exact_dot_q1_63_q2_62_with_backend(
                &samples,
                &coefficients,
                ExactDotBackend::Scalar,
            );
            let vector =
                exact_dot_q1_63_q2_62_with_backend(&samples, &coefficients, ExactDotBackend::Avx2);
            assert_eq!(vector, scalar, "length {length}");
        }
    }

    #[test]
    fn avx512_is_bit_exact_for_edges_and_random_values() {
        if !exact_dot_backend_available(ExactDotBackend::Avx512) {
            return;
        }
        let mut state = 0x1319_8a2e_0370_7344_u64;
        for length in 0..=257 {
            let mut samples = Vec::with_capacity(length);
            let mut coefficients = Vec::with_capacity(length);
            for index in 0..length {
                state = state
                    .wrapping_mul(2_862_933_555_777_941_757)
                    .wrapping_add(3_037_000_493);
                let sample = match index % 17 {
                    0 => i64::MIN,
                    1 => i64::MAX,
                    _ => (state as i64) >> 5,
                };
                state = state
                    .wrapping_mul(2_862_933_555_777_941_757)
                    .wrapping_add(3_037_000_493);
                let coefficient = match index % 19 {
                    0 => i64::MIN,
                    1 => i64::MAX,
                    _ => (state as i64) >> 9,
                };
                samples.push(Q1_63::from_raw(sample));
                coefficients.push(Q2_62::from_raw(coefficient));
            }
            let scalar = exact_dot_q1_63_q2_62_with_backend(
                &samples,
                &coefficients,
                ExactDotBackend::Scalar,
            );
            let vector = exact_dot_q1_63_q2_62_with_backend(
                &samples,
                &coefficients,
                ExactDotBackend::Avx512,
            );
            assert_eq!(vector, scalar, "length {length}");
        }
    }
}
