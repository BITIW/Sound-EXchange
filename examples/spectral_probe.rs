//! External validation only: MPFR -> integer PCM fixtures, f64 measurement.
//! No floating-point value from this executable enters the SeX signal API.
use rug::{Float, float::Constant, float::Round};
use std::error::Error;
use std::f64::consts::TAU;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

#[path = "spectral_probe/impulse_phases.rs"]
mod impulse_phases;
#[path = "spectral_probe/padded_sweep.rs"]
mod padded_sweep;

const SCALE: f64 = 2_147_483_648.0;
const PRECISION: u32 = 192;

#[derive(Clone, Debug)]
enum Kind {
    Silence,
    Dc,
    Impulse {
        input_frame: u32,
    },
    Alternating,
    Tone {
        hz: u32,
        amplitude: i64,
        pass: bool,
        stop: bool,
    },
    Multitone([u32; 3]),
    Sweep(u32),
}

#[derive(Clone, Debug)]
struct Signal {
    name: String,
    kind: Kind,
}

fn reduced_ratio(from: u32, to: u32) -> (u32, u32) {
    let (mut a, mut b) = (from, to);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    (to / a, from / a)
}

fn signals(input_rate: u32, output_rate: u32) -> Vec<Signal> {
    let lower = input_rate.min(output_rate) as f64 / 2.0;
    let frequency = |fraction: f64| (lower * fraction).round().max(2.0) as u32;
    let mut result = vec![
        Signal {
            name: "silence".into(),
            kind: Kind::Silence,
        },
        Signal {
            name: "dc".into(),
            kind: Kind::Dc,
        },
        Signal {
            name: "impulse".into(),
            kind: Kind::Impulse {
                input_frame: input_rate / 2,
            },
        },
        Signal {
            name: "alternating-fs".into(),
            kind: Kind::Alternating,
        },
        Signal {
            name: "full-scale".into(),
            kind: Kind::Tone {
                hz: frequency(0.1),
                amplitude: i64::from(i32::MAX),
                pass: true,
                stop: false,
            },
        },
        Signal {
            name: "one-hz".into(),
            kind: Kind::Tone {
                hz: 1,
                amplitude: 1 << 30,
                pass: true,
                stop: false,
            },
        },
    ];
    for fraction in [0.05, 0.125, 0.25, 0.5, 0.75, 0.85, 0.89] {
        let hz = frequency(fraction);
        result.push(Signal {
            name: format!("pass-{hz}"),
            kind: Kind::Tone {
                hz,
                amplitude: 1 << 30,
                pass: true,
                stop: false,
            },
        });
    }
    result.push(Signal {
        name: "multitone".into(),
        kind: Kind::Multitone([frequency(0.07), frequency(0.27), frequency(0.71)]),
    });
    result.push(Signal {
        name: "log-sweep".into(),
        kind: Kind::Sweep(frequency(0.85)),
    });
    result.push(Signal {
        name: "near-input-nyquist".into(),
        kind: Kind::Tone {
            hz: input_rate / 2 - 1,
            amplitude: 1 << 30,
            pass: false,
            stop: input_rate / 2 - 1 > output_rate / 2,
        },
    });
    if output_rate < input_rate {
        for (name, hz) in [
            (
                "stop-mid",
                ((input_rate + output_rate) / 4 + 17).min(input_rate / 2 - 1),
            ),
            (
                "stop-edge",
                (output_rate / 2 + output_rate / 200).min(input_rate / 2 - 1),
            ),
        ] {
            if hz <= output_rate / 2 {
                continue;
            }
            result.push(Signal {
                name: name.into(),
                kind: Kind::Tone {
                    hz,
                    amplitude: 1 << 30,
                    pass: false,
                    stop: true,
                },
            });
        }
    }
    // An input impulse's fractional output center repeats every M input
    // frames for reduced L/M. Adjacent offsets cover distinct residues even
    // when L > M; do not mistake the engine's L output phases for this period.
    // Keep the original channel indices and append at most seven independent
    // impulses. All remain far from the fixture boundaries at supported rates.
    let (_, denominator) = reduced_ratio(input_rate, output_rate);
    for offset in 1..denominator.min(8) {
        result.push(Signal {
            name: format!("impulse-offset-{offset}"),
            kind: Kind::Impulse {
                input_frame: input_rate / 2 + offset,
            },
        });
    }
    result
}

fn mp_sine(hz: u32, index: usize, rate: u32) -> Float {
    let mut phase = Float::with_val(PRECISION, Constant::Pi);
    phase *= 2;
    phase *= index;
    phase *= hz;
    phase /= rate;
    phase.sin()
}

