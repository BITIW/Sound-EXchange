use rug::Integer;
use sexdsp::{ClipPolicy, DitherConfig, DitherMode};
use sexfir::cache::{CacheError, CacheStatus, design_kaiser_big_cached, design_kaiser_cached};
use sexfir::certificate::{self, Limits as CertificateLimits};
use sexfir::harmonics::{self, Limits as HarmonicLimits};
use sexfir::refinement;
use sexfir::{
    BigKaiserSpec, DesignedBigFilter, DesignedFilter, KaiserSpec, design_kaiser, design_kaiser_big,
};
use sexio::{AudioMetadata, AudioSpec, InterleavedChunk, WaveReader, WaveWriter, WriteReport};
use sexio_sndfile::{SndFileError, SndFileLibrary, SndFileReader, SndFileWriter};
use sexplan::{
    BackendRequirement, DitherPolicy, ErrorFloor, PlanRequest, PrecisionPlan, QualityPreset,
    SignalPrecisionPlan, plan_precision,
};
use sexq::{BigQ, BigQFormat, OverflowPolicy, Q2_62, RoundingMode};
use sexrate::{RateRatio, SampleRate};
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "design_certificate.rs"]
mod design_certificate;
mod frontends;
#[path = "numerical_plan.rs"]
mod numerical_plan;
#[path = "pcm_report.rs"]
mod pcm_report;
#[path = "processing.rs"]
mod processing;
#[path = "quality_gates.rs"]
mod quality_gates;
#[path = "windows.rs"]
mod windows;
pub use frontends::{Frontend, entry, run};
use processing::process_audio_to_path;

static NEXT_OUTPUT_TRANSACTION: AtomicU64 = AtomicU64::new(0);

const HELP: &str = "\
SeX — deterministic offline audio processor

USAGE:
    sex INPUT OUTPUT
    sex INPUT -r RATE [QUALITY OPTIONS] OUTPUT
    sex INPUT OUTPUT rate RATE [QUALITY OPTIONS]
    sex plan INPUT -r RATE [QUALITY OPTIONS]
    sex analyze INPUT -r RATE [QUALITY OPTIONS] [--grid POINTS] [--certify]

CURRENT MILESTONE:
    Integer PCM WAVE, AIFF, and FLAC with deterministic fixed-point conversion.
    WAVE uses the built-in adapter; AIFF/FLAC require system libsndfile at runtime.
    Metadata known to libsndfile is preserved when both adapters support it.

OPTIONS:
    -r, --rate RATE      Exact Hz: 48000, 48k, 96000/2, 4.8e4
                        Fractional Hz: plan/analyze only; output adapters require integer Hz
    rate RATE           SoX-style positional rate effect
    --preset NAME       fast|sane|high|absurd|pointless|until-40k (default: sane)
    --window NAME       kaiser|rectangular|hann|blackman|dolph-chebyshev (default: kaiser)
    --designer NAME     windowed-sinc (default)|global-ls|remez; optimized methods exclude --window
    --designer-work N   Per-candidate optimized structural work limit (default 4000000000)
    --designer-total-work N Cumulative optimized structural work limit (default 8000000000)
    --designer-storage N Optimized estimated storage bytes (default 134217728)
    --designer-grid-limit N Optimized grid count limit (default 1000000)
    --designer-coefficients N Optimized coefficient count limit (default 1000000)
    --designer-precision-limit N Optimized MPFR width limit (default 16384)
    --error-floor -NdB  Filter target and pre-PCM numerical budget, e.g. -300dB
    --precision auto|BITS
                        Planner-selected or explicit MPFR designer precision
    --signal-precision auto|BITS
                        Planner-selected or explicit signal fractional precision
    --bits BITS         Output integer PCM width in 1..=32
    --gain RATIO        Non-negative linear gain, for example 1/2 or 4
    --dc-remove RATIO   First-order DC blocker radius in 0..1, for example 65535/65536
    --mix MATRIX        Exact output rows, for example '1/2,1/2' or '1,0;0,1'
    --convolve TAPS     Exact causal FIR taps, for example '1,1/2,-1/4'
    --block-frames N    Streaming input block size in frames (default: 4096)
    --dither MODE       none|tpdf|high-pass-tpdf|noise-shaped-1|noise-shaped-5|noise-shaped-9
    --seed INTEGER      Deterministic unsigned 64-bit dither seed (default: 0)
    --clip POLICY       saturate|error|normalize|allow-headroom (default: saturate)
    --grid POINTS       Response samples per pass/stop band (default: 65)
    -h, --help          Print this help
    -V, --version       Print version
    --build-info        Print architecture/ABI information for reproducibility
    --certify           Require continuous per-phase FIR amplitude certification
    --certify-design    Require rigorous joint Kaiser coefficient error certification
    --design-certificate-bits N       Initial endpoint precision (default 192)
    --design-certificate-max-bits N   Refinement ceiling (default 4096)
    --design-certificate-error-bits N Joint error target 2^-N (default: planner amplitude bits + 3)
    --design-certificate-work N       All-pass I0 term budget (default 10000000)
    --design-certificate-phase-work N Per-phase I0 term budget (default 1000000)
    --design-certificate-series N     Per-I0 term budget (default 16384)
    --design-certificate-input-bits N Rational input width limit (default 65536)
    --design-certificate-taps N       Maximum taps/phase (default 4097)
    --design-certificate-phases N     Maximum phase count (default 4096)
    --design-certificate-coefficients N Maximum bank coefficients (default 1048576)
    --certificate-work N          Exact-work budget (default 1000000000)
    --certificate-taps N          Maximum taps/phase for proof (default 1025)
    --certificate-depth N         Maximum subdivision depth (default 32)
    --certificate-endpoint-bits N  Directed band endpoint precision (8..128, default 32)
    --certificate-integer-bits N   Exact integer width limit (default 262144)
    --harmonics         Require sampled all-image transfer checks over the full input band
    --harmonic-work N              Harmonic work budget (default 100000000)
    --harmonic-coefficients N      Maximum bank coefficients (default 1000000)
    --harmonic-phases N            Maximum harmonic count (default 1024)
    --harmonic-precision-limit N   Maximum MPFR width (default 16384)
    --harmonic-storage-bytes N     MPFR storage estimate limit (default 134217728)
    --refine-quality    Compatibility alias: quality feedback is always enabled
    --refinement-attempts N        Maximum checked candidates (default 8, maximum 128)
    --refinement-coefficients N    Cumulative coefficient budget (normally 4000000)
    --refinement-terms N           Cumulative response-term budget (normally 200000000)
                                  Until-40k defaults reserve 4 initial-bank equivalents;
                                  explicit budgets remain hard limits (see sex plan)
    --refinement-precision-limit N Maximum MPFR width (default 16384)

ENVIRONMENT:
    SEX_COEFFICIENT_CACHE=PATH  Override the coefficient cache directory
    SEX_COEFFICIENT_CACHE=off   Disable the coefficient cache
";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct QualityOptions {
    window: refinement::Designer,
    designer_controls: windows::Controls,
    preset: QualityPreset,
    error_floor: Option<ErrorFloor>,
    working_precision_bits: Option<u32>,
    signal_fractional_bits: Option<u32>,
    dither_mode: Option<DitherMode>,
    dither_seed: u64,
    clip_policy: ClipPolicy,
    gain: Option<LinearGain>,
    dc_block_radius: Option<DcBlockRadius>,
    channel_mix: Option<ChannelMatrix>,
    convolution: Option<ConvolutionSpec>,
    block_frames: Option<u32>,
    refinement: quality_gates::RefinementBudget,
    gates: quality_gates::Gates,
    design_certificate: Option<design_certificate::Controls>,
}

#[derive(Default)]
struct RefinementArguments {
    limits: quality_gates::RefinementBudget,
    seen: Vec<String>,
}
impl RefinementArguments {
    fn consume(&mut self, args: &[String], index: &mut usize) -> Result<bool, String> {
        let option = args[*index].as_str();
        if option != "--refine-quality" && !option.starts_with("--refinement-") {
            return Ok(false);
        }
        if self.seen.iter().any(|old| old == option) {
            return Err(format!("{option} may be specified only once"));
        }
        self.seen.push(option.to_owned());
        let limits = &mut self.limits;
        if option == "--refine-quality" {
            *index += 1;
            return Ok(true);
        }
        let raw = args
            .get(*index + 1)
            .ok_or_else(|| format!("{option} requires a value"))?;
        let value = parse_u64(raw, "refinement limit")?;
        if value == 0 {
            return Err("refinement limits must be positive".to_owned());
        }
        match option {
            "--refinement-attempts" if value <= 128 => limits.max_attempts = Some(value as u32),
            "--refinement-attempts" => return Err("refinement attempts must be 1..128".to_owned()),
            "--refinement-coefficients" => limits.max_total_coefficients = Some(value),
            "--refinement-terms" => limits.max_response_terms = Some(value),
            "--refinement-precision-limit" => {
                limits.max_precision_bits =
                    Some(u32::try_from(value).map_err(|_| "refinement precision exceeds u32")?)
            }
            _ => return Err(format!("unknown refinement option: {option}")),
        }
        *index += 2;
        Ok(true)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LinearGain {
    numerator: u64,
    denominator: u64,
}

impl LinearGain {
    fn parse(raw: &str) -> Result<Self, String> {
        let (numerator, denominator) = match raw.split_once('/') {
            Some((numerator, denominator)) => (
                numerator
                    .parse::<u64>()
                    .map_err(|_| format!("invalid gain numerator: {numerator}"))?,
                denominator
                    .parse::<u64>()
                    .map_err(|_| format!("invalid gain denominator: {denominator}"))?,
            ),
            None => (
                raw.parse::<u64>()
                    .map_err(|_| format!("invalid exact linear gain: {raw}"))?,
                1,
            ),
        };
        if denominator == 0 {
            return Err("gain denominator must be non-zero".to_owned());
        }
        let divisor = gcd_u64(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    fn coefficient_raw(self) -> i128 {
        let scaled = u128::from(self.numerator) << Q2_62::FRACTIONAL_BITS;
        let denominator = u128::from(self.denominator);
        let quotient = scaled / denominator;
        let remainder = scaled % denominator;
        let twice_remainder = remainder * 2;
        let increment =
            twice_remainder > denominator || (twice_remainder == denominator && quotient & 1 == 1);
        i128::try_from(quotient + u128::from(increment)).expect("u64 ratio shifted by 62 fits i128")
    }
}

impl std::fmt::Display for LinearGain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.numerator, self.denominator)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DcBlockRadius(LinearGain);

impl DcBlockRadius {
    fn parse(raw: &str) -> Result<Self, String> {
        let ratio = LinearGain::parse(raw)?;
        if ratio.numerator >= ratio.denominator {
            return Err("DC blocker radius must be strictly less than one".to_owned());
        }
        Ok(Self(ratio))
    }

    fn coefficient(self) -> Q2_62 {
        Q2_62::from_raw(
            i64::try_from(self.0.coefficient_raw()).expect("validated DC radius fits Q2.62"),
        )
    }
}

impl std::fmt::Display for DcBlockRadius {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExactCoefficient {
    numerator: i64,
    denominator: u64,
    quantized: Q2_62,
}

impl ExactCoefficient {
    fn parse(raw: &str) -> Result<Self, String> {
        let (raw_numerator, raw_denominator) = match raw.split_once('/') {
            Some((numerator, denominator)) => (numerator, denominator),
            None => (raw, "1"),
        };
        let numerator = raw_numerator
            .parse::<i64>()
            .map_err(|_| format!("invalid coefficient numerator: {raw_numerator}"))?;
        let denominator = raw_denominator
            .parse::<u64>()
            .map_err(|_| format!("invalid coefficient denominator: {raw_denominator}"))?;
        if denominator == 0 {
            return Err("coefficient denominator must be non-zero".to_owned());
        }
        let divisor = gcd_u64(numerator.unsigned_abs(), denominator);
        let magnitude = numerator.unsigned_abs() / divisor;
        let numerator = if numerator < 0 {
            if magnitude == 1_u64 << 63 {
                i64::MIN
            } else {
                -i64::try_from(magnitude).unwrap()
            }
        } else {
            i64::try_from(magnitude).unwrap()
        };
        let denominator = denominator / divisor;
        let quantized = quantize_exact_q2_62(numerator, denominator)?;
        Ok(Self {
            numerator,
            denominator,
            quantized,
        })
    }
}

impl std::fmt::Display for ExactCoefficient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.numerator, self.denominator)
    }
}

fn quantize_exact_q2_62(numerator: i64, denominator: u64) -> Result<Q2_62, String> {
    let scaled = u128::from(numerator.unsigned_abs()) << Q2_62::FRACTIONAL_BITS;
    let denominator = u128::from(denominator);
    let quotient = scaled / denominator;
    let remainder = scaled % denominator;
    let twice_remainder = remainder * 2;
    let increment =
        twice_remainder > denominator || (twice_remainder == denominator && quotient & 1 == 1);
    let magnitude = quotient + u128::from(increment);
    let raw = if numerator < 0 {
        if magnitude == 1_u128 << 63 {
            i64::MIN
        } else {
            -i64::try_from(magnitude)
                .map_err(|_| "coefficient is below the Q2.62 range".to_owned())?
        }
    } else {
        i64::try_from(magnitude).map_err(|_| "coefficient is above the Q2.62 range".to_owned())?
    };
    Ok(Q2_62::from_raw(raw))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChannelMatrix {
    input_channels: u16,
    output_channels: u16,
    coefficients: Vec<ExactCoefficient>,
}

impl ChannelMatrix {
    fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Err("channel matrix must contain at least one row".to_owned());
        }
        let rows = raw.split(';').collect::<Vec<_>>();
        let output_channels = u16::try_from(rows.len())
            .map_err(|_| "channel matrix has too many output rows".to_owned())?;
        let first_columns = rows[0].split(',').count();
        if first_columns == 0 || rows[0].is_empty() {
            return Err("channel matrix rows must not be empty".to_owned());
        }
        let input_channels = u16::try_from(first_columns)
            .map_err(|_| "channel matrix has too many input columns".to_owned())?;
        let mut coefficients = Vec::new();
        for row in rows {
            let columns = row.split(',').collect::<Vec<_>>();
            if columns.len() != first_columns || columns.iter().any(|value| value.is_empty()) {
                return Err("every channel matrix row must have the same non-zero width".to_owned());
            }
            for coefficient in columns {
                coefficients.push(ExactCoefficient::parse(coefficient)?);
            }
        }
        Ok(Self {
            input_channels,
            output_channels,
            coefficients,
        })
    }

