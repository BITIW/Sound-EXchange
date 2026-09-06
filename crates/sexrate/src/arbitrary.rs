//! Arbitrary-fractional signal FIR and streaming APIs. There is no Q1.63 or
//! Q65.63 conversion in this path, including pre-roll, history, and tail.

use super::{
    FrameCountPolicy, InterleavedStreamStats, PolyphaseError, PolyphaseFirBigQ63, PolyphaseFirQ63,
    RateRatio, StreamError, StreamStats, Timeline, append_interleaved,
};
use sexq::{ArithmeticOutcome, BigMac, BigQ, BigQError, BigQFormat, OverflowPolicy, RoundingMode};
use std::sync::Arc;

#[cfg(test)]
mod tests;

/// Explicit signal/coefficient formats and accumulator policy for a phase bank.
/// `None` computes the exact minimum signed full-input-range accumulator width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BigFirSpec {
    pub phase_count: usize,
    pub taps_per_phase: usize,
    pub input_format: BigQFormat,
    pub output_format: BigQFormat,
    pub coefficient_format: BigQFormat,
    pub accumulator_bits: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolyphaseFirBig {
    spec: BigFirSpec,
    accumulator_bits: u32,
    required_accumulator_bits: u32,
    coefficients: Arc<[BigQ]>,
}

impl PolyphaseFirBig {
    pub fn new(spec: BigFirSpec, coefficients: Vec<BigQ>) -> Result<Self, PolyphaseError> {
        Self::from_shared(spec, coefficients.into())
    }

    /// Reuse an existing designer/cache bank without duplicating its GMP coefficients.
    pub fn from_q63_bank(
        bank: &PolyphaseFirBigQ63,
        input_format: BigQFormat,
        output_format: BigQFormat,
        accumulator_bits: Option<u32>,
    ) -> Result<Self, PolyphaseError> {
        Self::from_shared(
            BigFirSpec {
                phase_count: bank.phase_count(),
                taps_per_phase: bank.taps_per_phase(),
                input_format,
                output_format,
                coefficient_format: bank.coefficient_format(),
                accumulator_bits,
            },
            Arc::clone(&bank.coefficients),
        )
    }

    /// Losslessly promote native coefficients once, never through floating point.
    pub fn from_native_bank(
        bank: &PolyphaseFirQ63,
        input_format: BigQFormat,
        output_format: BigQFormat,
        accumulator_bits: Option<u32>,
    ) -> Result<Self, PolyphaseError> {
        let coefficient_format = BigQFormat::new(2, 62)?;
        let coefficients = bank
            .coefficients
            .iter()
            .map(|coefficient| BigQ::from_i64(coefficient.raw(), coefficient_format))
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(
            BigFirSpec {
                phase_count: bank.phase_count(),
                taps_per_phase: bank.taps_per_phase(),
                input_format,
                output_format,
                coefficient_format,
                accumulator_bits,
            },
            coefficients,
        )
    }

    fn from_shared(spec: BigFirSpec, coefficients: Arc<[BigQ]>) -> Result<Self, PolyphaseError> {
        if spec.phase_count == 0 {
            return Err(PolyphaseError::ZeroPhases);
        }
        if spec.taps_per_phase == 0 {
            return Err(PolyphaseError::ZeroTaps);
        }
        let expected = spec.phase_count.checked_mul(spec.taps_per_phase).ok_or(
            PolyphaseError::CoefficientCount {
                expected: usize::MAX,
                actual: coefficients.len(),
            },
        )?;
        if expected != coefficients.len() {
            return Err(PolyphaseError::CoefficientCount {
                expected,
                actual: coefficients.len(),
            });
        }
        BigMac::new(
            spec.input_format,
            spec.coefficient_format,
            spec.accumulator_bits,
        )?;
        let mut required_accumulator_bits = 1;
        for (phase, coefficients) in coefficients.chunks_exact(spec.taps_per_phase).enumerate() {
            let requirements = BigMac::exact_requirements_for(
                spec.input_format,
                spec.coefficient_format,
                coefficients,
            )?;
            if let Some(bits) = spec.accumulator_bits
                && bits < requirements.signed_bits
            {
                return Err(PolyphaseError::BigAccumulatorTooNarrow {
                    phase,
                    required_bits: requirements.signed_bits,
                    configured_bits: bits,
                });
            }
            required_accumulator_bits = required_accumulator_bits.max(requirements.signed_bits);
        }
        let accumulator_bits = spec.accumulator_bits.unwrap_or(required_accumulator_bits);
        BigMac::new(
            spec.input_format,
            spec.coefficient_format,
            Some(accumulator_bits),
        )?;
        Ok(Self {
            spec,
            accumulator_bits,
            required_accumulator_bits,
            coefficients,
        })
    }