fn fixture_code(signal: &Signal, index: usize, rate: u32) -> i32 {
    let value = match signal.kind {
        Kind::Silence => return 0,
        Kind::Dc => return 1 << 29,
        Kind::Impulse { input_frame } => {
            return if index == input_frame as usize {
                1 << 30
            } else {
                0
            };
        }
        Kind::Alternating => {
            return if index.is_multiple_of(2) {
                i32::MAX
            } else {
                i32::MIN
            };
        }
        Kind::Tone { hz, amplitude, .. } => mp_sine(hz, index, rate) * amplitude,
        Kind::Multitone(frequencies) => {
            let mut sum = Float::with_val(PRECISION, 0);
            for hz in frequencies {
                sum += mp_sine(hz, index, rate);
            }
            sum * (1 << 28)
        }
        Kind::Sweep(end) => {
            // f(t) = exp(log(end) * t), from 1 Hz at t=0 to end Hz at t=1.
            let logarithm = Float::with_val(PRECISION, end).ln();
            let mut phase = Float::with_val(PRECISION, &logarithm);
            phase *= index;
            phase /= rate;
            phase.exp_mut();
            phase -= 1;
            phase /= &logarithm;
            phase *= Float::with_val(PRECISION, Constant::Pi);
            phase *= 2;
            phase.sin() * (1 << 30)
        }
    };
    value
        .to_integer_round(Round::Nearest)
        .unwrap()
        .0
        .to_i32()
        .unwrap()
}

