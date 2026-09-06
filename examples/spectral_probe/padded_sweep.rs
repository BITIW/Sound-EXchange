//! A complete 1 Hz -> upper-passband chirp, with constant-frequency guards.
//! Measurement-only MPFR synthesis; the processor still receives integer PCM.
use super::*;

#[derive(Clone, Copy, Debug)]
struct Profile {
    preset: sexplan::QualityPreset,
    end_hz: u32,
    guard_quarters: u64,
    frames: u64,
    planned_taps: u64,
    guard_tap_capacity: u64,
    required_snr: u32,
}

impl Profile {
    fn new(
        preset: sexplan::QualityPreset,
        from: u32,
        to: u32,
        guard: Option<u64>,
    ) -> Result<Self, Box<dyn Error>> {
        let plan = sexplan::plan_precision(sexplan::PlanRequest {
            ratio: sexrate::RateRatio::from_fraction(u64::from(to), u64::from(from))?,
            preset,
            error_floor: None,
            working_precision_bits: None,
        })?;
        // Comparisons share a band supported by BOTH the selected preset and
        // reference filters. High/heavier presets still get their own actual
        // full-band FIR qualification inside the CLI; this sweep alone is not
        // evidence for their extra upper-passband width.
        let percent = if preset == sexplan::QualityPreset::Fast {
            79
        } else {
            89
        };
        let radius = plan.taps_per_phase / 2 + 1;
        let minimum_guard = (radius * 4).div_ceil(u64::from(from)).max(1);
        let guard_quarters = guard.unwrap_or(minimum_guard);
        if guard_quarters < minimum_guard {
            return Err(
                format!("sweep guard needs at least {minimum_guard} quarter-seconds").into(),
            );
        }
        let frames = guard_quarters
            .checked_mul(2)
            .and_then(|g| g.checked_add(4))
            .and_then(|g| g.checked_mul(u64::from(from)))
            .map(|n| n / 4 + 7)
            .ok_or("sweep fixture length overflow")?;
        if frames > 10_000_000 || rounded_frames(frames, from, to) > 10_000_000 {
            return Err("sweep fixture exceeds the 10-million-frame measurement limit".into());
        }
        Ok(Self {
            preset,
            end_hz: from.min(to) * percent / 200,
            guard_quarters,
            frames,
            planned_taps: plan.taps_per_phase,
            guard_tap_capacity: 2 * (u64::from(from) * guard_quarters / 4 - 1) + 1,
            required_snr: preset
                .definition()
                .stopband_attenuation_db
                .saturating_sub(6)
                .min(85),
        })
    }

