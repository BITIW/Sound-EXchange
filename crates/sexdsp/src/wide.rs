//! Fixed Q65.63 adapters over the runtime-format BigQ DSP implementation.
//! All conversions at this API boundary are exact; no fractional bits change.

use crate::arbitrary::{BigPipeline, BigStage};
use crate::{DspError, StageStats};
use sexq::{BigQ, BigQFormat, Q1_63, RoundingMode, WideQ63};

pub use crate::arbitrary::BigDspError as WideDspError;

fn signal_format() -> BigQFormat {
    BigQFormat::new(65, 63).expect("Q65.63 is a supported fixed format")
}

#[derive(Clone, Debug)]
pub struct WideStage(BigStage);

impl WideStage {
    pub fn gain(channels: u16, gain: BigQ, rounding: RoundingMode) -> Result<Self, WideDspError> {
        BigStage::gain(channels, signal_format(), gain, rounding).map(Self)
    }
    pub fn mix(
        input_channels: u16,
        output_channels: u16,
        matrix: Vec<BigQ>,
        rounding: RoundingMode,
    ) -> Result<Self, WideDspError> {
        BigStage::mix(
            input_channels,
            output_channels,
            signal_format(),
            matrix,
            rounding,
        )
        .map(Self)
    }
    pub fn dc_block(
        channels: u16,
        radius: BigQ,
        rounding: RoundingMode,
    ) -> Result<Self, WideDspError> {
        BigStage::dc_block(channels, signal_format(), radius, rounding).map(Self)
    }
    pub fn convolution(
        channels: u16,
        taps: Vec<BigQ>,
        rounding: RoundingMode,
    ) -> Result<Self, WideDspError> {
        BigStage::convolution(channels, signal_format(), taps, rounding).map(Self)
    }
    pub const fn input_channels(&self) -> u16 {
        self.0.input_channels()
    }
    pub const fn output_channels(&self) -> u16 {
        self.0.output_channels()
    }
    pub fn accumulator_bits(&self) -> u32 {
        self.0.accumulator_bits()
    }
}

/// Q65.63 façade with the same atomic block, history, and tail semantics as BigPipeline.
#[derive(Clone, Debug)]
pub struct WidePipeline {
    inner: BigPipeline,
}

impl WidePipeline {
    pub fn new(channels: u16, stages: Vec<WideStage>) -> Result<Self, WideDspError> {
        let stages = stages.into_iter().map(|stage| stage.0).collect();
        Ok(Self {
            inner: BigPipeline::new(channels, signal_format(), stages)?,
        })
    }

    pub const fn output_channels(&self) -> u16 {
        self.inner.output_channels()
    }
    pub const fn stats(&self) -> StageStats {
        self.inner.stats()
    }
    pub fn maximum_accumulator_bits(&self) -> u32 {
        self.inner.maximum_accumulator_bits()
    }

    pub fn process_q1_63(
        &mut self,
        input: &[Q1_63],
        output: &mut Vec<WideQ63>,
    ) -> Result<(), WideDspError> {
        self.process_raw(input.iter().map(|sample| i128::from(sample.raw())), output)
    }

    pub fn process_chunk(
        &mut self,
        input: &[WideQ63],
        output: &mut Vec<WideQ63>,
    ) -> Result<(), WideDspError> {
        self.process_raw(input.iter().map(|sample| sample.raw()), output)
    }

    fn process_raw(
        &mut self,
        input: impl ExactSizeIterator<Item = i128>,
        output: &mut Vec<WideQ63>,
    ) -> Result<(), WideDspError> {
        self.inner.ensure_ready()?;
        let result = (|| {
            let count = input.len();
            let channels = self.inner.input_channels();
            if !count.is_multiple_of(usize::from(channels)) {
                return Err(DspError::PartialInterleavedFrame {
                    samples: count,
                    channels,
                }
                .into());
            }
            let output_count = (count / usize::from(channels))
                .checked_mul(usize::from(self.output_channels()))
                .ok_or(DspError::AllocationFailed)?;
            // Reserve before committing the inner pipeline. Every emitted raw
            // value is already range-checked Q65.63, so subsequent export cannot fail.
            output
                .try_reserve(output_count)
                .map_err(|_| DspError::AllocationFailed)?;
            let mut imported = Vec::new();
            imported
                .try_reserve(count)
                .map_err(|_| DspError::AllocationFailed)?;
            for raw in input {
                imported.push(BigQ::from_i128(raw, signal_format())?);
            }
            let mut rendered = Vec::new();
            self.inner.process_chunk(&imported, &mut rendered)?;
            output.extend(rendered.into_iter().map(export_wide));
            Ok(())
        })();
        if result.is_err() {
            self.inner.poison();
        }
        result
    }

    pub fn finish(self, output: &mut Vec<WideQ63>) -> Result<StageStats, WideDspError> {
        let mut rendered = Vec::new();
        let stats = self.inner.finish(&mut rendered)?;
        output
            .try_reserve(rendered.len())
            .map_err(|_| DspError::AllocationFailed)?;
        output.extend(rendered.into_iter().map(export_wide));
        Ok(stats)
    }
}

