//! Seeded PCM quantization at the signal's actual fractional precision.
//! Uniform noise uses little-endian 64-bit PRNG limbs, with the final limb
//! masked. At F=63 this is exactly the native quantizer's PRNG consumption.

use crate::arbitrary::BigDspError;
use crate::{
    DitherConfig, DitherMode, DitherQuantizer, DitherStats, DspError, SplitMix64,
    noise_shaper_coefficients,
};
use rug::Integer;
use sexq::{BigQ, BigQError, BigQFormat, RoundingMode, quantize_big_raw_to_signed_pcm};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BigDitherQuantizer {
    format: BigQFormat,
    config: DitherConfig,
    working_fractional_bits: u32,
    discarded_bits: u32,
    generator: SplitMix64,
    previous_uniform: Integer,
    errors: [Integer; 9],
    stats: DitherStats,
    poisoned: bool,
}

impl BigDitherQuantizer {
    pub fn new(format: BigQFormat, config: DitherConfig) -> Result<Self, BigDspError> {
        // Shared native validation: supported PCM widths, shaping orders.
        DitherQuantizer::new(config)?;
        let working_fractional_bits = format.fractional_bits().max(config.target_bits - 1);
        let mut result = Self {
            format,
            config,
            working_fractional_bits,
            discarded_bits: working_fractional_bits - (config.target_bits - 1),
            generator: SplitMix64::new(config.seed),
            previous_uniform: Integer::new(),
            errors: std::array::from_fn(|_| Integer::new()),
            stats: DitherStats::default(),
            poisoned: false,
        };
        if config.mode == DitherMode::HighPassTpdf {
            result.previous_uniform = result.uniform();
        }
        Ok(result)
    }

    pub const fn config(&self) -> DitherConfig {
        self.config
    }
    pub const fn stats(&self) -> DitherStats {
        self.stats
    }

    pub fn quantize(&mut self, sample: &BigQ) -> Result<i64, BigDspError> {
        if self.poisoned {
            return Err(DspError::Poisoned.into());
        }
        let result = self.quantize_inner(sample);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn quantize_inner(&mut self, sample: &BigQ) -> Result<i64, BigDspError> {
        if sample.format() != self.format {
            return Err(BigQError::FormatMismatch {
                left: self.format,
                right: sample.format(),
            }
            .into());
        }
        let mut shaped =
            sample.raw().clone() << (self.working_fractional_bits - self.format.fractional_bits());
        shaped += self.noise();
        if let DitherMode::NoiseShaped { order } = self.config.mode {
            for (coefficient, error) in noise_shaper_coefficients(order)
                .expect("validated order")
                .iter()
                .zip(&self.errors)
            {
                shaped += error.clone() * *coefficient;
            }
        }
        let outcome = quantize_big_raw_to_signed_pcm(
            &shaped,
            self.working_fractional_bits,
            self.config.target_bits,
            self.config.rounding,
            self.config.overflow,
        )?;
        if let DitherMode::NoiseShaped { order } = self.config.mode {
            if outcome.saturated {
                self.errors
                    .iter_mut()
                    .for_each(|error| *error = Integer::new());
            } else {
                self.errors[..usize::from(order)].rotate_right(1);
                self.errors[0] = (Integer::from(outcome.value) << self.discarded_bits) - shaped;
            }
        }
        self.stats.quantized_samples = self
            .stats
            .quantized_samples
            .checked_add(1)
            .ok_or(DspError::CounterOverflow)?;
        self.stats.saturated_samples = self
            .stats
            .saturated_samples
            .checked_add(u64::from(outcome.saturated))
            .ok_or(DspError::CounterOverflow)?;
        Ok(outcome.value)
    }

    fn uniform(&mut self) -> Integer {
        let mut value = Integer::new();
        // Even a zero-width uniform consumes one PRNG value, matching native.
        let words = self.discarded_bits.div_ceil(64).max(1);
        for word in 0..words {
            let take = self.discarded_bits.saturating_sub(word * 64).min(64);
            let raw = self.generator.next();
            let raw = if take == 64 {
                raw
            } else {
                raw & ((1_u64 << take) - 1)
            };
            value += Integer::from(raw) << (word * 64);
        }
        value
    }

    fn noise(&mut self) -> Integer {
        match self.config.mode {
            DitherMode::None => Integer::new(),
            DitherMode::Tpdf | DitherMode::NoiseShaped { .. } => self.uniform() - self.uniform(),
            DitherMode::HighPassTpdf => {
                let current = self.uniform();
                let noise = Integer::from(&current - &self.previous_uniform);
                self.previous_uniform = current;
                noise
            }
        }
    }

    /// Conservative excursion in raw units at the supplied signal format.
    /// Normalization requires F >= B-1 so no internal promotion is needed.
    pub fn maximum_excursion_raw(
        format: BigQFormat,
        config: DitherConfig,
    ) -> Result<Integer, BigDspError> {
        DitherQuantizer::new(config)?;
        let bits = format
            .fractional_bits()
            .saturating_sub(config.target_bits - 1);
        let step = Integer::from(1) << bits;
        let mut bound = Integer::from(&step - 1);
        match config.mode {
            DitherMode::None => Ok(Integer::new()),
            DitherMode::Tpdf | DitherMode::HighPassTpdf => Ok(bound),
            DitherMode::NoiseShaped { order } => {
                let l1 = noise_shaper_coefficients(order)
                    .expect("validated order")
                    .iter()
                    .map(|coefficient| u32::from(coefficient.unsigned_abs()))
                    .sum::<u32>();
                let error = match config.rounding {
                    RoundingMode::NearestTiesToEven | RoundingMode::NearestTiesAwayFromZero => {
                        step >> 1
                    }
                    _ => Integer::from(&step - 1),
                };
                bound += error * l1;
                Ok(bound)
            }
        }
    }
}

/// Atomic interleaved block: commit all channel states and samples together,
/// or leave output unchanged and poison the complete quantizer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BigInterleavedDither {
    channels: u16,
    quantizers: Vec<BigDitherQuantizer>,
    poisoned: bool,
}

