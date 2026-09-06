//! Runtime-format BigQ stages with exact GMP products and accumulators.
//! Signal history and completed outputs retain the declared Qm.n format.
//! PCM clipping and precision changes are explicit downstream boundaries.

use crate::{DspError, StageStats};
use sexq::{BigMac, BigQ, BigQError, BigQFormat, OverflowPolicy, RoundingMode};
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BigDspError {
    Dsp(DspError),
    Arithmetic(BigQError),
    InvalidRadius,
}

impl fmt::Display for BigDspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dsp(error) => error.fmt(f),
            Self::Arithmetic(error) => error.fmt(f),
            Self::InvalidRadius => f.write_str("DC radius must be in [0, 1)"),
        }
    }
}

impl std::error::Error for BigDspError {}
impl From<DspError> for BigDspError {
    fn from(error: DspError) -> Self {
        Self::Dsp(error)
    }
}
impl From<BigQError> for BigDspError {
    fn from(error: BigQError) -> Self {
        Self::Arithmetic(error)
    }
}

#[derive(Clone, Debug)]
struct Row {
    coefficients: Arc<[BigQ]>,
    signal_format: BigQFormat,
    format: BigQFormat,
    accumulator_bits: u32,
}

impl Row {
    fn new(signal_format: BigQFormat, coefficients: Vec<BigQ>) -> Result<Self, BigDspError> {
        let format = coefficients.first().ok_or(DspError::ZeroTaps)?.format();
        let requirements = BigMac::requirements_for(signal_format, format, &coefficients)?;
        BigMac::new(signal_format, format, Some(requirements.signed_bits))?;
        Ok(Self {
            coefficients: coefficients.into(),
            signal_format,
            format,
            accumulator_bits: requirements.signed_bits,
        })
    }

    fn evaluate<'a>(
        &self,
        samples: impl IntoIterator<Item = &'a BigQ>,
        rounding: RoundingMode,
    ) -> Result<BigQ, BigDspError> {
        let mut mac = BigMac::new(self.signal_format, self.format, Some(self.accumulator_bits))?;
        for (sample, coefficient) in samples.into_iter().zip(self.coefficients.iter()) {
            mac.accumulate(sample, coefficient)?;
        }
        Ok(mac
            .finish(self.signal_format, rounding, OverflowPolicy::Error)?
            .value)
    }
}

#[derive(Clone, Debug)]
enum Kind {
    Gain(Row),
    Mix {
        rows: Vec<Row>,
    },
    Dc {
        row: Row,
        previous_input: Vec<BigQ>,
        previous_output: Vec<BigQ>,
    },
    Convolution {
        row: Row,
        history: Vec<VecDeque<BigQ>>,
        saw_input: bool,
    },
}

/// A validated stage with a specified input/output channel layout.
#[derive(Clone, Debug)]
pub struct BigStage {
    signal_format: BigQFormat,
    input_channels: u16,
    output_channels: u16,
    rounding: RoundingMode,
    kind: Kind,
}

impl BigStage {
    pub fn gain(
        channels: u16,
        signal_format: BigQFormat,
        gain: BigQ,
        rounding: RoundingMode,
    ) -> Result<Self, BigDspError> {
        check_channels(channels)?;
        Ok(Self {
            signal_format,
            input_channels: channels,
            output_channels: channels,
            rounding,
            kind: Kind::Gain(Row::new(signal_format, vec![gain])?),
        })
    }