fn generate(path: &Path, input_rate: u32, output_rate: u32) -> Result<(), Box<dyn Error>> {
    let channels = signals(input_rate, output_rate);
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = hound::WavWriter::new(
        BufWriter::new(file),
        hound::WavSpec {
            channels: u16::try_from(channels.len())?,
            sample_rate: input_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for frame in 0..input_rate as usize + 7 {
        for signal in &channels {
            writer.write_sample(fixture_code(signal, frame, input_rate))?;
        }
    }
    writer.finalize()?;
    let mut manifest = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.with_extension("channels.tsv"))?,
    );
    writeln!(manifest, "channel\tsignal\tdefinition")?;
    for (index, signal) in channels.iter().enumerate() {
        writeln!(manifest, "{index}\t{}\t{:?}", signal.name, signal.kind)?;
    }
    manifest.flush()?;
    println!("{}\t{}", input_rate as usize + 7, channels.len());
    Ok(())
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}
fn rms(values: &[f64]) -> f64 {
    (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt()
}
fn db(value: f64) -> f64 {
    20.0 * value.abs().log10()
}

fn tone_passes(gain: f64, phase: f64, snr: f64) -> bool {
    gain.is_finite()
        && phase.is_finite()
        && !snr.is_nan()
        && gain.abs() <= 0.01
        && phase.abs() <= 0.001
        && snr >= 85.0
}

#[derive(Debug)]
struct Fit {
    amplitude: f64,
    phase: f64,
    residual: f64,
}

fn fit_tone(values: &[f64], rate: u32, start: usize, hz: u32) -> Fit {
    // Least squares with sine, cosine AND DC. No integer-cycle assumption.
    let omega = TAU * hz as f64 / rate as f64;
    let sines: Vec<_> = (start..start + values.len())
        .map(|i| (omega * i as f64).sin())
        .collect();
    let cosines: Vec<_> = (start..start + values.len())
        .map(|i| (omega * i as f64).cos())
        .collect();
    let sm = mean(&sines);
    let cm = mean(&cosines);
    let ym = mean(values);
    let (mut ss, mut cc, mut sc, mut sy, mut cy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for ((s, c), y) in sines.iter().zip(&cosines).zip(values) {
        let s = s - sm;
        let c = c - cm;
        let y = y - ym;
        ss += s * s;
        cc += c * c;
        sc += s * c;
        sy += s * y;
        cy += c * y;
    }
    let determinant = ss * cc - sc * sc;
    let a = (sy * cc - cy * sc) / determinant;
    let b = (cy * ss - sy * sc) / determinant;
    let offset = ym - a * sm - b * cm;
    let residual: Vec<_> = sines
        .iter()
        .zip(&cosines)
        .zip(values)
        .map(|((s, c), y)| y - a * s - b * c - offset)
        .collect();
    Fit {
        amplitude: a.hypot(b),
        phase: b.atan2(a),
        residual: rms(&residual),
    }
}

fn analytic(kind: &Kind, time: f64) -> f64 {
    match kind {
        Kind::Silence | Kind::Impulse { .. } => 0.0,
        Kind::Dc => 0.25,
        Kind::Alternating => -0.5 / SCALE, // The asymmetric signed PCM endpoints' DC term.
        Kind::Tone { stop: true, .. } => 0.0,
        Kind::Tone { hz, amplitude, .. } => {
            *amplitude as f64 / SCALE * (TAU * *hz as f64 * time).sin()
        }
        Kind::Multitone(frequencies) => frequencies
            .iter()
            .map(|f| (TAU * *f as f64 * time).sin() * 0.125)
            .sum(),
        Kind::Sweep(end) => {
            let log = (*end as f64).ln();
            0.5 * (TAU * (log * time).exp_m1() / log).sin()
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    re: f64,
    im: f64,
}
impl Complex {
    fn product(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
    fn magnitude(self) -> f64 {
        self.re.hypot(self.im)
    }
}

fn fft(values: &mut [Complex]) {
    let n = values.len();
    assert!(n.is_power_of_two());
    if n == 1 {
        return;
    }
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - n.trailing_zeros());
        if i < j {
            values.swap(i, j);
        }
    }
    let mut width = 2;
    while width <= n {
        for block in values.chunks_exact_mut(width) {
            for j in 0..width / 2 {
                let angle = -TAU * j as f64 / width as f64;
                let v = block[j + width / 2].product(Complex {
                    re: angle.cos(),
                    im: angle.sin(),
                });
                let u = block[j];
                block[j] = Complex {
                    re: u.re + v.re,
                    im: u.im + v.im,
                };
                block[j + width / 2] = Complex {
                    re: u.re - v.re,
                    im: u.im - v.im,
                };
            }
        }
        width *= 2;
    }
}

fn spectrum(
    writer: &mut impl Write,
    name: &str,
    residual: &[f64],
    rate: u32,
) -> std::io::Result<()> {
    let size = (1_usize << residual.len().ilog2()).min(4096);
    let start = (residual.len() - size) / 2;
    let mut window_sum = 0.0;
    let mut values: Vec<_> = residual[start..start + size]
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let window = 0.5 - 0.5 * (TAU * i as f64 / size as f64).cos();
            window_sum += window;
            Complex {
                re: v * window,
                im: 0.0,
            }
        })
        .collect();
    fft(&mut values);
    for (bin, value) in values[..=size / 2].iter().enumerate() {
        let factor = if bin == 0 || bin == size / 2 {
            1.0
        } else {
            2.0
        };
        writeln!(
            writer,
            "{name}\t{bin}\t{:.9}\t{:.9}",
            bin as f64 * rate as f64 / size as f64,
            db(value.magnitude() * factor / window_sum)
        )?;
    }
    Ok(())
}

struct Audio {
    rate: u32,
    channels: Vec<Vec<f64>>,
}

fn read_wave(path: &Path) -> Result<Audio, Box<dyn Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int
        || spec.bits_per_sample != 32
        || spec.channels == 0
    {
        return Err("expected 32-bit integer multichannel WAVE".into());
    }
    let mut channels = vec![Vec::new(); spec.channels as usize];
    let mut count = 0;
    for (index, code) in reader.samples::<i32>().enumerate() {
        channels[index % spec.channels as usize].push(code? as f64 / SCALE);
        count += 1;
    }
    if count % spec.channels as usize != 0 {
        return Err("partial PCM frame".into());
    }
    Ok(Audio {
        rate: spec.sample_rate,
        channels,
    })
}

fn output_file(directory: &Path, name: &str) -> std::io::Result<BufWriter<std::fs::File>> {
    Ok(BufWriter::new(
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join(name))?,
    ))
}

fn rounded_frames(input: u64, from: u32, to: u32) -> u64 {
    let numerator = u128::from(input) * u128::from(to);
    let d = u128::from(from);
    let q = numerator / d;
    let r = numerator % d;
    (q + u128::from(r * 2 > d || (r * 2 == d && q % 2 != 0))) as u64
}

fn impulse_transfer(values: &[f64], rate: u32, center: f64, hz: f64) -> Complex {
    let mut response = Complex::default();
    for (index, value) in values.iter().enumerate() {
        // Integer-PCM impulse fixtures contain long exact-zero regions. Keep
        // original sample indices and nonzero summation order, skipping only
        // terms that contribute exactly zero at these finite probe frequencies.
        if *value == 0.0 {
            continue;
        }
        let angle = -TAU * hz * (index as f64 - center) / rate as f64;
        response.re += value * angle.cos();
        response.im += value * angle.sin();
    }
    response
}

