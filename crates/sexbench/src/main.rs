use sexq::{
    BigQ, BigQFormat, ExactDotBackend, OverflowPolicy, Q1_63, Q2_62, RoundingMode,
    automatic_exact_dot_backend, exact_dot_backend_available,
};
use sexrate::{
    FrameCountPolicy, InterleavedResamplerQ63, PolyphaseFirBigQ63, PolyphaseFirQ63, RateRatio,
    output_frames_for_input,
};
use std::error::Error;
use std::hint::black_box;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const HELP: &str = "\
sex-bench — deterministic SeX resampler execution benchmark

USAGE:
    sex-bench [OPTIONS]

OPTIONS:
    --backend native|bigint|both  Backends to measure (default: both)
    --native-kernel auto|scalar|avx2|avx512
                                 Exact native dot kernel (default: auto)
    --frames N                    Input frames per iteration (default: 262144)
    --iterations N                Timed iterations (default: 3)
    --channels N                  Interleaved channels (default: 2)
    --taps N                      Odd taps per phase (default: 257)
    --input-rate HZ               Input sample rate (default: 44100)
    --output-rate HZ              Output sample rate (default: 48000)
    -h, --help                    Print this help
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendSelection {
    Native,
    BigInt,
    Both,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeKernelSelection {
    Auto,
    Scalar,
    Avx2,
    Avx512,
}

impl NativeKernelSelection {
    fn resolve(self) -> Result<ExactDotBackend, String> {
        let backend = match self {
            Self::Auto => automatic_exact_dot_backend(),
            Self::Scalar => ExactDotBackend::Scalar,
            Self::Avx2 => ExactDotBackend::Avx2,
            Self::Avx512 => ExactDotBackend::Avx512,
        };
        if exact_dot_backend_available(backend) {
            Ok(backend)
        } else {
            Err(format!(
                "native kernel {backend} is unavailable on this CPU"
            ))
        }
    }
}

impl FromStr for NativeKernelSelection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "scalar" => Ok(Self::Scalar),
            "avx2" => Ok(Self::Avx2),
            "avx512" => Ok(Self::Avx512),
            _ => Err(format!("unknown native kernel {value:?}")),
        }
    }
}

impl FromStr for BackendSelection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "native" => Ok(Self::Native),
            "bigint" => Ok(Self::BigInt),
            "both" => Ok(Self::Both),
            _ => Err(format!("unknown backend {value:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Config {
    backend: BackendSelection,
    native_kernel: NativeKernelSelection,
    frames: usize,
    iterations: u32,
    channels: u16,
    taps: usize,
    input_rate: u64,
    output_rate: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: BackendSelection::Both,
            native_kernel: NativeKernelSelection::Auto,
            frames: 262_144,
            iterations: 3,
            channels: 2,
            taps: 257,
            input_rate: 44_100,
            output_rate: 48_000,
        }
    }
}

impl Config {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut config = Self::default();
        let arguments = arguments.into_iter().skip(1).collect::<Vec<_>>();
        let mut index = 0;
        while index < arguments.len() {
            if matches!(arguments[index].as_str(), "-h" | "--help") {
                return Ok(None);
            }
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("{} requires a value", arguments[index]))?;
            match arguments[index].as_str() {
                "--backend" => config.backend = value.parse()?,
                "--native-kernel" => config.native_kernel = value.parse()?,
                "--frames" => config.frames = parse_positive(value, "frames")?,
                "--iterations" => config.iterations = parse_positive(value, "iterations")?,
                "--channels" => config.channels = parse_positive(value, "channels")?,
                "--taps" => config.taps = parse_positive(value, "taps")?,
                "--input-rate" => config.input_rate = parse_positive(value, "input rate")?,
                "--output-rate" => config.output_rate = parse_positive(value, "output rate")?,
                option => return Err(format!("unknown option {option:?}")),
            }
            index += 2;
        }
        if config.taps < 3 || config.taps.is_multiple_of(2) {
            return Err(format!(
                "taps must be odd and at least 3, got {}",
                config.taps
            ));
        }
        Ok(Some(config))
    }
}

fn parse_positive<T>(value: &str, name: &str) -> Result<T, String>
where
    T: FromStr + PartialEq + Default,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| format!("invalid {name}: {value:?}"))?;
    if parsed == T::default() {
        return Err(format!("{name} must be positive"));
    }
    Ok(parsed)
}