    pub fn mix(
        input_channels: u16,
        output_channels: u16,
        signal_format: BigQFormat,
        matrix: Vec<BigQ>,
        rounding: RoundingMode,
    ) -> Result<Self, BigDspError> {
        check_channels(input_channels)?;
        check_channels(output_channels)?;
        let expected = usize::from(input_channels)
            .checked_mul(usize::from(output_channels))
            .ok_or(DspError::AllocationFailed)?;
        if matrix.len() != expected {
            return Err(DspError::MixMatrixSize {
                expected,
                actual: matrix.len(),
            }
            .into());
        }
        let rows = matrix
            .chunks_exact(usize::from(input_channels))
            .map(|row| Row::new(signal_format, row.to_vec()))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            signal_format,
            input_channels,
            output_channels,
            rounding,
            kind: Kind::Mix { rows },
        })
    }

    pub fn dc_block(
        channels: u16,
        signal_format: BigQFormat,
        radius: BigQ,
        rounding: RoundingMode,
    ) -> Result<Self, BigDspError> {
        check_channels(channels)?;
        let format = BigQFormat::new(
            radius.format().integer_bits().max(2),
            radius.format().fractional_bits(),
        )?;
        let radius = radius
            .rescale(format, rounding, OverflowPolicy::Error)?
            .value;
        let one = BigQ::one(format)?;
        if radius.raw() < &0 || radius.raw() >= one.raw() {
            return Err(BigDspError::InvalidRadius);
        }
        let minus_one = BigQ::from_raw(-one.raw().clone(), format)?;
        let row = Row::new(signal_format, vec![one, minus_one, radius])?;
        Ok(Self {
            signal_format,
            input_channels: channels,
            output_channels: channels,
            rounding,
            kind: Kind::Dc {
                row,
                previous_input: vec![BigQ::zero(signal_format); usize::from(channels)],
                previous_output: vec![BigQ::zero(signal_format); usize::from(channels)],
            },
        })
    }

    pub fn convolution(
        channels: u16,
        signal_format: BigQFormat,
        taps: Vec<BigQ>,
        rounding: RoundingMode,
    ) -> Result<Self, BigDspError> {
        check_channels(channels)?;
        Ok(Self {
            signal_format,
            input_channels: channels,
            output_channels: channels,
            rounding,
            kind: Kind::Convolution {
                row: Row::new(signal_format, taps)?,
                history: vec![VecDeque::new(); usize::from(channels)],
                saw_input: false,
            },
        })
    }

    pub const fn signal_format(&self) -> BigQFormat {
        self.signal_format
    }

    pub const fn input_channels(&self) -> u16 {
        self.input_channels
    }
    pub const fn output_channels(&self) -> u16 {
        self.output_channels
    }

    /// Conservative width proved from the declared signal range and coefficient L1.
    pub fn accumulator_bits(&self) -> u32 {
        match &self.kind {
            Kind::Gain(row) | Kind::Dc { row, .. } | Kind::Convolution { row, .. } => {
                row.accumulator_bits
            }
            Kind::Mix { rows } => rows
                .iter()
                .map(|row| row.accumulator_bits)
                .max()
                .unwrap_or(1),
        }
    }

    fn process(&mut self, input: &[BigQ]) -> Result<Vec<BigQ>, BigDspError> {
        check_frames(input.len(), self.input_channels)?;
        let frames = input.len() / usize::from(self.input_channels);
        let count = frames
            .checked_mul(usize::from(self.output_channels))
            .ok_or(DspError::AllocationFailed)?;
        let mut output = Vec::new();
        output
            .try_reserve(count)
            .map_err(|_| DspError::AllocationFailed)?;
        match &mut self.kind {
            Kind::Gain(row) => {
                for sample in input {
                    output.push(row.evaluate([sample], self.rounding)?);
                }
            }
            Kind::Mix { rows } => {
                for frame in input.chunks_exact(usize::from(self.input_channels)) {
                    for row in rows.iter() {
                        output.push(row.evaluate(frame.iter(), self.rounding)?);
                    }
                }
            }
            Kind::Dc {
                row,
                previous_input,
                previous_output,
            } => {
                for frame in input.chunks_exact(usize::from(self.input_channels)) {
                    for (channel, sample) in frame.iter().enumerate() {
                        let value = row.evaluate(
                            [sample, &previous_input[channel], &previous_output[channel]],
                            self.rounding,
                        )?;
                        previous_input[channel] = sample.clone();
                        previous_output[channel] = value.clone();
                        output.push(value);
                    }
                }
            }
            Kind::Convolution {
                row,
                history,
                saw_input,
            } => {
                *saw_input |= !input.is_empty();
                for frame in input.chunks_exact(usize::from(self.input_channels)) {
                    for (channel, sample) in frame.iter().enumerate() {
                        history[channel].push_front(sample.clone());
                        history[channel].truncate(row.coefficients.len());
                        output.push(row.evaluate(history[channel].iter(), self.rounding)?);
                    }
                }
            }
        }
        Ok(output)
    }

    fn finish(&mut self) -> Result<Vec<BigQ>, BigDspError> {
        let frames = match &self.kind {
            Kind::Convolution {
                row,
                saw_input: true,
                ..
            } => row.coefficients.len() - 1,
            _ => 0,
        };
        let count = frames
            .checked_mul(usize::from(self.input_channels))
            .ok_or(DspError::AllocationFailed)?;
        let mut zeros = Vec::new();
        zeros
            .try_reserve(count)
            .map_err(|_| DspError::AllocationFailed)?;
        zeros.resize(count, BigQ::zero(self.signal_format));
        self.process(&zeros)
    }
}