    pub const fn spec(&self) -> BigFirSpec {
        self.spec
    }
    pub const fn phase_count(&self) -> usize {
        self.spec.phase_count
    }
    pub const fn taps_per_phase(&self) -> usize {
        self.spec.taps_per_phase
    }
    pub const fn accumulator_bits(&self) -> u32 {
        self.accumulator_bits
    }
    pub const fn required_accumulator_bits(&self) -> u32 {
        self.required_accumulator_bits
    }

    pub fn phase(&self, phase: u64) -> Result<&[BigQ], PolyphaseError> {
        let index = usize::try_from(phase)
            .ok()
            .filter(|index| *index < self.spec.phase_count)
            .ok_or(PolyphaseError::PhaseOutOfRange {
                phase,
                phase_count: self.spec.phase_count,
            })?;
        let start = index * self.spec.taps_per_phase;
        Ok(&self.coefficients[start..start + self.spec.taps_per_phase])
    }

    /// Extended products, one complete exact sum, one selected output rounding.
    pub fn convolve(
        &self,
        phase: u64,
        samples: &[BigQ],
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<ArithmeticOutcome<BigQ>, PolyphaseError> {
        if samples.len() != self.spec.taps_per_phase {
            return Err(PolyphaseError::SampleCount {
                expected: self.spec.taps_per_phase,
                actual: samples.len(),
            });
        }
        let mut mac = BigMac::new(
            self.spec.input_format,
            self.spec.coefficient_format,
            Some(self.accumulator_bits),
        )?;
        for (sample, coefficient) in samples.iter().zip(self.phase(phase)?) {
            mac.accumulate(sample, coefficient)?;
        }
        Ok(mac.finish(self.spec.output_format, rounding, overflow)?)
    }

    fn validate_samples(&self, samples: &[BigQ]) -> Result<(), PolyphaseError> {
        for sample in samples {
            if sample.format() != self.spec.input_format {
                return Err(BigQError::FormatMismatch {
                    left: self.spec.input_format,
                    right: sample.format(),
                }
                .into());
            }
        }
        Ok(())
    }
}

/// The same finite/causal schedule as the native resampler, with owned BigQ history.
#[derive(Debug)]
pub struct CausalResamplerBig {
    timeline: Timeline<BigQ>,
    bank: Arc<PolyphaseFirBig>,
    rounding: RoundingMode,
    overflow: OverflowPolicy,
}

impl CausalResamplerBig {
    pub fn new_with_input_delay(
        ratio: RateRatio,
        bank: Arc<PolyphaseFirBig>,
        input_delay: u64,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, StreamError> {
        if u64::try_from(bank.phase_count()).ok() != Some(ratio.up()) {
            return Err(StreamError::PhaseCountMismatch {
                ratio_up: ratio.up(),
                bank_phases: bank.phase_count(),
            });
        }
        let timeline = Timeline::new(
            ratio,
            input_delay,
            bank.taps_per_phase(),
            BigQ::zero(bank.spec.input_format),
        )?;
        Ok(Self {
            timeline,
            bank,
            rounding,
            overflow,
        })
    }

    pub const fn stats(&self) -> StreamStats {
        self.timeline.stats()
    }
    pub fn retained_input_frames(&self) -> usize {
        self.timeline.retained_frames()
    }

    pub fn push_into(&mut self, input: &[BigQ], output: &mut Vec<BigQ>) -> Result<(), StreamError> {
        self.push(input, None, output)
    }

    pub fn push_finite_into(
        &mut self,
        input: &[BigQ],
        policy: FrameCountPolicy,
        output: &mut Vec<BigQ>,
    ) -> Result<(), StreamError> {
        self.push(input, Some(policy), output)
    }

    fn push(
        &mut self,
        input: &[BigQ],
        policy: Option<FrameCountPolicy>,
        output: &mut Vec<BigQ>,
    ) -> Result<(), StreamError> {
        self.timeline.ensure_ready()?;
        if let Err(error) = self.bank.validate_samples(input) {
            self.timeline.poison();
            return Err(error.into());
        }
        self.timeline.push(input, policy, output, |phase, samples| {
            self.bank
                .convolve(phase, samples, self.rounding, self.overflow)
        })
    }

    pub fn finish_into(self, output: &mut Vec<BigQ>) -> Result<StreamStats, StreamError> {
        self.finish(None, output)
    }

    pub fn finish_exact_frames(
        self,
        target: u64,
        output: &mut Vec<BigQ>,
    ) -> Result<StreamStats, StreamError> {
        self.finish(Some(target), output)
    }

    fn finish(
        self,
        target: Option<u64>,
        output: &mut Vec<BigQ>,
    ) -> Result<StreamStats, StreamError> {
        let Self {
            timeline,
            bank,
            rounding,
            overflow,
        } = self;
        timeline.finish(target, output, |phase, samples| {
            bank.convolve(phase, samples, rounding, overflow)
        })
    }
}

/// Independent BigQ history per channel, one immutable coefficient bank.
#[derive(Debug)]
pub struct InterleavedResamplerBig {
    channels: u16,
    bank: Arc<PolyphaseFirBig>,
    streams: Vec<CausalResamplerBig>,
    poisoned: bool,
}

impl InterleavedResamplerBig {
    pub fn new_with_input_delay(
        channels: u16,
        ratio: RateRatio,
        bank: Arc<PolyphaseFirBig>,
        delay: u64,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, StreamError> {
        if channels == 0 {
            return Err(StreamError::ZeroChannels);
        }
        let streams = (0..channels)
            .map(|_| {
                CausalResamplerBig::new_with_input_delay(
                    ratio,
                    Arc::clone(&bank),
                    delay,
                    rounding,
                    overflow,
                )
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            channels,
            bank,
            streams,
            poisoned: false,
        })
    }

    pub const fn channels(&self) -> u16 {
        self.channels
    }

    pub fn push_interleaved_into(
        &mut self,
        input: &[BigQ],
        output: &mut Vec<BigQ>,
    ) -> Result<(), StreamError> {
        self.push(input, None, output)
    }

    pub fn push_interleaved_finite_into(
        &mut self,
        input: &[BigQ],
        policy: FrameCountPolicy,
        output: &mut Vec<BigQ>,
    ) -> Result<(), StreamError> {
        self.push(input, Some(policy), output)
    }

    fn push(
        &mut self,
        input: &[BigQ],
        policy: Option<FrameCountPolicy>,
        output: &mut Vec<BigQ>,
    ) -> Result<(), StreamError> {
        if self.poisoned {
            return Err(StreamError::Poisoned);
        }
        let result = (|| {
            let channels = usize::from(self.channels);
            if !input.len().is_multiple_of(channels) {
                return Err(StreamError::PartialInterleavedFrame {
                    samples: input.len(),
                    channels: self.channels,
                });
            }
            self.bank.validate_samples(input)?;
            let mut inputs = vec![Vec::new(); channels];
            for channel in &mut inputs {
                channel
                    .try_reserve(input.len() / channels)
                    .map_err(|_| StreamError::AllocationFailed)?;
            }
            for frame in input.chunks_exact(channels) {
                for (channel, sample) in frame.iter().enumerate() {
                    inputs[channel].push(sample.clone());
                }
            }
            let mut outputs = vec![Vec::new(); channels];
            for channel in 0..channels {
                self.streams[channel].push(&inputs[channel], policy, &mut outputs[channel])?;
            }
            append_interleaved(&outputs, output)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    pub fn finish_exact_frames(
        self,
        target: u64,
        output: &mut Vec<BigQ>,
    ) -> Result<InterleavedStreamStats, StreamError> {
        self.finish(Some(target), output)
    }

    pub fn finish_into(
        self,
        output: &mut Vec<BigQ>,
    ) -> Result<InterleavedStreamStats, StreamError> {
        self.finish(None, output)
    }

    fn finish(
        self,
        target: Option<u64>,
        output: &mut Vec<BigQ>,
    ) -> Result<InterleavedStreamStats, StreamError> {
        if self.poisoned {
            return Err(StreamError::Poisoned);
        }
        let mut outputs = vec![Vec::new(); usize::from(self.channels)];
        let mut stats = Vec::new();
        for (channel, stream) in self.streams.into_iter().enumerate() {
            stats.push(stream.finish(target, &mut outputs[channel])?);
        }
        let first = stats[0];
        if stats
            .iter()
            .any(|s| s.input_frames != first.input_frames || s.output_frames != first.output_frames)
        {
            return Err(StreamError::ChannelOutputMismatch);
        }
        let saturated_samples = stats.iter().try_fold(0_u64, |total, s| {
            total
                .checked_add(s.saturated_outputs)
                .ok_or(StreamError::SaturationCountOverflow)
        })?;
        append_interleaved(&outputs, output)?;
        Ok(InterleavedStreamStats {
            input_frames: first.input_frames,
            output_frames: first.output_frames,
            saturated_samples,
        })
    }
}