#[derive(Clone)]
enum BenchmarkBank {
    Native(Arc<PolyphaseFirQ63>),
    BigInt(Arc<PolyphaseFirBigQ63>),
}

impl BenchmarkBank {
    fn label(&self) -> &'static str {
        match self {
            Self::Native(bank) => match bank.dot_backend() {
                ExactDotBackend::Scalar => "native-i128/Q2.62/scalar",
                ExactDotBackend::Avx2 => "native-i128/Q2.62/avx2-exact",
                ExactDotBackend::Avx512 => "native-i128/Q2.62/avx512-exact",
            },
            Self::BigInt(_) => "GMP-192/Q2.62",
        }
    }

    fn stream(
        &self,
        channels: u16,
        ratio: RateRatio,
        delay: u64,
    ) -> Result<InterleavedResamplerQ63, Box<dyn Error>> {
        match self {
            Self::Native(bank) => Ok(InterleavedResamplerQ63::new_with_input_delay(
                channels,
                ratio,
                Arc::clone(bank),
                delay,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )?),
            Self::BigInt(bank) => Ok(InterleavedResamplerQ63::new_big_with_input_delay(
                channels,
                ratio,
                Arc::clone(bank),
                delay,
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error,
            )?),
        }
    }
}

fn native_coefficients(ratio: RateRatio, taps: usize) -> Result<Vec<Q2_62>, Box<dyn Error>> {
    let phases = usize::try_from(ratio.up())?;
    let count = phases
        .checked_mul(taps)
        .ok_or("coefficient count overflow")?;
    let base = Q2_62::ONE.raw() / i64::try_from(taps)?;
    let mut coefficients = vec![Q2_62::from_raw(base); count];
    for phase in 0..phases {
        let start = phase * taps;
        let sum = i128::from(base) * i128::try_from(taps)?;
        let residual = i128::from(Q2_62::ONE.raw()) - sum;
        let center = start + taps / 2;
        coefficients[center] = Q2_62::from_raw(i64::try_from(i128::from(base) + residual)?);
    }
    Ok(coefficients)
}

fn benchmark_banks(
    ratio: RateRatio,
    taps: usize,
    selection: BackendSelection,
    native_kernel: NativeKernelSelection,
) -> Result<Vec<BenchmarkBank>, Box<dyn Error>> {
    let native = native_coefficients(ratio, taps)?;
    let mut banks = Vec::new();
    if matches!(selection, BackendSelection::Native | BackendSelection::Both) {
        banks.push(BenchmarkBank::Native(Arc::new(
            PolyphaseFirQ63::for_ratio_with_dot_backend(
                ratio,
                taps,
                native.clone(),
                native_kernel.resolve()?,
            )?,
        )));
    }
    if matches!(selection, BackendSelection::BigInt | BackendSelection::Both) {
        let format = BigQFormat::new(2, Q2_62::FRACTIONAL_BITS)?;
        let coefficients = native
            .into_iter()
            .map(|coefficient| BigQ::from_i64(coefficient.raw(), format))
            .collect::<Result<Vec<_>, _>>()?;
        banks.push(BenchmarkBank::BigInt(Arc::new(
            PolyphaseFirBigQ63::for_ratio(ratio, taps, format, 192, coefficients)?,
        )));
    }
    Ok(banks)
}

fn deterministic_input(frames: usize, channels: u16) -> Result<Vec<Q1_63>, Box<dyn Error>> {
    let samples = frames
        .checked_mul(usize::from(channels))
        .ok_or("input sample count overflow")?;
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let mut input = Vec::with_capacity(samples);
    for _ in 0..samples {
        // Modular wrap is isolated benchmark stimulus generation, not signal
        // processing arithmetic.
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        input.push(Q1_63::from_raw((state as i64) >> 2));
    }
    Ok(input)
}

fn render_once(
    bank: &BenchmarkBank,
    input: &[Q1_63],
    input_frames: usize,
    channels: u16,
    ratio: RateRatio,
    delay: u64,
) -> Result<Vec<Q1_63>, Box<dyn Error>> {
    let target = output_frames_for_input(
        u64::try_from(input_frames)?,
        ratio,
        FrameCountPolicy::NearestTiesToEven,
    )?;
    let mut stream = bank.stream(channels, ratio, delay)?;
    let mut output = Vec::new();
    let chunk_samples = 4_096_usize
        .checked_mul(usize::from(channels))
        .ok_or("chunk sample count overflow")?;
    for chunk in input.chunks(chunk_samples) {
        stream.push_interleaved_finite_into(
            chunk,
            FrameCountPolicy::NearestTiesToEven,
            &mut output,
        )?;
    }
    let stats = stream.finish_exact_frames(target, &mut output)?;
    if stats.output_frames != target {
        return Err("benchmark stream emitted the wrong frame count".into());
    }
    Ok(output)
}

