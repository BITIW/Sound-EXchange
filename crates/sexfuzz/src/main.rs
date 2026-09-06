use sexq::{
    BigQ, BigQFormat, ExactDotBackend, OverflowPolicy, Q1_63, Q2_62, RoundingMode, WideQ63,
    exact_dot_backend_available, round_shift_integer,
};
use sexrate::{
    FrameCountPolicy, InterleavedResamplerBig, InterleavedResamplerQ63,
    InterleavedResamplerWideQ63, InterleavedStreamStats, PolyphaseFirBig, PolyphaseFirBigQ63,
    PolyphaseFirQ63, RateRatio, output_frames_for_input,
};
use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;

mod wave;

const HELP: &str = "\
sex-fuzz — deterministic property driver for SeX

USAGE:
    sex-fuzz [--suite dsp|io|all] [--cases N] [--seed U64]
    sex-fuzz [--suite dsp|io|all] --case-seed U64

Every case compares scalar native one-shot, scalar native randomized chunks,
every available exact SIMD backend, and GMP execution. It also exercises exact
signed-PCM import/export, Q65.63 headroom, and arbitrary-fractional BigQ streams
against an independent absolute-coordinate raw-integer oracle.
The io suite checks integer WAVE round trips, malformed headers, truncation,
and short reads entirely in bounded memory. Default suite: dsp.
A failure prints the exact command needed for single-case replay.
";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Suite {
    #[default]
    Dsp,
    Io,
    All,
}

impl Suite {
    const fn name(self) -> &'static str {
        match self {
            Self::Dsp => "dsp",
            Self::Io => "io",
            Self::All => "all",
        }
    }

    fn run_case(self, seed: u64) -> Result<u64, Box<dyn Error>> {
        match self {
            Self::Dsp => run_case(seed),
            Self::Io => wave::run_case(seed),
            Self::All => Ok(run_case(seed)?.rotate_left(13) ^ wave::run_case(seed)?),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Config {
    cases: u64,
    seed: u64,
    suite: Suite,
    case_seed: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cases: 1_000,
            seed: 0x243f_6a88_85a3_08d3,
            suite: Suite::Dsp,
            case_seed: None,
        }
    }
}

impl Config {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let arguments = arguments.into_iter().skip(1).collect::<Vec<_>>();
        let mut config = Self::default();
        let mut index = 0;
        let mut seen = std::collections::HashSet::new();
        while index < arguments.len() {
            if matches!(arguments[index].as_str(), "-h" | "--help") {
                return Ok(None);
            }
            if !seen.insert(arguments[index].as_str()) {
                return Err(format!("duplicate option {}", arguments[index]));
            }
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("{} requires a value", arguments[index]))?;
            match arguments[index].as_str() {
                "--cases" => config.cases = parse_number(value, "case count")?,
                "--seed" => config.seed = parse_number(value, "seed")?,
                "--case-seed" => config.case_seed = Some(parse_number(value, "case seed")?),
                "--suite" => {
                    config.suite = match value.as_str() {
                        "dsp" => Suite::Dsp,
                        "io" => Suite::Io,
                        "all" => Suite::All,
                        _ => {
                            return Err(format!(
                                "unknown suite {value:?}; expected dsp, io, or all"
                            ));
                        }
                    }
                }
                option => return Err(format!("unknown option {option:?}")),
            }
            index += 2;
        }
        if config.case_seed.is_some() && (seen.contains("--seed") || seen.contains("--cases")) {
            return Err("--case-seed cannot be combined with --seed or --cases".to_owned());
        }
        if config.cases == 0 {
            return Err("case count must be positive".to_owned());
        }
        Ok(Some(config))
    }
}

