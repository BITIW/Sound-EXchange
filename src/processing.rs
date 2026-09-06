//! One arbitrary-precision signal pass for scan/render and same/rate-changing I/O.

use super::*;
use sexdsp::arbitrary::{BigPipeline, BigStage};
use sexdsp::big_dither::{BigDitherQuantizer, BigInterleavedDither};
use sexdsp::pcm_error::{PcmErrorMeter, PcmErrorStats};
use sexplan::{SignalPrecisionRequest, plan_signal_precision};
use sexq::{WideQ63, round_div_integer};
use sexrate::{
    FrameCountPolicy, InterleavedResamplerBig, InterleavedResamplerWideQ63, InterleavedStreamStats,
    PolyphaseFirBig, PolyphaseFirQ63, output_frames_for_input,
};
#[cfg(target_endian = "little")]
use sha2::{Digest, Sha256};
#[cfg(target_endian = "big")]
use sha2_software::{Digest, Sha256};

#[cfg(test)]
#[path = "normalization_tests.rs"]
mod normalization_tests;

fn ceil_log2(value: u128) -> u32 {
    if value <= 1 {
        0
    } else {
        128 - (value - 1).leading_zeros()
    }
}

fn coefficient_sum_bits(coefficients: &[ExactCoefficient]) -> u32 {
    let bound = coefficients
        .iter()
        .map(|c| u128::from(c.numerator.unsigned_abs()).div_ceil(u128::from(c.denominator)))
        .sum();
    ceil_log2(bound)
}

pub(super) fn initial_signal_plan(
    plan: &PrecisionPlan,
    quality: &QualityOptions,
    resampling: bool,
) -> Result<SignalPrecisionPlan, Box<dyn Error>> {
    let fir_bits = if resampling {
        ceil_log2(u128::from(plan.taps_per_phase)) + 1
    } else {
        0
    };
    let gain_bits = quality.gain.map_or(0, |g| {
        ceil_log2(u128::from(g.numerator).div_ceil(u128::from(g.denominator)))
    });
    let dc_error_bits = quality.dc_block_radius.map_or(0, |r| {
        ceil_log2(u128::from(r.0.denominator).div_ceil(u128::from(r.0.denominator - r.0.numerator)))
    });
    let mix_bits = quality.channel_mix.as_ref().map_or(0, |m| {
        m.coefficients
            .chunks_exact(usize::from(m.input_channels))
            .map(coefficient_sum_bits)
            .max()
            .unwrap_or(0)
    });
    let convolution_bits = quality
        .convolution
        .as_ref()
        .map_or(0, |c| coefficient_sum_bits(&c.taps));
    let dc_present = u32::from(quality.dc_block_radius.is_some());
    let rounded_stages = u32::from(resampling)
        + u32::from(quality.gain.is_some())
        + dc_present
        + u32::from(quality.channel_mix.is_some())
        + u32::from(quality.convolution.is_some())
        + u32::from(quality.clip_policy == ClipPolicy::Normalize);
    Ok(plan_signal_precision(
        plan,
        SignalPrecisionRequest {
            minimum_integer_bits: 2
                + fir_bits
                + gain_bits
                + dc_present
                + mix_bits
                + convolution_bits,
            rounded_stages,
            roundoff_amplification_bits: fir_bits
                + gain_bits
                + dc_error_bits
                + dc_present
                + mix_bits
                + convolution_bits,
            fractional_bits: quality.signal_fractional_bits,
        },
    )?)
}

fn coefficient(
    numerator: i128,
    denominator: u64,
    format: BigQFormat,
) -> Result<BigQ, Box<dyn Error>> {
    let scaled = Integer::from(numerator) << format.fractional_bits();
    let raw = round_div_integer(
        &scaled,
        &Integer::from(denominator),
        RoundingMode::NearestTiesToEven,
    )?;
    Ok(BigQ::from_raw(raw, format)?)
}