    #[cfg(test)]
    fn quantized(&self) -> Vec<Q2_62> {
        self.coefficients
            .iter()
            .map(|coefficient| coefficient.quantized)
            .collect()
    }
}

impl std::fmt::Display for ChannelMatrix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (row_index, row) in self
            .coefficients
            .chunks_exact(usize::from(self.input_channels))
            .enumerate()
        {
            if row_index != 0 {
                formatter.write_str(";")?;
            }
            for (column_index, coefficient) in row.iter().enumerate() {
                if column_index != 0 {
                    formatter.write_str(",")?;
                }
                coefficient.fmt(formatter)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConvolutionSpec {
    taps: Vec<ExactCoefficient>,
}

impl ConvolutionSpec {
    fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Err("convolution requires at least one tap".to_owned());
        }
        let taps = raw
            .split(',')
            .map(|tap| {
                if tap.is_empty() {
                    Err("convolution taps must not be empty".to_owned())
                } else {
                    ExactCoefficient::parse(tap)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { taps })
    }

    #[cfg(test)]
    fn quantized(&self) -> Vec<Q2_62> {
        self.taps.iter().map(|tap| tap.quantized).collect()
    }
}

impl std::fmt::Display for ConvolutionSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, tap) in self.taps.iter().enumerate() {
            if index != 0 {
                formatter.write_str(",")?;
            }
            tap.fmt(formatter)?;
        }
        Ok(())
    }
}

const fn gcd_u64(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Help,
    Version,
    BuildInfo,
    Convert {
        input: PathBuf,
        output: PathBuf,
        output_rate: Option<u32>,
        output_bits: Option<u16>,
        quality: QualityOptions,
    },
    Analyze {
        input: PathBuf,
        output_rate: SampleRate,
        quality: QualityOptions,
    },
    Plan {
        input: PathBuf,
        output_rate: SampleRate,
        quality: QualityOptions,
    },
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let _program = args.next();
    let Some(first) = args.next() else {
        return Ok(Command::Help);
    };
    if first == "-h" || first == "--help" {
        return Ok(Command::Help);
    }
    if first == "-V" || first == "--version" {
        return Ok(Command::Version);
    }
    if first == "--build-info" {
        if args.next().is_some() {
            return Err("--build-info takes no arguments".to_owned());
        }
        return Ok(Command::BuildInfo);
    }
    if first == "analyze" {
        return parse_analyze_args(args.collect());
    }
    if first == "plan" {
        return parse_plan_args(args.collect());
    }

    let input = PathBuf::from(first);
    let mut output = None;
    let mut output_rate = None;
    let mut preset = None;
    let mut error_floor = None;
    let mut working_precision_bits = None;
    let mut precision_seen = false;
    let mut signal_fractional_bits = None;
    let mut signal_precision_seen = false;
    let mut output_bits = None;
    let mut dither_mode = None;
    let mut dither_seed = None;
    let mut clip_policy = None;
    let mut gain = None;
    let mut dc_block_radius = None;
    let mut channel_mix = None;
    let mut convolution = None;
    let mut block_frames = None;
    let mut refinement_args = RefinementArguments::default();
    let mut gate_args = quality_gates::Arguments::default();
    let mut design_certificate_args = design_certificate::Arguments::default();
    let mut window_args = windows::Arguments::default();
    let rest = args.collect::<Vec<_>>();
    let mut index = 0;
    while index < rest.len() {
        if refinement_args.consume(&rest, &mut index)?
            || design_certificate_args.consume(&rest, &mut index)?
            || gate_args.consume(&rest, &mut index)?
            || window_args.consume(&rest, &mut index)?
        {
            continue;
        }
        match rest[index].as_str() {
            "-r" | "--rate" | "rate" => {
                let raw_rate = rest
                    .get(index + 1)
                    .ok_or_else(|| "output rate requires an exact sample rate".to_owned())?;
                let rate = parse_sample_rate(raw_rate)?
                    .container_hz()
                    .map_err(|error| error.to_string())?;
                if output_rate.replace(rate).is_some() {
                    return Err("-r may be specified only once".to_owned());
                }
                index += 2;
            }
            "--preset" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--preset requires a preset name".to_owned())?;
                let parsed = raw
                    .parse::<QualityPreset>()
                    .map_err(|error| error.to_string())?;
                if preset.replace(parsed).is_some() {
                    return Err("--preset may be specified only once".to_owned());
                }
                index += 2;
            }
            "--error-floor" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--error-floor requires a value such as -300dB".to_owned())?;
                let parsed = raw
                    .parse::<ErrorFloor>()
                    .map_err(|error| error.to_string())?;
                if error_floor.replace(parsed).is_some() {
                    return Err("--error-floor may be specified only once".to_owned());
                }
                index += 2;
            }
            "--precision" => {
                let raw = rest.get(index + 1).ok_or_else(|| {
                    "--precision requires auto or an integer bit count".to_owned()
                })?;
                if precision_seen {
                    return Err("--precision may be specified only once".to_owned());
                }
                precision_seen = true;
                working_precision_bits = parse_working_precision(raw)?;
                index += 2;
            }
            "--signal-precision" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or("--signal-precision requires auto or an integer bit count")?;
                if signal_precision_seen {
                    return Err("--signal-precision may be specified only once".to_owned());
                }
                signal_precision_seen = true;
                signal_fractional_bits = parse_working_precision(raw)?;
                index += 2;
            }
            "--bits" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--bits requires an integer PCM width".to_owned())?;
                let parsed = parse_pcm_bits(raw)?;
                if output_bits.replace(parsed).is_some() {
                    return Err("--bits may be specified only once".to_owned());
                }
                index += 2;
            }
            "--gain" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--gain requires an exact linear ratio".to_owned())?;
                let parsed = LinearGain::parse(raw)?;
                if gain.replace(parsed).is_some() {
                    return Err("--gain may be specified only once".to_owned());
                }
                index += 2;
            }
            "--dc-remove" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--dc-remove requires an exact radius ratio".to_owned())?;
                let parsed = DcBlockRadius::parse(raw)?;
                if dc_block_radius.replace(parsed).is_some() {
                    return Err("--dc-remove may be specified only once".to_owned());
                }
                index += 2;
            }
            "--mix" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--mix requires an exact rational matrix".to_owned())?;
                let parsed = ChannelMatrix::parse(raw)?;
                if channel_mix.replace(parsed).is_some() {
                    return Err("--mix may be specified only once".to_owned());
                }
                index += 2;
            }
            "--convolve" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--convolve requires exact rational taps".to_owned())?;
                let parsed = ConvolutionSpec::parse(raw)?;
                if convolution.replace(parsed).is_some() {
                    return Err("--convolve may be specified only once".to_owned());
                }
                index += 2;
            }
            "--block-frames" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--block-frames requires a positive frame count".to_owned())?;
                let parsed = parse_nonzero_u32(raw, "streaming block size")?;
                if block_frames.replace(parsed).is_some() {
                    return Err("--block-frames may be specified only once".to_owned());
                }
                index += 2;
            }
            "--dither" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--dither requires a mode".to_owned())?;
                let parsed = raw
                    .parse::<DitherMode>()
                    .map_err(|error| error.to_string())?;
                if dither_mode.replace(parsed).is_some() {
                    return Err("--dither may be specified only once".to_owned());
                }
                index += 2;
            }
            "--seed" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--seed requires an unsigned integer".to_owned())?;
                let parsed = parse_u64(raw, "dither seed")?;
                if dither_seed.replace(parsed).is_some() {
                    return Err("--seed may be specified only once".to_owned());
                }
                index += 2;
            }
            "--clip" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--clip requires a policy".to_owned())?;
                let parsed = raw
                    .parse::<ClipPolicy>()
                    .map_err(|error| error.to_string())?;
                if clip_policy.replace(parsed).is_some() {
                    return Err("--clip may be specified only once".to_owned());
                }
                index += 2;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option: {option}"));
            }
            path => {
                if output.replace(PathBuf::from(path)).is_some() {
                    return Err("exactly one output path is required".to_owned());
                }
                index += 1;
            }
        }
    }
    let output = output.ok_or_else(|| "an output path is required".to_owned())?;
    Ok(Command::Convert {
        input,
        output,
        output_rate,
        output_bits,
        quality: QualityOptions {
            preset: preset.unwrap_or_default(),
            error_floor,
            working_precision_bits,
            signal_fractional_bits,
            dither_mode,
            dither_seed: dither_seed.unwrap_or(0),
            clip_policy: clip_policy.unwrap_or_default(),
            gain,
            dc_block_radius,
            channel_mix,
            convolution,
            block_frames,
            refinement: refinement_args.limits,
            gates: gate_args.finish()?,
            design_certificate: design_certificate_args.finish()?,
            window: window_args.finish()?,
            designer_controls: window_args.controls,
        },
    })
}

fn parse_plan_args(args: Vec<String>) -> Result<Command, String> {
    match parse_analyze_args(args)? {
        Command::Analyze {
            input,
            output_rate,
            quality,
            ..
        } => Ok(Command::Plan {
            input,
            output_rate,
            quality,
        }),
        _ => unreachable!("analyze parser returns only Analyze"),
    }
}

