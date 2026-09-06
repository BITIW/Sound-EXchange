//! Streaming fixed-point DSP stages and deterministic PCM dithering for SeX.

use core::fmt;
use core::str::FromStr;
use sexq::{
    ArithmeticError, MacQ125, OverflowPolicy, Q1_63, Q2_62, RoundingMode, WideQ63,
    quantize_q1_63_raw_to_signed_pcm,
};
use std::collections::VecDeque;

#[cfg(feature = "bigint")]
pub mod arbitrary;
#[cfg(feature = "bigint")]
pub mod big_dither;
#[cfg(feature = "bigint")]
pub mod pcm_error;
#[cfg(feature = "bigint")]
pub mod wide;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClipPolicy {
    #[default]
    Saturate,
    Error,
    Normalize,
    AllowHeadroom,
}

impl fmt::Display for ClipPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Saturate => f.write_str("saturate"),
            Self::Error => f.write_str("error"),
            Self::Normalize => f.write_str("normalize"),
            Self::AllowHeadroom => f.write_str("allow-headroom"),
        }
    }
}

impl FromStr for ClipPolicy {
    type Err = DspError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "saturate" => Ok(Self::Saturate),
            "error" => Ok(Self::Error),
            "normalize" => Ok(Self::Normalize),
            "allow-headroom" => Ok(Self::AllowHeadroom),
            _ => Err(DspError::UnknownClipPolicy(value.to_owned())),
        }
    }
}

impl ClipPolicy {
    /// Map policies implementable by the current bounded Q1.63 output. The
    /// other policies require a wider intermediate pipeline and fail loudly.
    pub fn native_overflow_policy(self) -> Result<OverflowPolicy, DspError> {
        match self {
            Self::Saturate => Ok(OverflowPolicy::Saturate),
            Self::Error => Ok(OverflowPolicy::Error),
            Self::Normalize | Self::AllowHeadroom => {
                Err(DspError::ClipPolicyRequiresWidePipeline(self))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DitherMode {
    None,
    Tpdf,
    HighPassTpdf,
    /// Integer error-feedback noise shaping with TPDF injection. Supported
    /// orders are 1, 5, and 9 and are part of the reproducibility identity.
    NoiseShaped {
        order: u8,
    },
}

impl fmt::Display for DitherMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Tpdf => f.write_str("tpdf"),
            Self::HighPassTpdf => f.write_str("high-pass-tpdf"),
            Self::NoiseShaped { order } => write!(f, "noise-shaped-{order}"),
        }
    }
}

impl FromStr for DitherMode {
    type Err = DspError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "tpdf" => Ok(Self::Tpdf),
            "high-pass" | "high-pass-tpdf" => Ok(Self::HighPassTpdf),
            "noise-shaped" | "noise-shaped-1" => Ok(Self::NoiseShaped { order: 1 }),
            "noise-shaped-5" => Ok(Self::NoiseShaped { order: 5 }),
            "noise-shaped-9" => Ok(Self::NoiseShaped { order: 9 }),
            _ => Err(DspError::UnknownDitherMode(value.to_owned())),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DitherConfig {
    pub target_bits: u32,
    pub mode: DitherMode,
    pub seed: u64,
    pub rounding: RoundingMode,
    pub overflow: OverflowPolicy,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DitherStats {
    pub quantized_samples: u64,
    pub saturated_samples: u64,
}

/// Portable SplitMix64. Modular wrap is intentional PRNG arithmetic and is
/// isolated from the checked signal path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DitherQuantizer {
    config: DitherConfig,
    generator: SplitMix64,
    lsb_step: u64,
    previous_uniform: u64,
    quantization_error_history: [i128; 9],
    stats: DitherStats,
    poisoned: bool,
}

impl DitherQuantizer {
    pub fn new(config: DitherConfig) -> Result<Self, DspError> {
        if !(1..=64).contains(&config.target_bits) {
            return Err(DspError::InvalidTargetBits(config.target_bits));
        }
        if let DitherMode::NoiseShaped { order } = config.mode {
            noise_shaper_coefficients(order)
                .ok_or(DspError::UnsupportedNoiseShapingOrder(order))?;
        }
        let lsb_step = 1_u64 << (64 - config.target_bits);
        let mut generator = SplitMix64::new(config.seed);
        let previous_uniform = if config.mode == DitherMode::HighPassTpdf {
            generator.next() & (lsb_step - 1)
        } else {
            0
        };
        Ok(Self {
            config,
            generator,
            lsb_step,
            previous_uniform,
            quantization_error_history: [0; 9],
            stats: DitherStats::default(),
            poisoned: false,
        })
    }

    pub const fn config(&self) -> DitherConfig {
        self.config
    }

    pub const fn stats(&self) -> DitherStats {
        self.stats
    }

    pub fn quantize(&mut self, sample: Q1_63) -> Result<i64, DspError> {
        self.quantize_raw(i128::from(sample.raw()))
    }

    pub fn quantize_wide(&mut self, sample: WideQ63) -> Result<i64, DspError> {
        self.quantize_raw(sample.raw())
    }