fn export_wide(value: BigQ) -> WideQ63 {
    WideQ63::from_raw(
        value
            .raw_i128()
            .expect("validated Q65.63 stage output fits i128"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUND: RoundingMode = RoundingMode::NearestTiesToEven;
    fn coefficient(raw: i128) -> BigQ {
        BigQ::from_i128(raw, BigQFormat::new(65, 62).unwrap()).unwrap()
    }

    #[test]
    fn intermediate_headroom_survives_gain_and_mix_and_convolution() {
        let mut pipeline = WidePipeline::new(
            2,
            vec![
                WideStage::gain(2, coefficient(2_i128 << 62), ROUND).unwrap(),
                WideStage::mix(2, 1, vec![coefficient(1_i128 << 62); 2], ROUND).unwrap(),
                WideStage::convolution(1, vec![coefficient(1_i128 << 60); 2], ROUND).unwrap(),
            ],
        )
        .unwrap();
        let x = Q1_63::from_raw(3_i64 << 61); // .75; gain yields 1.5, mix 3.
        let mut output = Vec::new();
        pipeline.process_q1_63(&[x, x], &mut output).unwrap();
        let stats = pipeline.finish(&mut output).unwrap();
        assert_eq!(output, vec![WideQ63::from_q1_63(x); 2]);
        assert_eq!(
            (
                stats.input_samples,
                stats.output_samples,
                stats.saturated_samples
            ),
            (2, 2, 0)
        );
    }

    #[test]
    fn arbitrary_precision_coefficients_cancel_before_one_rounding() {
        let format = BigQFormat::new(2, 4096).unwrap();
        let one = BigQ::one(format).unwrap();
        let negative = BigQ::from_raw(-one.raw().clone(), format).unwrap();
        let stage = WideStage::mix(2, 1, vec![one, negative], ROUND).unwrap();
        assert_eq!(stage.accumulator_bits(), 4226);
        let mut pipeline = WidePipeline::new(2, vec![stage]).unwrap();
        let mut output = Vec::new();
        pipeline
            .process_chunk(&[WideQ63::MAX, WideQ63::MAX], &mut output)
            .unwrap();
        assert_eq!(output, [WideQ63::ZERO]);
    }

    #[test]
    fn wide_dc_state_is_chunk_invariant_and_can_exceed_full_scale() {
        let make = || {
            WidePipeline::new(
                1,
                vec![WideStage::dc_block(1, coefficient(1_i128 << 61), ROUND).unwrap()],
            )
            .unwrap()
        };
        let input = [Q1_63::MIN, Q1_63::MAX, Q1_63::ZERO, Q1_63::MIN];
        let mut whole = make();
        let mut split = make();
        let mut a = Vec::new();
        let mut b = Vec::new();
        whole.process_q1_63(&input, &mut a).unwrap();
        for sample in &input {
            split.process_q1_63(&[*sample], &mut b).unwrap();
        }
        assert_eq!(a, b);
        assert!(a.iter().any(|sample| sample.raw() > i128::from(i64::MAX)));
    }

    #[test]
    fn wide_output_overflow_rolls_back_history_and_poisons() {
        let mut pipeline = WidePipeline::new(
            1,
            vec![WideStage::convolution(1, vec![coefficient(1_i128 << 62); 2], ROUND).unwrap()],
        )
        .unwrap();
        let mut output = vec![WideQ63::from_raw(7)];
        assert!(
            pipeline
                .process_chunk(&[WideQ63::MAX, WideQ63::MAX], &mut output)
                .is_err()
        );
        assert_eq!(output, [WideQ63::from_raw(7)]);
        assert_eq!(pipeline.stats(), StageStats::default());
        assert_eq!(
            pipeline.process_chunk(&[], &mut output),
            Err(DspError::Poisoned.into())
        );
    }

    #[test]
    fn tails_pass_through_downstream_wide_stages() {
        let make = || {
            WidePipeline::new(
                1,
                vec![
                    WideStage::convolution(1, vec![coefficient(1_i128 << 62); 2], ROUND).unwrap(),
                    WideStage::convolution(1, vec![coefficient(1_i128 << 62); 2], ROUND).unwrap(),
                ],
            )
            .unwrap()
        };
        let mut pipeline = make();
        let mut output = Vec::new();
        pipeline
            .process_chunk(&[WideQ63::from_raw(2_i128 << 63)], &mut output)
            .unwrap();
        pipeline.finish(&mut output).unwrap();
        assert_eq!(
            output.iter().map(|v| v.raw()).collect::<Vec<_>>(),
            [2_i128 << 63, 4_i128 << 63, 2_i128 << 63]
        );
        let mut empty = Vec::new();
        make().finish(&mut empty).unwrap();
        assert!(empty.is_empty());
    }
}
