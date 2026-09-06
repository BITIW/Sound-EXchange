//! Exact streaming measurement relative to the immediate pre-PCM signal.
//! Includes dither, noise shaping and saturation; does not estimate a noise PSD.

use rug::Integer;
use sexq::{BigQ, BigQFormat};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeterError {
    InvalidTargetBits(u32),
    LengthMismatch,
    FormatMismatch,
    PcmOutOfRange,
    CounterOverflow,
}

impl fmt::Display for MeterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTargetBits(bits) => {
                write!(f, "PCM error meter target width {bits} is outside 1..=32")
            }
            Self::LengthMismatch => {
                f.write_str("PCM error meter input/output sample counts differ")
            }
            Self::FormatMismatch => f.write_str("PCM error meter signal format mismatch"),
            Self::PcmOutOfRange => {
                f.write_str("PCM error meter output is outside the declared signed PCM range")
            }
            Self::CounterOverflow => f.write_str("PCM error meter sample counter overflow"),
        }
    }
}
impl std::error::Error for MeterError {}

/// Exact aggregates over all interleaved samples (not per-channel averages).
/// For N>0, mean-square error is `sum_squared_raw / (N * 2^(2*F))`;
/// peak absolute error is `peak_raw / 2^F`. N=0 is undefined, not zero RMS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PcmErrorStats {
    pub samples: u64,
    pub fractional_bits: u32,
    pub peak_raw: Integer,
    pub sum_squared_raw: Integer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PcmErrorMeter {
    format: BigQFormat,
    target_bits: u32,
    stats: PcmErrorStats,
}

impl PcmErrorMeter {
    pub fn new(format: BigQFormat, target_bits: u32) -> Result<Self, MeterError> {
        if !(1..=32).contains(&target_bits) {
            return Err(MeterError::InvalidTargetBits(target_bits));
        }
        Ok(Self {
            format,
            target_bits,
            stats: PcmErrorStats {
                samples: 0,
                fractional_bits: format.fractional_bits().max(target_bits - 1),
                peak_raw: Integer::new(),
                sum_squared_raw: Integer::new(),
            },
        })
    }

    pub fn stats(&self) -> &PcmErrorStats {
        &self.stats
    }