    fn quantize_raw(&mut self, raw: i128) -> Result<i64, DspError> {
        if self.poisoned {
            return Err(DspError::Poisoned);
        }
        let result = self.quantize_inner(raw);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn quantize_inner(&mut self, sample_raw: i128) -> Result<i64, DspError> {
        let noise = self.next_noise();
        let feedback = self.error_feedback()?;
        let shaped = sample_raw
            .checked_add(noise)
            .and_then(|value| value.checked_add(feedback))
            .ok_or(DspError::ExtendedSignalOverflow)?;
        let outcome = quantize_q1_63_raw_to_signed_pcm(
            shaped,
            self.config.target_bits,
            self.config.rounding,
            self.config.overflow,
        )?;

        if let DitherMode::NoiseShaped { order } = self.config.mode {
            if outcome.saturated {
                self.quantization_error_history.fill(0);
            } else {
                let reconstructed = i128::from(outcome.value) << (64 - self.config.target_bits);
                let error = reconstructed
                    .checked_sub(shaped)
                    .ok_or(DspError::ExtendedSignalOverflow)?;
                let active = usize::from(order);
                self.quantization_error_history[..active].rotate_right(1);
                self.quantization_error_history[0] = error;
            }
        }
        self.stats.quantized_samples = self
            .stats
            .quantized_samples
            .checked_add(1)
            .ok_or(DspError::CounterOverflow)?;
        if outcome.saturated {
            self.stats.saturated_samples = self
                .stats
                .saturated_samples
                .checked_add(1)
                .ok_or(DspError::CounterOverflow)?;
        }
        Ok(outcome.value)
    }

    fn next_noise(&mut self) -> i128 {
        match self.config.mode {
            DitherMode::None => 0,
            DitherMode::Tpdf | DitherMode::NoiseShaped { .. } => {
                i128::from(self.next_uniform()) - i128::from(self.next_uniform())
            }
            DitherMode::HighPassTpdf => {
                let current = self.next_uniform();
                let noise = i128::from(current) - i128::from(self.previous_uniform);
                self.previous_uniform = current;
                noise
            }
        }
    }

    fn next_uniform(&mut self) -> u64 {
        self.generator.next() & (self.lsb_step - 1)
    }

    fn error_feedback(&self) -> Result<i128, DspError> {
        let DitherMode::NoiseShaped { order } = self.config.mode else {
            return Ok(0);
        };
        let coefficients = noise_shaper_coefficients(order)
            .ok_or(DspError::UnsupportedNoiseShapingOrder(order))?;
        coefficients
            .iter()
            .zip(self.quantization_error_history)
            .try_fold(0_i128, |sum, (&coefficient, error)| {
                error
                    .checked_mul(i128::from(coefficient))
                    .and_then(|term| sum.checked_add(term))
                    .ok_or(DspError::ExtendedSignalOverflow)
            })
    }

    /// Conservative absolute raw-Q1.63 excursion introduced by dither and
    /// noise-shaping feedback before final PCM quantization.
    pub fn maximum_excursion_raw(config: DitherConfig) -> Result<u128, DspError> {
        if !(1..=64).contains(&config.target_bits) {
            return Err(DspError::InvalidTargetBits(config.target_bits));
        }
        let step = 1_u128 << (64 - config.target_bits);
        match config.mode {
            DitherMode::None => Ok(0),
            DitherMode::Tpdf | DitherMode::HighPassTpdf => Ok(step - 1),
            DitherMode::NoiseShaped { order } => {
                let coefficients = noise_shaper_coefficients(order)
                    .ok_or(DspError::UnsupportedNoiseShapingOrder(order))?;
                let coefficient_l1 = coefficients
                    .iter()
                    .map(|coefficient| u128::from(coefficient.unsigned_abs()))
                    .sum::<u128>();
                let error_bound = match config.rounding {
                    RoundingMode::NearestTiesToEven | RoundingMode::NearestTiesAwayFromZero => {
                        step / 2
                    }
                    RoundingMode::TowardZero | RoundingMode::Floor | RoundingMode::Ceiling => {
                        step - 1
                    }
                };
                (step - 1)
                    .checked_add(
                        error_bound
                            .checked_mul(coefficient_l1)
                            .ok_or(DspError::ExtendedSignalOverflow)?,
                    )
                    .ok_or(DspError::ExtendedSignalOverflow)
            }
        }
    }
}

/// Error-feedback coefficients for `(1 - z^-1)^order - 1`, newest error first.
/// They are small exact integers, so the shaper adds no coefficient rounding.
fn noise_shaper_coefficients(order: u8) -> Option<&'static [i16]> {
    match order {
        1 => Some(&[-1]),
        5 => Some(&[-5, 10, -10, 5, -1]),
        9 => Some(&[-9, 36, -84, 126, -126, 84, -36, 9, -1]),
        _ => None,
    }
}

/// One deterministic quantizer per channel. Channel seeds are derived with the
/// same specified SplitMix64 permutation, so chunking never changes sequences.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterleavedDither {
    channels: u16,
    quantizers: Vec<DitherQuantizer>,
}

impl InterleavedDither {
    pub fn new(channels: u16, config: DitherConfig) -> Result<Self, DspError> {
        if channels == 0 {
            return Err(DspError::ZeroChannels);
        }
        let mut seed_deriver = SplitMix64::new(config.seed);
        let mut quantizers = Vec::with_capacity(usize::from(channels));
        for _ in 0..channels {
            quantizers.push(DitherQuantizer::new(DitherConfig {
                seed: seed_deriver.next(),
                ..config
            })?);
        }
        Ok(Self {
            channels,
            quantizers,
        })
    }