fn impulse_response(
    values: &[f64],
    rate: u32,
    from: u32,
    input_frame: u32,
    name: &str,
    directory: &Path,
) -> Result<(), Box<dyn Error>> {
    let (numerator, denominator) = reduced_ratio(from, rate);
    let center_numerator = u64::from(input_frame) * u64::from(numerator);
    let center = center_numerator as f64 / f64::from(denominator);
    let scale = 0.5 * rate as f64 / from as f64;
    let peak = values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    let area = values.iter().sum::<f64>() / scale;
    let mut summary = output_file(directory, &format!("{name}-summary.tsv"))?;
    writeln!(
        summary,
        "peak_frame\texpected_center_frame\tnormalized_area\tinput_frame\tcenter_numerator\tcenter_denominator\tfractional_phase_numerator"
    )?;
    writeln!(
        summary,
        "{peak}\t{center:.12}\t{area:.12}\t{input_frame}\t{center_numerator}\t{denominator}\t{}",
        center_numerator % u64::from(denominator)
    )?;
    summary.flush()?;
    if (peak as f64 - center).abs() > 1.0 || (area - 1.0).abs() > 0.0001 {
        return Err(
            format!("{name} time/area gate: peak {peak}, expected {center}, area {area}").into(),
        );
    }
    let mut writer = output_file(directory, &format!("{name}-response.tsv"))?;
    writeln!(writer, "frequency_hz\tamplitude_db\tphase_radians")?;
    let lower = rate.min(from) as f64 / 2.0;
    for bin in 0..=128 {
        let hz = bin as f64 * rate as f64 / 256.0;
        let response = impulse_transfer(values, rate, center, hz);
        let gain = db(response.magnitude() / scale);
        let phase = response.im.atan2(response.re);
        writeln!(writer, "{hz:.9}\t{gain:.9}\t{phase:.12}")?;
        if hz <= lower * 0.85 && (!gain.is_finite() || gain.abs() > 0.01 || phase.abs() > 0.001) {
            return Err(
                format!("{name} passband gate at {hz} Hz: {gain} dB, phase {phase}").into(),
            );
        }
    }
    writer.flush()?;
    Ok(())
}