    fn from_environment(from: u32, to: u32) -> Result<Self, Box<dyn Error>> {
        let preset = match std::env::var("SEX_SPECTRAL_PRESET") {
            Ok(value) => value.parse()?,
            Err(std::env::VarError::NotPresent) => sexplan::QualityPreset::Sane,
            Err(error) => return Err(error.into()),
        };
        let guard = match std::env::var("SEX_SPECTRAL_GUARD_QUARTERS") {
            Ok(value) => Some(value.parse()?),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        Self::new(preset, from, to, guard)
    }

    fn core(self, rate: u32) -> (usize, usize) {
        let start = u64::from(rate) * self.guard_quarters;
        (
            start.div_ceil(4) as usize,
            ((start + 4 * u64::from(rate)) / 4 + 1) as usize,
        )
    }

    fn start_seconds(self) -> f64 {
        self.guard_quarters as f64 / 4.0
    }

    fn accepts_taps(self, actual: u64) -> bool {
        actual <= self.guard_tap_capacity
    }
}

pub(super) fn check_guard(from: u32, to: u32, actual: u64) -> Result<(), Box<dyn Error>> {
    let profile = Profile::from_environment(from, to)?;
    if !profile.accepts_taps(actual) {
        return Err(format!("accepted FIR has {actual} taps, sweep guard supports {}; increase SEX_SPECTRAL_GUARD_QUARTERS and rerun in a fresh directory", profile.guard_tap_capacity).into());
    }
    println!("actual_taps\tguard_tap_capacity\tguard_quarters");
    println!(
        "{actual}\t{}\t{}",
        profile.guard_tap_capacity, profile.guard_quarters
    );
    Ok(())
}

fn cycles(t: f64, end: u32) -> f64 {
    if t <= 0.0 {
        t
    } else if t >= 1.0 {
        (1.0 + f64::from(end)) / 2.0 + f64::from(end) * (t - 1.0)
    } else {
        t + (f64::from(end) - 1.0) * (t / 2.0 - (std::f64::consts::PI * t).sin() / TAU)
    }
}

fn frequency(t: f64, end: u32) -> f64 {
    1.0 + (f64::from(end) - 1.0) * (1.0 - (std::f64::consts::PI * t.clamp(0.0, 1.0)).cos()) / 2.0
}

fn ideal(index: usize, rate: u32, profile: Profile) -> f64 {
    0.5 * (TAU
        * cycles(
            index as f64 / f64::from(rate) - profile.start_seconds(),
            profile.end_hz,
        ))
    .sin()
}

fn code(index: u64, rate: u32, profile: Profile) -> i32 {
    // Build t = index/rate - guard_quarters/4 exactly before MPFR evaluation.
    let end = profile.end_hz;
    let mut t = Float::with_val(
        PRECISION,
        i128::from(index) * 4 - i128::from(rate) * i128::from(profile.guard_quarters),
    );
    t /= u64::from(rate) * 4;
    let phase_cycles = if t <= 0 {
        t
    } else if t >= 1 {
        t -= 1;
        t *= end;
        t += Float::with_val(PRECISION, end + 1) / 2;
        t
    } else {
        let pi = Float::with_val(PRECISION, Constant::Pi);
        let mut sine = Float::with_val(PRECISION, &t * &pi);
        sine.sin_mut();
        sine /= pi;
        sine /= 2;
        let mut integral = Float::with_val(PRECISION, &t / 2);
        integral -= sine;
        integral *= end - 1;
        integral += t;
        integral
    };
    let mut phase = phase_cycles * Float::with_val(PRECISION, Constant::Pi);
    phase *= 2;
    (phase.sin() * (1_u32 << 30))
        .to_integer_round(Round::Nearest)
        .unwrap()
        .0
        .to_i32()
        .unwrap()
}

pub(super) fn generate(path: &Path, from: u32, to: u32) -> Result<(), Box<dyn Error>> {
    let profile = Profile::from_environment(from, to)?;
    let end = profile.end_hz;
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = hound::WavWriter::new(
        BufWriter::new(file),
        hound::WavSpec {
            channels: 1,
            sample_rate: from,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for i in 0..profile.frames {
        writer.write_sample(code(i, from, profile))?;
    }
    writer.finalize()?;
    let mut manifest = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.with_extension("channels.tsv"))?;
    writeln!(
        manifest,
        "channel\tsignal\tstart_hz\tend_hz\tcore_start_seconds\tcore_end_seconds\tpreset\tplanned_taps\tguard_tap_capacity\trequired_sex_snr_db"
    )?;
    writeln!(
        manifest,
        "0\traised-cosine-frequency-sweep\t1\t{end}\t{}\t{}\t{}\t{}\t{}\t{}",
        profile.start_seconds(),
        profile.start_seconds() + 1.0,
        profile.preset.as_str(),
        profile.planned_taps,
        profile.guard_tap_capacity,
        profile.required_snr
    )?;
    println!("{}\t1", profile.frames);
    Ok(())
}

fn snr_passes(snr: f64, strict: bool, profile: Profile) -> bool {
    // This NEW full-band measurement includes deliberate reference rolloff.
    // 58 dB is slightly below the 58.77 dB implied by a 0.01 dB gain error.
    // It does not change the existing legacy sweep's 80 dB gate. SeX has no
    // fitted gain/phase correction. Fast gets 74 dB (declared 80 dB minus a
    // 6 dB chirp allowance); Sane and higher retain the 85 dB execution gate.
    !snr.is_nan()
        && snr
            >= if strict {
                f64::from(profile.required_snr)
            } else {
                58.0
            }
}

pub(super) fn analyze(
    path: &Path,
    from: u32,
    to: u32,
    directory: &Path,
    strict: bool,
) -> Result<(), Box<dyn Error>> {
    let profile = Profile::from_environment(from, to)?;
    let audio = read_wave(path)?;
    if audio.rate != to || audio.channels.len() != 1 {
        return Err("padded sweep rate/channel mismatch".into());
    }
    let values = &audio.channels[0];
    let expected = rounded_frames(profile.frames, from, to);
    let delta = values.len() as i64 - expected as i64;
    if delta.abs() > i64::from(!strict) {
        return Err(format!(
            "padded sweep duration gate: {}, expected {expected}",
            values.len()
        )
        .into());
    }
    let (start, stop) = profile.core(to);
    if values.len() < stop {
        return Err("padded sweep is missing its complete core".into());
    }
    let end = profile.end_hz;
    let residual: Vec<_> = (start..stop)
        .map(|i| values[i] - ideal(i, to, profile))
        .collect();
    fs::create_dir(directory)?;
    let mut report = output_file(directory, "sweep.tsv")?;
    writeln!(
        report,
        "interval\tstart_frame\tend_frame_exclusive\tstart_frequency_hz\tlast_frequency_hz\terror_dbfs\twaveform_snr_db\trequired_snr_db\tpassed"
    )?;
    let mut passed = true;
    // Full core plus ten disjoint consecutive subintervals. In particular,
    // a good low-frequency majority cannot hide an upper-passband failure.
    for part in 0..=10 {
        let (a, b) = if part == 0 {
            (start, stop)
        } else {
            (
                start + (stop - start) * (part - 1) / 10,
                start + (stop - start) * part / 10,
            )
        };
        let reference: Vec<_> = (a..b).map(|i| ideal(i, to, profile)).collect();
        let error = db(rms(&residual[a - start..b - start]));
        let snr = db(rms(&reference)) - error;
        let pass = snr_passes(snr, strict, profile);
        passed &= pass;
        writeln!(
            report,
            "{part}\t{a}\t{b}\t{:.9}\t{:.9}\t{error:.9}\t{snr:.9}\t{}\t{pass}",
            frequency(a as f64 / f64::from(to) - profile.start_seconds(), end),
            frequency(
                (b - 1) as f64 / f64::from(to) - profile.start_seconds(),
                end
            ),
            if strict { profile.required_snr } else { 58 }
        )?;
    }
    report.flush()?;
    let mut spectra = output_file(directory, "error-spectrum.tsv")?;
    writeln!(spectra, "signal\tbin\tfrequency_hz\tpeak_amplitude_dbfs")?;
    spectrum(&mut spectra, "sweep-core", &residual, to)?;
    spectrum(
        &mut spectra,
        "sweep-upper-decile",
        &residual[residual.len() * 9 / 10..],
        to,
    )?;
    spectra.flush()?;
    if !passed {
        return Err(
            "padded sweep waveform gate failed; all intervals retained in sweep.tsv".into(),
        );
    }
    println!(
        "{}: complete 1..{end} Hz sweep, {} core frames, duration delta {delta}; gates passed",
        path.display(),
        stop - start
    );
    Ok(())
}

pub(super) fn compare(
    left: &Path,
    right: &Path,
    from: u32,
    to: u32,
    directory: &Path,
) -> Result<(), Box<dyn Error>> {
    let profile = Profile::from_environment(from, to)?;
    let a = read_wave(left)?;
    let b = read_wave(right)?;
    let (start, stop) = profile.core(to);
    if a.rate != to
        || b.rate != to
        || a.channels.len() != 1
        || b.channels.len() != 1
        || a.channels[0].len() < stop
        || b.channels[0].len() < stop
    {
        return Err("padded sweep comparison layout/length mismatch".into());
    }
    fs::create_dir(directory)?;
    let mut report = output_file(directory, "difference.tsv")?;
    writeln!(
        report,
        "start_frame\tend_frame_exclusive\tend_hz\tdifference_rms_dbfs\tdifference_peak_dbfs"
    )?;
    let residual: Vec<_> = a.channels[0][start..stop]
        .iter()
        .zip(&b.channels[0][start..stop])
        .map(|(a, b)| a - b)
        .collect();
    writeln!(
        report,
        "{start}\t{stop}\t{}\t{:.9}\t{:.9}",
        profile.end_hz,
        db(rms(&residual)),
        db(residual.iter().map(|v| v.abs()).fold(0.0, f64::max))
    )?;
    report.flush()?;
    let mut spectra = output_file(directory, "difference-spectrum.tsv")?;
    writeln!(spectra, "signal\tbin\tfrequency_hz\tpeak_amplitude_dbfs")?;
    spectrum(&mut spectra, "sweep-core-difference", &residual, to)?;
    spectra.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mpfr_pcm_matches_independent_analytic_phase_across_both_joins() {
        for rate in [1000, 1001, 44100, 48000, 192000] {
            let profile = Profile::new(sexplan::QualityPreset::Sane, rate, 48000, None).unwrap();
            for index in [
                0,
                1,
                rate / 4 - 1,
                rate / 4,
                rate / 4 + 1,
                rate / 2,
                rate,
                rate * 5 / 4 - 1,
                rate * 5 / 4,
                rate * 5 / 4 + 1,
                rate * 3 / 2,
            ] {
                let actual = f64::from(code(u64::from(index), rate, profile)) / SCALE;
                assert!(
                    (actual - ideal(index as usize, rate, profile)).abs() <= 0.55 / SCALE,
                    "rate {rate}, index {index}"
                );
            }
        }
    }
    #[test]
    fn core_includes_both_endpoint_samples_without_guard_or_decile_gaps() {
        for rate in [1000_u32, 1001, 1002, 1003, 44100, 192000] {
            let profile = Profile::new(sexplan::QualityPreset::Sane, rate, rate, None).unwrap();
            let (start, stop) = profile.core(rate);
            assert!(start * 4 >= rate as usize && (start - 1) * 4 < rate as usize);
            assert!((stop - 1) * 4 <= rate as usize * 5 && stop * 4 > rate as usize * 5);
            let indices: Vec<_> = (0..10)
                .flat_map(|p| {
                    start + (stop - start) * p / 10..start + (stop - start) * (p + 1) / 10
                })
                .collect();
            assert_eq!(indices, (start..stop).collect::<Vec<_>>());
            assert!(stop < profile.frames as usize);
        }
    }
    #[test]
    fn phase_frequency_and_slope_are_continuous_at_guards() {
        let end = 21360;
        assert_eq!(cycles(0.0, end), 0.0);
        assert_eq!(cycles(1.0, end), (1.0 + f64::from(end)) / 2.0);
        assert_eq!(frequency(-0.1, end), 1.0);
        assert_eq!(frequency(1.1, end), f64::from(end));
        // Frequency approaches each guard quadratically: its one-sided
        // first derivative tends to zero, matching the constant extension.
        for edge in [0.0, 1.0] {
            let direction = if edge == 0.0 { 1.0 } else { -1.0 };
            let large = (frequency(edge + direction * 1e-3, end) - frequency(edge, end)).abs();
            let small = (frequency(edge + direction * 5e-4, end) - frequency(edge, end)).abs();
            assert!((large / small - 4.0).abs() < 1e-5);
        }
        for t in [0.0, 0.13, 0.5, 0.87, 1.0] {
            let derivative = (cycles(t + 1e-5, end) - cycles(t - 1e-5, end)) / 2e-5;
            assert!((derivative - frequency(t, end)).abs() < 2e-5);
        }
        let profile = Profile::new(sexplan::QualityPreset::Sane, 48000, 48000, None).unwrap();
        assert!(snr_passes(85.0, true, profile));
        assert!(!snr_passes(84.99, true, profile));
        assert!(snr_passes(58.0, false, profile));
        assert!(!snr_passes(57.99, false, profile));
        assert!(!snr_passes(f64::NAN, false, profile));
        assert!(!snr_passes(f64::NEG_INFINITY, true, profile));
    }

    #[test]
    fn profiles_keep_fixtures_in_common_passband_and_cover_planned_fir_support() {
        use sexplan::QualityPreset::*;
        for (preset, quarters, threshold) in [
            (Fast, 1, 74),
            (Sane, 1, 85),
            (High, 2, 85),
            (Absurd, 5, 85),
            (Pointless, 9, 85),
            (Until40k, 134, 85),
        ] {
            let p = Profile::new(preset, 48000, 1000, None).unwrap();
            assert_eq!(p.guard_quarters, quarters);
            assert_eq!(p.required_snr, threshold);
            assert!(p.guard_tap_capacity >= p.planned_taps);
            assert!(p.accepts_taps(p.guard_tap_capacity));
            assert!(!p.accepts_taps(p.guard_tap_capacity + 1));
            let transition = preset.definition().transition_width_of_lower_nyquist;
            assert!(
                u64::from(p.end_hz) * 2 * transition.denominator()
                    < 1000 * (transition.denominator() - transition.numerator())
            );
            assert!(snr_passes(f64::from(threshold), true, p));
            assert!(!snr_passes(f64::from(threshold) - 0.01, true, p));
            assert!(Profile::new(preset, 48000, 1000, Some(quarters - 1)).is_err());
            assert!(Profile::new(preset, 48000, 1000, Some(quarters + 1)).is_ok());
        }
        assert!(Profile::new(Sane, 48000, 1000, Some(u64::MAX)).is_err());
        assert!(Profile::new(Until40k, 1000, 192000, None).is_err());
    }

    #[test]
    fn shifted_guards_preserve_exact_chirp_time_and_pcm() {
        for preset in sexplan::QualityPreset::ALL {
            let short = Profile::new(preset, 48000, 1000, None).unwrap();
            let long = Profile::new(preset, 48000, 1000, Some(short.guard_quarters + 3)).unwrap();
            let short_start = short.guard_quarters * 12000;
            for offset in [0, 1, 12000, 47999, 48000, 48001] {
                let index = short_start + offset;
                assert_eq!(code(index, 48000, short), code(index + 36000, 48000, long));
                assert!(
                    (f64::from(code(index, 48000, short)) / SCALE
                        - ideal(index as usize, 48000, short))
                    .abs()
                        < 0.55 / SCALE
                );
            }
            assert_eq!(long.core(1000).0 - short.core(1000).0, 750);
            assert_eq!(long.frames - short.frames, 72000);
        }
    }
}