/// Streaming pipeline. Each call commits state and output together, or
/// appends nothing and poisons the pipeline. All stage tails flow downstream.
#[derive(Clone, Debug)]
pub struct BigPipeline {
    signal_format: BigQFormat,
    channels: u16,
    output_channels: u16,
    stages: Vec<BigStage>,
    stats: StageStats,
    poisoned: bool,
}

impl BigPipeline {
    pub fn new(
        channels: u16,
        signal_format: BigQFormat,
        stages: Vec<BigStage>,
    ) -> Result<Self, BigDspError> {
        check_channels(channels)?;
        let mut layout = channels;
        for stage in &stages {
            require_format(signal_format, stage.signal_format)?;
            if stage.input_channels != layout {
                return Err(DspError::MixMatrixSize {
                    expected: usize::from(layout),
                    actual: usize::from(stage.input_channels),
                }
                .into());
            }
            layout = stage.output_channels;
        }
        Ok(Self {
            signal_format,
            channels,
            output_channels: layout,
            stages,
            stats: StageStats::default(),
            poisoned: false,
        })
    }

    pub const fn output_channels(&self) -> u16 {
        self.output_channels
    }
    pub const fn stats(&self) -> StageStats {
        self.stats
    }

    pub fn maximum_accumulator_bits(&self) -> u32 {
        self.stages
            .iter()
            .map(BigStage::accumulator_bits)
            .max()
            .unwrap_or(0)
    }

    pub const fn input_channels(&self) -> u16 {
        self.channels
    }
    pub const fn signal_format(&self) -> BigQFormat {
        self.signal_format
    }
    pub(crate) fn poison(&mut self) {
        self.poisoned = true;
    }
    pub(crate) fn ensure_ready(&self) -> Result<(), BigDspError> {
        if self.poisoned {
            Err(DspError::Poisoned.into())
        } else {
            Ok(())
        }
    }

    pub fn process_chunk(
        &mut self,
        input: &[BigQ],
        output: &mut Vec<BigQ>,
    ) -> Result<(), BigDspError> {
        if self.poisoned {
            return Err(DspError::Poisoned.into());
        }
        let result = (|| {
            check_frames(input.len(), self.channels)?;
            for sample in input {
                require_format(self.signal_format, sample.format())?;
            }
            let mut candidate = self.stages.clone();
            let rendered = process_from(&mut candidate, input.to_vec())?;
            let stats = StageStats {
                input_samples: self
                    .stats
                    .input_samples
                    .checked_add(input.len() as u64)
                    .ok_or(DspError::CounterOverflow)?,
                output_samples: self
                    .stats
                    .output_samples
                    .checked_add(rendered.len() as u64)
                    .ok_or(DspError::CounterOverflow)?,
                saturated_samples: 0,
            };
            output
                .try_reserve(rendered.len())
                .map_err(|_| DspError::AllocationFailed)?;
            output.extend(rendered);
            self.stages = candidate;
            self.stats = stats;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    pub fn finish(mut self, output: &mut Vec<BigQ>) -> Result<StageStats, BigDspError> {
        if self.poisoned {
            return Err(DspError::Poisoned.into());
        }
        let mut rendered = Vec::new();
        for index in 0..self.stages.len() {
            let tail = self.stages[index].finish()?;
            let tail = process_from(&mut self.stages[index + 1..], tail)?;
            rendered
                .try_reserve(tail.len())
                .map_err(|_| DspError::AllocationFailed)?;
            rendered.extend(tail);
        }
        self.stats.output_samples = self
            .stats
            .output_samples
            .checked_add(rendered.len() as u64)
            .ok_or(DspError::CounterOverflow)?;
        output
            .try_reserve(rendered.len())
            .map_err(|_| DspError::AllocationFailed)?;
        output.extend(rendered);
        Ok(self.stats)
    }
}

fn process_from(stages: &mut [BigStage], mut samples: Vec<BigQ>) -> Result<Vec<BigQ>, BigDspError> {
    for stage in stages {
        samples = stage.process(&samples)?;
    }
    Ok(samples)
}

fn check_channels(channels: u16) -> Result<(), DspError> {
    if channels == 0 {
        Err(DspError::ZeroChannels)
    } else {
        Ok(())
    }
}

fn check_frames(samples: usize, channels: u16) -> Result<(), DspError> {
    if !samples.is_multiple_of(usize::from(channels)) {
        Err(DspError::PartialInterleavedFrame { samples, channels })
    } else {
        Ok(())
    }
}

fn require_format(expected: BigQFormat, actual: BigQFormat) -> Result<(), BigDspError> {
    if expected == actual {
        Ok(())
    } else {
        Err(BigQError::FormatMismatch {
            left: expected,
            right: actual,
        }
        .into())
    }
}

#[cfg(test)]
mod tests;