fn coefficient_row(
    row: &[ExactCoefficient],
    format: BigQFormat,
) -> Result<Vec<BigQ>, Box<dyn Error>> {
    row.iter()
        .map(|c| coefficient(i128::from(c.numerator), c.denominator, format))
        .collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct SignalPeak {
    pub magnitude: Integer,
}
impl SignalPeak {
    fn observe(&mut self, samples: &[BigQ]) {
        for sample in samples {
            let magnitude = sample.raw().clone().abs();
            if magnitude > self.magnitude {
                self.magnitude = magnitude;
            }
        }
    }
}

pub(super) fn normalization_summary(
    peak: SignalPeak,
    format: BigQFormat,
    config: DitherConfig,
) -> Result<NormalizationSummary, Box<dyn Error>> {
    let guard = BigDitherQuantizer::maximum_excursion_raw(format, config)?;
    let endpoint = ((Integer::from(1) << (config.target_bits - 1)) - 1)
        << (format.fractional_bits() - (config.target_bits - 1));
    let target: Integer = endpoint - &guard;
    if target < 0 {
        return Err("dither/noise-shaping guard consumes the complete PCM output range".into());
    }
    let (numerator, denominator) = if peak.magnitude > target {
        (target, peak.magnitude.clone())
    } else {
        (Integer::from(1), Integer::from(1))
    };
    Ok(NormalizationSummary {
        measured_peak_raw: peak.magnitude,
        dither_guard_raw: guard,
        scale_numerator: numerator,
        scale_denominator: denominator,
    })
}

enum SignalBank {
    Native {
        bank: Arc<PolyphaseFirQ63>,
        delay: u64,
    },
    Big {
        bank: Arc<PolyphaseFirBig>,
        delay: u64,
    },
}
impl SignalBank {
    fn from_design(
        design: PlannedDesign,
        format: BigQFormat,
        summary: &mut FilterSummary,
    ) -> Result<Self, Box<dyn Error>> {
        let (big, delay) = prepare_signal_bank(&design, format, summary)?;
        if let Some(bank) = big {
            Ok(Self::Big {
                delay,
                bank: Arc::new(bank),
            })
        } else {
            let PlannedDesign::Native(design) = design else {
                unreachable!("only Q65.63 with native coefficients uses the native stream")
            };
            Ok(Self::Native {
                delay,
                bank: Arc::new(design.into_bank()),
            })
        }
    }
    fn stream(
        &self,
        channels: u16,
        ratio: RateRatio,
        format: BigQFormat,
    ) -> Result<SignalStream, Box<dyn Error>> {
        let rounding = RoundingMode::NearestTiesToEven;
        let overflow = OverflowPolicy::Error;
        match self {
            Self::Native { bank, delay } => Ok(SignalStream::Native {
                stream: InterleavedResamplerWideQ63::new_with_input_delay(
                    channels,
                    ratio,
                    bank.clone(),
                    *delay,
                    rounding,
                    overflow,
                )?,
                format,
            }),
            Self::Big { bank, delay } => Ok(SignalStream::Big(
                InterleavedResamplerBig::new_with_input_delay(
                    channels,
                    ratio,
                    bank.clone(),
                    *delay,
                    rounding,
                    overflow,
                )?,
            )),
        }
    }
}

/// Prove the native stream's extended-input interval with phase-bounded scratch.
fn native_signal_accumulator_bits(
    bank: &PolyphaseFirQ63,
    format: BigQFormat,
) -> Result<u32, Box<dyn Error>> {
    if format == BigQFormat::new(65, 63)? {
        return Ok(bank.minimum_wide_accumulator_bits());
    }
    let cf = BigQFormat::new(2, 62)?;
    let mut required = 1;
    // Promote one phase at a time for the proof, not the entire coefficient bank.
    for phase in 0..bank.phase_count() {
        let coefficients = bank
            .phase(u64::try_from(phase)?)?
            .iter()
            .map(|c| BigQ::from_i64(c.raw(), cf))
            .collect::<Result<Vec<_>, _>>()?;
        required = required
            .max(sexq::BigMac::exact_requirements_for(format, cf, &coefficients)?.signed_bits);
    }
    Ok(required)
}

#[cfg(test)]
mod accumulator_tests {
    use super::*;
    use sexq::Q2_62;

    #[test]
    fn native_signal_exact_bound_checks_every_phase_and_signed_endpoint() {
        let format = BigQFormat::new(65, 63).unwrap();
        let positive = PolyphaseFirQ63::new(1, 2, vec![Q2_62::ONE, Q2_62::ZERO]).unwrap();
        assert_eq!(
            native_signal_accumulator_bits(&positive, format).unwrap(),
            190
        );
        let both = PolyphaseFirQ63::new(
            2,
            2,
            vec![
                Q2_62::ONE,
                Q2_62::ZERO,
                Q2_62::from_raw(-Q2_62::ONE.raw()),
                Q2_62::ZERO,
            ],
        )
        .unwrap();
        assert_eq!(native_signal_accumulator_bits(&both, format).unwrap(), 191);
        let wide = BigQFormat::new(65, 4096).unwrap();
        assert_eq!(
            native_signal_accumulator_bits(&positive, wide).unwrap(),
            4223
        );
        assert_eq!(native_signal_accumulator_bits(&both, wide).unwrap(), 4224);
    }
}

/// Prepare precisely the same format/bounds/backend for analysis and execution.
/// Big coefficient storage is shared; conversion reuses the returned bank.
fn prepare_signal_bank(
    design: &PlannedDesign,
    format: BigQFormat,
    summary: &mut FilterSummary,
) -> Result<(Option<PolyphaseFirBig>, u64), Box<dyn Error>> {
    let extra = format
        .total_bits()
        .checked_sub(64)
        .ok_or("signal format is narrower than Q1.63")?;
    summary.plan.planned_accumulator_bits = summary
        .plan
        .planned_accumulator_bits
        .checked_add(extra)
        .ok_or("accumulator plan overflow")?;
    summary.accumulator_bits = summary
        .accumulator_bits
        .checked_add(extra)
        .ok_or("accumulator width overflow")?;
    let bits = summary.plan.planned_accumulator_bits;
    let (bank, delay) = match design {
        PlannedDesign::Native(design) if format == BigQFormat::new(65, 63)? => {
            summary.accumulator_bits = native_signal_accumulator_bits(design.bank(), format)?;
            if summary.accumulator_bits > bits {
                return Err("native signal accumulator bound exceeds its planned width".into());
            }
            summary.execution_backend = "checked-i128/GMP-fallback";
            return Ok((None, design.input_delay_frames()));
        }
        PlannedDesign::Native(design) => (
            PolyphaseFirBig::from_native_bank(design.bank(), format, format, Some(bits))?,
            design.input_delay_frames(),
        ),
        PlannedDesign::Big(design) => (
            PolyphaseFirBig::from_q63_bank(design.bank(), format, format, Some(bits))?,
            design.input_delay_frames(),
        ),
        PlannedDesign::Windowed(design) => (
            PolyphaseFirBig::from_q63_bank(design.bank(), format, format, Some(bits))?,
            design.input_delay_frames(),
        ),
        PlannedDesign::Optimized(design) => (
            PolyphaseFirBig::from_q63_bank(design.bank(), format, format, Some(bits))?,
            design.input_delay_frames(),
        ),
    };
    summary.accumulator_bits = bank.required_accumulator_bits();
    summary.execution_backend = "gmp-bigint";
    Ok((Some(bank), delay))
}

pub(super) fn summarize_signal_execution(
    design: &PlannedDesign,
    format: BigQFormat,
    summary: &mut FilterSummary,
) -> Result<(), Box<dyn Error>> {
    prepare_signal_bank(design, format, summary)?;
    Ok(())
}

enum SignalStream {
    Native {
        stream: InterleavedResamplerWideQ63,
        format: BigQFormat,
    },
    Big(InterleavedResamplerBig),
}
impl SignalStream {
    fn push(&mut self, input: &[BigQ], output: &mut Vec<BigQ>) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Big(stream) => stream.push_interleaved_finite_into(
                input,
                FrameCountPolicy::NearestTiesToEven,
                output,
            )?,
            Self::Native { stream, format } => {
                let input = input
                    .iter()
                    .map(|s| {
                        s.raw_i128()
                            .map(WideQ63::from_raw)
                            .ok_or("native signal input exceeds Q65.63")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut wide = Vec::new();
                stream.push_interleaved_finite_wide_into(
                    &input,
                    FrameCountPolicy::NearestTiesToEven,
                    &mut wide,
                )?;
                for sample in wide {
                    output.push(BigQ::from_i128(sample.raw(), *format)?);
                }
            }
        }
        Ok(())
    }
    fn finish(
        self,
        target: u64,
        output: &mut Vec<BigQ>,
    ) -> Result<InterleavedStreamStats, Box<dyn Error>> {
        match self {
            Self::Big(stream) => Ok(stream.finish_exact_frames(target, output)?),
            Self::Native { stream, format } => {
                let mut wide = Vec::new();
                let stats = stream.finish_wide_exact_frames(target, &mut wide)?;
                for sample in wide {
                    output.push(BigQ::from_i128(sample.raw(), format)?);
                }
                Ok(stats)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct PassReport {
    input_frames: u64,
    output_frames: u64,
    input_pcm_sha256: Option<[u8; 32]>,
}

impl PassReport {
    fn verify_normalization_replay(&self, rendered: &Self) -> Result<(), Box<dyn Error>> {
        if self.input_frames != rendered.input_frames
            || self.output_frames != rendered.output_frames
        {
            return Err("input frame count changed between normalization passes".into());
        }
        if self.input_pcm_sha256.is_none() || self.input_pcm_sha256 != rendered.input_pcm_sha256 {
            return Err("input PCM changed between normalization passes".into());
        }
        Ok(())
    }
}

fn run_pass(
    reader: &mut AudioReader,
    pipeline: &BigPipeline,
    bank: Option<&SignalBank>,
    ratio: RateRatio,
    block_frames: usize,
    verify_input: bool,
    mut emit: impl FnMut(&[BigQ]) -> Result<(), Box<dyn Error>>,
) -> Result<PassReport, Box<dyn Error>> {
    // Hash decoded input, before gain/mix/FIR can erase a difference. Fixed
    // signed little-endian Q1.63 bytes exclude chunking and CPU word order.
    let mut input_hash = verify_input.then(Sha256::new);
    let mut input_frames = 0_u64;
    let format = pipeline.signal_format();
    let channels = usize::from(pipeline.output_channels());
    let mut stream = bank
        .map(|bank| bank.stream(pipeline.output_channels(), ratio, format))
        .transpose()?;
    let mut pipeline = pipeline.clone();
    let mut imported = Vec::new();
    let mut processed = Vec::new();
    let mut filtered = Vec::new();
    let mut effect_frames = 0_u64;
    let mut dispatch = |samples: &[BigQ]| -> Result<(), Box<dyn Error>> {
        if !samples.len().is_multiple_of(channels) {
            return Err("effect output has a partial frame".into());
        }
        effect_frames = effect_frames
            .checked_add(u64::try_from(samples.len() / channels)?)
            .ok_or("effect frame count overflow")?;
        if let Some(stream) = &mut stream {
            filtered.clear();
            stream.push(samples, &mut filtered)?;
            emit(&filtered)
        } else {
            emit(samples)
        }
    };
    loop {
        let chunk = reader.read_frames(block_frames)?;
        if chunk.frames == 0 {
            break;
        }
        input_frames = input_frames
            .checked_add(u64::try_from(chunk.frames)?)
            .ok_or("input frame count overflow")?;
        imported.clear();
        for sample in chunk.samples {
            if let Some(hash) = &mut input_hash {
                hash.update(sample.raw().to_le_bytes());
            }
            imported.push(
                BigQ::from_signed_pcm(
                    sample.raw(),
                    64,
                    format,
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error,
                )?
                .value,
            );
        }
        processed.clear();
        pipeline.process_chunk(&imported, &mut processed)?;
        dispatch(&processed)?;
    }
    processed.clear();
    pipeline.finish(&mut processed)?;
    dispatch(&processed)?;
    let target =
        output_frames_for_input(effect_frames, ratio, FrameCountPolicy::NearestTiesToEven)?;
    if let Some(stream) = stream {
        filtered.clear();
        let stats = stream.finish(target, &mut filtered)?;
        if stats.saturated_samples != 0 {
            return Err("resampler unexpectedly clipped an intermediate sample".into());
        }
        emit(&filtered)?;
    }
    Ok(PassReport {
        input_frames,
        output_frames: target,
        input_pcm_sha256: input_hash.map(|hash| hash.finalize().into()),
    })
}

pub(super) fn validate_input_layout(
    input_spec: AudioSpec,
    quality: &QualityOptions,
) -> Result<(), Box<dyn Error>> {
    if let Some(matrix) = &quality.channel_mix
        && matrix.input_channels != input_spec.channels
    {
        return Err(format!(
            "channel matrix has {} input column(s), but the source has {} channel(s)",
            matrix.input_channels, input_spec.channels
        )
        .into());
    }
    Ok(())
}

pub(super) fn process_audio_to_path(
    input_path: &Path,
    output_path: &Path,
    requested_rate: Option<u32>,
    requested_bits: Option<u16>,
    quality: QualityOptions,
) -> Result<ProcessingSummary, Box<dyn Error>> {
    process_audio_with_reopen(
        input_path,
        output_path,
        requested_rate,
        requested_bits,
        quality,
        AudioReader::open,
    )
}

fn process_audio_with_reopen(
    input_path: &Path,
    output_path: &Path,
    requested_rate: Option<u32>,
    requested_bits: Option<u16>,
    quality: QualityOptions,
    reopen: impl FnOnce(&Path) -> Result<AudioReader, Box<dyn Error>>,
) -> Result<ProcessingSummary, Box<dyn Error>> {
    let mut reader = AudioReader::open(input_path)?;
    let input_backend = reader.backend_identity()?;
    let input_spec = reader.spec();
    let metadata = reader.metadata();
    validate_input_layout(input_spec, &quality)?;
    let channels = quality
        .channel_mix
        .as_ref()
        .map_or(input_spec.channels, |m| m.output_channels);
    let output_rate = requested_rate.unwrap_or(input_spec.sample_rate);
    let resampling = output_rate != input_spec.sample_rate;
    if !resampling && quality.design_certificate.is_some() {
        return Err(
            "same-rate conversion bypasses FIR; --certify-design requires a rate change".into(),
        );
    }
    if !resampling && quality.gates.has_supplemental() {
        return Err(
            "same-rate conversion bypasses FIR; --certify and --harmonics require a rate change"
                .into(),
        );
    }
    if !resampling && quality.window != refinement::Designer::Kaiser {
        if matches!(
            quality.window,
            refinement::Designer::GlobalLeastSquares | refinement::Designer::Equiripple
        ) {
            return Err(
                "same-rate conversion bypasses FIR; selected --designer requires a rate change"
                    .into(),
            );
        }
        return Err(
            "same-rate conversion bypasses FIR; non-Kaiser --window requires a rate change".into(),
        );
    }
    let ratio = RateRatio::from_rates(u64::from(input_spec.sample_rate), u64::from(output_rate))?;
    let execution = numerical_plan::plan_execution(ratio, &quality, resampling)?;
    let (execution, materialized) = if resampling {
        let prepared = prepare_design(execution, &quality)?;
        eprint!("{}", prepared.design_certificate_report);
        (
            prepared.execution,
            Some((prepared.design, prepared.cache, prepared.assessment)),
        )
    } else {
        (execution, None)
    };
    let numerical_reference =
        numerical_plan::reference(execution.certified_fir_reference).to_owned();
    let plan = execution.filter;
    let signal_plan = execution.signal;
    let mut numerical_report = numerical_plan::report(
        &execution.numerical,
        plan.amplitude_error_bits + 2,
        execution.refinements,
    );
    if let Some(l1) = &execution.fir_l1 {
        numerical_report.push('\n');
        numerical_report.push_str(&numerical_plan::fir_l1_report(l1));
    }
    let format = BigQFormat::new(signal_plan.integer_bits, signal_plan.fractional_bits)?;
    let coefficient_format = BigQFormat::new(2, signal_plan.effect_coefficient_fractional_bits)?;
    let round = RoundingMode::NearestTiesToEven;
    let mut stages = Vec::new();
    let gain = if let Some(g) = quality.gain {
        let gain_format = BigQFormat::new(65, signal_plan.effect_coefficient_fractional_bits)?;
        let coefficient = if gain_format.fractional_bits() == 62 {
            BigQ::from_i128(g.coefficient_raw(), gain_format)?
        } else {
            coefficient(i128::from(g.numerator), g.denominator, gain_format)?
        };
        stages.push(BigStage::gain(
            input_spec.channels,
            format,
            coefficient.clone(),
            round,
        )?);
        Some(GainSummary {
            requested: g,
            coefficient_raw: coefficient.raw().clone(),
        })
    } else {
        None
    };
    let dc_block = if let Some(r) = quality.dc_block_radius {
        let coefficient = if coefficient_format.fractional_bits() == 62 {
            BigQ::from_i64(r.coefficient().raw(), coefficient_format)?
        } else {
            coefficient(
                i128::from(r.0.numerator),
                r.0.denominator,
                coefficient_format,
            )?
        };
        stages.push(BigStage::dc_block(
            input_spec.channels,
            format,
            coefficient.clone(),
            round,
        )?);
        Some(DcBlockSummary {
            requested_radius: r,
            coefficient_raw: coefficient.raw().clone(),
        })
    } else {
        None
    };
    let channel_mix = if let Some(matrix) = &quality.channel_mix {
        let coefficients = coefficient_row(&matrix.coefficients, coefficient_format)?;
        stages.push(BigStage::mix(
            matrix.input_channels,
            matrix.output_channels,
            format,
            coefficients.clone(),
            round,
        )?);
        Some(ChannelMixSummary {
            matrix: matrix.clone(),
            coefficients,
        })
    } else {
        None
    };
    let convolution = if let Some(spec) = &quality.convolution {
        let coefficients = coefficient_row(&spec.taps, coefficient_format)?;
        stages.push(BigStage::convolution(
            channels,
            format,
            coefficients.clone(),
            round,
        )?);
        Some(ConvolutionSummary {
            spec: spec.clone(),
            coefficients,
        })
    } else {
        None
    };
    let pipeline = BigPipeline::new(input_spec.channels, format, stages)?;
    let output_spec = AudioSpec {
        channels,
        sample_rate: output_rate,
        bits_per_sample: requested_bits.unwrap_or(input_spec.bits_per_sample),
    };
    let block_frames = quality.block_frames.unwrap_or(4096);
    let mut summary = ProcessingSummary {
        input_backend,
        output_backend: String::new(),
        block_frames,
        dsp_accumulator_bits: pipeline.maximum_accumulator_bits(),
        signal_plan,
        numerical_report,
        numerical_reference,
        output_spec,
        frames: 0,
        saturated_samples: 0,
        filter: None,
        quantization: None,
        pcm_error: None,
        normalization: None,
        gain,
        dc_block,
        channel_mix,
        convolution,
    };
    if !resampling
        && output_spec.bits_per_sample == input_spec.bits_per_sample
        && quality.dither_mode.is_none()
        && quality.clip_policy == ClipPolicy::Saturate
        && quality.gain.is_none()
        && quality.dc_block_radius.is_none()
        && quality.channel_mix.is_none()
        && quality.convolution.is_none()
    {
        let mut writer = AudioWriter::create(output_path, output_spec, &metadata)?;
        summary.output_backend = writer.backend_identity()?;
        loop {
            let chunk = reader.read_frames(usize::try_from(block_frames)?)?;
            if chunk.frames == 0 {
                break;
            }
            writer.write_frames(&chunk.samples, round, OverflowPolicy::Error)?;
        }
        let report = writer.finalize()?;
        summary.frames = report.frames;
        summary.saturated_samples = report.saturated_samples;
        // The bypass preserves integer PCM at the same depth exactly.
        summary.pcm_error = Some(PcmErrorStats {
            samples: report
                .frames
                .checked_mul(u64::from(output_spec.channels))
                .ok_or("PCM error sample count overflow")?,
            fractional_bits: 63,
            peak_raw: Integer::new(),
            sum_squared_raw: Integer::new(),
        });
        return Ok(summary);
    }
    let bank = if let Some((design, cache, assessment)) = materialized {
        let mut filter = design.summary(plan.clone(), cache, &assessment, quality.gates)?;
        let bank = SignalBank::from_design(design, format, &mut filter)?;
        summary.filter = Some(filter);
        Some(bank)
    } else {
        None
    };
    let mode = match quality.dither_mode {
        Some(mode) => mode,
        None if resampling || requested_bits.is_some() => select_dither_mode(None, plan.dither)?,
        None => DitherMode::None,
    };
    let config = DitherConfig {
        target_bits: u32::from(output_spec.bits_per_sample),
        mode,
        seed: quality.dither_seed,
        rounding: round,
        overflow: if quality.clip_policy == ClipPolicy::Saturate {
            OverflowPolicy::Saturate
        } else {
            OverflowPolicy::Error
        },
    };
    let normalization_scan = if quality.clip_policy == ClipPolicy::Normalize {
        let mut peak = SignalPeak::default();
        let target = run_pass(
            &mut reader,
            &pipeline,
            bank.as_ref(),
            ratio,
            usize::try_from(block_frames)?,
            true,
            |samples| {
                peak.observe(samples);
                Ok(())
            },
        )?;
        summary.normalization = Some(normalization_summary(peak, format, config)?);
        reader = reopen(input_path)?;
        if reader.spec() != input_spec {
            return Err("input audio format changed between normalization passes".into());
        }
        if reader.metadata() != metadata {
            return Err("input metadata changed between normalization passes".into());
        }
        if reader.backend_identity()? != summary.input_backend {
            return Err("input decoder changed between normalization passes".into());
        }
        Some(target)
    } else {
        None
    };
    let mut writer = AudioWriter::create(output_path, output_spec, &metadata)?;
    summary.output_backend = writer.backend_identity()?;
    let mut dither = BigInterleavedDither::new(channels, format, config)?;
    let mut pcm_error = PcmErrorMeter::new(format, config.target_bits)?;
    let mut scaled = Vec::new();
    let mut pcm = Vec::new();
    let rendered = run_pass(
        &mut reader,
        &pipeline,
        bank.as_ref(),
        ratio,
        usize::try_from(block_frames)?,
        normalization_scan.is_some(),
        |samples| {
            let render = if let Some(normalization) = &summary.normalization {
                scaled.clear();
                for sample in samples {
                    scaled.push(
                        sample
                            .scale_ratio(
                                &normalization.scale_numerator,
                                &normalization.scale_denominator,
                                round,
                                OverflowPolicy::Error,
                            )?
                            .value,
                    );
                }
                &scaled
            } else {
                samples
            };
            pcm.clear();
            dither.quantize_interleaved_into(render, &mut pcm)?;
            pcm_error.observe(render, &pcm)?;
            writer.write_signed_pcm_frames(&pcm)
        },
    )?;
    if let Some(scan) = normalization_scan {
        scan.verify_normalization_replay(&rendered)?;
    }
    let stats = dither.stats()?;
    let report = writer.finalize()?;
    if report.frames != rendered.output_frames {
        return Err("output length differs from the exact planned duration".into());
    }
    summary.frames = report.frames;
    summary.saturated_samples = stats
        .saturated_samples
        .checked_add(report.saturated_samples)
        .ok_or("saturation count overflow")?;
    summary.quantization = Some(QuantizationSummary {
        mode,
        seed: quality.dither_seed,
        clip_policy: quality.clip_policy,
    });
    summary.pcm_error = Some(pcm_error.stats().clone());
    Ok(summary)
}