    pub fn quantize_interleaved_into(
        &mut self,
        input: &[Q1_63],
        output: &mut Vec<i64>,
    ) -> Result<(), DspError> {
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            return Err(DspError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        output
            .try_reserve(input.len())
            .map_err(|_| DspError::AllocationFailed)?;
        for frame in input.chunks_exact(channels) {
            for (channel, sample) in frame.iter().enumerate() {
                output.push(self.quantizers[channel].quantize(*sample)?);
            }
        }
        Ok(())
    }

    pub fn quantize_interleaved_wide_into(
        &mut self,
        input: &[WideQ63],
        output: &mut Vec<i64>,
    ) -> Result<(), DspError> {
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            return Err(DspError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        output
            .try_reserve(input.len())
            .map_err(|_| DspError::AllocationFailed)?;
        for frame in input.chunks_exact(channels) {
            for (channel, sample) in frame.iter().enumerate() {
                output.push(self.quantizers[channel].quantize_wide(*sample)?);
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> Result<DitherStats, DspError> {
        self.quantizers
            .iter()
            .try_fold(DitherStats::default(), |mut total, quantizer| {
                total.quantized_samples = total
                    .quantized_samples
                    .checked_add(quantizer.stats.quantized_samples)
                    .ok_or(DspError::CounterOverflow)?;
                total.saturated_samples = total
                    .saturated_samples
                    .checked_add(quantizer.stats.saturated_samples)
                    .ok_or(DspError::CounterOverflow)?;
                Ok(total)
            })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StageStats {
    pub input_samples: u64,
    pub output_samples: u64,
    pub saturated_samples: u64,
}

pub trait Stage {
    fn name(&self) -> &'static str;

    fn process(&mut self, input: &[Q1_63], output: &mut Vec<Q1_63>)
    -> Result<StageStats, DspError>;

    fn finish(&mut self, _output: &mut Vec<Q1_63>) -> Result<StageStats, DspError> {
        Ok(StageStats::default())
    }
}

/// Baseline streaming composition. It accepts variable-length stages, and its
/// drain order guarantees that each upstream tail flows through every
/// downstream stage before the downstream stage itself is finalized.
pub struct Pipeline {
    stages: Vec<Box<dyn Stage>>,
    stats: StageStats,
    poisoned: bool,
}

impl Pipeline {
    pub fn new(stages: Vec<Box<dyn Stage>>) -> Self {
        Self {
            stages,
            stats: StageStats::default(),
            poisoned: false,
        }
    }

    pub const fn stats(&self) -> StageStats {
        self.stats
    }

    pub fn process_chunk(
        &mut self,
        input: &[Q1_63],
        output: &mut Vec<Q1_63>,
    ) -> Result<(), DspError> {
        if self.poisoned {
            return Err(DspError::Poisoned);
        }
        let result =
            process_from(&mut self.stages, 0, input).and_then(|(processed, saturated_samples)| {
                output
                    .try_reserve(processed.len())
                    .map_err(|_| DspError::AllocationFailed)?;
                let next_stats = StageStats {
                    input_samples: self
                        .stats
                        .input_samples
                        .checked_add(input.len() as u64)
                        .ok_or(DspError::CounterOverflow)?,
                    output_samples: self
                        .stats
                        .output_samples
                        .checked_add(processed.len() as u64)
                        .ok_or(DspError::CounterOverflow)?,
                    saturated_samples: self
                        .stats
                        .saturated_samples
                        .checked_add(saturated_samples)
                        .ok_or(DspError::CounterOverflow)?,
                };
                output.extend(processed);
                self.stats = next_stats;
                Ok(())
            });
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    pub fn finish(mut self, output: &mut Vec<Q1_63>) -> Result<StageStats, DspError> {
        if self.poisoned {
            return Err(DspError::Poisoned);
        }
        let mut drained_output = Vec::new();
        for index in 0..self.stages.len() {
            let mut tail = Vec::new();
            let tail_stats = self.stages[index].finish(&mut tail)?;
            self.stats.saturated_samples = self
                .stats
                .saturated_samples
                .checked_add(tail_stats.saturated_samples)
                .ok_or(DspError::CounterOverflow)?;
            let (processed, downstream_saturations) =
                process_from(&mut self.stages, index + 1, &tail)?;
            self.stats.saturated_samples = self
                .stats
                .saturated_samples
                .checked_add(downstream_saturations)
                .ok_or(DspError::CounterOverflow)?;
            self.stats.output_samples = self
                .stats
                .output_samples
                .checked_add(processed.len() as u64)
                .ok_or(DspError::CounterOverflow)?;
            drained_output
                .try_reserve(processed.len())
                .map_err(|_| DspError::AllocationFailed)?;
            drained_output.extend(processed);
        }
        output
            .try_reserve(drained_output.len())
            .map_err(|_| DspError::AllocationFailed)?;
        output.extend(drained_output);
        Ok(self.stats)
    }
}

fn process_from(
    stages: &mut [Box<dyn Stage>],
    start: usize,
    input: &[Q1_63],
) -> Result<(Vec<Q1_63>, u64), DspError> {
    let mut current = Vec::new();
    current
        .try_reserve(input.len())
        .map_err(|_| DspError::AllocationFailed)?;
    current.extend_from_slice(input);
    let mut saturated_samples = 0_u64;
    for stage in &mut stages[start..] {
        let mut next = Vec::new();
        let stats = stage.process(&current, &mut next)?;
        saturated_samples = saturated_samples
            .checked_add(stats.saturated_samples)
            .ok_or(DspError::CounterOverflow)?;
        current = next;
    }
    Ok((current, saturated_samples))
}

pub struct GainStage {
    gain: Q2_62,
    rounding: RoundingMode,
    overflow: OverflowPolicy,
    poisoned: bool,
}

impl GainStage {
    pub const fn new(gain: Q2_62, rounding: RoundingMode, overflow: OverflowPolicy) -> Self {
        Self {
            gain,
            rounding,
            overflow,
            poisoned: false,
        }
    }
}

impl Stage for GainStage {
    fn name(&self) -> &'static str {
        "gain-q2.62"
    }

    fn process(
        &mut self,
        input: &[Q1_63],
        output: &mut Vec<Q1_63>,
    ) -> Result<StageStats, DspError> {
        if self.poisoned {
            return Err(DspError::Poisoned);
        }
        let result = (|| {
            let mut rendered = Vec::new();
            rendered
                .try_reserve(input.len())
                .map_err(|_| DspError::AllocationFailed)?;
            let mut saturated_samples = 0_u64;
            for sample in input {
                let mut accumulator = MacQ125::new();
                accumulator.accumulate(*sample, self.gain)?;
                let outcome = accumulator.finish(self.rounding, self.overflow)?;
                rendered.push(outcome.value);
                saturated_samples = saturated_samples
                    .checked_add(u64::from(outcome.saturated))
                    .ok_or(DspError::CounterOverflow)?;
            }
            output
                .try_reserve(rendered.len())
                .map_err(|_| DspError::AllocationFailed)?;
            output.extend(rendered);
            Ok(StageStats {
                input_samples: input.len() as u64,
                output_samples: input.len() as u64,
                saturated_samples,
            })
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

/// Streaming interleaved channel matrix. Coefficients are stored output-row
/// major: every consecutive `input_channels` values define one output channel.
/// Each output sample is accumulated exactly at Q3.125 and rounded once.
pub struct ChannelMixStage {
    input_channels: u16,
    output_channels: u16,
    matrix: Vec<Q2_62>,
    rounding: RoundingMode,
    overflow: OverflowPolicy,
    poisoned: bool,
}

impl ChannelMixStage {
    pub fn new(
        input_channels: u16,
        output_channels: u16,
        matrix: Vec<Q2_62>,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, DspError> {
        if input_channels == 0 {
            return Err(DspError::ZeroChannels);
        }
        if output_channels == 0 {
            return Err(DspError::ZeroOutputChannels);
        }
        let expected = usize::from(input_channels)
            .checked_mul(usize::from(output_channels))
            .ok_or(DspError::AllocationFailed)?;
        if matrix.len() != expected {
            return Err(DspError::MixMatrixSize {
                expected,
                actual: matrix.len(),
            });
        }
        for row in matrix.chunks_exact(usize::from(input_channels)) {
            validate_native_mac(row)?;
        }
        Ok(Self {
            input_channels,
            output_channels,
            matrix,
            rounding,
            overflow,
            poisoned: false,
        })
    }

    pub const fn input_channels(&self) -> u16 {
        self.input_channels
    }

    pub const fn output_channels(&self) -> u16 {
        self.output_channels
    }
}

impl Stage for ChannelMixStage {
    fn name(&self) -> &'static str {
        "channel-mix-q2.62"
    }

    fn process(
        &mut self,
        input: &[Q1_63],
        output: &mut Vec<Q1_63>,
    ) -> Result<StageStats, DspError> {
        if self.poisoned {
            return Err(DspError::Poisoned);
        }
        let input_channels = usize::from(self.input_channels);
        if !input.len().is_multiple_of(input_channels) {
            self.poisoned = true;
            return Err(DspError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.input_channels,
            });
        }
        let frames = input.len() / input_channels;
        let output_samples = frames
            .checked_mul(usize::from(self.output_channels))
            .ok_or(DspError::AllocationFailed)?;
        let result = (|| {
            let mut rendered = Vec::new();
            rendered
                .try_reserve(output_samples)
                .map_err(|_| DspError::AllocationFailed)?;
            let mut saturated_samples = 0_u64;
            for frame in input.chunks_exact(input_channels) {
                for row in self.matrix.chunks_exact(input_channels) {
                    let mut accumulator = MacQ125::new();
                    for (sample, coefficient) in frame.iter().zip(row) {
                        accumulator.accumulate(*sample, *coefficient)?;
                    }
                    let outcome = accumulator.finish(self.rounding, self.overflow)?;
                    rendered.push(outcome.value);
                    saturated_samples = saturated_samples
                        .checked_add(u64::from(outcome.saturated))
                        .ok_or(DspError::CounterOverflow)?;
                }
            }
            output
                .try_reserve(rendered.len())
                .map_err(|_| DspError::AllocationFailed)?;
            output.extend(rendered);
            Ok(StageStats {
                input_samples: input.len() as u64,
                output_samples: output_samples as u64,
                saturated_samples,
            })
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

/// First-order streaming DC blocker:
/// `y[n] = x[n] - x[n-1] + radius * y[n-1]`.
///
/// The radius is constrained to `[0, 1)`, which keeps the recursive pole
/// stable. The three products are accumulated exactly and rounded once.
pub struct DcBlockStage {
    channels: u16,
    radius: Q2_62,
    rounding: RoundingMode,
    overflow: OverflowPolicy,
    previous_input: Vec<Q1_63>,
    previous_output: Vec<Q1_63>,
    poisoned: bool,
}

impl DcBlockStage {
    pub fn new(
        channels: u16,
        radius: Q2_62,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<Self, DspError> {
        if channels == 0 {
            return Err(DspError::ZeroChannels);
        }
        if !(0..Q2_62::ONE.raw()).contains(&radius.raw()) {
            return Err(DspError::InvalidDcRadius(radius.raw()));
        }
        validate_native_mac(&[Q2_62::ONE, Q2_62::from_raw(-Q2_62::ONE.raw()), radius])?;
        Ok(Self {
            channels,
            radius,
            rounding,
            overflow,
            previous_input: vec![Q1_63::ZERO; usize::from(channels)],
            previous_output: vec![Q1_63::ZERO; usize::from(channels)],
            poisoned: false,
        })
    }

    pub const fn channels(&self) -> u16 {
        self.channels
    }
}

impl Stage for DcBlockStage {
    fn name(&self) -> &'static str {
        "dc-block-q2.62"
    }

    fn process(
        &mut self,
        input: &[Q1_63],
        output: &mut Vec<Q1_63>,
    ) -> Result<StageStats, DspError> {
        if self.poisoned {
            return Err(DspError::Poisoned);
        }
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            self.poisoned = true;
            return Err(DspError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        let result = (|| {
            let mut previous_input = self.previous_input.clone();
            let mut previous_output = self.previous_output.clone();
            let mut rendered = Vec::new();
            rendered
                .try_reserve(input.len())
                .map_err(|_| DspError::AllocationFailed)?;
            let mut saturated_samples = 0_u64;
            for frame in input.chunks_exact(channels) {
                for (channel, sample) in frame.iter().enumerate() {
                    let mut accumulator = MacQ125::new();
                    accumulator.accumulate(*sample, Q2_62::ONE)?;
                    accumulator
                        .accumulate(previous_input[channel], Q2_62::from_raw(-Q2_62::ONE.raw()))?;
                    accumulator.accumulate(previous_output[channel], self.radius)?;
                    let outcome = accumulator.finish(self.rounding, self.overflow)?;
                    previous_input[channel] = *sample;
                    previous_output[channel] = outcome.value;
                    rendered.push(outcome.value);
                    saturated_samples = saturated_samples
                        .checked_add(u64::from(outcome.saturated))
                        .ok_or(DspError::CounterOverflow)?;
                }
            }
            output
                .try_reserve(rendered.len())
                .map_err(|_| DspError::AllocationFailed)?;
            output.extend(rendered);
            self.previous_input = previous_input;
            self.previous_output = previous_output;
            Ok(StageStats {
                input_samples: input.len() as u64,
                output_samples: input.len() as u64,
                saturated_samples,
            })
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

/// Causal streaming FIR convolution with independent history per interleaved
/// channel. `taps[0]` multiplies the current sample. Optional tail draining
/// appends exactly `taps.len() - 1` zero-input frames during `finish`.
pub struct ConvolutionStage {
    channels: u16,
    taps: Vec<Q2_62>,
    rounding: RoundingMode,
    overflow: OverflowPolicy,
    histories: Vec<VecDeque<Q1_63>>,
    drain_tail: bool,
    saw_input: bool,
    drained: bool,
    poisoned: bool,
}

impl ConvolutionStage {
    pub fn new(
        channels: u16,
        taps: Vec<Q2_62>,
        rounding: RoundingMode,
        overflow: OverflowPolicy,
        drain_tail: bool,
    ) -> Result<Self, DspError> {
        if channels == 0 {
            return Err(DspError::ZeroChannels);
        }
        if taps.is_empty() {
            return Err(DspError::ZeroTaps);
        }
        validate_native_mac(&taps)?;
        Ok(Self {
            channels,
            taps,
            rounding,
            overflow,
            histories: vec![VecDeque::new(); usize::from(channels)],
            drain_tail,
            saw_input: false,
            drained: false,
            poisoned: false,
        })
    }

    pub const fn channels(&self) -> u16 {
        self.channels
    }

    pub fn taps(&self) -> &[Q2_62] {
        &self.taps
    }

    fn process_internal(
        &mut self,
        input: &[Q1_63],
        output: &mut Vec<Q1_63>,
        mark_real_input: bool,
    ) -> Result<StageStats, DspError> {
        let channels = usize::from(self.channels);
        if !input.len().is_multiple_of(channels) {
            return Err(DspError::PartialInterleavedFrame {
                samples: input.len(),
                channels: self.channels,
            });
        }
        let mut histories = self.histories.clone();
        let mut rendered = Vec::new();
        rendered
            .try_reserve(input.len())
            .map_err(|_| DspError::AllocationFailed)?;
        let mut saturated_samples = 0_u64;
        for frame in input.chunks_exact(channels) {
            for (channel, sample) in frame.iter().enumerate() {
                histories[channel].push_front(*sample);
                if histories[channel].len() > self.taps.len() {
                    histories[channel].pop_back();
                }
                let mut accumulator = MacQ125::new();
                for (history_sample, coefficient) in histories[channel].iter().zip(&self.taps) {
                    accumulator.accumulate(*history_sample, *coefficient)?;
                }
                let outcome = accumulator.finish(self.rounding, self.overflow)?;
                rendered.push(outcome.value);
                saturated_samples = saturated_samples
                    .checked_add(u64::from(outcome.saturated))
                    .ok_or(DspError::CounterOverflow)?;
            }
        }
        output
            .try_reserve(rendered.len())
            .map_err(|_| DspError::AllocationFailed)?;
        output.extend(rendered);
        self.histories = histories;
        if mark_real_input && !input.is_empty() {
            self.saw_input = true;
        }
        Ok(StageStats {
            input_samples: input.len() as u64,
            output_samples: input.len() as u64,
            saturated_samples,
        })
    }
}

impl Stage for ConvolutionStage {
    fn name(&self) -> &'static str {
        "convolution-q2.62"
    }

    fn process(
        &mut self,
        input: &[Q1_63],
        output: &mut Vec<Q1_63>,
    ) -> Result<StageStats, DspError> {
        if self.poisoned || self.drained {
            return Err(DspError::Poisoned);
        }
        let result = self.process_internal(input, output, true);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn finish(&mut self, output: &mut Vec<Q1_63>) -> Result<StageStats, DspError> {
        if self.poisoned {
            return Err(DspError::Poisoned);
        }
        if self.drained {
            return Ok(StageStats::default());
        }
        self.drained = true;
        if !self.drain_tail || !self.saw_input || self.taps.len() == 1 {
            return Ok(StageStats::default());
        }
        let tail_samples = (self.taps.len() - 1)
            .checked_mul(usize::from(self.channels))
            .ok_or(DspError::AllocationFailed)?;
        let mut zeros = Vec::new();
        zeros
            .try_reserve(tail_samples)
            .map_err(|_| DspError::AllocationFailed)?;
        zeros.resize(tail_samples, Q1_63::ZERO);
        let result = self
            .process_internal(&zeros, output, false)
            .map(|mut stats| {
                stats.input_samples = 0;
                stats
            });
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

fn validate_native_mac(coefficients: &[Q2_62]) -> Result<(), DspError> {
    let requirements = MacQ125::requirements_for(coefficients)?;
    if requirements.signed_bits > i128::BITS as u16 {
        return Err(DspError::AccumulatorTooNarrow {
            required_bits: requirements.signed_bits,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DspError {
    InvalidTargetBits(u32),
    UnsupportedNoiseShapingOrder(u8),
    UnknownDitherMode(String),
    UnknownClipPolicy(String),
    ClipPolicyRequiresWidePipeline(ClipPolicy),
    ZeroChannels,
    ZeroOutputChannels,
    ZeroTaps,
    InvalidDcRadius(i64),
    MixMatrixSize { expected: usize, actual: usize },
    AccumulatorTooNarrow { required_bits: u16 },
    PartialInterleavedFrame { samples: usize, channels: u16 },
    ExtendedSignalOverflow,
    CounterOverflow,
    AllocationFailed,
    Poisoned,
    Arithmetic(ArithmeticError),
}

impl fmt::Display for DspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTargetBits(bits) => {
                write!(f, "dither target width must be in 1..=64, got {bits}")
            }
            Self::UnsupportedNoiseShapingOrder(order) => write!(
                f,
                "noise-shaping order {order} is unsupported; expected 1, 5, or 9"
            ),
            Self::UnknownDitherMode(mode) => write!(
                f,
                "unknown dither mode {mode:?}; expected none, tpdf, high-pass-tpdf, noise-shaped-1, noise-shaped-5, or noise-shaped-9"
            ),
            Self::UnknownClipPolicy(policy) => write!(
                f,
                "unknown clipping policy {policy:?}; expected saturate, error, normalize, or allow-headroom"
            ),
            Self::ClipPolicyRequiresWidePipeline(policy) => write!(
                f,
                "clipping policy {policy} requires a wider-than-Q1.63 processing path"
            ),
            Self::ZeroChannels => f.write_str("interleaved DSP requires at least one channel"),
            Self::ZeroOutputChannels => {
                f.write_str("channel mixer requires at least one output channel")
            }
            Self::ZeroTaps => f.write_str("convolution stage requires at least one tap"),
            Self::InvalidDcRadius(raw) => write!(
                f,
                "DC blocker radius raw Q2.62 value must be in [0, 2^62), got {raw}"
            ),
            Self::MixMatrixSize { expected, actual } => write!(
                f,
                "channel mix matrix needs {expected} coefficients, got {actual}"
            ),
            Self::AccumulatorTooNarrow { required_bits } => write!(
                f,
                "DSP coefficient row requires a {required_bits}-bit accumulator; native limit is 128"
            ),
            Self::PartialInterleavedFrame { samples, channels } => write!(
                f,
                "chunk has {samples} samples, not a multiple of {channels} channels"
            ),
            Self::ExtendedSignalOverflow => {
                f.write_str("extended Q1.63 signal arithmetic overflow")
            }
            Self::CounterOverflow => f.write_str("DSP statistics counter overflow"),
            Self::AllocationFailed => f.write_str("DSP output allocation failed"),
            Self::Poisoned => f.write_str("pipeline cannot continue after an earlier error"),
            Self::Arithmetic(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DspError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Arithmetic(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ArithmeticError> for DspError {
    fn from(value: ArithmeticError) -> Self {
        Self::Arithmetic(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(mode: DitherMode, seed: u64) -> DitherConfig {
        DitherConfig {
            target_bits: 16,
            mode,
            seed,
            rounding: RoundingMode::NearestTiesToEven,
            overflow: OverflowPolicy::Saturate,
        }
    }

    fn quantized(mode: DitherMode, seed: u64, input: &[Q1_63]) -> Vec<i64> {
        let mut quantizer = DitherQuantizer::new(config(mode, seed)).unwrap();
        input
            .iter()
            .map(|sample| quantizer.quantize(*sample).unwrap())
            .collect()
    }

    #[test]
    fn seeded_dither_is_bit_reproducible_and_mode_specific() {
        assert_eq!("tpdf".parse::<DitherMode>(), Ok(DitherMode::Tpdf));
        assert_eq!(
            "high-pass".parse::<DitherMode>(),
            Ok(DitherMode::HighPassTpdf)
        );
        assert!("magic".parse::<DitherMode>().is_err());
        let input = vec![Q1_63::ZERO; 256];
        let first = quantized(DitherMode::Tpdf, 0x1234, &input);
        assert_eq!(first, quantized(DitherMode::Tpdf, 0x1234, &input));
        assert_ne!(first, quantized(DitherMode::Tpdf, 0x1235, &input));
        assert_ne!(first, quantized(DitherMode::HighPassTpdf, 0x1234, &input));
        assert!(first.iter().all(|sample| (-1..=1).contains(sample)));
    }

    #[test]
    fn clipping_policies_never_silently_degrade() {
        assert_eq!(
            "saturate"
                .parse::<ClipPolicy>()
                .unwrap()
                .native_overflow_policy(),
            Ok(OverflowPolicy::Saturate)
        );
        assert_eq!(
            "error"
                .parse::<ClipPolicy>()
                .unwrap()
                .native_overflow_policy(),
            Ok(OverflowPolicy::Error)
        );
        assert!(
            "normalize"
                .parse::<ClipPolicy>()
                .unwrap()
                .native_overflow_policy()
                .is_err()
        );
        assert!("wrap".parse::<ClipPolicy>().is_err());
    }

    #[test]
    fn no_dither_matches_plain_pcm_quantization() {
        let input = [
            Q1_63::MIN,
            Q1_63::from_raw(-1),
            Q1_63::ZERO,
            Q1_63::from_raw(1),
            Q1_63::MAX,
        ];
        let output = quantized(DitherMode::None, 99, &input);
        let expected = input
            .iter()
            .map(|sample| {
                sample
                    .to_signed_pcm(
                        16,
                        RoundingMode::NearestTiesToEven,
                        OverflowPolicy::Saturate,
                    )
                    .unwrap()
                    .value
            })
            .collect::<Vec<_>>();
        assert_eq!(output, expected);
    }

    #[test]
    fn noise_shapers_are_deterministic_distinct_and_validated() {
        let input = (0..512)
            .map(|index| Q1_63::from_raw(i64::from(index) << 35))
            .collect::<Vec<_>>();
        let order_one = quantized(DitherMode::NoiseShaped { order: 1 }, 7, &input);
        assert_eq!(
            order_one,
            quantized(DitherMode::NoiseShaped { order: 1 }, 7, &input)
        );
        let order_five = quantized(DitherMode::NoiseShaped { order: 5 }, 7, &input);
        let order_nine = quantized(DitherMode::NoiseShaped { order: 9 }, 7, &input);
        assert_ne!(order_one, quantized(DitherMode::Tpdf, 7, &input));
        assert_ne!(order_one, order_five);
        assert_ne!(order_five, order_nine);
        assert_eq!(
            DitherQuantizer::new(config(DitherMode::NoiseShaped { order: 2 }, 7)),
            Err(DspError::UnsupportedNoiseShapingOrder(2))
        );
        let config = config(DitherMode::NoiseShaped { order: 5 }, 7);
        let step = 1_u128 << 48;
        assert_eq!(
            DitherQuantizer::maximum_excursion_raw(config).unwrap(),
            step - 1 + 31 * (step / 2)
        );
    }

    #[test]
    fn wide_quantization_defers_clipping_to_the_pcm_boundary() {
        let beyond_positive = WideQ63::from_raw(i128::from(i64::MAX) + (1_i128 << 50));
        let mut saturating = DitherQuantizer::new(config(DitherMode::None, 0)).unwrap();
        assert_eq!(
            saturating.quantize_wide(beyond_positive).unwrap(),
            i64::from(i16::MAX)
        );
        assert_eq!(saturating.stats().saturated_samples, 1);

        let mut failing = DitherQuantizer::new(DitherConfig {
            overflow: OverflowPolicy::Error,
            ..config(DitherMode::None, 0)
        })
        .unwrap();
        assert!(failing.quantize_wide(beyond_positive).is_err());
    }

    #[test]
    fn saturation_is_counted_and_error_policy_is_honored() {
        let mut saturating = DitherQuantizer::new(config(DitherMode::Tpdf, 1)).unwrap();
        for _ in 0..32 {
            saturating.quantize(Q1_63::MAX).unwrap();
        }
        assert!(saturating.stats().saturated_samples > 0);

        let mut failing = DitherQuantizer::new(DitherConfig {
            overflow: OverflowPolicy::Error,
            ..config(DitherMode::Tpdf, 1)
        })
        .unwrap();
        let result = (0..32).try_for_each(|_| failing.quantize(Q1_63::MAX).map(|_| ()));
        assert!(result.is_err());
        assert_eq!(failing.quantize(Q1_63::ZERO), Err(DspError::Poisoned));
    }

    #[test]
    fn interleaved_channels_have_independent_chunk_stable_sequences() {
        let input = vec![Q1_63::ZERO; 400];
        let run = |chunks: &[&[Q1_63]]| {
            let mut dither =
                InterleavedDither::new(2, config(DitherMode::HighPassTpdf, 42)).unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                dither
                    .quantize_interleaved_into(chunk, &mut output)
                    .unwrap();
            }
            (output, dither.stats().unwrap())
        };
        assert_eq!(run(&[&input]), run(&[&input[..100], &input[100..]]));
        assert!(
            InterleavedDither::new(2, config(DitherMode::Tpdf, 1))
                .unwrap()
                .quantize_interleaved_into(&input[..3], &mut Vec::new())
                .is_err()
        );
    }

    #[test]
    fn pipeline_applies_gain_and_preserves_chunk_equivalence() {
        let input = (0..101)
            .map(|index| Q1_63::from_raw(i64::from(index) * 1_000_003 - 50_000_150))
            .collect::<Vec<_>>();
        let run = |chunks: &[&[Q1_63]]| {
            let stage = GainStage::new(
                Q2_62::from_raw(1_i64 << 61),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            );
            let mut pipeline = Pipeline::new(vec![Box::new(stage)]);
            let mut output = Vec::new();
            for chunk in chunks {
                pipeline.process_chunk(chunk, &mut output).unwrap();
            }
            let stats = pipeline.finish(&mut output).unwrap();
            (output, stats)
        };
        assert_eq!(
            run(&[&input]),
            run(&[&input[..1], &input[1..37], &input[37..]])
        );
    }

    #[test]
    fn gain_failure_does_not_append_a_partial_block() {
        let mut gain = GainStage::new(
            Q2_62::from_raw(3_i64 << 61),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        );
        let mut output = vec![Q1_63::ZERO];
        assert!(
            gain.process(&[Q1_63::ZERO, Q1_63::from_raw(3_i64 << 61)], &mut output,)
                .is_err()
        );
        assert_eq!(output, vec![Q1_63::ZERO]);
        assert_eq!(
            gain.process(&[Q1_63::ZERO], &mut output),
            Err(DspError::Poisoned)
        );
    }

    #[test]
    fn channel_mix_is_exact_and_chunk_stable() {
        let half = Q2_62::from_raw(1_i64 << 61);
        let input = [
            Q1_63::from_raw(1_i64 << 62),
            Q1_63::from_raw(-(1_i64 << 61)),
            Q1_63::from_raw(-(1_i64 << 62)),
            Q1_63::from_raw(1_i64 << 61),
        ];
        let run = |chunks: &[&[Q1_63]]| {
            let mut mixer = ChannelMixStage::new(
                2,
                1,
                vec![half, half],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            let mut stats = StageStats::default();
            for chunk in chunks {
                let chunk_stats = mixer.process(chunk, &mut output).unwrap();
                stats.input_samples += chunk_stats.input_samples;
                stats.output_samples += chunk_stats.output_samples;
            }
            (output, stats)
        };
        let expected = vec![
            Q1_63::from_raw(1_i64 << 60),
            Q1_63::from_raw(-(1_i64 << 60)),
        ];
        assert_eq!(run(&[&input]).0, expected);
        assert_eq!(run(&[&input]), run(&[&input[..2], &input[2..]]));
    }

    #[test]
    fn channel_mix_rejects_invalid_layout_and_rolls_back_failed_blocks() {
        assert!(matches!(
            ChannelMixStage::new(
                2,
                1,
                vec![Q2_62::ONE],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            ),
            Err(DspError::MixMatrixSize {
                expected: 2,
                actual: 1
            })
        ));
        assert!(matches!(
            ChannelMixStage::new(
                2,
                1,
                vec![Q2_62::MIN, Q2_62::MIN],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            ),
            Err(DspError::AccumulatorTooNarrow { required_bits: 129 })
        ));

        let mut mixer = ChannelMixStage::new(
            2,
            1,
            vec![Q2_62::ONE, Q2_62::ONE],
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )
        .unwrap();
        let mut output = vec![Q1_63::ZERO];
        let loud = Q1_63::from_raw(3_i64 << 61);
        assert!(mixer.process(&[loud, loud], &mut output).is_err());
        assert_eq!(output, vec![Q1_63::ZERO]);
        assert_eq!(
            mixer.process(&[Q1_63::ZERO, Q1_63::ZERO], &mut output),
            Err(DspError::Poisoned)
        );
    }

    #[test]
    fn dc_blocker_is_per_channel_and_chunk_stable() {
        let input = [
            Q1_63::from_raw(1_i64 << 61),
            Q1_63::ZERO,
            Q1_63::from_raw(1_i64 << 61),
            Q1_63::ZERO,
            Q1_63::from_raw(1_i64 << 61),
            Q1_63::ZERO,
        ];
        let run = |chunks: &[&[Q1_63]]| {
            let mut blocker = DcBlockStage::new(
                2,
                Q2_62::from_raw(1_i64 << 61),
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let mut output = Vec::new();
            for chunk in chunks {
                blocker.process(chunk, &mut output).unwrap();
            }
            output
        };
        let expected = vec![
            Q1_63::from_raw(1_i64 << 61),
            Q1_63::ZERO,
            Q1_63::from_raw(1_i64 << 60),
            Q1_63::ZERO,
            Q1_63::from_raw(1_i64 << 59),
            Q1_63::ZERO,
        ];
        assert_eq!(run(&[&input]), expected);
        assert_eq!(
            run(&[&input]),
            run(&[&input[..2], &input[2..4], &input[4..]])
        );
        assert!(matches!(
            DcBlockStage::new(
                1,
                Q2_62::ONE,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            ),
            Err(DspError::InvalidDcRadius(_))
        ));
    }

    #[test]
    fn convolution_emits_exact_impulse_response_and_tail() {
        let taps = vec![
            Q2_62::ONE,
            Q2_62::from_raw(1_i64 << 61),
            Q2_62::from_raw(-(1_i64 << 60)),
        ];
        let mut convolution = ConvolutionStage::new(
            1,
            taps,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
            true,
        )
        .unwrap();
        let mut output = Vec::new();
        convolution
            .process(&[Q1_63::from_raw(1_i64 << 62)], &mut output)
            .unwrap();
        let tail_stats = convolution.finish(&mut output).unwrap();
        assert_eq!(
            output,
            vec![
                Q1_63::from_raw(1_i64 << 62),
                Q1_63::from_raw(1_i64 << 61),
                Q1_63::from_raw(-(1_i64 << 60)),
            ]
        );
        assert_eq!(tail_stats.input_samples, 0);
        assert_eq!(tail_stats.output_samples, 2);
        assert_eq!(convolution.finish(&mut output), Ok(StageStats::default()));
    }

    #[test]
    fn convolution_and_variable_channel_pipeline_are_chunk_stable() {
        let input = [
            Q1_63::from_raw(1_i64 << 61),
            Q1_63::from_raw(1_i64 << 61),
            Q1_63::from_raw(-(1_i64 << 61)),
            Q1_63::from_raw(-(1_i64 << 61)),
        ];
        let run = |chunks: &[&[Q1_63]]| {
            let mixer = ChannelMixStage::new(
                2,
                1,
                vec![Q2_62::from_raw(1_i64 << 61), Q2_62::from_raw(1_i64 << 61)],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
            let convolution = ConvolutionStage::new(
                1,
                vec![Q2_62::ONE, Q2_62::from_raw(1_i64 << 61)],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
                true,
            )
            .unwrap();
            let mut pipeline = Pipeline::new(vec![Box::new(mixer), Box::new(convolution)]);
            let mut output = Vec::new();
            for chunk in chunks {
                pipeline.process_chunk(chunk, &mut output).unwrap();
            }
            let stats = pipeline.finish(&mut output).unwrap();
            (output, stats)
        };
        let whole = run(&[&input]);
        assert_eq!(whole, run(&[&input[..2], &input[2..]]));
        assert_eq!(
            whole.0,
            vec![
                Q1_63::from_raw(1_i64 << 61),
                Q1_63::from_raw(-(1_i64 << 60)),
                Q1_63::from_raw(-(1_i64 << 60)),
            ]
        );
        assert_eq!(whole.1.input_samples, 4);
        assert_eq!(whole.1.output_samples, 3);
    }

    #[test]
    fn pipeline_propagates_upstream_tail_through_downstream_stages() {
        struct TailStage(Q1_63);

        impl Stage for TailStage {
            fn name(&self) -> &'static str {
                "test-tail"
            }

            fn process(
                &mut self,
                input: &[Q1_63],
                output: &mut Vec<Q1_63>,
            ) -> Result<StageStats, DspError> {
                output.extend_from_slice(input);
                Ok(StageStats {
                    input_samples: input.len() as u64,
                    output_samples: input.len() as u64,
                    saturated_samples: 0,
                })
            }

            fn finish(&mut self, output: &mut Vec<Q1_63>) -> Result<StageStats, DspError> {
                output.push(self.0);
                Ok(StageStats {
                    input_samples: 0,
                    output_samples: 1,
                    saturated_samples: 0,
                })
            }
        }

        let full_scale_half = Q1_63::from_raw(1_i64 << 62);
        let gain_half = GainStage::new(
            Q2_62::from_raw(1_i64 << 61),
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        );
        let pipeline = Pipeline::new(vec![
            Box::new(TailStage(full_scale_half)),
            Box::new(gain_half),
        ]);
        let mut output = Vec::new();
        pipeline.finish(&mut output).unwrap();
        assert_eq!(output, vec![Q1_63::from_raw(1_i64 << 61)]);
    }
}