fn analyze(
    path: &Path,
    from: u32,
    to: u32,
    directory: &Path,
    strict_duration: bool,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir(directory)?;
    let Audio { rate, channels } = read_wave(path)?;
    let definitions = signals(from, to);
    if rate != to || channels.len() != definitions.len() {
        return Err("unexpected output rate/channel layout".into());
    }
    let frames = channels[0].len();
    let expected = rounded_frames(u64::from(from) + 7, from, to);
    let difference = frames as i64 - expected as i64;
    if difference.abs() > i64::from(!strict_duration) {
        return Err(format!(
            "duration gate: {frames}, expected {expected}, strict={strict_duration}"
        )
        .into());
    }
    let start = to as usize / 5;
    let end = frames - to as usize / 5;
    if end <= start + 64 {
        return Err("insufficient settled audio for analysis".into());
    }
    let mut metrics = output_file(directory, "metrics.tsv")?;
    let mut spectra = output_file(directory, "error-spectrum.tsv")?;
    writeln!(
        metrics,
        "signal\tfrequency_hz\trms_dbfs\terror_dbfs\twaveform_snr_db\tgain_db\tphase_radians\tfit_residual_dbfs\tfolded_frequency_hz\talias_peak_dbfs"
    )?;
    writeln!(spectra, "signal\tbin\tfrequency_hz\tpeak_amplitude_dbfs")?;
    let mut gains = Vec::new();
    for (signal, all) in definitions.iter().zip(&channels) {
        let values = &all[start..end];
        if let Kind::Impulse { input_frame } = signal.kind {
            impulse_response(all, to, from, input_frame, &signal.name, directory)?;
            continue;
        }
        let ideal: Vec<_> = (start..end)
            .map(|i| analytic(&signal.kind, i as f64 / to as f64))
            .collect();
        let residual: Vec<_> = values.iter().zip(&ideal).map(|(a, b)| a - b).collect();
        let error = db(rms(&residual));
        let snr = if matches!(
            signal.kind,
            Kind::Dc | Kind::Tone { pass: true, .. } | Kind::Multitone(_) | Kind::Sweep(_)
        ) {
            db(rms(&ideal)) - error
        } else {
            f64::NAN
        };
        let mut hz = 0;
        let mut gain = f64::NAN;
        let mut phase = f64::NAN;
        let mut fitted = f64::NAN;
        let mut folded = f64::NAN;
        let mut alias_peak = f64::NAN;
        match signal.kind {
            Kind::Silence if all.iter().any(|v| *v != 0.0) => {
                return Err("silence is not exactly zero with dither disabled".into());
            }
            Kind::Dc if (mean(values) - 0.25).abs() > 0.000_001 => {
                return Err("DC gain gate failed".into());
            }
            Kind::Tone {
                hz: frequency,
                amplitude,
                pass,
                stop,
            } => {
                hz = frequency;
                if pass {
                    let fit = fit_tone(values, to, start, hz);
                    gain = db(fit.amplitude / (amplitude as f64 / SCALE));
                    phase = fit.phase;
                    fitted = db(fit.residual);
                    // References have different deliberate passband shapes.
                    // Keep their waveform error, but test fitted residuals
                    // separately from the explicit gain/phase limits.
                    // SeX must still meet the uncorrected waveform SNR gate.
                    let gate_snr = if strict_duration {
                        snr
                    } else {
                        db(fit.amplitude / 2_f64.sqrt()) - fitted
                    };
                    if !tone_passes(gain, phase, gate_snr) {
                        return Err(format!(
                            "{} passband gate: gain={gain}, phase={phase}, SNR={snr}",
                            signal.name
                        )
                        .into());
                    }
                    if amplitude == 1 << 30 {
                        gains.push(gain);
                    }
                }
                let remainder = hz % to;
                let alias = remainder.min(to - remainder);
                folded = f64::from(alias);
                if stop && alias > 0 && alias * 2 < to {
                    alias_peak = db(fit_tone(values, to, start, alias).amplitude);
                }
                if stop && db(rms(values)) > -95.0 {
                    return Err(format!(
                        "{} stopband RMS gate: {} dBFS",
                        signal.name,
                        db(rms(values))
                    )
                    .into());
                }
            }
            Kind::Multitone(_) | Kind::Sweep(_) if snr.is_nan() || snr < 80.0 => {
                return Err(format!("{} waveform SNR gate: {snr}", signal.name).into());
            }
            _ => {}
        }
        writeln!(
            metrics,
            "{}\t{hz}\t{:.9}\t{error:.9}\t{snr:.9}\t{gain:.9}\t{phase:.12}\t{fitted:.9}\t{folded:.9}\t{alias_peak:.9}",
            signal.name,
            db(rms(values))
        )?;
        if matches!(signal.kind, Kind::Multitone(_) | Kind::Sweep(_))
            || signal.name == "near-input-nyquist"
            || signal.name == "stop-mid"
            || signal.name == "full-scale"
        {
            spectrum(&mut spectra, &signal.name, &residual, to)?;
        }
    }
    metrics.flush()?;
    spectra.flush()?;
    let min = gains.iter().copied().fold(f64::INFINITY, f64::min);
    let max = gains.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut summary = output_file(directory, "summary.tsv")?;
    writeln!(
        summary,
        "frames\texpected_frames\tduration_delta\tpassband_tone_ripple_db"
    )?;
    writeln!(
        summary,
        "{frames}\t{expected}\t{difference}\t{:.12}",
        max - min
    )?;
    summary.flush()?;
    println!(
        "{}: {frames} frames (delta {difference}), tone ripple {:.9} dB; gates passed",
        path.display(),
        max - min
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 4 && args[1] == "input-phase-count" {
        println!(
            "{}",
            reduced_ratio(parse_rate(&args[2])?, parse_rate(&args[3])?).1
        );
        return Ok(());
    }
    if args.len() == 7 && args[1] == "generate-phases" {
        return impulse_phases::generate(
            Path::new(&args[2]),
            parse_rate(&args[3])?,
            parse_rate(&args[4])?,
            args[5].parse()?,
            args[6].parse()?,
        );
    }
    if (args.len() == 8 || args.len() == 9) && args[1] == "analyze-phases" {
        if args.len() == 9 && args[8] != "strict" {
            return Err("expected strict duration flag".into());
        }
        return impulse_phases::analyze(
            Path::new(&args[2]),
            parse_rate(&args[3])?,
            parse_rate(&args[4])?,
            args[5].parse()?,
            args[6].parse()?,
            Path::new(&args[7]),
            args.len() == 9,
        );
    }
    if args.len() == 5 && args[1] == "check-sweep-guard" {
        return padded_sweep::check_guard(
            parse_rate(&args[2])?,
            parse_rate(&args[3])?,
            args[4].parse()?,
        );
    }
    if args.len() == 5 && (args[1] == "generate" || args[1] == "generate-sweep") {
        let generate = if args[1] == "generate-sweep" {
            padded_sweep::generate
        } else {
            generate
        };
        generate(
            Path::new(&args[2]),
            parse_rate(&args[3])?,
            parse_rate(&args[4])?,
        )
    } else if (args.len() == 6 || args.len() == 7)
        && (args[1] == "analyze" || args[1] == "analyze-sweep")
    {
        if args.len() == 7 && args[6] != "strict" {
            return Err("expected strict duration flag".into());
        }
        let analyze = if args[1] == "analyze-sweep" {
            padded_sweep::analyze
        } else {
            analyze
        };
        analyze(
            Path::new(&args[2]),
            parse_rate(&args[3])?,
            parse_rate(&args[4])?,
            Path::new(&args[5]),
            args.len() == 7,
        )
    } else if args.len() == 7 && (args[1] == "compare" || args[1] == "compare-sweep") {
        let compare = if args[1] == "compare-sweep" {
            padded_sweep::compare
        } else {
            compare
        };
        compare(
            Path::new(&args[2]),
            Path::new(&args[3]),
            parse_rate(&args[4])?,
            parse_rate(&args[5])?,
            Path::new(&args[6]),
        )
    } else {
        Err("usage: spectral_probe generate[-sweep] NEW_WAVE INPUT_RATE OUTPUT_RATE | analyze[-sweep] WAVE INPUT_RATE OUTPUT_RATE NEW_REPORT_DIRECTORY [strict] | compare[-sweep] LEFT_WAVE RIGHT_WAVE INPUT_RATE OUTPUT_RATE NEW_REPORT_DIRECTORY | check-sweep-guard INPUT_RATE OUTPUT_RATE ACTUAL_TAPS".into())
    }
}

fn parse_rate(text: &str) -> Result<u32, Box<dyn Error>> {
    let rate = text.parse()?;
    // Bounded one-second measurement fixtures, not a restriction on SeX itself.
    if !(1000..=192000).contains(&rate) {
        return Err("spectral fixture rates must be 1000..192000 Hz".into());
    }
    Ok(rate)
}

fn compare(
    left: &Path,
    right: &Path,
    from: u32,
    to: u32,
    directory: &Path,
) -> Result<(), Box<dyn Error>> {
    let a = read_wave(left)?;
    let b = read_wave(right)?;
    let definitions = signals(from, to);
    if a.rate != to
        || b.rate != to
        || a.channels.len() != definitions.len()
        || b.channels.len() != definitions.len()
    {
        return Err("pairwise comparison rate/channel mismatch".into());
    }
    let start = to as usize / 5;
    let frames = a.channels[0].len().min(b.channels[0].len());
    if frames <= start * 2 + 64 {
        return Err("pairwise comparison is too short".into());
    }
    let end = frames - start;
    fs::create_dir(directory)?;
    let mut metrics = output_file(directory, "difference.tsv")?;
    let mut spectra = output_file(directory, "difference-spectrum.tsv")?;
    writeln!(
        metrics,
        "signal\tdifference_rms_dbfs\tdifference_peak_dbfs\tleft_to_difference_db"
    )?;
    writeln!(spectra, "signal\tbin\tfrequency_hz\tpeak_amplitude_dbfs")?;
    for ((signal, a), b) in definitions.iter().zip(&a.channels).zip(&b.channels) {
        let residual: Vec<_> = a[start..end]
            .iter()
            .zip(&b[start..end])
            .map(|(a, b)| a - b)
            .collect();
        let error = db(rms(&residual));
        let peak = db(residual.iter().map(|v| v.abs()).fold(0.0, f64::max));
        writeln!(
            metrics,
            "{}\t{error:.9}\t{peak:.9}\t{:.9}",
            signal.name,
            db(rms(&a[start..end])) - error
        )?;
        if matches!(signal.kind, Kind::Multitone(_) | Kind::Sweep(_))
            || signal.name == "near-input-nyquist"
            || signal.name == "stop-mid"
            || signal.name == "full-scale"
        {
            spectrum(&mut spectra, &signal.name, &residual, to)?;
        }
    }
    metrics.flush()?;
    spectra.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_impulse_dft_matches_dense_oracle_bit_for_bit() {
        let mut values = vec![0.0; 4099];
        for (index, value) in values.iter_mut().enumerate() {
            if index % 47 == 0 {
                *value = (index as f64 - 2048.0) / SCALE;
            } else if index % 3 == 0 {
                *value = -0.0;
            }
        }
        for hz in [0.0, 1.0, 1234.5, 22050.0] {
            for center in [2048.0, 2048.0 + 17.0 / 441.0] {
                let mut dense = Complex::default();
                for (index, value) in values.iter().enumerate() {
                    let angle = -TAU * hz * (index as f64 - center) / 44100.0;
                    dense.re += value * angle.cos();
                    dense.im += value * angle.sin();
                }
                let sparse = impulse_transfer(&values, 44100, center, hz);
                assert_eq!(sparse.re.to_bits(), dense.re.to_bits());
                assert_eq!(sparse.im.to_bits(), dense.im.to_bits());
            }
        }
    }
    #[test]
    fn impulses_cover_distinct_input_phases_with_bounded_storage() {
        for (from, to, count) in [
            (48000, 16000, 3),
            (44100, 48000, 8),
            (48000, 44100, 8),
            (44100, 47900, 8),
            (48000, 1000, 8),
            (1000, 192000, 1),
            (192000, 1000, 8),
            (1001, 1000, 8),
            (44100, 44100, 1),
        ] {
            let (l, m) = reduced_ratio(from, to);
            let mut phases = std::collections::BTreeSet::new();
            let mut names = std::collections::BTreeSet::new();
            for signal in signals(from, to) {
                let Kind::Impulse { input_frame } = signal.kind else {
                    continue;
                };
                assert!(names.insert(signal.name.clone()));
                assert!(phases.insert(u64::from(input_frame) * u64::from(l) % u64::from(m)));
                assert!(input_frame >= from / 5 && input_frame < from - from / 5);
                for index in input_frame - 1..=input_frame + 1 {
                    assert_eq!(
                        fixture_code(&signal, index as usize, from),
                        if index == input_frame { 1 << 30 } else { 0 }
                    );
                }
            }
            assert_eq!(phases.len(), count, "ratio {to}/{from}");
            if m <= 8 {
                assert_eq!(phases, (0..u64::from(m)).collect());
            }
        }
    }

    #[test]
    fn impulse_dft_uses_fractional_center_without_peak_realignment() {
        let mut values = vec![0.0; 1017];
        values[500] = 0.5;
        let response = impulse_transfer(&values, 1000, 500.0, 217.0);
        assert_eq!(response.re, 0.5);
        assert_eq!(response.im, 0.0);
        let fractional = impulse_transfer(&values, 1000, 500.0 + 1.0 / 3.0, 217.0);
        assert!((fractional.magnitude() - 0.5).abs() < 1e-14);
        assert!((fractional.im.atan2(fractional.re) - TAU * 217.0 / 3000.0).abs() < 1e-12);
        // The same delay is removed only if BOTH the impulse and its exact
        // expected center move. Fitting the observed peak would hide a bug.
        values[500] = 0.0;
        values[507] = 0.5;
        let translated = impulse_transfer(&values, 1000, 507.0 + 1.0 / 3.0, 217.0);
        assert_eq!(translated.re, fractional.re);
        assert_eq!(translated.im, fractional.im);
        let wrong_center = impulse_transfer(&values, 1000, 500.0 + 1.0 / 3.0, 217.0);
        assert!((wrong_center.re - fractional.re).abs() > 0.1);
    }

    #[test]
    fn fit_recovers_non_coherent_tone_with_dc_and_phase() {
        let start = 193;
        let values: Vec<_> = (start..start + 2117)
            .map(|i| 0.37 * (TAU * 713.0 * i as f64 / 48000.0 + 0.29).sin() + 0.11)
            .collect();
        let fit = fit_tone(&values, 48000, start, 713);
        assert!((fit.amplitude - 0.37).abs() < 1e-13);
        assert!((fit.phase - 0.29).abs() < 1e-13);
        assert!(fit.residual < 1e-13);
    }
    #[test]
    fn low_frequency_fit_does_not_require_an_integer_number_of_cycles() {
        let values: Vec<_> = (9600..38407)
            .map(|i| 0.5 * (TAU * i as f64 / 48000.0).sin())
            .collect();
        let fit = fit_tone(&values, 48000, 9600, 1);
        assert!((fit.amplitude - 0.5).abs() < 1e-13);
        assert!(fit.phase.abs() < 1e-13);
    }
    #[test]
    fn fft_matches_independent_direct_dft() {
        for size in [1_usize, 2, 4, 8, 32, 128] {
            let original: Vec<_> = (0..size)
                .map(|i| Complex {
                    re: ((i * 17 % 31) as f64 - 15.0) / 32.0,
                    im: (i % 5) as f64 / 8.0,
                })
                .collect();
            let mut actual = original.clone();
            fft(&mut actual);
            for (bin, actual) in actual.iter().enumerate() {
                let mut expected = Complex::default();
                for (i, value) in original.iter().enumerate() {
                    let angle = -TAU * bin as f64 * i as f64 / size as f64;
                    let term = value.product(Complex {
                        re: angle.cos(),
                        im: angle.sin(),
                    });
                    expected.re += term.re;
                    expected.im += term.im;
                }
                assert!((actual.re - expected.re).abs() < 1e-11);
                assert!((actual.im - expected.im).abs() < 1e-11);
            }
        }
    }
    #[test]
    fn mpfr_fixtures_match_analytic_reference_at_pcm_resolution() {
        for signal in signals(48000, 16000) {
            if matches!(
                signal.kind,
                Kind::Impulse { .. } | Kind::Alternating | Kind::Tone { stop: true, .. }
            ) {
                continue;
            }
            for index in [0, 1, 17, 9001, 23456, 47999] {
                let actual = fixture_code(&signal, index, 48000) as f64 / SCALE;
                let expected = analytic(&signal.kind, index as f64 / 48000.0);
                assert!(
                    (actual - expected).abs() <= 0.51 / SCALE,
                    "{} at {index}",
                    signal.name
                );
            }
        }
    }
    #[test]
    fn integer_duration_is_ties_to_even() {
        assert_eq!(rounded_frames(1, 2, 1), 0);
        assert_eq!(rounded_frames(3, 2, 1), 2);
        assert_eq!(rounded_frames(48007, 48000, 16000), 16002);
    }
    #[test]
    fn full_scale_fixture_reaches_both_endpoints() {
        let signal = Signal {
            name: "full".into(),
            kind: Kind::Tone {
                hz: 1000,
                amplitude: i64::from(i32::MAX),
                pass: true,
                stop: false,
            },
        };
        assert_eq!(fixture_code(&signal, 12, 48000), i32::MAX);
        assert_eq!(fixture_code(&signal, 36, 48000), -i32::MAX);
        let alternating = Signal {
            name: "alternating".into(),
            kind: Kind::Alternating,
        };
        assert_eq!(fixture_code(&alternating, 0, 48000), i32::MAX);
        assert_eq!(fixture_code(&alternating, 1, 48000), i32::MIN);
    }
    #[test]
    fn analytic_sweep_phase_starts_at_zero() {
        assert_eq!(analytic(&Kind::Sweep(6000), 0.0), 0.0);
        assert!(analytic(&Kind::Sweep(6000), 1e-6) > 0.0);
    }

    #[test]
    fn gates_fail_closed_for_nan_and_detect_gain_phase_and_error() {
        assert!(tone_passes(0.0, 0.0, f64::INFINITY));
        assert!(!tone_passes(f64::NAN, 0.0, 120.0));
        assert!(!tone_passes(0.0, f64::NAN, 120.0));
        assert!(!tone_passes(0.0, 0.0, f64::NAN));
        assert!(!tone_passes(0.02, 0.0, 120.0));
        assert!(!tone_passes(0.0, 0.002, 120.0));
        assert!(!tone_passes(0.0, 0.0, 84.0));
    }

    #[test]
    fn hann_spectrum_reports_peak_amplitude_not_rms_or_psd() {
        let values: Vec<_> = (0..1024)
            .map(|i| 0.25 * (TAU * 37.0 * i as f64 / 1024.0).sin())
            .collect();
        let mut output = Vec::new();
        spectrum(&mut output, "known", &values, 48000).unwrap();
        let text = String::from_utf8(output).unwrap();
        let row = text
            .lines()
            .nth(37)
            .unwrap()
            .split('\t')
            .collect::<Vec<_>>();
        assert!((row[3].parse::<f64>().unwrap() - db(0.25)).abs() < 1e-8);
    }

    #[test]
    fn measurement_rates_are_bounded_before_fixture_allocation() {
        for value in ["0", "999", "192001", "4294967295", "-1", "NaN"] {
            assert!(parse_rate(value).is_err());
        }
        assert_eq!(parse_rate("47900").unwrap(), 47900);
    }

    #[test]
    fn stop_probes_do_not_collapse_to_a_zero_phase_dc_alias() {
        for (from, to) in [(48000, 16000), (48000, 44100), (48000, 1000)] {
            let definition = signals(from, to);
            let Kind::Tone { hz, stop, .. } = definition
                .iter()
                .find(|s| s.name == "stop-mid")
                .unwrap()
                .kind
            else {
                panic!()
            };
            assert!(stop && hz > to / 2 && hz < from / 2);
            assert_ne!(hz % to, 0);
            assert_ne!((hz % to) * 2, to);
        }
        assert!(
            signals(48000, 47999)
                .iter()
                .all(|s| !matches!(s.kind, Kind::Tone { stop: true, .. }))
        );
    }
}