fn parse_number<T>(value: &str, name: &str) -> Result<T, String>
where
    T: FromStr,
{
    value
        .parse()
        .map_err(|_| format!("invalid {name}: {value:?}"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SplitMix64(u64);

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        // Modular wrap is intentional stimulus-generator arithmetic and never
        // participates in the signal path being tested.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn range(&mut self, upper_exclusive: u64) -> u64 {
        self.next() % upper_exclusive
    }
}

struct Case {
    ratio: RateRatio,
    taps: usize,
    channels: u16,
    frames: usize,
    input: Vec<Q1_63>,
    native_coefficients: Vec<Q2_62>,
    native: Arc<PolyphaseFirQ63>,
    bigint: Arc<PolyphaseFirBigQ63>,
}

impl Case {
    fn generate(seed: u64) -> Result<Self, Box<dyn Error>> {
        let mut random = SplitMix64::new(seed);
        let ratio = RateRatio::from_fraction(random.range(16) + 1, random.range(16) + 1)?;
        let taps = usize::try_from(2 * random.range(7) + 3)?;
        let channels = u16::try_from(random.range(4) + 1)?;
        let frames = usize::try_from(random.range(128) + 1)?;
        let sample_count = frames
            .checked_mul(usize::from(channels))
            .ok_or("fuzz sample count overflow")?;
        let input = (0..sample_count)
            .map(|_| Q1_63::from_raw((random.next() as i64) >> random.range(4)))
            .collect::<Vec<_>>();
        let coefficients = random_unity_coefficients(ratio, taps, &mut random)?;
        let native = Arc::new(PolyphaseFirQ63::for_ratio(
            ratio,
            taps,
            coefficients.clone(),
        )?);
        let format = BigQFormat::new(2, Q2_62::FRACTIONAL_BITS)?;
        let big_coefficients = coefficients
            .iter()
            .copied()
            .map(|coefficient| BigQ::from_i64(coefficient.raw(), format))
            .collect::<Result<Vec<_>, _>>()?;
        let bigint = Arc::new(PolyphaseFirBigQ63::for_ratio(
            ratio,
            taps,
            format,
            192,
            big_coefficients,
        )?);
        Ok(Self {
            ratio,
            taps,
            channels,
            frames,
            input,
            native_coefficients: coefficients,
            native,
            bigint,
        })
    }
}

fn random_unity_coefficients(
    ratio: RateRatio,
    taps: usize,
    random: &mut SplitMix64,
) -> Result<Vec<Q2_62>, Box<dyn Error>> {
    let phases = usize::try_from(ratio.up())?;
    let mut result = Vec::with_capacity(
        phases
            .checked_mul(taps)
            .ok_or("fuzz coefficient count overflow")?,
    );
    for _ in 0..phases {
        let weights = (0..taps)
            .map(|_| u128::from(random.range(1_000) + 1))
            .collect::<Vec<_>>();
        let total_weight = weights.iter().copied().sum::<u128>();
        let unity = i128::from(Q2_62::ONE.raw());
        let mut raw_sum = 0_i128;
        for (index, weight) in weights.into_iter().enumerate() {
            let raw = if index + 1 == taps {
                unity - raw_sum
            } else {
                let product = u128::try_from(unity)?
                    .checked_mul(weight)
                    .ok_or("fuzz coefficient product overflow")?;
                i128::try_from(product / total_weight)?
            };
            raw_sum = raw_sum
                .checked_add(raw)
                .ok_or("fuzz coefficient sum overflow")?;
            result.push(Q2_62::from_raw(i64::try_from(raw)?));
        }
        if raw_sum != unity {
            return Err("fuzz coefficient phase does not sum to unity".into());
        }
    }
    Ok(result)
}

fn target_frames(case: &Case) -> Result<u64, Box<dyn Error>> {
    Ok(output_frames_for_input(
        u64::try_from(case.frames)?,
        case.ratio,
        FrameCountPolicy::NearestTiesToEven,
    )?)
}

fn native_stream(case: &Case) -> Result<InterleavedResamplerQ63, Box<dyn Error>> {
    Ok(InterleavedResamplerQ63::new_with_input_delay(
        case.channels,
        case.ratio,
        Arc::clone(&case.native),
        u64::try_from((case.taps - 1) / 2)?,
        RoundingMode::NearestTiesToEven,
        OverflowPolicy::Error,
    )?)
}

fn native_stream_with_backend(
    case: &Case,
    backend: ExactDotBackend,
) -> Result<InterleavedResamplerQ63, Box<dyn Error>> {
    let bank = Arc::new(PolyphaseFirQ63::for_ratio_with_dot_backend(
        case.ratio,
        case.taps,
        case.native_coefficients.clone(),
        backend,
    )?);
    Ok(InterleavedResamplerQ63::new_with_input_delay(
        case.channels,
        case.ratio,
        bank,
        u64::try_from((case.taps - 1) / 2)?,
        RoundingMode::NearestTiesToEven,
        OverflowPolicy::Error,
    )?)
}

fn bigint_stream(case: &Case) -> Result<InterleavedResamplerQ63, Box<dyn Error>> {
    Ok(InterleavedResamplerQ63::new_big_with_input_delay(
        case.channels,
        case.ratio,
        Arc::clone(&case.bigint),
        u64::try_from((case.taps - 1) / 2)?,
        RoundingMode::NearestTiesToEven,
        OverflowPolicy::Error,
    )?)
}

fn render_one_shot(
    mut stream: InterleavedResamplerQ63,
    case: &Case,
) -> Result<(Vec<Q1_63>, InterleavedStreamStats), Box<dyn Error>> {
    let mut output = Vec::new();
    stream.push_interleaved_finite_into(
        &case.input,
        FrameCountPolicy::NearestTiesToEven,
        &mut output,
    )?;
    let stats = stream.finish_exact_frames(target_frames(case)?, &mut output)?;
    Ok((output, stats))
}

fn render_random_chunks(
    mut stream: InterleavedResamplerQ63,
    case: &Case,
    seed: u64,
) -> Result<(Vec<Q1_63>, InterleavedStreamStats), Box<dyn Error>> {
    let mut random = SplitMix64::new(seed);
    let channels = usize::from(case.channels);
    let mut position = 0;
    let mut output = Vec::new();
    while position < case.input.len() {
        let remaining_frames = (case.input.len() - position) / channels;
        let chunk_frames = usize::try_from(random.range(remaining_frames as u64) + 1)?;
        let end = position + chunk_frames * channels;
        stream.push_interleaved_finite_into(
            &case.input[position..end],
            FrameCountPolicy::NearestTiesToEven,
            &mut output,
        )?;
        position = end;
    }
    let stats = stream.finish_exact_frames(target_frames(case)?, &mut output)?;
    Ok((output, stats))
}

fn check_pcm_round_trip(random: &mut SplitMix64) -> Result<(), Box<dyn Error>> {
    for _ in 0..32 {
        let bits = u32::try_from(random.range(32) + 1)?;
        let half_range = 1_i64 << (bits - 1);
        let span = if bits == 32 {
            1_u64 << 32
        } else {
            (2 * half_range) as u64
        };
        let sample = i64::try_from(random.range(span))? - half_range;
        let q = Q1_63::from_signed_pcm(sample, bits)?;
        let recovered =
            q.to_signed_pcm(bits, RoundingMode::NearestTiesToEven, OverflowPolicy::Error)?;
        if recovered.value != sample || recovered.saturated {
            return Err(format!(
                "{bits}-bit PCM round trip changed {sample} to {}",
                recovered.value
            )
            .into());
        }
    }
    Ok(())
}

fn render_wide(
    case: &Case,
    input: &[WideQ63],
    bigint: bool,
    chunk_seed: Option<u64>,
) -> Result<(Vec<WideQ63>, InterleavedStreamStats), Box<dyn Error>> {
    let delay = u64::try_from((case.taps - 1) / 2)?;
    let mut stream = if bigint {
        InterleavedResamplerWideQ63::new_big_with_input_delay(
            case.channels,
            case.ratio,
            Arc::clone(&case.bigint),
            delay,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )?
    } else {
        InterleavedResamplerWideQ63::new_with_input_delay(
            case.channels,
            case.ratio,
            Arc::clone(&case.native),
            delay,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )?
    };
    let mut random = chunk_seed.map(SplitMix64::new);
    let mut position = 0;
    let mut output = Vec::new();
    let channels = usize::from(case.channels);
    while position < input.len() {
        let remaining = (input.len() - position) / channels;
        let frames = match random.as_mut() {
            Some(random) => usize::try_from(random.range(remaining as u64) + 1)?,
            None => remaining,
        };
        let end = position + frames * channels;
        stream.push_interleaved_finite_wide_into(
            &input[position..end],
            FrameCountPolicy::NearestTiesToEven,
            &mut output,
        )?;
        position = end;
    }
    let stats = stream.finish_wide_exact_frames(target_frames(case)?, &mut output)?;
    Ok((output, stats))
}

fn output_checksum(samples: &[Q1_63]) -> u64 {
    samples.iter().fold(0x1319_8a2e_0370_7344, |hash, sample| {
        hash.rotate_left(11).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ sample.raw() as u64
    })
}

fn check_arbitrary_signal(
    case: &Case,
    wide_input: &[WideQ63],
    seed: u64,
) -> Result<u64, Box<dyn Error>> {
    let mut random = SplitMix64::new(seed);
    let fractional_bits = [63, 127, 257, 4096][usize::try_from(random.range(4))?];
    let format = BigQFormat::new(65, fractional_bits)?;
    let bank = Arc::new(PolyphaseFirBig::from_q63_bank(
        &case.bigint,
        format,
        format,
        None,
    )?);
    let input = wide_input
        .iter()
        .map(|sample| {
            let mut raw = BigQ::from_i128(sample.raw(), BigQFormat::new(65, 63)?)?
                .raw()
                .clone();
            raw <<= fractional_bits - 63;
            if fractional_bits > 63 {
                raw += random.next();
            }
            BigQ::from_raw(raw, format)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let channels = usize::from(case.channels);
    let delay = u64::try_from((case.taps - 1) / 2)?;
    let target = target_frames(case)?;
    let render = |chunk_seed: Option<u64>| -> Result<_, Box<dyn Error>> {
        let mut stream = InterleavedResamplerBig::new_with_input_delay(
            case.channels,
            case.ratio,
            bank.clone(),
            delay,
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )?;
        let mut output = Vec::new();
        let mut position = 0;
        let mut random = chunk_seed.map(SplitMix64::new);
        while position < input.len() {
            let remaining = (input.len() - position) / channels;
            let frames = if let Some(random) = &mut random {
                usize::try_from(random.range(remaining as u64) + 1)?
            } else {
                remaining
            };
            let end = position + frames * channels;
            stream.push_interleaved_finite_into(
                &input[position..end],
                FrameCountPolicy::NearestTiesToEven,
                &mut output,
            )?;
            position = end;
        }
        let stats = stream.finish_exact_frames(target, &mut output)?;
        Ok((output, stats))
    };
    let whole = render(None)?;
    if whole != render(Some(seed.rotate_left(11)))? {
        return Err("arbitrary-fractional output or stats changed with block boundaries".into());
    }
    let expected_samples = usize::try_from(target)?
        .checked_mul(channels)
        .ok_or("arbitrary sample count overflow")?;
    if whole.0.len() != expected_samples
        || whole.1.input_frames != case.frames as u64
        || whole.1.output_frames != target
        || whole.1.saturated_samples != 0
    {
        return Err(
            "arbitrary-fractional stream violated its length or statistics contract".into(),
        );
    }
    let mut checksum = u64::from(fractional_bits);
    for frame in 0..target {
        let coordinate = u128::from(frame) * u128::from(case.ratio.down());
        let anchor = u128::from(delay) + coordinate / u128::from(case.ratio.up());
        let phase = (coordinate % u128::from(case.ratio.up())) as u64;
        for channel in 0..channels {
            let mut sum = BigQ::zero(format).raw().clone();
            for (tap, coefficient) in bank.phase(phase)?.iter().enumerate() {
                if let Some(index) = anchor.checked_sub(tap as u128)
                    && index < case.frames as u128
                {
                    sum += input[index as usize * channels + channel].raw().clone()
                        * coefficient.raw();
                }
            }
            let expected = round_shift_integer(
                &sum,
                bank.spec().coefficient_format.fractional_bits(),
                RoundingMode::NearestTiesToEven,
            );
            let actual = &whole.0[usize::try_from(frame)? * channels + channel];
            if actual.raw() != &expected || actual.format() != format {
                return Err(format!("arbitrary-fractional stream disagrees with absolute-coordinate oracle at frame {frame}, channel {channel}").into());
            }
            for byte in actual.raw().to_string_radix(16).bytes() {
                checksum = checksum.rotate_left(7) ^ u64::from(byte);
            }
        }
    }
    Ok(checksum)
}

fn run_case(seed: u64) -> Result<u64, Box<dyn Error>> {
    let case = Case::generate(seed)?;
    let native_one_shot = render_one_shot(native_stream(&case)?, &case)?;
    let native_chunked = render_random_chunks(native_stream(&case)?, &case, seed ^ u64::MAX)?;
    let bigint = render_random_chunks(bigint_stream(&case)?, &case, seed.rotate_left(29))?;
    if native_one_shot != native_chunked {
        return Err("native output or statistics changed with chunk boundaries".into());
    }
    if native_one_shot != bigint {
        return Err("native and bigint execution diverged".into());
    }
    for backend in [ExactDotBackend::Avx2, ExactDotBackend::Avx512] {
        if exact_dot_backend_available(backend) {
            let vector = render_random_chunks(
                native_stream_with_backend(&case, backend)?,
                &case,
                seed.rotate_right(u32::from(backend == ExactDotBackend::Avx512) + 7),
            )?;
            if native_one_shot != vector {
                return Err(format!("scalar and {backend} execution diverged").into());
            }
        }
    }
    let mut random = SplitMix64::new(seed ^ 0xa409_3822_299f_31d0);
    check_pcm_round_trip(&mut random)?;
    // Positive unity-sum phases bound outputs by the input extrema, even
    // when exact products require far more than 128 bits. The upper shift
    // reaches the signed Q65.63 endpoint without overflowing the stimulus.
    let wide_input = case
        .input
        .iter()
        .map(|sample| WideQ63::from_raw(i128::from(sample.raw()) << random.range(65)))
        .collect::<Vec<_>>();
    let wide = render_wide(&case, &wide_input, false, None)?;
    if wide != render_wide(&case, &wide_input, false, Some(seed.rotate_left(13)))? {
        return Err("wide native output or statistics changed with chunk boundaries".into());
    }
    if wide != render_wide(&case, &wide_input, true, Some(seed.rotate_right(19)))? {
        return Err("wide checked-i128/GMP fallback and bigint execution diverged".into());
    }
    let checksum = wide
        .0
        .iter()
        .fold(output_checksum(&native_one_shot.0), |hash, sample| {
            hash.rotate_left(11) ^ sample.raw() as u64 ^ (sample.raw() >> 64) as u64
        });
    Ok(checksum ^ check_arbitrary_signal(&case, &wide_input, seed.rotate_right(17))?)
}

fn run(config: Config) -> Result<(), Box<dyn Error>> {
    if let Some(seed) = config.case_seed {
        let checksum = config.suite.run_case(seed).map_err(|error| {
            format!(
                "replay: sex-fuzz --suite {} --case-seed {seed}: {error}",
                config.suite.name()
            )
        })?;
        println!(
            "{} case passed; case seed {seed}; checksum {checksum:016x}",
            config.suite.name()
        );
        return Ok(());
    }
    let mut seed_generator = SplitMix64::new(config.seed);
    let mut checksum = 0_u64;
    for case_index in 0..config.cases {
        let case_seed = seed_generator.next();
        let case_checksum = config.suite.run_case(case_seed).map_err(|error| {
            format!("case {case_index} failed; replay: sex-fuzz --suite {} --case-seed {case_seed}: {error}", config.suite.name())
        })?;
        checksum = checksum.rotate_left(7) ^ case_checksum;
    }
    if config.suite != Suite::Dsp {
        print!("{} suite; ", config.suite.name());
    }
    println!(
        "{} deterministic cases passed; root seed {}; checksum {:016x}",
        config.cases, config.seed, checksum
    );
    Ok(())
}

fn main() {
    match Config::parse(std::env::args()) {
        Ok(None) => print!("{HELP}"),
        Ok(Some(config)) => {
            if let Err(error) = run(config) {
                eprintln!("sex-fuzz: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("sex-fuzz: {error}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_replayable_and_validated() {
        assert_eq!(
            Config::parse([
                "sex-fuzz".to_owned(),
                "--cases".to_owned(),
                "12".to_owned(),
                "--seed".to_owned(),
                "42".to_owned(),
            ]),
            Ok(Some(Config {
                cases: 12,
                seed: 42,
                ..Config::default()
            }))
        );
        assert!(
            Config::parse(["sex-fuzz".to_owned(), "--cases".to_owned(), "0".to_owned(),]).is_err()
        );
    }

    #[test]
    fn direct_replay_and_suite_options_are_unambiguous() {
        let parse = |args: &[&str]| Config::parse(args.iter().map(|arg| (*arg).to_owned()));
        assert_eq!(
            parse(&["sex-fuzz", "--suite", "io", "--case-seed", "42"]),
            Ok(Some(Config {
                suite: Suite::Io,
                case_seed: Some(42),
                ..Config::default()
            }))
        );
        for args in [
            vec!["--case-seed", "42", "--seed", "42"],
            vec!["--case-seed", "42", "--cases", "1"],
            vec!["--case-seed", "42", "--case-seed", "42"],
            vec!["--suite", "io", "--suite", "all"],
            vec!["--seed", "1", "--seed", "2"],
            vec!["--cases", "1", "--cases", "2"],
            vec!["--suite", "unknown"],
            vec!["--case-seed", "-1"],
            vec!["--case-seed"],
        ] {
            let mut command = vec!["sex-fuzz"];
            command.extend(args);
            assert!(parse(&command).is_err(), "accepted {command:?}");
        }
    }

    #[test]
    fn fixed_seed_property_batch_passes() {
        run(Config {
            cases: 64,
            seed: 0xdead_beef_cafe_babe,
            ..Config::default()
        })
        .unwrap();
    }
}
