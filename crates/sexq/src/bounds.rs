//! Attainable signed MAC intervals, including the asymmetric negative endpoint.

use crate::{ArithmeticError, Operation};

/// Exact raw interval `[-negative_magnitude, positive_magnitude]` for a MAC
/// whose samples independently range over their entire declared signed format.
/// It contains every partial sum as well as the final sum (zero is an input).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExactAccumulatorRequirements {
    pub negative_magnitude: u128,
    pub positive_magnitude: u128,
    /// Minimum signed two's-complement width containing the complete interval.
    pub signed_bits: u16,
}

impl ExactAccumulatorRequirements {
    pub const fn fits_i128(self) -> bool {
        self.negative_magnitude <= 1_u128 << 127 && self.positive_magnitude <= i128::MAX as u128
    }
}

pub(crate) fn exact_native_requirements(
    sample_bits: u32,
    coefficients: impl Iterator<Item = i64>,
) -> Result<ExactAccumulatorRequirements, ArithmeticError> {
    let overflow = ArithmeticError::Overflow(Operation::Accumulate);
    let mut positive = 0_u128;
    let mut negative = 0_u128;
    for coefficient in coefficients {
        let sum = if coefficient < 0 {
            &mut negative
        } else {
            &mut positive
        };
        *sum = sum
            .checked_add(u128::from(coefficient.unsigned_abs()))
            .ok_or(overflow)?;
    }
    let magnitude = positive
        .checked_add(negative)
        .and_then(|sum| sum.checked_mul(1_u128 << (sample_bits - 1)))
        .ok_or(overflow)?;
    let negative_magnitude = magnitude - negative;
    let positive_magnitude = magnitude - positive;
    let negative_bits = u128::BITS - negative_magnitude.saturating_sub(1).leading_zeros();
    let positive_bits = u128::BITS - positive_magnitude.leading_zeros();
    Ok(ExactAccumulatorRequirements {
        negative_magnitude,
        positive_magnitude,
        signed_bits: (1 + negative_bits.max(positive_bits)) as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MacQ125, MacQ126, NativeMac, NativeQ, Q1_63, Q2_62, QFormat};

    #[test]
    fn exact_mac_intervals_match_exhaustive_signed_samples_and_prefixes() {
        let cf = QFormat::new(3, 0).unwrap();
        for width in 1..=4 {
            let sf = QFormat::new(1, width - 1).unwrap();
            let minimum = sf.minimum_raw();
            let maximum = sf.maximum_raw();
            for encoded in 0..125 {
                let coefficients = [encoded % 5 - 2, encoded / 5 % 5 - 2, encoded / 25 - 2]
                    .map(|raw| NativeQ::from_raw(raw, cf).unwrap());
                let bound = NativeMac::exact_requirements_for(sf, cf, &coefficients).unwrap();
                let mut observed_min = i128::MAX;
                let mut observed_max = i128::MIN;
                for a in minimum..=maximum {
                    for b in minimum..=maximum {
                        for c in minimum..=maximum {
                            let mut sum = 0_i128;
                            for (sample, coefficient) in [a, b, c].iter().zip(coefficients) {
                                sum += i128::from(*sample) * i128::from(coefficient.raw());
                                assert!(sum >= -(bound.negative_magnitude as i128));
                                assert!(sum <= bound.positive_magnitude as i128);
                            }
                            observed_min = observed_min.min(sum);
                            observed_max = observed_max.max(sum);
                        }
                    }
                }
                assert_eq!(observed_min, -(bound.negative_magnitude as i128));
                assert_eq!(observed_max, bound.positive_magnitude as i128);
                let endpoint = 1_i128 << (bound.signed_bits - 1);
                assert!(observed_min >= -endpoint && observed_max < endpoint);
                if bound.signed_bits > 1 {
                    assert!(observed_min < -(endpoint / 2) || observed_max >= endpoint / 2);
                }
                #[cfg(feature = "bigint")]
                {
                    use crate::{BigMac, BigQ, BigQFormat};
                    let big_cf = BigQFormat::new(3, 0).unwrap();
                    let big = BigMac::exact_requirements_for(
                        BigQFormat::new(1, width - 1).unwrap(),
                        big_cf,
                        &coefficients.map(|c| BigQ::from_i64(c.raw(), big_cf).unwrap()),
                    )
                    .unwrap();
                    assert_eq!(big.minimum_raw, observed_min);
                    assert_eq!(big.maximum_raw, observed_max);
                    assert_eq!(big.signed_bits, u32::from(bound.signed_bits));
                }
            }
        }
        assert_eq!(
            NativeMac::exact_requirements_for(cf, cf, &[])
                .unwrap()
                .signed_bits,
            1
        );
        assert!(
            NativeMac::exact_requirements_for(
                cf,
                cf,
                &[NativeQ::from_raw(0, QFormat::new(2, 0).unwrap()).unwrap()]
            )
            .is_err()
        );
    }

    #[test]
    fn exact_native_mac_distinguishes_negative_i128_endpoint_from_positive_overflow() {
        let positive = [Q2_62::ONE; 4];
        let bound = MacQ125::exact_requirements_for(&positive).unwrap();
        assert_eq!(bound.signed_bits, 128);
        assert_eq!(bound.negative_magnitude, 1_u128 << 127);
        assert!(bound.fits_i128());
        assert!(!MacQ125::requirements_for(&positive).unwrap().fits_i128());
        let mut mac = MacQ125::new();
        for coefficient in positive {
            mac.accumulate(Q1_63::MIN, coefficient).unwrap();
        }
        assert_eq!(mac.raw(), i128::MIN);
        let negative = positive.map(|c| Q2_62::from_raw(-c.raw()));
        let bound = MacQ125::exact_requirements_for(&negative).unwrap();
        assert_eq!(bound.signed_bits, 129);
        assert_eq!(bound.positive_magnitude, 1_u128 << 127);
        assert!(!bound.fits_i128());
        assert_eq!(
            MacQ126::exact_requirements_for(&[Q1_63::MIN])
                .unwrap()
                .signed_bits,
            128
        );
        assert!(exact_native_requirements(64, [i64::MIN; 4].into_iter()).is_err());
    }

    #[cfg(feature = "bigint")]
    #[test]
    fn exact_big_mac_endpoints_are_attained_at_8192_bits() {
        use crate::{BigMac, BigQ, BigQFormat};
        use rug::Integer;
        for width in [1, 2, 31, 32, 63, 64, 127, 128, 4096, 8192] {
            let sf = BigQFormat::new(1, width - 1).unwrap();
            let cf = BigQFormat::new(3, 0).unwrap();
            for raw in [
                vec![],
                vec![0],
                vec![1],
                vec![-1],
                vec![1, -1],
                vec![2, 1, -2],
            ] {
                let coefficients = raw
                    .iter()
                    .map(|c| BigQ::from_i64(*c, cf).unwrap())
                    .collect::<Vec<_>>();
                let bound = BigMac::exact_requirements_for(sf, cf, &coefficients).unwrap();
                for (want_min, expected) in
                    [(true, &bound.minimum_raw), (false, &bound.maximum_raw)]
                {
                    let mut mac = BigMac::new(sf, cf, Some(bound.signed_bits)).unwrap();
                    for c in &coefficients {
                        let sample = if (c.raw() >= &0) == want_min {
                            sf.minimum_raw()
                        } else {
                            sf.maximum_raw()
                        };
                        mac.accumulate(&BigQ::from_raw(sample, sf).unwrap(), c)
                            .unwrap();
                    }
                    assert_eq!(mac.raw(), expected);
                }
                if bound.signed_bits > 1 {
                    let half = Integer::from(1) << (bound.signed_bits - 2);
                    assert!(bound.minimum_raw < -half.clone() || bound.maximum_raw >= half);
                }
                if raw == [1] {
                    assert_eq!(bound.signed_bits, width);
                }
                if raw == [-1] {
                    assert_eq!(bound.signed_bits, width + 1);
                }
            }
        }
        let sf = BigQFormat::new(1, 63).unwrap();
        assert!(
            BigMac::exact_requirements_for(sf, sf, &[BigQ::zero(BigQFormat::new(2, 62).unwrap())])
                .is_err()
        );
    }
}