    /// Observe one corresponding block. Errors leave all aggregates unchanged;
    /// the caller can correct the rejected block and retry. No audio is modified.
    pub fn observe(&mut self, source: &[BigQ], pcm: &[i64]) -> Result<(), MeterError> {
        if source.len() != pcm.len() {
            return Err(MeterError::LengthMismatch);
        }
        let samples = self
            .stats
            .samples
            .checked_add(u64::try_from(source.len()).map_err(|_| MeterError::CounterOverflow)?)
            .ok_or(MeterError::CounterOverflow)?;
        let endpoint = 1_i64 << (self.target_bits - 1);
        for (source, &pcm) in source.iter().zip(pcm) {
            if source.format() != self.format {
                return Err(MeterError::FormatMismatch);
            }
            if !(-endpoint..endpoint).contains(&pcm) {
                return Err(MeterError::PcmOutOfRange);
            }
        }
        let signal_shift = self.stats.fractional_bits - self.format.fractional_bits();
        let pcm_shift = self.stats.fractional_bits - (self.target_bits - 1);
        for (source, &pcm) in source.iter().zip(pcm) {
            let mut error = Integer::from(pcm) << pcm_shift;
            error -= source.raw().clone() << signal_shift;
            error.abs_mut();
            if error > self.stats.peak_raw {
                self.stats.peak_raw.clone_from(&error);
            }
            error.square_mut();
            self.stats.sum_squared_raw += error;
        }
        self.stats.samples = samples;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::big_dither::BigDitherQuantizer;
    use crate::{DitherConfig, DitherMode};
    use sexq::{OverflowPolicy, RoundingMode};

    #[test]
    fn pcm_error_exact_known_ties_and_saturation() {
        let format = BigQFormat::new(4, 4).unwrap();
        let mut meter = PcmErrorMeter::new(format, 3).unwrap();
        let source: Vec<_> = [0, 1, 2, 3, 4, -3, -4]
            .map(|raw| BigQ::from_i64(raw, format).unwrap())
            .into();
        meter.observe(&source, &[0, 0, 0, 1, 1, -1, -1]).unwrap();
        assert_eq!(meter.stats.samples, 7);
        assert_eq!(meter.stats.peak_raw, 2);
        assert_eq!(meter.stats.sum_squared_raw, 7);
        // Positive 2 FS -> largest PCM3 value (3/4 FS): -20/16 error.
        meter
            .observe(&[BigQ::from_i64(32, format).unwrap()], &[3])
            .unwrap();
        assert_eq!(meter.stats.peak_raw, 20);
        assert_eq!(meter.stats.sum_squared_raw, 407);
    }

    #[test]
    fn pcm_error_matches_reconstructed_pcm_for_all_dithers_widths_and_chunks() {
        for fractional_bits in [0, 4, 63, 96, 4096] {
            let format = BigQFormat::new(65, fractional_bits).unwrap();
            for target_bits in [1, 8, 16, 24, 32] {
                for mode in [
                    DitherMode::None,
                    DitherMode::Tpdf,
                    DitherMode::HighPassTpdf,
                    DitherMode::NoiseShaped { order: 1 },
                    DitherMode::NoiseShaped { order: 5 },
                    DitherMode::NoiseShaped { order: 9 },
                ] {
                    let source: Vec<_> = (0_i64..37)
                        .map(|index| {
                            let raw = ((Integer::from(index % 7 - 3) << fractional_bits) / 3)
                                + (index % 5 - 2);
                            BigQ::from_raw(raw, format).unwrap()
                        })
                        .collect();
                    let config = DitherConfig {
                        target_bits,
                        mode,
                        seed: 42,
                        rounding: RoundingMode::NearestTiesToEven,
                        overflow: OverflowPolicy::Saturate,
                    };
                    let mut quantizer = BigDitherQuantizer::new(format, config).unwrap();
                    let pcm: Vec<_> = source
                        .iter()
                        .map(|x| quantizer.quantize(x).unwrap())
                        .collect();
                    let exact_format =
                        BigQFormat::new(65, fractional_bits.max(target_bits - 1)).unwrap();
                    let errors: Vec<_> = source
                        .iter()
                        .zip(&pcm)
                        .map(|(source, &pcm)| {
                            let reconstructed = BigQ::from_signed_pcm(
                                pcm,
                                target_bits,
                                exact_format,
                                RoundingMode::NearestTiesToEven,
                                OverflowPolicy::Error,
                            )
                            .unwrap()
                            .value;
                            let source = source
                                .rescale(
                                    exact_format,
                                    RoundingMode::NearestTiesToEven,
                                    OverflowPolicy::Error,
                                )
                                .unwrap()
                                .value;
                            Integer::from(reconstructed.raw() - source.raw()).abs()
                        })
                        .collect();
                    let peak = errors.iter().max().unwrap();
                    let squares: Integer = errors
                        .iter()
                        .map(|error| Integer::from(error * error))
                        .sum();
                    for chunk in [1, 2, 7, 4096] {
                        let mut meter = PcmErrorMeter::new(format, target_bits).unwrap();
                        for (source, pcm) in source.chunks(chunk).zip(pcm.chunks(chunk)) {
                            meter.observe(source, pcm).unwrap();
                        }
                        assert_eq!(meter.stats.samples, source.len() as u64);
                        assert_eq!(meter.stats.fractional_bits, exact_format.fractional_bits());
                        assert_eq!(&meter.stats.peak_raw, peak);
                        assert_eq!(meter.stats.sum_squared_raw, squares);
                    }
                }
            }
        }
    }

    #[test]
    fn pcm_error_rejected_blocks_are_atomic_and_retryable() {
        let format = BigQFormat::new(4, 4).unwrap();
        assert_eq!(
            PcmErrorMeter::new(format, 0),
            Err(MeterError::InvalidTargetBits(0))
        );
        assert_eq!(
            PcmErrorMeter::new(format, 33),
            Err(MeterError::InvalidTargetBits(33))
        );
        let zero = BigQ::zero(format);
        let mut meter = PcmErrorMeter::new(format, 3).unwrap();
        meter.observe(std::slice::from_ref(&zero), &[1]).unwrap();
        let previous = meter.clone();
        for (source, pcm, error) in [
            (vec![zero.clone()], vec![], MeterError::LengthMismatch),
            (
                vec![zero.clone(), BigQ::zero(BigQFormat::new(4, 5).unwrap())],
                vec![1, 0],
                MeterError::FormatMismatch,
            ),
            (
                vec![zero.clone(), zero.clone()],
                vec![1, 4],
                MeterError::PcmOutOfRange,
            ),
            (
                vec![zero.clone(), zero.clone()],
                vec![1, -5],
                MeterError::PcmOutOfRange,
            ),
        ] {
            assert_eq!(meter.observe(&source, &pcm), Err(error));
            assert_eq!(meter, previous);
        }
        meter.observe(std::slice::from_ref(&zero), &[0]).unwrap();
        assert_eq!(meter.stats.samples, 2);
        meter.stats.samples = u64::MAX;
        let previous = meter.clone();
        assert_eq!(
            meter.observe(&[zero], &[0]),
            Err(MeterError::CounterOverflow)
        );
        assert_eq!(meter, previous);
        meter.observe(&[], &[]).unwrap();
        assert_eq!(meter, previous);
    }
}