fn checksum(samples: &[Q1_63]) -> u64 {
    samples.iter().fold(0x510e_527f_ade6_82d1, |hash, sample| {
        hash.rotate_left(9).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ sample.raw() as u64
    })
}

fn benchmark(
    config: Config,
    bank: &BenchmarkBank,
    input: &[Q1_63],
    ratio: RateRatio,
) -> Result<(Duration, u64, usize), Box<dyn Error>> {
    let delay = u64::try_from((config.taps - 1) / 2)?;
    let mut elapsed = Duration::ZERO;
    let mut combined_checksum = 0_u64;
    let mut output_samples = 0;
    for _ in 0..config.iterations {
        let started = Instant::now();
        let output = render_once(bank, input, config.frames, config.channels, ratio, delay)?;
        elapsed += started.elapsed();
        output_samples = output.len();
        combined_checksum ^= black_box(checksum(black_box(&output)));
    }
    Ok((elapsed, combined_checksum, output_samples))
}

fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let ratio = RateRatio::from_rates(config.input_rate, config.output_rate)?;
    let banks = benchmark_banks(ratio, config.taps, config.backend, config.native_kernel)?;
    let input = deterministic_input(config.frames, config.channels)?;
    println!(
        "ratio {}/{}; {} input frames × {} channel(s); {} taps/phase; {} iteration(s)",
        ratio.up(),
        ratio.down(),
        config.frames,
        config.channels,
        config.taps,
        config.iterations
    );
    for bank in &banks {
        let (elapsed, hash, output_samples) = benchmark(config, bank, &input, ratio)?;
        let total_output = output_samples as f64 * f64::from(config.iterations);
        let samples_per_second = total_output / elapsed.as_secs_f64();
        println!(
            "{}: {:.3}s, {:.3} Msamples/s, output {}, checksum {:016x}",
            bank.label(),
            elapsed.as_secs_f64(),
            samples_per_second / 1_000_000.0,
            output_samples,
            hash
        );
    }
    if cfg!(debug_assertions) {
        eprintln!("warning: debug assertions are enabled; use --release for timing results");
    }
    Ok(())
}

fn main() {
    match Config::parse(std::env::args()) {
        Ok(None) => print!("{HELP}"),
        Ok(Some(config)) => {
            if let Err(error) = run(config) {
                eprintln!("sex-bench: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("sex-bench: {error}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_validated() {
        assert_eq!(
            Config::parse(["sex-bench".to_owned(), "--taps".to_owned(), "4".to_owned()]),
            Err("taps must be odd and at least 3, got 4".to_owned())
        );
        assert!(
            Config::parse(["sex-bench".to_owned(), "--help".to_owned()])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn native_and_bigint_bench_paths_are_bit_identical() {
        let ratio = RateRatio::from_fraction(3, 2).unwrap();
        let banks = benchmark_banks(
            ratio,
            7,
            BackendSelection::Both,
            NativeKernelSelection::Auto,
        )
        .unwrap();
        let input = deterministic_input(257, 2).unwrap();
        let native = render_once(&banks[0], &input, 257, 2, ratio, 3).unwrap();
        let bigint = render_once(&banks[1], &input, 257, 2, ratio, 3).unwrap();
        assert_eq!(native, bigint);
        assert_ne!(checksum(&native), 0);
    }

    #[test]
    fn scalar_and_avx2_bench_paths_are_bit_identical_when_available() {
        if !exact_dot_backend_available(ExactDotBackend::Avx2) {
            return;
        }
        let ratio = RateRatio::from_fraction(7, 5).unwrap();
        let scalar = benchmark_banks(
            ratio,
            13,
            BackendSelection::Native,
            NativeKernelSelection::Scalar,
        )
        .unwrap()
        .remove(0);
        let vector = benchmark_banks(
            ratio,
            13,
            BackendSelection::Native,
            NativeKernelSelection::Avx2,
        )
        .unwrap()
        .remove(0);
        let input = deterministic_input(509, 3).unwrap();
        assert_eq!(
            render_once(&scalar, &input, 509, 3, ratio, 6).unwrap(),
            render_once(&vector, &input, 509, 3, ratio, 6).unwrap()
        );
    }
}