impl BigInterleavedDither {
    pub fn new(
        channels: u16,
        format: BigQFormat,
        config: DitherConfig,
    ) -> Result<Self, BigDspError> {
        if channels == 0 {
            return Err(DspError::ZeroChannels.into());
        }
        let mut seeds = SplitMix64::new(config.seed);
        let quantizers = (0..channels)
            .map(|_| {
                BigDitherQuantizer::new(
                    format,
                    DitherConfig {
                        seed: seeds.next(),
                        ..config
                    },
                )
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            channels,
            quantizers,
            poisoned: false,
        })
    }

    pub fn quantize_interleaved_into(
        &mut self,
        input: &[BigQ],
        output: &mut Vec<i64>,
    ) -> Result<(), BigDspError> {
        if self.poisoned {
            return Err(DspError::Poisoned.into());
        }
        let result = (|| {
            if !input.len().is_multiple_of(usize::from(self.channels)) {
                return Err(DspError::PartialInterleavedFrame {
                    samples: input.len(),
                    channels: self.channels,
                }
                .into());
            }
            let mut candidate = self.quantizers.clone();
            let mut rendered = Vec::new();
            rendered
                .try_reserve(input.len())
                .map_err(|_| DspError::AllocationFailed)?;
            for frame in input.chunks_exact(usize::from(self.channels)) {
                for (channel, sample) in frame.iter().enumerate() {
                    rendered.push(candidate[channel].quantize(sample)?);
                }
            }
            output
                .try_reserve(rendered.len())
                .map_err(|_| DspError::AllocationFailed)?;
            output.extend(rendered);
            self.quantizers = candidate;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    pub fn stats(&self) -> Result<DitherStats, BigDspError> {
        self.quantizers
            .iter()
            .try_fold(DitherStats::default(), |mut total, q| {
                total.quantized_samples = total
                    .quantized_samples
                    .checked_add(q.stats.quantized_samples)
                    .ok_or(DspError::CounterOverflow)?;
                total.saturated_samples = total
                    .saturated_samples
                    .checked_add(q.stats.saturated_samples)
                    .ok_or(DspError::CounterOverflow)?;
                Ok(total)
            })
    }
}

#[cfg(test)]
mod tests;