fn parse_analyze_args(args: Vec<String>) -> Result<Command, String> {
    let input = args
        .first()
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| "analyze requires an input WAVE path".to_owned())?;
    let mut output_rate = None;
    let mut gate_args = quality_gates::Arguments::default();
    let mut design_certificate_args = design_certificate::Arguments::default();
    let mut window_args = windows::Arguments::default();
    let mut preset = None;
    let mut error_floor = None;
    let mut working_precision_bits = None;
    let mut precision_seen = false;
    let mut signal_fractional_bits = None;
    let mut signal_precision_seen = false;
    let mut dither_mode = None;
    let mut dither_seed = None;
    let mut clip_policy = None;
    let mut gain = None;
    let mut dc_block_radius = None;
    let mut channel_mix = None;
    let mut convolution = None;
    let mut index = 1;
    let mut refinement_args = RefinementArguments::default();
    while index < args.len() {
        if refinement_args.consume(&args, &mut index)?
            || design_certificate_args.consume(&args, &mut index)?
            || gate_args.consume(&args, &mut index)?
            || window_args.consume(&args, &mut index)?
        {
            continue;
        }
        let option = args[index].as_str();
        if !matches!(
            option,
            "-r" | "--rate"
                | "--preset"
                | "--error-floor"
                | "--precision"
                | "--signal-precision"
                | "--dither"
                | "--seed"
                | "--clip"
                | "--gain"
                | "--dc-remove"
                | "--mix"
                | "--convolve"
        ) {
            return Err(format!("unknown analyze option: {option}"));
        }
        let raw = args
            .get(index + 1)
            .ok_or_else(|| format!("{} requires a value", args[index]))?;
        match option {
            "--gain" => {
                if gain.replace(LinearGain::parse(raw)?).is_some() {
                    return Err("--gain may be specified only once".to_owned());
                }
            }
            "--dc-remove" => {
                if dc_block_radius
                    .replace(DcBlockRadius::parse(raw)?)
                    .is_some()
                {
                    return Err("--dc-remove may be specified only once".to_owned());
                }
            }
            "--mix" => {
                if channel_mix.replace(ChannelMatrix::parse(raw)?).is_some() {
                    return Err("--mix may be specified only once".to_owned());
                }
            }
            "--convolve" => {
                if convolution.replace(ConvolutionSpec::parse(raw)?).is_some() {
                    return Err("--convolve may be specified only once".to_owned());
                }
            }
            "-r" | "--rate" => {
                let value = parse_sample_rate(raw)?;
                if output_rate.replace(value).is_some() {
                    return Err("-r may be specified only once".to_owned());
                }
            }
            "--preset" => {
                let parsed = raw
                    .parse::<QualityPreset>()
                    .map_err(|error| error.to_string())?;
                if preset.replace(parsed).is_some() {
                    return Err("--preset may be specified only once".to_owned());
                }
            }
            "--error-floor" => {
                let parsed = raw
                    .parse::<ErrorFloor>()
                    .map_err(|error| error.to_string())?;
                if error_floor.replace(parsed).is_some() {
                    return Err("--error-floor may be specified only once".to_owned());
                }
            }
            "--precision" => {
                if precision_seen {
                    return Err("--precision may be specified only once".to_owned());
                }
                precision_seen = true;
                working_precision_bits = parse_working_precision(raw)?;
            }
            "--signal-precision" => {
                if signal_precision_seen {
                    return Err("--signal-precision may be specified only once".to_owned());
                }
                signal_precision_seen = true;
                signal_fractional_bits = parse_working_precision(raw)?;
            }
            "--dither" => {
                let parsed = raw
                    .parse::<DitherMode>()
                    .map_err(|error| error.to_string())?;
                if dither_mode.replace(parsed).is_some() {
                    return Err("--dither may be specified only once".to_owned());
                }
            }
            "--seed" => {
                let parsed = parse_u64(raw, "dither seed")?;
                if dither_seed.replace(parsed).is_some() {
                    return Err("--seed may be specified only once".to_owned());
                }
            }
            "--clip" => {
                let parsed = raw
                    .parse::<ClipPolicy>()
                    .map_err(|error| error.to_string())?;
                if clip_policy.replace(parsed).is_some() {
                    return Err("--clip may be specified only once".to_owned());
                }
            }
            _ => unreachable!("option validated above"),
        }
        index += 2;
    }
    Ok(Command::Analyze {
        input: input.into(),
        output_rate: output_rate.ok_or_else(|| "analyze requires -r RATE".to_owned())?,
        quality: QualityOptions {
            preset: preset.unwrap_or_default(),
            error_floor,
            working_precision_bits,
            signal_fractional_bits,
            dither_mode,
            dither_seed: dither_seed.unwrap_or(0),
            clip_policy: clip_policy.unwrap_or_default(),
            gain,
            dc_block_radius,
            channel_mix,
            convolution,
            block_frames: None,
            refinement: refinement_args.limits,
            gates: gate_args.finish()?,
            design_certificate: design_certificate_args.finish()?,
            window: window_args.finish()?,
            designer_controls: window_args.controls,
        },
    })
}

fn parse_nonzero_u32(raw: &str, name: &str) -> Result<u32, String> {
    let value = raw
        .parse::<u32>()
        .map_err(|_| format!("invalid integer {name}: {raw}"))?;
    if value == 0 {
        return Err(format!("{name} must be non-zero"));
    }
    Ok(value)
}

fn parse_sample_rate(raw: &str) -> Result<SampleRate, String> {
    raw.parse()
        .map_err(|error: sexrate::ParseRateError| error.to_string())
}

fn parse_working_precision(raw: &str) -> Result<Option<u32>, String> {
    if raw == "auto" {
        Ok(None)
    } else {
        parse_nonzero_u32(raw, "working precision").map(Some)
    }
}

fn parse_u64(raw: &str, name: &str) -> Result<u64, String> {
    raw.parse::<u64>()
        .map_err(|_| format!("invalid unsigned integer {name}: {raw}"))
}

fn parse_pcm_bits(raw: &str) -> Result<u16, String> {
    let bits = raw
        .parse::<u16>()
        .map_err(|_| format!("invalid integer PCM width: {raw}"))?;
    if !(1..=32).contains(&bits) {
        return Err(format!("integer PCM width must be in 1..=32, got {bits}"));
    }
    Ok(bits)
}

#[derive(Debug)]
struct ProcessingSummary {
    input_backend: String,
    output_backend: String,
    block_frames: u32,
    dsp_accumulator_bits: u32,
    signal_plan: SignalPrecisionPlan,
    numerical_report: String,
    numerical_reference: String,
    output_spec: AudioSpec,
    frames: u64,
    saturated_samples: u64,
    filter: Option<FilterSummary>,
    quantization: Option<QuantizationSummary>,
    pcm_error: Option<sexdsp::pcm_error::PcmErrorStats>,
    normalization: Option<NormalizationSummary>,
    gain: Option<GainSummary>,
    dc_block: Option<DcBlockSummary>,
    channel_mix: Option<ChannelMixSummary>,
    convolution: Option<ConvolutionSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GainSummary {
    requested: LinearGain,
    coefficient_raw: Integer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DcBlockSummary {
    requested_radius: DcBlockRadius,
    coefficient_raw: Integer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChannelMixSummary {
    matrix: ChannelMatrix,
    coefficients: Vec<BigQ>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConvolutionSummary {
    spec: ConvolutionSpec,
    coefficients: Vec<BigQ>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NormalizationSummary {
    measured_peak_raw: Integer,
    dither_guard_raw: Integer,
    scale_numerator: Integer,
    scale_denominator: Integer,
}

#[derive(Clone, Copy, Debug)]
struct QuantizationSummary {
    mode: DitherMode,
    seed: u64,
    clip_policy: ClipPolicy,
}

#[derive(Debug)]
struct FilterSummary {
    plan: PrecisionPlan,
    taps_per_phase: usize,
    accumulator_bits: u32,
    execution_backend: &'static str,
    max_coefficient_error: String,
    response_error_bound: String,
    coefficient_sha256: String,
    cache_status: Option<CacheStatus>,
    qualification_report: String,
}

#[derive(Debug)]
enum PlannedDesign {
    Native(DesignedFilter),
    Big(DesignedBigFilter),
    Windowed(sexfir::windowed::Designed),
    Optimized(sexfir::optimized::Designed),
}

enum AudioReader {
    Wave {
        reader: WaveReader<BufReader<File>>,
        metadata: AudioMetadata,
    },
    SndFile(SndFileReader),
}

impl AudioReader {
    fn open(path: &Path) -> Result<Self, Box<dyn Error>> {
        if is_wave_path(path) {
            let input = File::open(path)?;
            let reader = WaveReader::new(BufReader::new(input))?;
            let metadata = match SndFileLibrary::load() {
                Ok(library) => library.open(path)?.metadata().clone(),
                Err(SndFileError::LibraryUnavailable(_) | SndFileError::MissingSymbol { .. }) => {
                    AudioMetadata::default()
                }
                Err(error) => return Err(error.into()),
            };
            Ok(Self::Wave { reader, metadata })
        } else {
            Ok(Self::SndFile(SndFileLibrary::load()?.open(path)?))
        }
    }

    fn spec(&self) -> AudioSpec {
        match self {
            Self::Wave { reader, .. } => reader.spec(),
            Self::SndFile(reader) => reader.spec(),
        }
    }

    fn metadata(&self) -> AudioMetadata {
        match self {
            Self::Wave { metadata, .. } => metadata.clone(),
            Self::SndFile(reader) => reader.metadata().clone(),
        }
    }

    fn backend_identity(&self) -> Result<String, SndFileError> {
        match self {
            Self::Wave { .. } => Ok("sexio built-in integer WAVE".to_owned()),
            Self::SndFile(reader) => Ok(format!("sexio-sndfile ({})", reader.library_version()?)),
        }
    }

    fn read_frames(&mut self, max_frames: usize) -> Result<InterleavedChunk, Box<dyn Error>> {
        match self {
            Self::Wave { reader, .. } => Ok(reader.read_frames(max_frames)?),
            Self::SndFile(reader) => Ok(reader.read_frames(max_frames)?),
        }
    }
}

enum AudioWriter {
    Wave(WaveWriter<BufWriter<File>>),
    SndFile(SndFileWriter),
}

impl AudioWriter {
    fn create(
        path: &Path,
        spec: AudioSpec,
        metadata: &AudioMetadata,
    ) -> Result<Self, Box<dyn Error>> {
        if is_wave_path(path) && metadata.entries().is_empty() {
            let output = File::create(path)?;
            Ok(Self::Wave(WaveWriter::new(BufWriter::new(output), spec)?))
        } else {
            Ok(Self::SndFile(
                SndFileLibrary::load()?.create(path, spec, metadata)?,
            ))
        }
    }

    fn write_frames(
        &mut self,
        interleaved: &[sexq::Q1_63],
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Wave(writer) => Ok(writer.write_frames(interleaved, rounding, overflow)?),
            Self::SndFile(writer) => Ok(writer.write_frames(interleaved, rounding, overflow)?),
        }
    }

    fn write_signed_pcm_frames(&mut self, interleaved: &[i64]) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Wave(writer) => Ok(writer.write_signed_pcm_frames(interleaved)?),
            Self::SndFile(writer) => Ok(writer.write_signed_pcm_frames(interleaved)?),
        }
    }

    fn backend_identity(&self) -> Result<String, SndFileError> {
        match self {
            Self::Wave(_) => Ok("sexio built-in integer WAVE".to_owned()),
            Self::SndFile(writer) => Ok(format!("sexio-sndfile ({})", writer.library_version()?)),
        }
    }

    fn finalize(self) -> Result<WriteReport, Box<dyn Error>> {
        match self {
            Self::Wave(writer) => Ok(writer.finalize()?),
            Self::SndFile(writer) => Ok(writer.finalize()?),
        }
    }
}

fn is_wave_path(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("wav") || extension.eq_ignore_ascii_case("wave")
    })
}

struct OutputTransaction {
    destination: PathBuf,
    temporary: PathBuf,
    published: bool,
}

impl OutputTransaction {
    fn reserve(destination: &Path) -> Result<Self, std::io::Error> {
        let directory = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        loop {
            let sequence = NEXT_OUTPUT_TRANSACTION.fetch_add(1, Ordering::Relaxed);
            let mut name = OsString::from(format!(".sex-output-{}-{sequence}", std::process::id()));
            if let Some(extension) = destination.extension() {
                name.push(".");
                name.push(extension);
            }
            let temporary = directory.join(name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => {
                    drop(file);
                    return Ok(Self {
                        destination: destination.to_owned(),
                        temporary,
                        published: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn temporary_path(&self) -> &Path {
        &self.temporary
    }

    fn publish(mut self) -> Result<(), std::io::Error> {
        File::open(&self.temporary)?.sync_all()?;
        fs::rename(&self.temporary, &self.destination)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for OutputTransaction {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

impl PlannedDesign {
    fn summary(
        &self,
        plan: PrecisionPlan,
        cache_status: Option<CacheStatus>,
        assessment: &refinement::Assessment,
        gates: quality_gates::Gates,
    ) -> Result<FilterSummary, Box<dyn Error>> {
        let hash = match self {
            Self::Native(filter) => &filter.report().coefficient_sha256,
            Self::Big(filter) => &filter.report().coefficient_sha256,
            Self::Windowed(filter) => &filter.report().coefficient_sha256,
            Self::Optimized(filter) => &filter.report().quantization.coefficient_sha256,
        };
        let mut qualification_report = qualified_report(&plan, gates, assessment, hash)?;
        if let Self::Windowed(filter) = self {
            let spec = filter.spec();
            qualification_report = qualification_report.replace("FIR qualification scope:", &format!(
                "qualified designer: {}; window={:?}; taps/phase={}; MPFR={}; configured Q1.63 MAC={} bits; rounding=nearest-ties-to-even; exact DC correction=last-largest\nFIR qualification scope:",
                filter.report().algorithm, spec.core.window, spec.core.taps_per_phase,
                spec.core.working_precision_bits, spec.accumulator_bits));
        }
        if let Self::Optimized(filter) = self {
            qualification_report = qualification_report.replace("FIR qualification scope:", &format!(
                "qualified designer: {}; spec={:?}; work={:?}; solver={:?}; rounding=nearest-ties-to-even; exact DC correction=last-largest\nFIR qualification scope:",
                filter.spec().method.algorithm(), filter.spec(), filter.report().work, filter.report().solution));
        }
        Ok(match self {
            Self::Native(design) => FilterSummary {
                plan,
                taps_per_phase: design.bank().taps_per_phase(),
                accumulator_bits: u32::from(design.report().required_accumulator_bits),
                execution_backend: design.bank().dot_backend().name(),
                max_coefficient_error: design.report().max_abs_coefficient_error_decimal.clone(),
                response_error_bound: design.report().max_phase_l1_error_decimal.clone(),
                coefficient_sha256: design.report().coefficient_sha256.clone(),
                cache_status,
                qualification_report,
            },
            Self::Big(design) => FilterSummary {
                plan,
                taps_per_phase: design.bank().taps_per_phase(),
                accumulator_bits: design.report().required_accumulator_bits,
                execution_backend: "gmp-bigint",
                max_coefficient_error: design.report().max_abs_coefficient_error_decimal.clone(),
                response_error_bound: design.report().max_phase_l1_error_decimal.clone(),
                coefficient_sha256: design.report().coefficient_sha256.clone(),
                cache_status,
                qualification_report,
            },
            Self::Windowed(design) => FilterSummary {
                plan,
                taps_per_phase: design.bank().taps_per_phase(),
                accumulator_bits: design.report().required_accumulator_bits,
                execution_backend: "gmp-bigint",
                max_coefficient_error: design.report().max_abs_coefficient_error_decimal.clone(),
                response_error_bound: design.report().max_phase_l1_error_decimal.clone(),
                coefficient_sha256: design.report().coefficient_sha256.clone(),
                cache_status,
                qualification_report,
            },
            Self::Optimized(design) => FilterSummary {
                plan,
                taps_per_phase: design.bank().taps_per_phase(),
                accumulator_bits: design.report().quantization.required_accumulator_bits,
                execution_backend: "gmp-bigint",
                max_coefficient_error: design
                    .report()
                    .quantization
                    .max_abs_coefficient_error_decimal
                    .clone(),
                response_error_bound: design
                    .report()
                    .quantization
                    .max_phase_l1_error_decimal
                    .clone(),
                coefficient_sha256: design.report().quantization.coefficient_sha256.clone(),
                cache_status,
                qualification_report,
            },
        })
    }
}

fn process_audio(
    input_path: &Path,
    output_path: &Path,
    requested_rate: Option<u32>,
    requested_bits: Option<u16>,
    quality: QualityOptions,
) -> Result<ProcessingSummary, Box<dyn Error>> {
    let paths_alias = input_path == output_path
        || (output_path.exists()
            && std::fs::canonicalize(input_path)? == std::fs::canonicalize(output_path)?);
    if paths_alias {
        return Err("input and output paths must differ".into());
    }
    let transaction = OutputTransaction::reserve(output_path)?;
    let summary = process_audio_to_path(
        input_path,
        transaction.temporary_path(),
        requested_rate,
        requested_bits,
        quality,
    )?;
    transaction.publish()?;
    Ok(summary)
}

fn analyze_rate_design(
    input_path: &Path,
    output_rate: SampleRate,
    quality: QualityOptions,
) -> Result<(), Box<dyn Error>> {
    let reader = AudioReader::open(input_path)?;
    let input_spec = reader.spec();
    processing::validate_input_layout(input_spec, &quality)?;
    let ratio = output_rate.ratio_from(SampleRate::from_hz(u64::from(input_spec.sample_rate))?)?;
    let execution = numerical_plan::plan_execution(ratio, &quality, ratio.up() != ratio.down())?;
    let prepared = prepare_design(execution, &quality)?;
    if ratio.up() == ratio.down() {
        println!(
            "unity-rate analysis designs a hypothetical FIR; same-rate conversion bypasses this bank"
        );
    }
    let execution = prepared.execution;
    let plan = execution.filter.clone();
    let design = prepared.design;
    let cache_status = prepared.cache;
    let assessment = prepared.assessment;
    let response = &assessment.response;
    let mut filter = design.summary(plan.clone(), cache_status, &assessment, quality.gates)?;
    let signal_format = BigQFormat::new(
        execution.signal.integer_bits,
        execution.signal.fractional_bits,
    )?;
    processing::summarize_signal_execution(&design, signal_format, &mut filter)?;
    let accumulator_headroom = filter
        .plan
        .planned_accumulator_bits
        .checked_sub(filter.accumulator_bits)
        .ok_or("execution accumulator plan is narrower than the required bound")?;

    print_precision_plan(&execution, &quality)?;
    if output_rate.container_hz().is_err() {
        println!(
            "requested output rate: {output_rate} Hz; analysis only, current output adapters require u32 integer Hz"
        );
    }
    println!("response scope: resampler FIR only; effect-chain values above are numerical bounds");
    println!(
        "filter: {}, {} phases × {} taps, MPFR precision: {} bits",
        match &design {
            PlannedDesign::Native(design) => design.report().algorithm,
            PlannedDesign::Big(design) => design.report().algorithm,
            PlannedDesign::Windowed(design) => design.report().algorithm,
            PlannedDesign::Optimized(design) => design.report().quantization.algorithm,
        },
        plan.ratio.up(),
        filter.taps_per_phase,
        plan.working_precision_bits
    );
    println!(
        "passband end: {} × input rate",
        response.passband_end_decimal
    );
    println!("passband ripple: {} dB", response.passband_ripple_db);
    println!(
        "maximum passband deviation: {}",
        response.max_passband_deviation_decimal
    );
    println!(
        "stopband start: {} × input rate",
        response.stopband_start_decimal
    );
    println!("stopband peak: {} dB", response.stopband_peak_db);
    println!(
        "coefficient quantization error: {} dB (max coefficient)",
        match &design {
            PlannedDesign::Native(design) => &design.report().max_abs_coefficient_error_db,
            PlannedDesign::Big(design) => &design.report().max_abs_coefficient_error_db,
            PlannedDesign::Windowed(design) => &design.report().max_abs_coefficient_error_db,
            PlannedDesign::Optimized(design) =>
                &design.report().quantization.max_abs_coefficient_error_db,
        }
    );
    println!(
        "quantization response-error bound: {} dB",
        match &design {
            PlannedDesign::Native(design) => &design.report().max_phase_l1_error_db,
            PlannedDesign::Big(design) => &design.report().max_phase_l1_error_db,
            PlannedDesign::Windowed(design) => &design.report().max_phase_l1_error_db,
            PlannedDesign::Optimized(design) => &design.report().quantization.max_phase_l1_error_db,
        }
    );
    println!(
        "accumulator: {} bits required, {} planned headroom bit(s)",
        filter.accumulator_bits, accumulator_headroom
    );
    println!(
        "accumulator scope: full declared Q{}.{} input range; {} bits planned; not measured audio peak",
        signal_format.integer_bits(),
        signal_format.fractional_bits(),
        filter.plan.planned_accumulator_bits
    );
    println!("execution backend: {}", filter.execution_backend);
    println!("coefficient sha256: {}", filter.coefficient_sha256);
    if let Some(status) = filter.cache_status {
        println!(
            "coefficient cache: {}",
            match status {
                CacheStatus::Hit => "hit",
                CacheStatus::DesignedAndStored => "miss; designed and stored",
            }
        );
    } else {
        println!("coefficient cache: disabled");
    }
    print!("{}", filter.qualification_report);
    print!("{}", prepared.design_certificate_report);
    Ok(())
}

fn harmonic_report(report: &harmonics::Report) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    writeln!(&mut text,
            "harmonic scope: sampled complete quantized-bank transfer for unit complex input tones; all images, not a continuous proof, effects, or PCM noise"
        ).expect("writing a String cannot fail");
    writeln!(&mut text,
            "harmonic grid: {} input frequencies across [0, 1/2] × {} output harmonics; MPFR {} bits; planned work {}",
            report.frequency_points,
            report.harmonics_per_point,
            report.precision_bits,
            report.planned_work
        ).expect("writing a String cannot fail");
    writeln!(
        &mut text,
        "harmonic input bands: passband [0, {}], stopband [{}, 1/2]",
        report.passband_end, report.stopband_start
    )
    .expect("writing a String cannot fail");
    writeln!(
        &mut text,
        "main harmonic passband ripple: {} dB; complex error: {} dB; phase error: {} rad",
        report.main_passband_ripple_db,
        report.main_complex_error_db,
        report.main_phase_error_radians
    )
    .expect("writing a String cannot fail");
    if let Some(peak) = &report.image_peak {
        writeln!(
            &mut text,
            "image peak: {} dB; input frequency {}; image {}; output frequency {}",
            peak.amplitude_db, peak.input_frequency, peak.image, peak.output_frequency
        )
        .expect("writing a String cannot fail");
        writeln!(
            &mut text,
            "image L2 peak: {} dB; input frequency {}",
            report.image_l2_peak_db.as_ref().unwrap(),
            report.image_l2_peak_input_frequency.as_ref().unwrap()
        )
        .expect("writing a String cannot fail");
    } else {
        writeln!(
            &mut text,
            "image peak: not applicable (L=1; no separate image components)"
        )
        .expect("writing a String cannot fail");
    }
    let peak = &report.stopband_peak;
    writeln!(
        &mut text,
        "stopband harmonic peak: {} dB; input frequency {}; image {}; output frequency {}",
        peak.amplitude_db, peak.input_frequency, peak.image, peak.output_frequency
    )
    .expect("writing a String cannot fail");
    writeln!(&mut text,
            "harmonic sampled compliance ({}): main-complex-error={}, images={}, image-L2={}, stopband={}",
            report.target,
            report.main_complex_error_meets_target,
            report.image_peak_meets_target,
            report.image_l2_meets_target,
            report.stopband_meets_target
        ).expect("writing a String cannot fail");
    text
}

fn qualified_report(
    plan: &PrecisionPlan,
    gates: quality_gates::Gates,
    assessment: &refinement::Assessment,
    hash: &str,
) -> Result<String, Box<dyn Error>> {
    if assessment.sampled.target != plan.target_error_floor
        || assessment.response.grid_points_per_band != gates.grid
        || assessment.response.analyzed_phases as u64 != plan.ratio.up()
        || !assessment.coefficient_budget_passed
        || !assessment.response_passed()
        || gates.certificate.is_some() != assessment.continuous.is_some()
        || gates.harmonics.is_some() != assessment.harmonic.is_some()
    {
        return Err("cannot report an inconsistently qualified FIR".into());
    }
    let mut text = format!(
        "qualified FIR sha256: {hash}\n\
         qualification target: {}; grid: {} points/band; coefficient-budget: {}; actual C: {}\n\
         error-floor compliance: passband={}, stopband={}, coefficient-quantization={}\n",
        plan.target_error_floor,
        gates.grid,
        assessment.coefficient_budget_passed,
        assessment.actual_coefficient_fractional_bits,
        assessment.sampled.passband_deviation_meets_target,
        assessment.sampled.stopband_meets_target,
        assessment.sampled.coefficient_quantization_meets_target,
    );
    text.push_str(&format!(
        "qualified phase grid: ripple={} dB; deviation={}; stopband={} dB; pass-end={}; stop-start={}\n\
         coefficient L1 quantization bound: {} * 2^-{}; budget 2^-{}\n",
        assessment.response.passband_ripple_db, assessment.response.max_passband_deviation_decimal,
        assessment.response.stopband_peak_db, assessment.response.passband_end_decimal,
        assessment.response.stopband_start_decimal, plan.taps_per_phase - 1,
        assessment.actual_coefficient_fractional_bits,
        sexplan::required_amplitude_bits(plan.target_error_floor) + 3,
    ));
    if plan.ratio.up() == plan.ratio.down() {
        text.push_str("stopband geometry: Nyquist endpoint only (unity ratio); not a finite-width rejection band\n");
    }
    if let Some(limits) = gates.harmonics {
        text.push_str(&format!("harmonic limits: work={}, coefficients={}, phases={}, precision={}, storage-bytes={}\n", limits.max_work, limits.max_coefficients, limits.max_phases, limits.max_precision_bits, limits.max_storage_bytes));
        text.push_str(&harmonic_report(assessment.harmonic.as_ref().unwrap()));
    }
    if let Some(limits) = gates.certificate {
        text.push_str(&format!(
            "certificate limits: work={}, taps={}, depth={}, endpoint-bits={}, integer-bits={}\n",
            limits.max_work,
            limits.max_taps,
            limits.max_depth,
            limits.endpoint_bits,
            limits.max_integer_bits
        ));
        text.push_str(&certificate_report(
            assessment.continuous.as_ref().unwrap(),
            plan.target_error_floor,
        ));
    }
    text.push_str("FIR qualification scope: actual quantized bank; excludes signal rounding, effects, MPFR design error, and final PCM; sampled grids are not continuous proofs\n");
    Ok(text)
}

fn certificate_report(proof: &certificate::Certificate, target: ErrorFloor) -> String {
    let mut report = format!(
        "continuous certificate: {:?}; target {target}\n\
         certificate scope: actual quantized per-phase FIR amplitude; not design error, effects, PCM, or full expanded anti-imaging band\n\
         certificate method: exact rational power polynomial + Bernstein subdivision; directed MPFR endpoints/targets\n\
         certificate work: {}; cells: {}; complete phases: {}/{}\n",
        proof.outcome,
        proof.charged_work,
        proof.visited_cells,
        proof.certified_phases,
        proof.expected_phases,
    );
    if let Some((deviation, stopband)) = proof.upper_enclosures() {
        report.push_str(&format!(
            "continuous passband deviation upper bound: {deviation}\n\
             continuous stopband upper bound: {stopband} dB (input-frequency scope)\n"
        ));
    }
    if let Some(witness) = &proof.witness {
        report.push_str(&format!(
            "certificate violation witness: phase {}, {:?}, cos(2*pi*f)={}\n",
            witness.phase, witness.band, witness.cosine_coordinate,
        ));
    }
    report
}

fn show_rate_plan(
    input_path: &Path,
    output_rate: SampleRate,
    quality: QualityOptions,
) -> Result<(), Box<dyn Error>> {
    let reader = AudioReader::open(input_path)?;
    processing::validate_input_layout(reader.spec(), &quality)?;
    let ratio =
        output_rate.ratio_from(SampleRate::from_hz(u64::from(reader.spec().sample_rate))?)?;
    let execution = numerical_plan::plan_execution(ratio, &quality, ratio.up() != ratio.down())?;
    print_precision_plan(&execution, &quality)?;
    if output_rate.container_hz().is_err() {
        println!(
            "requested output rate: {output_rate} Hz; analysis only, current output adapters require u32 integer Hz"
        );
    }
    if ratio.up() == ratio.down() {
        println!(
            "quality feedback: same-rate conversion bypasses FIR; no filter qualification work"
        );
        if quality.design_certificate.is_some() {
            println!(
                "design certificate is not applicable to same-rate conversion; --certify-design is rejected there, while analyze certifies a hypothetical FIR"
            );
        }
        if matches!(
            quality.window,
            refinement::Designer::GlobalLeastSquares | refinement::Designer::Equiripple
        ) {
            println!(
                "selected designer is not applicable: conversion rejects --designer global-ls|remez without a rate change"
            );
        } else if quality.window != refinement::Designer::Kaiser {
            println!(
                "selected window is not applicable: conversion rejects non-Kaiser --window without a rate change"
            );
        }
        if quality.gates.has_supplemental() {
            println!(
                "initial gate configuration: not applicable; conversion rejects --certify/--harmonics without a rate change"
            );
        }
    } else {
        println!(
            "quality feedback: mandatory at materialization; this plan is an unqualified initial candidate"
        );
        let limits = quality
            .refinement
            .resolve(&execution.filter, quality.gates.grid)?;
        let coefficients = execution.filter.coefficient_count;
        let grid = quality.gates.grid;
        let terms = coefficients
            .checked_mul(2 * u128::from(grid))
            .ok_or("initial response-term count overflow")?;
        println!(
            "initial quality work ({grid} points/band): {coefficients} coefficients, {terms} response terms, {} MPFR bits; cumulative limits: {} coefficients, {} terms, {} bits, {} candidates",
            execution.filter.working_precision_bits,
            limits.max_total_coefficients,
            limits.max_response_terms,
            limits.max_precision_bits,
            limits.max_attempts,
        );
        if coefficients > u128::from(limits.max_total_coefficients) {
            println!("initial candidate needs --refinement-coefficients at least {coefficients}");
        }
        if terms > u128::from(limits.max_response_terms) {
            println!("initial candidate needs --refinement-terms at least {terms}");
        }
        if execution.filter.working_precision_bits > limits.max_precision_bits {
            println!(
                "initial candidate needs --refinement-precision-limit at least {}",
                execution.filter.working_precision_bits
            );
        }
        println!("requested gate settings: {:?}", quality.gates);
        match windows::preflight(&execution.filter, quality.window, quality.designer_controls) {
            Ok(()) => {
                println!("initial designer preflight: within static limits; not a quality pass")
            }
            Err(error) => println!("initial designer preflight: blocked: {error}"),
        }
        match quality.gates.preflight(&execution.filter) {
            Ok(work) => {
                println!(
                    "initial supplemental preflight: within static limits; no response or proof evaluated"
                );
                if let Some(work) = work {
                    println!("initial harmonic work: {work} (per candidate)");
                }
            }
            Err(error) => println!("initial supplemental preflight: blocked: {error}"),
        }
    }
    Ok(())
}

fn print_precision_plan(
    execution: &numerical_plan::ExecutionPlan,
    quality: &QualityOptions,
) -> Result<(), Box<dyn Error>> {
    let plan = &execution.filter;
    let signal = execution.signal;
    println!("ratio: {}/{}", plan.ratio.up(), plan.ratio.down());
    if let Some(controls) = &quality.design_certificate {
        println!("{}", controls.plan_description(plan));
        if let Err(error) = controls.preflight(plan, quality) {
            println!("initial design certificate preflight: blocked: {error}");
        }
    }
    if let Some(spec) = quality.window.optimized_spec(plan)? {
        println!(
            "designer: {}; initial length heuristic: Kaiser; acceptance: measured/proven actual bank",
            quality.window.as_str()
        );
        println!("optimized FIR specification: {spec:?}");
        println!(
            "optimized designer limits: {:?}; cumulative work limit {}",
            quality.designer_controls.limits, quality.designer_controls.total_work
        );
        match spec.preflight(quality.designer_controls.limits) {
            Ok(work) => println!(
                "initial optimized designer work: {work:?}; unqualified, no solver executed"
            ),
            Err(error) => println!("initial optimized designer preflight: blocked: {error}"),
        }
        println!(
            "optimized shape search: double half-length without changing target, bands, weights or method; LS grid follows global prototype size"
        );
    } else {
        println!(
            "window: {}; initial length heuristic: Kaiser; acceptance: measured/proven actual selected window",
            quality.window.as_str()
        );
        match quality.window {
            refinement::Designer::Kaiser => {}
            refinement::Designer::DolphChebyshev => println!(
                "window shape: Dolph attenuation {} dB (window parameter, not achieved FIR rejection); response search doubles half length and adds 12 dB",
                plan.design_attenuation_db
            ),
            _ => println!(
                "window shape: fixed; design attenuation is only an initial length heuristic, not a window parameter; response search doubles half length"
            ),
        }
    }
    println!(
        "plan: preset {}, target {}, design attenuation -{}dB",
        plan.preset, plan.target_error_floor, plan.design_attenuation_db
    );
    println!(
        "transition: {}/{} of lower Nyquist; {} phases × {} taps = {} coefficients",
        plan.transition_width_of_lower_nyquist.numerator(),
        plan.transition_width_of_lower_nyquist.denominator(),
        plan.ratio.up(),
        plan.taps_per_phase,
        plan.coefficient_count
    );
    println!(
        "precision: {} amplitude bits, {} coefficient fractional bits, {} sum guard bits, {} MPFR bits",
        plan.amplitude_error_bits,
        plan.coefficient_fractional_bits,
        plan.coefficient_sum_guard_bits,
        plan.working_precision_bits
    );
    println!(
        "accumulator: {} bits planned; backend: {}",
        plan.planned_accumulator_bits, plan.backend
    );
    println!(
        "signal: Q{}.{}; effect coefficient fractional bits: {}; execution accumulator plan: {} bits",
        signal.integer_bits,
        signal.fractional_bits,
        signal.effect_coefficient_fractional_bits,
        plan.planned_accumulator_bits + signal.integer_bits + signal.fractional_bits - 64
    );
    println!(
        "{}",
        numerical_plan::report(
            &execution.numerical,
            plan.amplitude_error_bits + 2,
            execution.refinements
        )
    );
    for stage in &execution.numerical.stages {
        println!(
            "  {}: ideal peak <= {}; numerical error <= {}",
            stage.name,
            numerical_plan::format_bound(&execution.numerical, &stage.ideal_peak_raw),
            numerical_plan::format_bound(&execution.numerical, &stage.total_error_raw())
        );
    }
    if let Some(l1) = &execution.fir_l1 {
        println!("{}", numerical_plan::fir_l1_report(l1));
    }
    println!(
        "{}",
        numerical_plan::reference(execution.certified_fir_reference)
    );
    if let Some(mode) = quality.dither_mode {
        println!(
            "final dither override: {mode}, deterministic seed {}",
            quality.dither_seed
        );
    } else {
        println!(
            "planned final dither: {}, deterministic seed {}",
            plan.dither, quality.dither_seed
        );
    }
    println!("clipping policy: {}", quality.clip_policy);
    Ok(())
}

fn select_dither_mode(
    override_mode: Option<DitherMode>,
    policy: DitherPolicy,
) -> Result<DitherMode, Box<dyn Error>> {
    if let Some(mode) = override_mode {
        return Ok(mode);
    }
    match policy {
        DitherPolicy::Tpdf => Ok(DitherMode::Tpdf),
        DitherPolicy::HighPassTpdf => Ok(DitherMode::HighPassTpdf),
        DitherPolicy::NoiseShaped { order } => Ok(DitherMode::NoiseShaped { order }),
    }
}

fn plan_and_design(
    plan: PrecisionPlan,
) -> Result<(PrecisionPlan, PlannedDesign, Option<CacheStatus>), Box<dyn Error>> {
    let cache_directory = coefficient_cache_directory();
    let cache_is_explicit = env::var_os("SEX_COEFFICIENT_CACHE").is_some();
    let (design, cache_status) = match plan.backend {
        BackendRequirement::NativeI128 => {
            let spec = KaiserSpec::from_precision_plan(&plan)?;
            if let Some(directory) = &cache_directory {
                match design_kaiser_cached(directory, &spec) {
                    Ok((design, status)) => (PlannedDesign::Native(design), Some(status)),
                    Err(CacheError::Io(error)) if !cache_is_explicit => {
                        eprintln!(
                            "sex: default coefficient cache is unavailable ({error}); designing without cache"
                        );
                        (PlannedDesign::Native(design_kaiser(&spec)?), None)
                    }
                    Err(error) => return Err(error.into()),
                }
            } else {
                (PlannedDesign::Native(design_kaiser(&spec)?), None)
            }
        }
        BackendRequirement::WideInteger { .. } | BackendRequirement::BigInt { .. } => {
            let spec = BigKaiserSpec::from_precision_plan(&plan)?;
            if let Some(directory) = &cache_directory {
                match design_kaiser_big_cached(directory, &spec) {
                    Ok((design, status)) => (PlannedDesign::Big(design), Some(status)),
                    Err(CacheError::Io(error)) if !cache_is_explicit => {
                        eprintln!(
                            "sex: default coefficient cache is unavailable ({error}); designing without cache"
                        );
                        (PlannedDesign::Big(design_kaiser_big(&spec)?), None)
                    }
                    Err(error) => return Err(error.into()),
                }
            } else {
                (PlannedDesign::Big(design_kaiser_big(&spec)?), None)
            }
        }
    };
    Ok((plan, design, cache_status))
}

struct PreparedDesign {
    execution: numerical_plan::ExecutionPlan,
    design: PlannedDesign,
    cache: Option<CacheStatus>,
    assessment: refinement::Assessment,
    design_certificate_report: String,
    response_work: (u32, u64, u64),
}

fn prepare_design(
    mut execution: numerical_plan::ExecutionPlan,
    quality: &QualityOptions,
) -> Result<PreparedDesign, Box<dyn Error>> {
    let Some(controls) = &quality.design_certificate else {
        return prepare_response_design(execution, quality);
    };
    let mut remaining_quality = quality.clone();
    let mut limits = quality
        .refinement
        .resolve(&execution.filter, quality.gates.grid)?;
    let mut remaining_work = controls.work_limit();
    let mut redesigns = 0_u32;
    loop {
        remaining_quality.refinement = limits.into();
        let mut prepared = prepare_response_design(execution, &remaining_quality)?;
        limits.max_attempts = limits
            .max_attempts
            .checked_sub(prepared.response_work.0)
            .ok_or("response attempt accounting overflow")?;
        limits.max_total_coefficients = limits
            .max_total_coefficients
            .checked_sub(prepared.response_work.1)
            .ok_or("response coefficient accounting overflow")?;
        limits.max_response_terms = limits
            .max_response_terms
            .checked_sub(prepared.response_work.2)
            .ok_or("response work accounting overflow")?;
        controls.preflight(&prepared.execution.filter, quality)?;
        let target = numerical_plan::certificate_target(&prepared.execution)?;
        match controls.certify(
            &prepared.execution.filter,
            &prepared.design,
            target.as_ref(),
            remaining_work,
        ) {
            Ok((mut report, certificate)) => {
                remaining_work = remaining_work
                    .checked_sub(certificate.total_series_terms())
                    .ok_or("certificate work accounting overflow")?;
                numerical_plan::merge_design_certificate(
                    &mut prepared.execution,
                    quality,
                    &certificate,
                )?;
                use std::fmt::Write as _;
                writeln!(
                    report,
                    "design certificate aggregate: {redesigns} bank redesign(s), {} total I0 terms across all banks; remaining response candidates {}",
                    controls.work_limit() - remaining_work,
                    limits.max_attempts
                )?;
                prepared.design_certificate_report = report;
                return Ok(prepared);
            }
            Err(error) => {
                let Some(sexfir::enclosure::CertificationError::TargetViolated {
                    lower_bound,
                    upper_bound,
                    target,
                    total_series_terms,
                    ..
                }) = error.downcast_ref::<sexfir::enclosure::CertificationError>()
                else {
                    return Err(format!(
                        "design certificate failed: {error}; no design-error compliance is claimed"
                    )
                    .into());
                };
                remaining_work = remaining_work
                    .checked_sub(*total_series_terms)
                    .ok_or("certificate work accounting overflow")?;
                let rejected_identity = match &prepared.design {
                    PlannedDesign::Native(bank) => &bank.report().coefficient_sha256,
                    PlannedDesign::Big(bank) => &bank.report().coefficient_sha256,
                    _ => return Err("certified redesign requires a Kaiser bank".into()),
                };
                eprintln!("design certificate refuted coefficient sha256 {rejected_identity}");
                if remaining_work == 0
                    || limits.max_attempts == 0
                    || limits.max_total_coefficients == 0
                    || limits.max_response_terms == 0
                {
                    return Err("design certificate proved a target violation, but aggregate redesign resources are exhausted; no output published".into());
                }
                let next = design_certificate::redesign_plan(
                    &prepared.execution.filter,
                    upper_bound,
                    target,
                    quality.working_precision_bits,
                )?;
                if next.working_precision_bits > limits.max_precision_bits {
                    return Err("certified bank redesign exceeds refinement precision limit; no output published".into());
                }
                eprintln!(
                    "design certificate proved joint error above target (lower bound {lower_bound}); redesign {}: C {} -> {}, MPFR {} -> {}; shape and response target unchanged",
                    redesigns + 1,
                    prepared
                        .execution
                        .filter
                        .coefficient_fractional_bits
                        .max(62),
                    next.coefficient_fractional_bits,
                    prepared.execution.filter.working_precision_bits,
                    next.working_precision_bits
                );
                execution = numerical_plan::plan_from_filter(
                    next,
                    quality,
                    prepared.execution.filter.ratio.up() != prepared.execution.filter.ratio.down(),
                )?;
                redesigns += 1;
            }
        }
    }
}

fn prepare_response_design(
    execution: numerical_plan::ExecutionPlan,
    quality: &QualityOptions,
) -> Result<PreparedDesign, Box<dyn Error>> {
    if let Some(controls) = &quality.design_certificate {
        controls.preflight(&execution.filter, quality)?;
    }
    let quality_gates::Gates {
        grid,
        certificate,
        harmonics,
    } = quality.gates;
    quality.gates.preflight(&execution.filter)?;
    windows::preflight(&execution.filter, quality.window, quality.designer_controls)?;
    let resampling = execution.filter.ratio.up() != execution.filter.ratio.down();
    let request = refinement::Request {
        designer: quality.window,
        designer_limits: quality.designer_controls.limits,
        max_design_terms: quality.designer_controls.total_work,
        grid,
        limits: quality.refinement.resolve(&execution.filter, grid)?,
        explicit_working_bits: quality.working_precision_bits,
        certificate,
        harmonics,
    };
    if execution.filter.preset == QualityPreset::Until40k {
        eprintln!(
            "Until-40k cumulative quality budgets: {} coefficients, {} response terms, {} MPFR bits, {} candidates; unset coefficient/term budgets reserve four initial-bank equivalents above generic floors; explicit limits are never raised",
            request.limits.max_total_coefficients,
            request.limits.max_response_terms,
            request.limits.max_precision_bits,
            request.limits.max_attempts,
        );
    }
    eprintln!(
        "quality gates: phase grid ({grid}/band), exact coefficient budget; continuous={}, all-image-grid={}; excludes MPFR design error and final PCM; grid measurements are not continuous bounds",
        certificate.is_some(),
        harmonics.is_some(),
    );
    let result = refinement::qualify(
        execution.filter,
        request,
        |plan| {
            numerical_plan::plan_from_filter(plan, quality, resampling)
                .map(|execution| execution.filter)
        },
        |plan| {
            let (_, design, cache) = if quality.window == refinement::Designer::Kaiser {
                plan_and_design(plan.clone())?
            } else {
                windows::plan_and_design(plan.clone(), quality.window, quality.designer_controls)?
            };
            Ok((design, cache))
        },
        |(design, _), plan, request| match design {
            PlannedDesign::Native(filter) => refinement::assess_native(filter, plan, request),
            PlannedDesign::Big(filter) => refinement::assess_big(filter, plan, request),
            PlannedDesign::Windowed(filter) => refinement::assess_windowed(filter, plan, request),
            PlannedDesign::Optimized(filter) => refinement::assess_optimized(filter, plan, request),
        },
    );
    let solver_failures = match &result {
        Ok(value) => &value.solver_failures,
        Err(error) => &error.solver_failures,
    };
    for failure in solver_failures {
        eprintln!(
            "designer numerical failure: {} taps/phase, C={}, MPFR={}; {}; next MPFR={:?}; no bank qualified",
            failure.plan.taps_per_phase,
            failure.plan.coefficient_fractional_bits,
            failure.plan.working_precision_bits,
            failure.error,
            failure.next_working_bits,
        );
    }
    let attempts = match &result {
        Ok(value) => &value.attempts,
        Err(error) => &error.attempts,
    };
    for (index, attempt) in attempts.iter().enumerate() {
        eprintln!(
            "quality candidate {}: {} taps/phase, C={}, MPFR={}, coefficient-budget={}, response={}, next={:?}",
            index + 1,
            attempt.plan.taps_per_phase,
            attempt.assessment.actual_coefficient_fractional_bits,
            attempt.plan.working_precision_bits,
            attempt.assessment.coefficient_budget_passed,
            attempt.assessment.response_passed(),
            attempt.next_change
        );
        if !attempt.assessment.response_passed() {
            let check = &attempt.assessment;
            eprintln!(
                "quality candidate {} sampled response: target {}, passband={}, deviation={}, stopband={}, peak={} dB; grid measurements are not continuous bounds",
                index + 1,
                attempt.plan.target_error_floor,
                check.sampled.passband_deviation_meets_target,
                check.response.max_passband_deviation_decimal,
                check.sampled.stopband_meets_target,
                check.response.stopband_peak_db,
            );
            if let Some(proof) = &check.continuous {
                eprint!(
                    "{}",
                    certificate_report(proof, attempt.plan.target_error_floor)
                );
            }
        }
    }
    let result = result?;
    if result.charged_design_terms != 0 {
        eprintln!(
            "optimized designer cumulative work: {} (cached candidates charged equally)",
            result.charged_design_terms
        );
    }
    eprintln!(
        "quality feedback accepted {} candidate(s): {} coefficient work, {} response terms; verification is scoped to the requested gates",
        result.attempts.len(),
        result.charged_coefficients,
        result.charged_response_terms
    );
    let mut execution = numerical_plan::plan_from_filter(result.plan.clone(), quality, resampling)?;
    if execution.filter != result.plan {
        return Err("final numerical replan changed the qualified filter".into());
    }
    numerical_plan::bind_fir_l1(&mut execution, quality, &result.design.0)?;
    Ok(PreparedDesign {
        design_certificate_report: String::new(),
        response_work: (
            (result.attempts.len() + result.solver_failures.len()) as u32,
            result.charged_coefficients,
            result.charged_response_terms,
        ),
        execution,
        design: result.design.0,
        cache: result.design.1,
        assessment: result
            .attempts
            .last()
            .ok_or("quality feedback accepted no assessed candidate")?
            .assessment
            .clone(),
    })
}

fn coefficient_cache_directory() -> Option<PathBuf> {
    if let Some(value) = env::var_os("SEX_COEFFICIENT_CACHE") {
        if value.is_empty() || value.to_str() == Some("off") {
            return None;
        }
        return Some(PathBuf::from(value));
    }
    env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|value| PathBuf::from(value).join(".cache"))
        })
        .map(|root| root.join("sex").join("coefficients"))
}

fn execute(command: Command, frontend: Frontend) -> Result<(), Box<dyn Error>> {
    match command {
        Command::Help => print!("{}", frontends::help(frontend)),
        Command::Version => println!("{} {}", frontend.name(), env!("CARGO_PKG_VERSION")),
        Command::BuildInfo => println!(
            "{} {}; arch={}; os={}; pointer_bits={}; endian={}; cross_gmp={}; qemu_only={}",
            frontend.name(),
            env!("CARGO_PKG_VERSION"),
            env::consts::ARCH,
            env::consts::OS,
            usize::BITS,
            if cfg!(target_endian = "little") {
                "little"
            } else {
                "big"
            },
            cfg!(feature = "cross-gmp"),
            matches!(option_env!("SEX_QEMU_VALIDATION_ONLY"), Some("1")),
        ),
        Command::Convert {
            input,
            output,
            output_rate,
            output_bits,
            quality,
        } => {
            let summary = process_audio(&input, &output, output_rate, output_bits, quality)?;
            eprintln!(
                "wrote {} frames, {} channel(s), {} Hz, {}-bit integer PCM; saturated samples: {}",
                summary.frames,
                summary.output_spec.channels,
                summary.output_spec.sample_rate,
                summary.output_spec.bits_per_sample,
                summary.saturated_samples
            );
            eprintln!(
                "I/O backends: input {}; output {}",
                summary.input_backend, summary.output_backend
            );
            eprintln!("streaming block: {} frame(s)", summary.block_frames);
            let signal = summary.signal_plan;
            eprintln!(
                "signal: Q{}.{}; pre-rate DSP accumulator: {} bits (0 means no effects)",
                signal.integer_bits, signal.fractional_bits, summary.dsp_accumulator_bits
            );
            eprintln!("{}", summary.numerical_report);
            eprintln!("{}", summary.numerical_reference);
            if let Some(gain) = summary.gain {
                eprintln!(
                    "input gain: exact linear {}, Q65.{} raw {}",
                    gain.requested, signal.effect_coefficient_fractional_bits, gain.coefficient_raw
                );
            }
            if let Some(dc_block) = summary.dc_block {
                eprintln!(
                    "DC removal: exact radius {}, Q2.{} raw {}",
                    dc_block.requested_radius,
                    signal.effect_coefficient_fractional_bits,
                    dc_block.coefficient_raw
                );
            }
            if let Some(channel_mix) = summary.channel_mix {
                let matrix = channel_mix.matrix;
                let raw = channel_mix
                    .coefficients
                    .iter()
                    .map(|coefficient| coefficient.raw().to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                eprintln!(
                    "channel mix: {} output(s) x {} input(s), exact [{}], Q2.{} raw [{}]",
                    matrix.output_channels,
                    matrix.input_channels,
                    matrix,
                    signal.effect_coefficient_fractional_bits,
                    raw
                );
            }
            if let Some(convolution) = summary.convolution {
                let raw = convolution
                    .coefficients
                    .iter()
                    .map(|tap| tap.raw().to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                eprintln!(
                    "convolution: {} causal tap(s), exact [{}], Q2.{} raw [{}]",
                    convolution.spec.taps.len(),
                    convolution.spec,
                    signal.effect_coefficient_fractional_bits,
                    raw
                );
            }
            if let Some(filter) = summary.filter {
                eprint!("{}", filter.qualification_report);
                eprintln!(
                    "plan: preset {}, target {}, {} coefficient fractional bits, {} sum guard bits, {}-bit accumulator",
                    filter.plan.preset,
                    filter.plan.target_error_floor,
                    filter.plan.coefficient_fractional_bits,
                    filter.plan.coefficient_sum_guard_bits,
                    filter.plan.planned_accumulator_bits
                );
                eprintln!(
                    "filter: {} taps/phase, {}-bit minimum accumulator, {} execution, coefficients sha256 {}",
                    filter.taps_per_phase,
                    filter.accumulator_bits,
                    filter.execution_backend,
                    filter.coefficient_sha256
                );
                eprintln!(
                    "coefficient max error: {}; quantization response-error bound: {}",
                    filter.max_coefficient_error, filter.response_error_bound
                );
                eprintln!(
                    "coefficient cache: {}",
                    match filter.cache_status {
                        Some(CacheStatus::Hit) => "hit",
                        Some(CacheStatus::DesignedAndStored) => "miss; designed and stored",
                        None => "disabled",
                    }
                );
            }
            if let Some(quantization) = summary.quantization {
                eprintln!(
                    "final quantization: {}, deterministic seed {}; clipping: {}",
                    quantization.mode, quantization.seed, quantization.clip_policy
                );
            }
            if let Some(error) = &summary.pcm_error {
                eprint!("{}", pcm_report::report(error));
            }
            if let Some(normalization) = summary.normalization {
                eprintln!(
                    "normalization: measured peak raw {}, dither guard {}, exact scale {}/{}",
                    normalization.measured_peak_raw,
                    normalization.dither_guard_raw,
                    normalization.scale_numerator,
                    normalization.scale_denominator
                );
            }
        }
        Command::Analyze {
            input,
            output_rate,
            quality,
        } => analyze_rate_design(&input, output_rate, quality)?,
        Command::Plan {
            input,
            output_rate,
            quality,
        } => show_rate_plan(&input, output_rate, quality)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(ToString::to_string))
    }

    #[test]
    fn gate_configuration_and_validation_are_identical_in_all_three_modes() {
        let controls = [
            "--certificate-work",
            "777",
            "--certify",
            "--harmonic-work",
            "999",
            "--harmonics",
            "--grid",
            "17",
            "--certificate-depth",
            "0",
        ];
        let mut settings = Vec::new();
        for prefix in [
            vec!["sex", "in.wav", "out.wav", "rate", "48000"],
            vec!["sex", "analyze", "in.wav", "-r", "48000"],
            vec!["sex", "plan", "in.wav", "-r", "48000"],
        ] {
            let mut args = prefix.clone();
            args.extend(controls);
            let quality = match parse(&args).unwrap() {
                Command::Convert { quality, .. }
                | Command::Analyze { quality, .. }
                | Command::Plan { quality, .. } => quality,
                _ => panic!(),
            };
            assert_eq!(quality.gates.grid, 17);
            assert_eq!(quality.gates.certificate.unwrap().max_work, 777);
            assert_eq!(quality.gates.certificate.unwrap().max_depth, 0);
            assert_eq!(quality.gates.harmonics.unwrap().max_work, 999);
            settings.push(quality.gates);
            for invalid in [
                vec!["--grid", "1"],
                vec!["--grid", "2", "--grid", "3"],
                vec!["--grid", "4294967296"],
                vec!["--certify", "--certify"],
                vec!["--harmonics", "--harmonics"],
                vec!["--certificate-work", "1"],
                vec!["--harmonic-work", "1"],
                vec!["--certify", "--certificate-taps", "0"],
                vec!["--certify", "--certificate-depth", "65"],
                vec!["--harmonics", "--harmonic-phases", "0"],
                vec!["--harmonics", "--harmonic-precision-limit", "4294967296"],
                vec!["--harmonic-unknown", "1"],
                vec!["--certificate-unknown", "1"],
            ] {
                let mut args = prefix.clone();
                args.extend(invalid);
                assert!(parse(&args).is_err(), "{args:?}");
            }
        }
        assert_eq!(settings[0], settings[1]);
        assert_eq!(settings[0], settings[2]);
    }

    #[test]
    fn parses_sox_like_rate_placement() {
        assert_eq!(
            parse(&["sex", "in.wav", "-r", "48000", "out.wav"]),
            Ok(Command::Convert {
                input: "in.wav".into(),
                output: "out.wav".into(),
                output_rate: Some(48_000),
                output_bits: None,
                quality: QualityOptions::default(),
            })
        );
        assert_eq!(
            parse(&[
                "sex",
                "in.wav",
                "out.wav",
                "rate",
                "48000",
                "--precision",
                "auto",
                "--error-floor",
                "-400dB",
            ]),
            Ok(Command::Convert {
                input: "in.wav".into(),
                output: "out.wav".into(),
                output_rate: Some(48_000),
                output_bits: None,
                quality: QualityOptions {
                    error_floor: Some(ErrorFloor::new(400).unwrap()),
                    ..QualityOptions::default()
                },
            })
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_arguments() {
        assert!(parse(&["sex", "in.wav", "one.wav", "two.wav"]).is_err());
        assert!(parse(&["sex", "in.wav", "-r", "0", "out.wav"]).is_err());
        assert!(parse(&["sex", "in.wav", "--mystery", "out.wav"]).is_err());
        assert!(parse(&["sex", "in.wav", "--preset", "turbo", "out.wav"]).is_err());
        assert!(parse(&["sex", "in.wav", "--error-floor", "300dB", "out.wav"]).is_err());
        assert!(parse(&["sex", "in.wav", "out.wav", "-r", "48000", "rate", "44100",]).is_err());
        assert!(
            parse(&[
                "sex",
                "in.wav",
                "out.wav",
                "--precision",
                "auto",
                "--precision",
                "256",
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_analyze_mode() {
        assert_eq!(
            parse(&["sex", "analyze", "in.wav", "-r", "48000", "--grid", "129"]),
            Ok(Command::Analyze {
                input: "in.wav".into(),
                output_rate: SampleRate::from_hz(48_000).unwrap(),
                quality: QualityOptions {
                    gates: quality_gates::Gates {
                        grid: 129,
                        ..quality_gates::Gates::default()
                    },
                    ..QualityOptions::default()
                },
            })
        );
        assert!(parse(&["sex", "analyze", "in.wav"]).is_err());
    }

    #[test]
    fn build_info_is_an_explicit_argument_free_command() {
        assert_eq!(parse(&["sex", "--build-info"]), Ok(Command::BuildInfo));
        assert!(parse(&["sex", "--build-info", "unexpected"]).is_err());
    }

    #[test]
    fn continuous_certificate_controls_are_explicit_and_shared() {
        let parsed = parse(&[
            "sex",
            "analyze",
            "in.wav",
            "-r",
            "16000",
            "--certify",
            "--certificate-work",
            "777",
            "--certificate-taps",
            "99",
            "--certificate-depth",
            "0",
            "--certificate-endpoint-bits",
            "64",
            "--certificate-integer-bits",
            "10000",
        ])
        .unwrap();
        let Command::Analyze { quality, .. } = parsed else {
            panic!()
        };
        let limits = quality.gates.certificate.unwrap();
        assert_eq!(limits.max_work, 777);
        assert_eq!(limits.max_taps, 99);
        assert_eq!(limits.max_depth, 0);
        assert_eq!(limits.endpoint_bits, 64);
        assert_eq!(limits.max_integer_bits, 10000);
        assert!(parse(&["sex", "plan", "in.wav", "-r", "16000", "--certify"]).is_ok());
        assert!(
            parse(&[
                "sex",
                "analyze",
                "in.wav",
                "-r",
                "16000",
                "--certificate-work",
                "10"
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "sex",
                "analyze",
                "in.wav",
                "-r",
                "16000",
                "--certify",
                "--certify"
            ])
            .is_err()
        );
        for (option, bad) in [
            ("--certificate-work", "0"),
            ("--certificate-depth", "65"),
            ("--certificate-taps", "0"),
            ("--certificate-endpoint-bits", "7"),
            ("--certificate-integer-bits", "0"),
        ] {
            assert!(
                parse(&[
                    "sex",
                    "analyze",
                    "in.wav",
                    "-r",
                    "16000",
                    "--certify",
                    option,
                    bad
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn harmonic_controls_are_explicit_bounded_and_shared() {
        let parsed = parse(&[
            "sex",
            "analyze",
            "in.wav",
            "-r",
            "48000",
            "--harmonics",
            "--harmonic-work",
            "777",
            "--harmonic-coefficients",
            "99",
            "--harmonic-phases",
            "3",
            "--harmonic-precision-limit",
            "4096",
            "--harmonic-storage-bytes",
            "123456",
        ])
        .unwrap();
        let Command::Analyze { quality, .. } = parsed else {
            panic!()
        };
        let limits = quality.gates.harmonics.unwrap();
        assert_eq!(limits.max_work, 777);
        assert_eq!(limits.max_coefficients, 99);
        assert_eq!(limits.max_phases, 3);
        assert_eq!(limits.max_precision_bits, 4096);
        assert_eq!(limits.max_storage_bytes, 123456);
        for mode in ["plan", "in.wav"] {
            assert!(parse(&["sex", mode, "in.wav", "-r", "48000", "--harmonics"]).is_ok());
        }
        assert!(
            parse(&[
                "sex",
                "analyze",
                "in.wav",
                "-r",
                "48000",
                "--harmonics",
                "--harmonics"
            ])
            .is_err()
        );
        for flag in [
            "--harmonic-work",
            "--harmonic-coefficients",
            "--harmonic-phases",
            "--harmonic-precision-limit",
            "--harmonic-storage-bytes",
        ] {
            assert!(parse(&["sex", "analyze", "in.wav", "-r", "48000", flag, "99"]).is_err());
            assert!(
                parse(&[
                    "sex",
                    "analyze",
                    "in.wav",
                    "-r",
                    "48000",
                    "--harmonics",
                    flag,
                    "0"
                ])
                .is_err()
            );
            assert!(
                parse(&[
                    "sex",
                    "analyze",
                    "in.wav",
                    "-r",
                    "48000",
                    "--harmonics",
                    flag,
                    "99",
                    flag,
                    "100"
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn quality_feedback_options_are_shared_and_plan_remains_non_materializing() {
        for args in [
            vec![
                "sex",
                "in.wav",
                "out.wav",
                "--refine-quality",
                "--refinement-attempts",
                "3",
            ],
            vec![
                "sex",
                "analyze",
                "in.wav",
                "-r",
                "48000",
                "--refine-quality",
                "--refinement-attempts",
                "3",
            ],
            vec![
                "sex",
                "plan",
                "in.wav",
                "-r",
                "48000",
                "--refine-quality",
                "--refinement-attempts",
                "3",
            ],
        ] {
            let quality = match parse(&args).unwrap() {
                Command::Convert { quality, .. }
                | Command::Analyze { quality, .. }
                | Command::Plan { quality, .. } => quality,
                _ => panic!(),
            };
            assert_eq!(quality.refinement.max_attempts, Some(3));
        }
        for values in [
            vec!["--refine-quality", "--refine-quality"],
            vec!["--refinement-attempts", "129"],
            vec!["--refinement-terms", "0"],
            vec![
                "--refinement-coefficients",
                "5",
                "--refinement-coefficients",
                "9",
            ],
            vec!["--refinement-unknown", "8"],
            vec!["--refinement-precision-limit", "4294967296"],
        ] {
            let mut args = vec!["sex", "in.wav", "out.wav"];
            args.extend(values);
            assert!(parse(&args).is_err());
        }
    }

    #[test]
    fn until_budget_flags_have_shared_order_independent_semantics() {
        let initial = plan_precision(PlanRequest {
            ratio: RateRatio::from_fraction(160, 147).unwrap(),
            preset: QualityPreset::Until40k,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        for prefix in [
            vec!["sex", "in.wav", "out.wav"],
            vec!["sex", "plan", "in.wav", "-r", "48000"],
            vec!["sex", "analyze", "in.wav", "-r", "48000"],
        ] {
            for suffix in [
                vec![
                    "--preset",
                    "until-40k",
                    "--refine-quality",
                    "--refinement-attempts",
                    "2",
                    "--refinement-coefficients",
                    "11",
                ],
                vec![
                    "--refinement-coefficients",
                    "11",
                    "--refinement-attempts",
                    "2",
                    "--refine-quality",
                    "--preset",
                    "until-40k",
                ],
            ] {
                let mut args = prefix.clone();
                args.extend(suffix);
                let quality = match parse(&args).unwrap() {
                    Command::Convert { quality, .. }
                    | Command::Analyze { quality, .. }
                    | Command::Plan { quality, .. } => quality,
                    _ => panic!(),
                };
                let limits = quality
                    .refinement
                    .resolve(&initial, quality.gates.grid)
                    .unwrap();
                assert_eq!(limits.max_attempts, 2);
                assert_eq!(limits.max_total_coefficients, 11);
                assert_eq!(limits.max_response_terms, 5_563_334_400);
            }
        }
    }

    #[test]
    fn response_feedback_replans_the_whole_chain_before_materializing_each_bank() {
        let quality = QualityOptions {
            preset: QualityPreset::Sane,
            gain: Some(LinearGain::parse("1152921504606846976").unwrap()),
            clip_policy: ClipPolicy::Normalize,
            ..QualityOptions::default()
        };
        let mut initial = plan_precision(PlanRequest {
            ratio: RateRatio::from_fraction(1, 3).unwrap(),
            preset: QualityPreset::Sane,
            error_floor: None,
            working_precision_bits: None,
        })
        .unwrap();
        initial.taps_per_phase = 769;
        initial.coefficient_count = 769;
        initial.design_attenuation_db = 110;
        initial.kaiser_beta = sexplan::Rational::new(10, 1).unwrap();
        let mut prepared_widths = Vec::new();
        let qualified = refinement::qualify(
            initial,
            refinement::Request {
                certificate: Some(CertificateLimits::default()),
                ..refinement::Request::default()
            },
            |plan| {
                let execution = numerical_plan::plan_from_filter(plan, &quality, true)?;
                prepared_widths.push(execution.signal);
                Ok(execution.filter)
            },
            |plan| {
                Ok(design_kaiser_big(&BigKaiserSpec::from_precision_plan(
                    plan,
                )?)?)
            },
            refinement::assess_big,
        )
        .unwrap();
        assert_eq!(qualified.attempts.len(), 2);
        assert_eq!(prepared_widths.len(), 2);
        assert!(qualified.plan.coefficient_fractional_bits > 62);
        assert_eq!(
            qualified.design.bank().taps_per_phase(),
            qualified.plan.taps_per_phase as usize
        );
        let final_plan =
            numerical_plan::plan_from_filter(qualified.plan.clone(), &quality, true).unwrap();
        assert_eq!(final_plan.filter, qualified.plan);
        assert_eq!(final_plan.signal, *prepared_widths.last().unwrap());
        assert!(
            final_plan
                .numerical
                .meets_target_bits(qualified.plan.amplitude_error_bits + 2)
        );
    }

    #[test]
    fn plan_and_analyze_accept_effects_and_reject_duplicates() {
        for mode in ["plan", "analyze"] {
            let args = [
                "sex",
                mode,
                "in.wav",
                "-r",
                "48000",
                "--gain",
                "3/5",
                "--dc-remove",
                "10/11",
                "--mix",
                "1/3,2/7",
                "--convolve",
                "1,-1/5",
            ];
            let quality = match parse(&args).unwrap() {
                Command::Plan { quality, .. } | Command::Analyze { quality, .. } => quality,
                _ => panic!("unexpected command"),
            };
            assert_eq!(quality.gain, Some(LinearGain::parse("3/5").unwrap()));
            assert_eq!(
                quality.dc_block_radius,
                Some(DcBlockRadius::parse("10/11").unwrap())
            );
            assert_eq!(
                quality.channel_mix,
                Some(ChannelMatrix::parse("1/3,2/7").unwrap())
            );
            assert_eq!(
                quality.convolution,
                Some(ConvolutionSpec::parse("1,-1/5").unwrap())
            );
            for (option, value) in [
                ("--gain", "1"),
                ("--dc-remove", "1/2"),
                ("--mix", "1"),
                ("--convolve", "1"),
            ] {
                assert!(
                    parse(&[
                        "sex", mode, "in.wav", "-r", "48000", option, value, option, value
                    ])
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn signal_precision_is_independent_and_validated_in_all_modes() {
        for prefix in [
            vec!["sex", "in.wav", "out.wav"],
            vec!["sex", "plan", "in.wav", "-r", "48000"],
            vec!["sex", "analyze", "in.wav", "-r", "48000"],
        ] {
            for (value, expected) in [("auto", None), ("4096", Some(4096))] {
                let mut args = prefix.clone();
                args.extend(["--signal-precision", value, "--precision", "1024"]);
                let quality = match parse(&args).unwrap() {
                    Command::Convert { quality, .. }
                    | Command::Plan { quality, .. }
                    | Command::Analyze { quality, .. } => quality,
                    _ => panic!("expected processing command"),
                };
                assert_eq!(quality.signal_fractional_bits, expected);
                assert_eq!(quality.working_precision_bits, Some(1024));
            }
            for value in ["0", "-1", "1.5", "4294967296"] {
                let mut args = prefix.clone();
                args.extend(["--signal-precision", value]);
                assert!(parse(&args).is_err());
            }
            let mut args = prefix;
            args.extend(["--signal-precision", "auto", "--signal-precision", "4096"]);
            assert!(parse(&args).is_err());
        }
    }

    #[test]
    fn parses_exact_quality_options_in_any_convert_position() {
        assert_eq!(
            parse(&[
                "sex",
                "in.wav",
                "--error-floor",
                "-300dB",
                "out.wav",
                "--preset",
                "sane",
                "--precision",
                "256",
                "--gain",
                "6/8",
                "--dc-remove",
                "65535/65536",
                "--mix",
                "1/2,1/2",
                "--convolve",
                "1,-1/4",
                "--block-frames",
                "7",
                "-r",
                "48000",
            ]),
            Ok(Command::Convert {
                input: "in.wav".into(),
                output: "out.wav".into(),
                output_rate: Some(48_000),
                output_bits: None,
                quality: QualityOptions {
                    preset: QualityPreset::Sane,
                    error_floor: Some(ErrorFloor::new(300).unwrap()),
                    working_precision_bits: Some(256),
                    gain: Some(LinearGain {
                        numerator: 3,
                        denominator: 4,
                    }),
                    dc_block_radius: Some(DcBlockRadius(LinearGain {
                        numerator: 65_535,
                        denominator: 65_536,
                    })),
                    channel_mix: Some(ChannelMatrix {
                        input_channels: 2,
                        output_channels: 1,
                        coefficients: vec![
                            ExactCoefficient::parse("1/2").unwrap(),
                            ExactCoefficient::parse("1/2").unwrap(),
                        ],
                    }),
                    convolution: Some(ConvolutionSpec {
                        taps: vec![
                            ExactCoefficient::parse("1").unwrap(),
                            ExactCoefficient::parse("-1/4").unwrap(),
                        ],
                    }),
                    block_frames: Some(7),
                    ..QualityOptions::default()
                },
            })
        );
    }

    #[test]
    fn exact_linear_gain_is_validated_and_quantized_with_ties_to_even() {
        assert_eq!(
            LinearGain::parse("1/2").unwrap().coefficient_raw(),
            1_i128 << 61
        );
        assert_eq!(
            LinearGain::parse("1/3").unwrap().coefficient_raw(),
            1_537_228_672_809_129_301
        );
        assert_eq!(
            LinearGain::parse("2/1").unwrap().coefficient_raw(),
            1_i128 << 63
        );
        assert_eq!(
            LinearGain::parse("18446744073709551615")
                .unwrap()
                .coefficient_raw(),
            i128::from(u64::MAX) << 62
        );
        assert!(LinearGain::parse("1/0").is_err());
        assert!(LinearGain::parse("-1/2").is_err());
    }

    #[test]
    fn exact_dc_radius_is_stable_and_representable() {
        let radius = DcBlockRadius::parse("65535/65536").unwrap();
        assert_eq!(radius.0.numerator, 65_535);
        assert_eq!(radius.0.denominator, 65_536);
        assert_eq!(
            radius.coefficient(),
            Q2_62::from_raw((1_i64 << 62) - (1_i64 << 46))
        );
        assert!(DcBlockRadius::parse("1/1").is_err());
        assert!(DcBlockRadius::parse("18446744073709551614/18446744073709551615").is_ok());
    }

    #[test]
    fn exact_channel_matrix_is_rectangular_reduced_and_q2_62_bounded() {
        let matrix = ChannelMatrix::parse("2/4,-1/2;0,1").unwrap();
        assert_eq!(matrix.input_channels, 2);
        assert_eq!(matrix.output_channels, 2);
        assert_eq!(matrix.to_string(), "1/2,-1/2;0/1,1/1");
        assert_eq!(
            matrix.quantized(),
            vec![
                Q2_62::from_raw(1_i64 << 61),
                Q2_62::from_raw(-(1_i64 << 61)),
                Q2_62::from_raw(0),
                Q2_62::ONE,
            ]
        );
        assert_eq!(
            ExactCoefficient::parse("-2").unwrap().quantized,
            Q2_62::from_raw(i64::MIN)
        );
        assert!(ExactCoefficient::parse("2").is_err());
        assert!(ChannelMatrix::parse("").is_err());
        assert!(ChannelMatrix::parse("1,0;1").is_err());
        assert!(ChannelMatrix::parse("1,,0").is_err());
    }

    #[test]
    fn exact_convolution_taps_are_reduced_and_validated() {
        let convolution = ConvolutionSpec::parse("2/4,-1/4,0").unwrap();
        assert_eq!(convolution.to_string(), "1/2,-1/4,0/1");
        assert_eq!(
            convolution.quantized(),
            vec![
                Q2_62::from_raw(1_i64 << 61),
                Q2_62::from_raw(-(1_i64 << 60)),
                Q2_62::from_raw(0),
            ]
        );
        assert!(ConvolutionSpec::parse("").is_err());
        assert!(ConvolutionSpec::parse("1,,1").is_err());
        assert!(ConvolutionSpec::parse("2").is_err());
    }

    #[test]
    fn parses_plan_without_materializing_the_backend() {
        assert_eq!(
            parse(&[
                "sex",
                "plan",
                "in.wav",
                "-r",
                "48000",
                "--preset",
                "until-40k",
            ]),
            Ok(Command::Plan {
                input: "in.wav".into(),
                output_rate: SampleRate::from_hz(48_000).unwrap(),
                quality: QualityOptions {
                    preset: QualityPreset::Until40k,
                    ..QualityOptions::default()
                },
            })
        );
        assert!(parse(&["sex", "plan", "in.wav", "-r", "48000", "--grid", "9"]).is_ok());
    }

    #[test]
    fn parses_target_depth_and_deterministic_dither() {
        assert_eq!(
            parse(&[
                "sex",
                "in.wav",
                "out.wav",
                "--bits",
                "16",
                "--dither",
                "noise-shaped-1",
                "--seed",
                "18446744073709551615",
                "--clip",
                "error",
            ]),
            Ok(Command::Convert {
                input: "in.wav".into(),
                output: "out.wav".into(),
                output_rate: None,
                output_bits: Some(16),
                quality: QualityOptions {
                    dither_mode: Some(DitherMode::NoiseShaped { order: 1 }),
                    dither_seed: u64::MAX,
                    clip_policy: ClipPolicy::Error,
                    ..QualityOptions::default()
                },
            })
        );
        assert!(parse(&["sex", "in.wav", "out.wav", "--bits", "0"]).is_err());
        assert!(parse(&["sex", "in.wav", "out.wav", "--clip", "wrap"]).is_err());
        assert!(parse(&["sex", "in.wav", "out.wav", "--block-frames", "0"]).is_err());
    }

    #[test]
    fn normalization_scale_is_exact_and_reserves_dither_guard() {
        let config = DitherConfig {
            target_bits: 16,
            mode: DitherMode::NoiseShaped { order: 5 },
            seed: 0,
            rounding: RoundingMode::NearestTiesToEven,
            overflow: OverflowPolicy::Error,
        };
        let format = BigQFormat::new(65, 63).unwrap();
        let peak = processing::SignalPeak {
            magnitude: Integer::from(i64::MAX) * 2,
        };
        let normalization =
            processing::normalization_summary(peak.clone(), format, config).unwrap();
        assert!(normalization.dither_guard_raw > 0);
        assert_eq!(normalization.scale_denominator, peak.magnitude);
        assert_eq!(
            normalization.scale_numerator,
            Integer::from(32_767_u128 << 48) - &normalization.dither_guard_raw
        );
        let scaled = BigQ::from_raw(peak.magnitude, format)
            .unwrap()
            .scale_ratio(
                &normalization.scale_numerator,
                &normalization.scale_denominator,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )
            .unwrap();
        assert_eq!(scaled.value.raw(), &normalization.scale_numerator);
    }
}
