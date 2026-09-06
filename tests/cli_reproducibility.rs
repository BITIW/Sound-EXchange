use sexio::{AudioMetadata, AudioSpec, MetadataEntry, MetadataKey};
use sexio_sndfile::SndFileLibrary;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn original_error_floor_command_refines_precision_across_rate_forms_and_chunks() {
    let case = CaseDirectory::new("original-error-floor-command");
    let input = case.join("input.wav");
    let cache = case.join("cache");
    // 257 input frames yield 280 outputs, exercising every one of 160 phases.
    // Keep level low so clipping cannot hide a signal discrepancy.
    let samples: Vec<i32> = (0..514)
        .map(|i| ((i * 104729) % 2097152) - 1048576)
        .collect();
    write_pcm(&input, 2, &samples);
    let mut expected = None;
    let mut expected_qualification = None;
    for (form, block) in [(0, "1"), (1, "7"), (2, "4096")] {
        let output = case.join(&format!("form-{form}.wav"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_sex"));
        command.env("SEX_COEFFICIENT_CACHE", &cache).arg(&input);
        match form {
            0 => {
                command.arg(&output).args(["--rate", "48000"]);
            }
            1 => {
                command.args(["-r", "48000"]).arg(&output);
            }
            _ => {
                command.arg(&output).args(["rate", "48000"]);
            }
        }
        let result = command
            .args([
                "--error-floor",
                "-300dB",
                "--seed",
                "42",
                "--clip",
                "error",
                "--block-frames",
                block,
            ])
            .output()
            .unwrap();
        let trace = String::from_utf8(result.stderr).unwrap();
        assert!(result.status.success(), "{trace}");
        assert!(trace.contains("C=62, MPFR=192, coefficient-budget=false, response=true, next=Some(CoefficientPrecision)"), "{trace}");
        assert!(
            trace.contains("C=78, MPFR=192, coefficient-budget=true, response=true, next=None"),
            "{trace}"
        );
        assert!(trace.contains("signal: Q65.78"), "{trace}");
        assert!(trace.contains("saturated samples: 0"), "{trace}");
        assert!(trace.contains("qualification target: -300dB"), "{trace}");
        let qualification = fir_qualification(&trace).to_owned();
        assert!(
            qualification.contains("passband=true, stopband=true, coefficient-quantization=true")
        );
        let bytes = fs::read(&output).unwrap();
        let reader = hound::WavReader::new(bytes.as_slice()).unwrap();
        assert_eq!(reader.duration(), 280);
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 48000);
        if let Some(expected) = &expected {
            assert_eq!(&bytes, expected);
            assert_eq!(expected_qualification.as_ref().unwrap(), &qualification);
            assert!(trace.contains("coefficient cache: hit"), "{trace}");
        }
        expected = Some(bytes);
        expected_qualification = Some(qualification);
    }
    let analysis = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg("analyze")
        .arg(&input)
        .args(["-r", "48000", "--error-floor", "-300dB", "--seed", "42"])
        .output()
        .unwrap();
    assert!(
        analysis.status.success(),
        "{}",
        String::from_utf8_lossy(&analysis.stderr)
    );
    let report = String::from_utf8(analysis.stdout).unwrap();
    assert_eq!(fir_qualification(&report), expected_qualification.unwrap());
    assert!(report.contains("accumulator scope: full declared Q65.78 input range"));
}

#[test]
fn measured_pcm_error_matches_exported_samples_and_keeps_pre_pcm_reference() {
    let case = CaseDirectory::new("measured-pcm-error");
    let input = case.join("input.wav");
    let source = [
        0, 1, -1, 127, 128, 129, -129, 1_234_567, -2_345_678, 4_000_001,
    ];
    write_pcm(&input, 2, &source);
    for mode in [
        "none",
        "tpdf",
        "high-pass-tpdf",
        "noise-shaped-1",
        "noise-shaped-5",
        "noise-shaped-9",
    ] {
        let mut previous = None;
        for block in ["1", "7", "4096"] {
            let output = case.join(&format!("{mode}-{block}.wav"));
            let result = Command::new(env!("CARGO_BIN_EXE_sex"))
                .arg(&input)
                .arg(&output)
                .args([
                    "--bits",
                    "16",
                    "--dither",
                    mode,
                    "--seed",
                    "42",
                    "--block-frames",
                    block,
                ])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let stderr = String::from_utf8(result.stderr).unwrap();
            let line = stderr
                .lines()
                .find(|line| line.starts_with("measured final PCM error:"))
                .unwrap();
            assert!(line.ends_with("10 interleaved samples"));
            let rms: f64 = line
                .split("RMS ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let peak: f64 = line
                .split("peak ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let pcm: Vec<i32> = hound::WavReader::open(&output)
                .unwrap()
                .into_samples()
                .map(Result::unwrap)
                .collect();
            // Independent PCM24/PCM16 oracle. Float is only the test's final
            // dB comparison, not the measured aggregates or signal path.
            let errors: Vec<i64> = pcm
                .iter()
                .zip(source)
                .map(|(&pcm, source)| (i64::from(pcm) << 8) - i64::from(source))
                .collect();
            let sum: i128 = errors.iter().map(|&error| i128::from(error).pow(2)).sum();
            let exact_peak = errors.iter().map(|error| error.abs()).max().unwrap();
            let expected_rms = 10.0 * ((sum as f64 / source.len() as f64) / 2_f64.powi(46)).log10();
            let expected_peak = 20.0 * (exact_peak as f64 / 2_f64.powi(23)).log10();
            assert!((rms - expected_rms).abs() < 1e-10, "{line}");
            assert!((peak - expected_peak).abs() < 1e-10, "{line}");
            let identity = (line.to_owned(), fs::read(output).unwrap());
            if let Some(previous) = &previous {
                assert_eq!(&identity, previous);
            }
            previous = Some(identity);
        }
    }
    // Normalization must measure relative to the normalized signal, not 2 FS
    // before normalization. With no dither this single endpoint is exact.
    write_pcm(&input, 1, &[1 << 22]);
    for clip in ["normalize", "saturate"] {
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .arg(&input)
            .arg(case.join(&format!("{clip}.wav")))
            .args([
                "--gain", "4", "--bits", "16", "--dither", "none", "--clip", clip,
            ])
            .output()
            .unwrap();
        assert!(result.status.success());
        let stderr = String::from_utf8(result.stderr).unwrap();
        let line = stderr
            .lines()
            .find(|line| line.starts_with("measured final PCM error:"))
            .unwrap();
        // Saturating 2 FS to just below 1 FS leaves slightly more than 1 FS error.
        if clip == "saturate" {
            let rms: f64 = line
                .split("RMS ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert!(rms > 0.0 && rms < 0.001, "{line}");
        } else {
            assert!(line.contains("RMS -inf dBFS; peak -inf dBFS"), "{line}");
        }
    }
    for samples in [&[][..], &[0, 1, -1][..]] {
        write_pcm(&input, 1, samples);
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .arg(&input)
            .arg(case.join("bypass.wav"))
            .output()
            .unwrap();
        assert!(result.status.success());
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stderr.contains(if samples.is_empty() {
            "RMS n/a (empty) dBFS"
        } else {
            "RMS -inf dBFS"
        }));
    }
}

static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

#[test]
fn equivalent_integer_rate_spellings_produce_identical_pcm() {
    let case = CaseDirectory::new("exact-integer-rate-spellings");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 131072, -262144, 524288, 0]);
    let cache = case.join("cache");
    let mut reference = None;
    for (index, (option, rate)) in [
        ("-r", "66150"),
        ("--rate", "66150.000"),
        ("rate", "66.15k"),
        ("--rate", "132300/2"),
        ("-r", "6.615e4"),
    ]
    .into_iter()
    .enumerate()
    {
        let output = case.join(&format!("{index}.wav"));
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .arg(&input)
            .arg(&output)
            .args([option, rate, "--preset", "fast", "--dither", "none"])
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = fs::read(output).unwrap();
        if let Some(reference) = &reference {
            assert_eq!(&bytes, reference);
        } else {
            reference = Some(bytes);
        }
    }
}

#[test]
fn fractional_rate_analysis_uses_reduced_ratio_and_shared_coefficient_identity() {
    let case = CaseDirectory::new("exact-fractional-analysis");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0]);
    let cache = case.join("cache");
    let mut previous = None;
    for (index, rate) in ["16537.5", "33075/2", "16.5375k"].into_iter().enumerate() {
        let result = Command::new(env!("CARGO_BIN_EXE_sex-analyze"))
            .arg(&input)
            .args(["--rate", rate, "--preset", "fast", "--certify-design"])
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report = String::from_utf8(result.stdout).unwrap();
        assert!(report.contains("ratio: 3/8"));
        assert!(report.contains("requested output rate: 33075/2 Hz; analysis only"));
        assert!(report.contains("design certificate: passed; all 3 phases"));
        if index > 0 {
            assert!(report.contains("coefficient cache: hit"));
        }
        let stable: Vec<_> = report
            .lines()
            .filter(|line| !line.starts_with("coefficient cache:"))
            .map(str::to_owned)
            .collect();
        if let Some(previous) = &previous {
            assert_eq!(&stable, previous);
        } else {
            previous = Some(stable);
        }
    }
}

#[test]
fn nonintegral_container_rate_never_rounds_or_overwrites() {
    let case = CaseDirectory::new("fractional-container-refusal");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let cache = case.join("must-not-exist");
    write_pcm(&input, 1, &[0]);
    fs::write(&output, b"existing output").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sex-rate"))
        .arg(&input)
        .arg(&output)
        .args(["--rate", "16537.5"])
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("never silently rounded")
    );
    assert_eq!(fs::read(output).unwrap(), b"existing output");
    assert!(!cache.exists());
}

#[test]
fn exotic_decimal_plan_remains_exact_without_materializing_billions_of_phases() {
    let case = CaseDirectory::new("exotic-exact-rate-plan");
    let input = case.join("input.wav");
    let cache = case.join("must-not-exist");
    write_pcm(&input, 1, &[0]);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .arg("plan")
        .arg(&input)
        .args(["--rate", "141421.356237", "--preset", "fast"])
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = String::from_utf8(result.stdout).unwrap();
    assert!(report.contains("ratio: 6734350297/2100000000"));
    assert!(report.contains("requested output rate: 141421356237/1000000 Hz"));
    assert!(!cache.exists());
}

#[test]
fn certified_error_redesign_requalifies_and_obeys_aggregate_budgets() {
    let case = CaseDirectory::new("certified-error-redesign");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 1, -1, 131072, -131072]);
    let cache = case.join("cache");
    let analyze = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_sex"))
            .arg("analyze")
            .arg(&input)
            .args([
                "-r",
                "66150",
                "--preset",
                "fast",
                "--certify-design",
                "--design-certificate-error-bits",
                "200",
            ])
            .args(extra)
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .output()
            .unwrap()
    };
    let first = analyze(&[]);
    let trace = String::from_utf8(first.stderr).unwrap();
    assert!(first.status.success(), "{trace}");
    assert_eq!(
        trace
            .matches("quality feedback accepted 1 candidate(s)")
            .count(),
        2
    );
    assert!(trace.contains("redesign 1: C 62 -> 213, MPFR 128 -> 288"));
    let report = String::from_utf8(first.stdout).unwrap();
    assert!(report.contains("design certificate aggregate: 1 bank redesign(s)"));
    assert!(report.contains("actual coefficient fractional bits 213"));
    assert!(report.contains("mathematical normalized Kaiser FIR"));
    let aggregate = report
        .lines()
        .find(|line| line.starts_with("design certificate aggregate:"))
        .unwrap();
    let work: u64 = aggregate
        .split(", ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let exact = analyze(&["--design-certificate-work", &work.to_string()]);
    assert!(
        exact.status.success(),
        "{}",
        String::from_utf8_lossy(&exact.stderr)
    );
    assert!(String::from_utf8(exact.stdout).unwrap().contains(aggregate));
    let short_work = (work - 1).to_string();
    for extra in [
        vec!["--design-certificate-work", short_work.as_str()],
        vec!["--refinement-attempts", "1"],
        vec!["--refinement-coefficients", "387"],
        vec!["--refinement-terms", "50310"],
        vec!["--precision", "128"],
    ] {
        let result = analyze(&extra);
        assert!(!result.status.success(), "accepted {extra:?}");
        assert!(
            !String::from_utf8(result.stdout)
                .unwrap()
                .contains("design certificate: passed")
        );
    }
    let output = case.join("render.wav");
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .arg(&input)
        .arg(&output)
        .args([
            "-r",
            "66150",
            "--preset",
            "fast",
            "--certify-design",
            "--design-certificate-error-bits",
            "200",
            "--dither",
            "none",
        ])
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains(aggregate)
    );
    assert!(output.exists());
}

#[test]
fn certified_chain_rechecks_gain_dc_mix_convolution_and_normalization() {
    let case = CaseDirectory::new("certified-numerical-chain");
    let input = case.join("input.wav");
    write_pcm(
        &input,
        2,
        &[0, 100, 131072, -131072, 524288, 262144, -1048576, 0],
    );
    let mut outputs = Vec::new();
    for enabled in [false, true] {
        let output = case.join(&format!("{enabled}.wav"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_sex"));
        command
            .arg(&input)
            .arg(&output)
            .args([
                "-r",
                "66150",
                "--preset",
                "fast",
                "--gain",
                "1152921504606846976",
                "--dc-remove",
                "3/4",
                "--mix",
                "1/2,1/2",
                "--convolve",
                "1/3,1/7",
                "--clip",
                "normalize",
                "--dither",
                "none",
            ])
            .env("SEX_COEFFICIENT_CACHE", case.join("cache"));
        if enabled {
            command.args(["--certify-design", "--design-certificate-bits", "32"]);
        }
        let result = command.output().unwrap();
        let trace = String::from_utf8(result.stderr).unwrap();
        assert!(result.status.success(), "{trace}");
        if enabled {
            assert!(
                trace.contains("includes certified MPFR design/quantization/DC-correction error")
            );
            assert!(trace.contains("design certificate effective coefficient target:"));
            assert!(!trace.contains("design certificate arithmetic: 32 endpoint bits"));
        } else {
            assert!(trace.contains("excludes ideal-filter approximation, MPFR design error"));
        }
        outputs.push(fs::read(output).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn design_certificates_cover_actual_native_and_bigint_cache_hits_and_render_banks() {
    let case = CaseDirectory::new("design-certificates");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 131072, -262144, 524288, -1048576, 0, 7]);
    let lines = |text: &str| {
        text.lines()
            .filter(|line| {
                line.starts_with("design certificate:")
                    || line.starts_with("design certificate arithmetic:")
                    || line.starts_with("design certificate target:")
                    || line.starts_with("design certificate exact")
            })
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    for (preset, bits) in [("fast", "62"), ("high", "96")] {
        let cache = case.join(preset);
        let mut reports = Vec::new();
        for expected in ["miss; designed and stored", "hit"] {
            let result = Command::new(env!("CARGO_BIN_EXE_sex"))
                .args(["analyze"])
                .arg(&input)
                .args([
                    "-r",
                    "66150",
                    "--preset",
                    preset,
                    "--certify-design",
                    "--design-certificate-bits",
                    "32",
                ])
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let text = String::from_utf8(result.stdout).unwrap();
            assert!(text.contains(&format!("coefficient cache: {expected}")));
            assert!(text.contains("design certificate: passed; all 3 phases;"));
            assert!(text.contains(&format!("actual coefficient fractional bits {bits}")));
            assert!(text.contains(
                "numerical reference: exact effects + mathematical normalized Kaiser FIR"
            ));
            assert!(
                text.contains("includes certified MPFR design/quantization/DC-correction error")
            );
            let identity = text
                .lines()
                .find_map(|line| line.strip_prefix("coefficient sha256: "))
                .unwrap();
            assert!(text.contains(&format!("actual coefficient sha256 {identity}")));
            reports.push(lines(&text));
        }
        assert_eq!(reports[0], reports[1]);
        let mut outputs = Vec::new();
        for enabled in [false, true] {
            let output = case.join(&format!("{preset}-{enabled}.wav"));
            let mut command = Command::new(env!("CARGO_BIN_EXE_sex"));
            command
                .arg(&input)
                .arg(&output)
                .args(["-r", "66150", "--preset", preset, "--dither", "none"])
                .env("SEX_COEFFICIENT_CACHE", &cache);
            if enabled {
                command.args(["--certify-design", "--design-certificate-bits", "32"]);
            }
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            if enabled {
                assert_eq!(
                    lines(&String::from_utf8(result.stderr).unwrap()),
                    reports[0]
                );
            }
            outputs.push(fs::read(output).unwrap());
        }
        assert_eq!(outputs[0], outputs[1]);
    }
}

#[test]
fn design_certificate_failures_never_publish_output_or_claim_compliance() {
    let case = CaseDirectory::new("design-certificate-failures");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 100, -100]);
    for (label, rate, extra, expected) in [
        ("same-rate", "44100", vec![], "requires a rate change"),
        (
            "inconclusive",
            "66150",
            vec![
                "--design-certificate-bits",
                "16",
                "--design-certificate-max-bits",
                "16",
            ],
            "inconclusive enclosure",
        ),
        (
            "chain-loose-target",
            "66150",
            vec![
                "--gain",
                "1152921504606846976",
                "--clip",
                "normalize",
                "--design-certificate-bits",
                "32",
                "--design-certificate-max-bits",
                "32",
                "--design-certificate-error-bits",
                "1",
            ],
            "target not met",
        ),
        (
            "non-kaiser",
            "66150",
            vec!["--window", "hann"],
            "requires the Kaiser",
        ),
        (
            "too-many-phases",
            "66150",
            vec!["--design-certificate-phases", "2"],
            "preflight",
        ),
        (
            "work",
            "66150",
            vec!["--design-certificate-work", "1"],
            "design certificate failed",
        ),
        (
            "target",
            "66150",
            vec![
                "--design-certificate-error-bits",
                "200",
                "--design-certificate-max-bits",
                "192",
            ],
            "target not met",
        ),
    ] {
        let output = case.join(&format!("{label}.wav"));
        fs::write(&output, b"existing user output").unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .arg(&input)
            .arg(&output)
            .args(["-r", rate, "--preset", "fast", "--certify-design"])
            .args(extra)
            .env("SEX_COEFFICIENT_CACHE", case.join("cache"))
            .output()
            .unwrap();
        assert!(!result.status.success());
        let diagnostic = String::from_utf8(result.stderr).unwrap();
        assert!(diagnostic.contains(expected), "{label}: {diagnostic}");
        if label == "inconclusive" {
            assert!(!diagnostic.contains("redesign 1:"));
        }
        assert!(!diagnostic.contains("design certificate: passed"));
        assert_eq!(fs::read(output).unwrap(), b"existing user output");
    }
}

#[test]
fn plan_reports_design_certificate_request_without_building_or_certifying() {
    let case = CaseDirectory::new("design-certificate-plan");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0]);
    let cache = case.join("absent-cache");
    for rate in ["66150", "44100"] {
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .arg("plan")
            .arg(&input)
            .args(["-r", rate, "--certify-design"])
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .output()
            .unwrap();
        assert!(result.status.success());
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.contains("design certificate requested:"));
        assert!(text.contains("not executed by plan"));
        assert!(!text.contains("design certificate: passed"));
        assert!(!cache.exists());
        if rate == "44100" {
            assert!(text.contains("design certificate is not applicable to same-rate conversion"));
        }
    }
}

#[test]
fn design_certificate_checks_the_feedback_accepted_bank_not_initial_taps() {
    let case = CaseDirectory::new("design-certificate-feedback");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 100, -100]);
    let output = case.join("must-not-exist.wav");
    let cache = case.join("cache");
    let denied = Command::new(env!("CARGO_BIN_EXE_sex"))
        .arg(&input)
        .arg(&output)
        .args(["-r", "22050", "--preset", "absurd", "--certify-design"])
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .output()
        .unwrap();
    let trace = String::from_utf8(denied.stderr).unwrap();
    assert!(!denied.status.success());
    assert!(
        trace.contains("quality feedback accepted 2 candidate(s)"),
        "{trace}"
    );
    assert!(trace.contains("design certificate preflight"));
    assert!(!trace.contains("design certificate: passed"));
    assert!(!output.exists());
    let accepted = Command::new(env!("CARGO_BIN_EXE_sex"))
        .arg("analyze")
        .arg(&input)
        .args([
            "-r",
            "22050",
            "--preset",
            "absurd",
            "--certify-design",
            "--design-certificate-taps",
            "5000",
        ])
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .output()
        .unwrap();
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let report = String::from_utf8(accepted.stdout).unwrap();
    assert!(report.contains("1 phases × 4309 taps"));
    assert!(report.contains("design certificate: passed; all 1 phases; actual coefficient sha256 0a817eb4728f4ccfe322e3ba34269b787ffc5ad8613d49db09e9d398b95540a2"));
    assert!(report.contains("coefficient cache: hit"));
}

#[cfg(unix)]
#[test]
fn frontends_reject_non_utf8_arguments_without_panicking() {
    use std::os::unix::ffi::OsStringExt;
    for binary in [
        env!("CARGO_BIN_EXE_sex"),
        env!("CARGO_BIN_EXE_sex-rate"),
        env!("CARGO_BIN_EXE_sex-analyze"),
    ] {
        let result = Command::new(binary)
            .arg(std::ffi::OsString::from_vec(vec![0xff]))
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        let diagnostic = String::from_utf8(result.stderr).unwrap();
        assert!(diagnostic.contains("arguments must be valid UTF-8"));
        assert!(!diagnostic.contains("panicked"));
    }
}

#[test]
fn standalone_frontends_and_library_execute_the_same_qualified_pipeline() {
    let case = CaseDirectory::new("standalone-frontends");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 131072, 262144, -524288, 1048576, 524288, 0]);
    for (label, quality) in [
        ("kaiser", vec!["-r", "66150", "--preset", "sane"]),
        (
            "remez",
            vec!["-r", "29400", "--preset", "fast", "--designer", "remez"],
        ),
    ] {
        let cache = case.join(label);
        let mut quality = quality;
        quality.extend([
            "--grid",
            "9",
            "--certify",
            "--harmonics",
            "--clip",
            "normalize",
            "--dither",
            "none",
            "--gain",
            "2",
        ]);
        let original = case.join(&format!("{label}-sex.wav"));
        let rate = case.join(&format!("{label}-rate.wav"));
        let mut traces = Vec::new();
        for (binary, output, block) in [
            (env!("CARGO_BIN_EXE_sex"), &original, "1"),
            (env!("CARGO_BIN_EXE_sex-rate"), &rate, "7"),
        ] {
            let result = Command::new(binary)
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg(&input)
                .arg(output)
                .args(&quality)
                .args(["--block-frames", block])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            traces.push(String::from_utf8(result.stderr).unwrap());
        }
        assert_eq!(fs::read(&original).unwrap(), fs::read(&rate).unwrap());
        assert_eq!(fir_qualification(&traces[0]), fir_qualification(&traces[1]));
        for (binary, prefix) in [
            (env!("CARGO_BIN_EXE_sex"), Some("analyze")),
            (env!("CARGO_BIN_EXE_sex-analyze"), None),
        ] {
            let mut command = Command::new(binary);
            command.env("SEX_COEFFICIENT_CACHE", &cache);
            if let Some(prefix) = prefix {
                command.arg(prefix);
            }
            let result = command.arg(&input).args(&quality).output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                fir_qualification(&String::from_utf8(result.stdout).unwrap()),
                fir_qualification(&traces[0])
            );
        }
    }
    assert_eq!(sex::q::Q1_63::ZERO.raw(), 0);
    assert_eq!(
        sex::rate::RateRatio::from_rates(44100, 48000).unwrap().up(),
        160
    );
    assert!(
        sex::cli::run(
            sex::cli::Frontend::Rate,
            ["embedded", "in.wav", "out.wav"].map(str::to_owned)
        )
        .is_err()
    );
}

#[test]
fn standalone_frontends_reject_wrong_modes_and_preserve_existing_output() {
    let case = CaseDirectory::new("frontend-failures");
    let input = case.join("input.wav");
    let output = case.join("protected.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[1]);
    fs::write(&output, b"preserved destination").unwrap();
    for (binary, name) in [
        (env!("CARGO_BIN_EXE_sex-rate"), "sex-rate"),
        (env!("CARGO_BIN_EXE_sex-analyze"), "sex-analyze"),
    ] {
        for informational in ["--help", "--version", "--build-info"] {
            let result = Command::new(binary).arg(informational).output().unwrap();
            assert!(result.status.success());
            assert!(String::from_utf8_lossy(&result.stdout).contains(name));
        }
        let result = Command::new(binary)
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg(&input)
            .arg(&output)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).starts_with(&format!("{name}:")));
        assert_eq!(fs::read(&output).unwrap(), b"preserved destination");
        assert!(!cache.exists());
    }
    let result = Command::new(env!("CARGO_BIN_EXE_sex-rate"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg(&input)
        .arg(&output)
        .args([
            "-r",
            "48000",
            "--preset",
            "until-40k",
            "--refinement-coefficients",
            "4000000",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"preserved destination");
    assert!(!cache.exists());
    let result = Command::new(env!("CARGO_BIN_EXE_sex-rate"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg("plan")
        .arg(&input)
        .args(["-r", "48000", "--preset", "until-40k"])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(!cache.exists());
}

#[test]
fn unity_optimized_analysis_qualifies_and_reuses_a_hypothetical_bank() {
    let case = CaseDirectory::new("unity-optimized");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 131072, 0]);
    for designer in ["global-ls", "remez"] {
        let cache = case.join(designer);
        let options = [
            "--designer",
            designer,
            "--preset",
            "fast",
            "-r",
            "44100",
            "--grid",
            "9",
            "--certify",
            "--harmonics",
        ];
        let plan = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg("plan")
            .arg(&input)
            .args(options)
            .output()
            .unwrap();
        assert!(
            plan.status.success(),
            "{}",
            String::from_utf8_lossy(&plan.stderr)
        );
        assert!(!cache.exists());
        assert!(!String::from_utf8_lossy(&plan.stdout).contains("qualified FIR sha256:"));
        let mut previous = None;
        for warm in [false, true] {
            let analysis = Command::new(env!("CARGO_BIN_EXE_sex"))
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg("analyze")
                .arg(&input)
                .args(options)
                .output()
                .unwrap();
            assert!(
                analysis.status.success(),
                "{designer}: {}",
                String::from_utf8_lossy(&analysis.stderr)
            );
            let report = String::from_utf8_lossy(&analysis.stdout);
            assert!(report.contains("unity-rate analysis designs a hypothetical FIR"));
            assert!(report.contains("continuous certificate: Certified"));
            assert!(report.contains("-unity-v"));
            assert!(report.contains("stopband geometry: Nyquist endpoint only"));
            assert!(
                !String::from_utf8_lossy(&analysis.stderr).contains("designer numerical failure:")
            );
            if warm {
                assert!(report.contains("coefficient cache: hit"));
            }
            let qualification = fir_qualification(&report);
            if let Some(previous) = previous {
                assert_eq!(qualification, previous);
            }
            previous = Some(qualification.to_owned());
        }
        assert_eq!(fs::read_dir(cache).unwrap().count(), 1);
    }
}

#[test]
fn optimized_cli_qualifies_and_streams_the_same_cached_bank() {
    let case = CaseDirectory::new("optimized-cli");
    let input = case.join("input.wav");
    write_pcm(
        &input,
        1,
        &[
            0, 131072, 262144, 524288, 1048576, 524288, 262144, 131072, 0,
        ],
    );
    for (label, designer, rate, gain) in [
        ("ls", "global-ls", "29400", "1"),
        ("remez", "remez", "29400", "1"),
        ("ls-gain", "global-ls", "14700", "1152921504606846976"),
    ] {
        let cache = case.join(label);
        let options = [
            "--designer",
            designer,
            "--preset",
            "fast",
            "-r",
            rate,
            "--gain",
            gain,
            "--grid",
            "9",
            "--certify",
            "--harmonics",
            "--dither",
            "none",
            "--clip",
            "normalize",
        ];
        let analysis = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg("analyze")
            .arg(&input)
            .args(options)
            .output()
            .unwrap();
        assert!(
            analysis.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&analysis.stderr)
        );
        let report = String::from_utf8_lossy(&analysis.stdout);
        let qualification = fir_qualification(&report);
        assert!(qualification.contains("qualified designer:"));
        assert!(qualification.contains("continuous certificate: Certified"));
        assert!(
            qualification
                .contains("main-complex-error=true, images=true, image-L2=true, stopband=true")
        );
        assert!(!qualification.contains("window="));
        let mut outputs = Vec::new();
        for block in ["1", "7", "4096"] {
            let output = case.join(&format!("{label}-{block}.wav"));
            let conversion = Command::new(env!("CARGO_BIN_EXE_sex"))
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg(&input)
                .arg(&output)
                .args(options)
                .args(["--block-frames", block])
                .output()
                .unwrap();
            let trace = String::from_utf8_lossy(&conversion.stderr);
            assert!(conversion.status.success(), "{label}/{block}: {trace}");
            assert_eq!(fir_qualification(&trace), qualification);
            assert!(trace.contains("coefficient cache: hit"));
            outputs.push(fs::read(output).unwrap());
        }
        assert_eq!(outputs[0], outputs[1]);
        assert_eq!(outputs[0], outputs[2]);
        assert!(
            hound::WavReader::new(outputs[0].as_slice())
                .unwrap()
                .into_samples::<i32>()
                .any(|sample| sample.unwrap() != 0)
        );
        let protected = case.join(&format!("{label}-1.wav"));
        for failure in [
            ["--certificate-work", "1"],
            ["--designer-work", "1"],
            ["--designer-total-work", "1"],
        ] {
            let result = Command::new(env!("CARGO_BIN_EXE_sex"))
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg(&input)
                .arg(&protected)
                .args(options)
                .args(failure)
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert!(!String::from_utf8_lossy(&result.stderr).contains("qualified FIR sha256:"));
            assert_eq!(fs::read(&protected).unwrap(), outputs[0]);
        }
    }
}

#[test]
fn optimized_cli_options_and_preflight_never_silently_fall_back() {
    let case = CaseDirectory::new("optimized-options");
    let input = case.join("input.wav");
    let output = case.join("protected.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[1]);
    fs::write(&output, b"preserved").unwrap();
    for command in ["plan", "analyze", "convert"] {
        for options in [
            vec!["--designer", "bad"],
            vec!["--designer", "remez", "--window", "hann"],
            vec!["--designer", "global-ls", "--designer", "remez"],
            vec!["--designer-work", "1"],
            vec!["--designer", "remez", "--designer-storage", "0"],
            vec!["--window", "global-ls"],
        ] {
            let mut process = Command::new(env!("CARGO_BIN_EXE_sex"));
            process.env("SEX_COEFFICIENT_CACHE", &cache);
            if command != "convert" {
                process.arg(command);
            }
            process.arg(&input);
            if command == "convert" {
                process.arg(&output);
            }
            let result = process
                .args(["-r", "29400"])
                .args(options)
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert_eq!(fs::read(&output).unwrap(), b"preserved");
            assert!(!cache.exists());
        }
    }
    for designer in ["global-ls", "remez"] {
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg("plan")
            .arg(&input)
            .args([
                "-r",
                "48000",
                "--designer",
                designer,
                "--preset",
                "until-40k",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report = String::from_utf8_lossy(&result.stdout);
        assert!(report.contains("initial designer preflight: blocked:"));
        assert!(report.contains("unqualified initial candidate"));
        assert!(!report.contains("qualified FIR sha256:"));
        assert!(!cache.exists());
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg(&input)
            .arg(&output)
            .args(["--designer", designer])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("requires a rate change"));
        assert_eq!(fs::read(&output).unwrap(), b"preserved");
    }
}

#[test]
fn window_cli_uses_the_same_qualified_bank_in_analysis_and_streaming() {
    let case = CaseDirectory::new("window-cli");
    let input = case.join("input.wav");
    write_pcm(
        &input,
        1,
        &[
            0, 131072, 262144, 524288, 1048576, 524288, 262144, 131072, 0,
        ],
    );
    for (window, options) in [
        ("hann", vec!["-r", "66150", "--preset", "fast"]),
        (
            "blackman",
            vec![
                "-r",
                "14700",
                "--preset",
                "fast",
                "--gain",
                "1152921504606846976",
            ],
        ),
        ("dolph-chebyshev", vec!["-r", "22050", "--preset", "high"]),
    ] {
        let cache = case.join(window);
        let mut options = options;
        options.extend([
            "--window",
            window,
            "--grid",
            "9",
            "--certify",
            "--certificate-taps",
            "4097",
            "--harmonics",
            "--dither",
            "none",
            "--clip",
            "normalize",
        ]);
        let analysis = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg("analyze")
            .arg(&input)
            .args(&options)
            .output()
            .unwrap();
        assert!(
            analysis.status.success(),
            "{window}: {}",
            String::from_utf8_lossy(&analysis.stderr)
        );
        let report = String::from_utf8_lossy(&analysis.stdout);
        let qualification = fir_qualification(&report);
        assert!(qualification.contains("continuous certificate: Certified"));
        assert!(
            qualification.contains(&format!("qualified designer: windowed-sinc-{window}-big-v"))
        );
        assert!(
            qualification
                .contains("main-complex-error=true, images=true, image-L2=true, stopband=true")
        );
        let mut outputs = Vec::new();
        for block in ["1", "7", "4096"] {
            let path = case.join(&format!("{window}-{block}.wav"));
            let conversion = Command::new(env!("CARGO_BIN_EXE_sex"))
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg(&input)
                .arg(&path)
                .args(&options)
                .args(["--block-frames", block])
                .output()
                .unwrap();
            let trace = String::from_utf8_lossy(&conversion.stderr);
            assert!(conversion.status.success(), "{window}/{block}: {trace}");
            assert_eq!(fir_qualification(&trace), qualification);
            assert!(trace.contains("coefficient cache: hit"));
            outputs.push(fs::read(path).unwrap());
        }
        let protected = case.join(&format!("{window}-1.wav"));
        let failure = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg(&input)
            .arg(&protected)
            .args(&options)
            .args(["--certificate-work", "1"])
            .output()
            .unwrap();
        let error = String::from_utf8_lossy(&failure.stderr);
        assert!(!failure.status.success());
        assert!(error.contains("Inconclusive(Work)"), "{error}");
        assert!(!error.contains("qualified FIR sha256:"));
        assert_eq!(fs::read(protected).unwrap(), outputs[0]);
        assert_eq!(outputs[0], outputs[1]);
        assert_eq!(outputs[0], outputs[2]);
        assert!(
            hound::WavReader::new(outputs[0].as_slice())
                .unwrap()
                .into_samples::<i32>()
                .any(|s| s.unwrap() != 0)
        );
    }
}

#[test]
fn window_failures_preserve_output_and_do_not_substitute_kaiser() {
    let case = CaseDirectory::new("window-failures");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_pcm(&input, 1, &[0, 131072, 262144]);
    fs::write(&output, b"previous output").unwrap();
    for (name, options, expected, preflight) in [
        (
            "rectangular",
            vec![
                "-r",
                "66150",
                "--window",
                "rectangular",
                "--preset",
                "fast",
                "--refinement-attempts",
                "1",
            ],
            "candidate limit exhausted",
            false,
        ),
        (
            "dolph-until",
            vec![
                "-r",
                "66150",
                "--window",
                "dolph-chebyshev",
                "--preset",
                "until-40k",
            ],
            "Dolph-Chebyshev series",
            true,
        ),
        (
            "same-rate",
            vec!["--window", "hann"],
            "requires a rate change",
            true,
        ),
    ] {
        let cache = case.join(name);
        for _ in 0..2 {
            let result = Command::new(env!("CARGO_BIN_EXE_sex"))
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg(&input)
                .arg(&output)
                .args(&options)
                .output()
                .unwrap();
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success(), "{name}");
            assert!(error.contains(expected), "{name}: {error}");
            assert!(!error.contains("qualified FIR sha256:"));
            assert_eq!(fs::read(&output).unwrap(), b"previous output");
            if preflight {
                assert!(!cache.exists());
            }
        }
    }
}

#[test]
fn window_options_are_shared_strict_and_plan_does_not_materialize() {
    let case = CaseDirectory::new("window-options");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[1]);
    for command in ["plan", "analyze", "convert"] {
        for (options, expected) in [
            (
                vec!["--window", "hann", "--window", "blackman"],
                "only once",
            ),
            (vec!["--window", "unknown"], "unknown FIR window"),
            (vec!["--window"], "requires a value"),
        ] {
            let mut process = Command::new(env!("CARGO_BIN_EXE_sex"));
            if command != "convert" {
                process.arg(command);
            }
            process.arg(&input);
            if command == "convert" {
                process.arg(case.join("unused.wav"));
            }
            let result = process
                .args(["-r", "66150"])
                .args(options)
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
        }
    }
    let cache = case.join("plan-cache");
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg("plan")
        .arg(&input)
        .args([
            "-r",
            "66150",
            "--window",
            "dolph-chebyshev",
            "--preset",
            "until-40k",
        ])
        .output()
        .unwrap();
    assert!(result.status.success());
    let report = String::from_utf8_lossy(&result.stdout);
    assert!(report.contains("window: dolph-chebyshev"));
    assert!(report.contains("initial designer preflight: blocked:"));
    assert!(!report.contains("qualified FIR sha256:"));
    assert!(!cache.exists());
}

fn fir_qualification(text: &str) -> &str {
    let start = text
        .find("qualified FIR sha256: ")
        .expect("qualified bank report");
    let end = text[start..]
        .find("FIR qualification scope: ")
        .expect("report scope")
        + start;
    let end = text[end..].find('\n').unwrap() + end + 1;
    &text[start..end]
}

#[test]
fn shared_gates_certify_the_same_bank_in_analysis_and_pcm() {
    let case = CaseDirectory::new("shared-gates-audio");
    let input = case.join("input.wav");
    write_pcm(
        &input,
        1,
        &[
            0, 131072, 262144, 524288, 1048576, 524288, 262144, 131072, 0,
        ],
    );
    for (name, options) in [
        (
            "native",
            vec![
                "-r",
                "66150",
                "--preset",
                "sane",
                "--grid",
                "9",
                "--certify",
                "--harmonics",
            ],
        ),
        (
            "bigint",
            vec![
                "-r",
                "22050",
                "--preset",
                "high",
                "--grid",
                "17",
                "--certify",
                "--certificate-taps",
                "2049",
                "--harmonics",
            ],
        ),
        (
            "amplified",
            vec![
                "-r",
                "14700",
                "--preset",
                "fast",
                "--gain",
                "1152921504606846976",
                "--clip",
                "normalize",
                "--grid",
                "9",
                "--certify",
                "--harmonics",
            ],
        ),
    ] {
        let cache = case.join(name);
        let analysis = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg("analyze")
            .arg(&input)
            .args(&options)
            .args(["--dither", "none"])
            .output()
            .unwrap();
        assert!(
            analysis.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&analysis.stderr)
        );
        let report = String::from_utf8_lossy(&analysis.stdout);
        let qualification = fir_qualification(&report);
        assert!(qualification.contains("continuous certificate: Certified"));
        assert!(
            qualification
                .contains("main-complex-error=true, images=true, image-L2=true, stopband=true")
        );
        if name == "amplified" {
            let bits = qualification
                .split("actual C: ")
                .nth(1)
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .parse::<u32>()
                .unwrap();
            assert!(bits > 62);
        }
        let mut outputs = Vec::new();
        for block in ["1", "7", "4096"] {
            let output = case.join(&format!("{name}-{block}.wav"));
            let conversion = Command::new(env!("CARGO_BIN_EXE_sex"))
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg(&input)
                .arg(&output)
                .args(&options)
                .args(["--dither", "none", "--block-frames", block])
                .output()
                .unwrap();
            let trace = String::from_utf8_lossy(&conversion.stderr);
            assert!(conversion.status.success(), "{name}/{block}: {trace}");
            assert_eq!(fir_qualification(&trace), qualification);
            assert!(trace.contains("coefficient cache: hit"));
            outputs.push(fs::read(output).unwrap());
        }
        assert_eq!(outputs[0], outputs[1]);
        assert_eq!(outputs[0], outputs[2]);
        assert!(
            hound::WavReader::new(outputs[0].as_slice())
                .unwrap()
                .into_samples::<i32>()
                .any(|sample| sample.unwrap() != 0)
        );
    }
}

#[test]
fn shared_gate_failures_preserve_existing_output_even_with_a_warm_cache() {
    let case = CaseDirectory::new("shared-gates-failures");
    let input = case.join("input.wav");
    let output = case.join("existing.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0, 1, -1]);
    fs::write(&output, b"existing audio").unwrap();
    let warm = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg("analyze")
        .arg(&input)
        .args(["-r", "14700", "--preset", "fast"])
        .output()
        .unwrap();
    assert!(warm.status.success());
    let entries = fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    let cached = fs::read(&entries[0]).unwrap();
    for (options, failure) in [
        (
            vec!["--certify", "--certificate-work", "1"],
            "Inconclusive(Work)",
        ),
        (vec!["--harmonics", "--harmonic-work", "1"], "ResourceLimit"),
        (
            vec!["--certify", "--certificate-taps", "3"],
            "Inconclusive(Taps)",
        ),
        (
            vec!["--grid", "129", "--refinement-terms", "1"],
            "response-term limit",
        ),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg(&input)
            .arg(&output)
            .args(["-r", "14700", "--preset", "fast"])
            .args(options)
            .output()
            .unwrap();
        let trace = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(trace.contains(failure), "{trace}");
        assert!(!trace.contains("qualified FIR sha256:"));
        assert_eq!(fs::read(&output).unwrap(), b"existing audio");
        assert_eq!(fs::read(&entries[0]).unwrap(), cached);
        assert_eq!(fs::read_dir(&case.0).unwrap().count(), 3);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
    }
}

#[test]
fn shared_gate_plan_accounts_for_selected_grid_without_designing_a_bank() {
    let case = CaseDirectory::new("shared-gates-plan");
    let input = case.join("input.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0]);
    for blocked in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sex"));
        command
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg("plan")
            .arg(&input)
            .args([
                "-r",
                "88200",
                "--preset",
                "fast",
                "--grid",
                "9",
                "--certify",
                "--harmonics",
            ]);
        if blocked {
            command.args(["--certificate-taps", "3"]);
        }
        let result = command.output().unwrap();
        let text = String::from_utf8_lossy(&result.stdout);
        assert!(result.status.success(), "{text}");
        assert!(text.contains("258 coefficients, 4644 response terms"));
        assert!(text.contains("unqualified initial candidate"));
        assert!(!text.contains("continuous certificate: Certified"));
        if blocked {
            assert!(text.contains("initial supplemental preflight: blocked:"));
        } else {
            assert!(text.contains("initial harmonic work: 41310"));
        }
        assert!(!cache.exists());
    }
}

#[test]
fn shared_supplemental_gates_are_not_silently_ignored_on_same_rate_conversion() {
    let case = CaseDirectory::new("shared-gates-no-fir");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[1]);
    fs::write(&output, b"existing audio").unwrap();
    for gate in ["--certify", "--harmonics"] {
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg(&input)
            .arg(&output)
            .arg(gate)
            .output()
            .unwrap();
        let trace = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(trace.contains("require a rate change"));
        assert!(!trace.contains("qualified FIR sha256:"));
        assert!(!cache.exists());
        assert_eq!(fs::read(&output).unwrap(), b"existing audio");
        assert_eq!(fs::read_dir(&case.0).unwrap().count(), 2);
    }
}

#[test]
fn shared_proof_success_does_not_publish_audio_after_late_pcm_clipping_failure() {
    let case = CaseDirectory::new("shared-gates-late-failure");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_pcm(&input, 1, &[8388607; 9]);
    fs::write(&output, b"existing audio").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg(&input)
        .arg(&output)
        .args([
            "-r",
            "88200",
            "--preset",
            "fast",
            "--gain",
            "2",
            "--clip",
            "error",
            "--dither",
            "none",
            "--grid",
            "9",
            "--certify",
            "--harmonics",
        ])
        .output()
        .unwrap();
    let trace = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "{trace}");
    assert!(
        trace.contains("quality feedback accepted 1 candidate(s)"),
        "{trace}"
    );
    assert!(!trace.contains("qualified FIR sha256:"));
    assert_eq!(fs::read(&output).unwrap(), b"existing audio");
    assert_eq!(fs::read_dir(&case.0).unwrap().count(), 2);
}

#[test]
fn default_and_explicit_error_floor_qualify_the_same_pcm() {
    let case = CaseDirectory::new("quality-feedback-pcm");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 123456, -654321, 1, -1, 0]);
    let reference = case.join("reference.wav");
    let baseline = run_captured(
        &input,
        &reference,
        &["-r", "14700", "--preset", "sane", "--dither", "none"],
    );
    assert!(
        String::from_utf8_lossy(&baseline.stderr)
            .contains("quality feedback accepted 1 candidate(s)")
    );
    for block in ["1", "7", "4096"] {
        let output = case.join(&format!("output-{block}.wav"));
        let result = run_captured(
            &input,
            &output,
            &[
                "-r",
                "14700",
                "--preset",
                "sane",
                "--dither",
                "none",
                "--error-floor",
                "-110dB",
                "--block-frames",
                block,
            ],
        );
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("quality feedback accepted 1 candidate(s)")
        );
        assert_eq!(fs::read(&reference).unwrap(), fs::read(output).unwrap());
    }
}

#[test]
fn default_refinement_repairs_absurd_and_pointless_without_weakening_targets() {
    let case = CaseDirectory::new("quality-feedback-wide-presets");
    let input = case.join("input.wav");
    write_pcm(
        &input,
        1,
        &[
            0, 131072, 262144, 524288, 1048576, 524288, 262144, 131072, 0,
        ],
    );
    for (preset, target, old_taps, new_taps, bits, identity) in [
        (
            "absurd",
            "-240dB",
            4097,
            4309,
            160,
            "0a817eb4728f4ccfe322e3ba34269b787ffc5ad8613d49db09e9d398b95540a2",
        ),
        (
            "pointless",
            "-360dB",
            8193,
            8473,
            256,
            "8a992539c36f920b544520a1614624440fb15cfa6c4e7efba572287b369a79ba",
        ),
    ] {
        let cache = case.join(preset);
        let analyze = |attempts: Option<&str>| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_sex"));
            command
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg("analyze")
                .arg(&input)
                .args(["-r", "22050", "--preset", preset, "--grid", "65"]);
            if let Some(attempts) = attempts {
                command.args(["--refinement-attempts", attempts]);
            }
            command.output().unwrap()
        };
        let initial = analyze(Some("1"));
        assert!(!initial.status.success());
        let rejected = String::from_utf8_lossy(&initial.stderr);
        assert!(rejected.contains(&format!(
            "quality candidate 1: {old_taps} taps/phase, C={bits}"
        )));
        assert!(rejected.contains("coefficient-budget=true, response=false"));
        assert!(rejected.contains("no quality pass is claimed"));

        let refined = analyze(None);
        assert!(
            refined.status.success(),
            "{preset}: {}",
            String::from_utf8_lossy(&refined.stderr)
        );
        let trace = String::from_utf8_lossy(&refined.stderr);
        let report = String::from_utf8_lossy(&refined.stdout);
        assert!(trace.contains(&format!(
            "quality candidate 2: {new_taps} taps/phase, C={bits}"
        )));
        assert!(trace.contains("quality feedback accepted 2 candidate(s)"));
        assert!(report.contains(&format!("preset {preset}, target {target}")));
        assert!(report.contains(identity));
        assert!(report.contains(
            "error-floor compliance: passband=true, stopband=true, coefficient-quantization=true"
        ));
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
        let mut outputs = Vec::new();
        for block in ["1", "7", "4096"] {
            let output = case.join(&format!("{preset}-{block}.wav"));
            let conversion = Command::new(env!("CARGO_BIN_EXE_sex"))
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg(&input)
                .arg(&output)
                .args([
                    "-r",
                    "22050",
                    "--preset",
                    preset,
                    "--block-frames",
                    block,
                    "--dither",
                    "none",
                    "--clip",
                    "error",
                ])
                .output()
                .unwrap();
            let trace = String::from_utf8_lossy(&conversion.stderr);
            assert!(conversion.status.success(), "{trace}");
            assert!(trace.contains("quality feedback accepted 2 candidate(s)"));
            assert!(trace.contains(&format!("{new_taps} taps/phase")));
            assert!(trace.contains(identity));
            assert!(trace.contains("coefficient cache: hit"));
            outputs.push(fs::read(output).unwrap());
        }
        assert_eq!(outputs[0], outputs[1]);
        assert_eq!(outputs[0], outputs[2]);
        let reader = hound::WavReader::new(outputs[0].as_slice()).unwrap();
        assert_eq!(reader.duration(), 4);
        assert!(
            reader
                .into_samples::<i32>()
                .any(|sample| sample.unwrap() != 0)
        );
    }
}

#[test]
fn explicit_until_work_limit_precedes_cache_and_preserves_existing_output() {
    let case = CaseDirectory::new("default-until-preflight");
    let input = case.join("input.wav");
    let output = case.join("existing.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0]);
    fs::write(&output, b"existing audio").unwrap();
    for analyze in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sex"));
        command.env("SEX_COEFFICIENT_CACHE", &cache);
        if analyze {
            command.arg("analyze");
        }
        command.arg(&input);
        if !analyze {
            command.arg(&output);
        }
        let result = command
            .args([
                "-r",
                "48000",
                "--preset",
                "until-40k",
                "--refinement-coefficients",
                "4000000",
            ])
            .output()
            .unwrap();
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(error.contains("after 0 assessed candidate(s)"), "{error}");
        assert!(error.contains("cumulative coefficient limit: requires 10698720, limit 4000000"));
        assert!(error.contains("no quality pass is claimed"));
        assert!(!cache.exists());
        assert_eq!(fs::read(&output).unwrap(), b"existing audio");
        assert_eq!(fs::read_dir(&case.0).unwrap().count(), 2);
    }
}

#[test]
fn default_until_plan_reports_geometry_budgets_without_materializing() {
    let case = CaseDirectory::new("until-geometry-budget-plan");
    let input = case.join("input.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0]);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg("plan")
        .arg(&input)
        .args([
            "-r",
            "48000",
            "--preset",
            "until-40k",
            "--precision",
            "auto",
            "--error-floor",
            "-400dB",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let plan = String::from_utf8_lossy(&result.stdout);
    assert!(
        plan.contains("10698720 coefficients, 1390833600 response terms"),
        "{plan}"
    );
    assert!(
        plan.contains("cumulative limits: 42794880 coefficients, 5563334400 terms"),
        "{plan}"
    );
    assert!(
        !plan.contains("initial candidate needs --refinement-"),
        "{plan}"
    );
    assert!(plan.contains("unqualified initial candidate"), "{plan}");
    assert!(!cache.exists());
    assert_eq!(fs::read_dir(&case.0).unwrap().count(), 1);
}

#[test]
fn same_rate_conversion_has_no_fir_to_qualify_even_under_tiny_limits() {
    let case = CaseDirectory::new("default-same-rate-bypass");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let cache = case.join("cache");
    let samples = [0, 1, -1, 8388607, -8388608];
    write_pcm(&input, 1, &samples);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg(&input)
        .arg(&output)
        .args([
            "--preset",
            "until-40k",
            "--refinement-coefficients",
            "1",
            "--dither",
            "none",
            "--clip",
            "error",
        ])
        .output()
        .unwrap();
    let trace = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{trace}");
    assert!(!trace.contains("quality candidate"));
    assert!(!cache.exists());
    let actual = hound::WavReader::open(&output)
        .unwrap()
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(actual, samples);
    let plan = Command::new(env!("CARGO_BIN_EXE_sex"))
        .arg("plan")
        .arg(&input)
        .args(["-r", "44100"])
        .output()
        .unwrap();
    assert!(plan.status.success());
    assert!(String::from_utf8_lossy(&plan.stdout).contains("same-rate conversion bypasses FIR"));
}

#[test]
fn refinement_budget_failure_preserves_existing_output_and_never_creates_a_cache() {
    let case = CaseDirectory::new("quality-feedback-budget");
    let input = case.join("input.wav");
    let output = case.join("existing.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0]);
    fs::write(&output, b"existing user bytes").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg(&input)
        .arg(&output)
        .args(["-r", "48000", "--refine-quality", "--refinement-terms", "1"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no quality pass is claimed"));
    assert!(!cache.exists());
    assert_eq!(fs::read(&output).unwrap(), b"existing user bytes");
    assert_eq!(fs::read_dir(&case.0).unwrap().count(), 2);
}

#[test]
fn explicitly_limited_plan_reports_required_work_without_materializing_the_candidate() {
    let case = CaseDirectory::new("quality-feedback-plan");
    let input = case.join("input.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0]);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg("plan")
        .arg(&input)
        .args([
            "-r",
            "48000",
            "--preset",
            "until-40k",
            "--refinement-coefficients",
            "4000000",
            "--refinement-terms",
            "200000000",
        ])
        .output()
        .unwrap();
    assert!(result.status.success());
    let report = String::from_utf8_lossy(&result.stdout);
    assert!(report.contains("unqualified initial candidate"));
    assert!(report.contains("10698720 coefficients, 1390833600 response terms"));
    assert!(report.contains("--refinement-coefficients at least 10698720"));
    assert!(report.contains("--refinement-terms at least 1390833600"));
    assert!(!cache.exists());
}

#[test]
fn analyze_harmonics_reports_all_images_and_their_exact_frequency_locations() {
    let case = CaseDirectory::new("harmonic-full-bank");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0]);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg("analyze")
        .arg(&input)
        .args([
            "-r",
            "66150",
            "--preset",
            "sane",
            "--harmonics",
            "--grid",
            "9",
            "--certify",
        ])
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&result.stdout);
    assert!(
        result.status.success(),
        "{report}\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(report.contains("17 input frequencies across [0, 1/2] × 3 output harmonics"));
    assert!(report.contains("image peak:") && report.contains("output frequency"));
    assert!(report.contains("image L2 peak:"));
    assert!(report.contains("main-complex-error=true, images=true, image-L2=true, stopband=true"));
    assert!(report.contains("all images, not a continuous proof"));
    assert!(report.contains("continuous certificate: Certified"));
}

#[test]
fn harmonic_resource_failure_precedes_design_and_does_not_create_a_cache() {
    let case = CaseDirectory::new("harmonic-preflight");
    let input = case.join("input.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0]);
    for extra in [
        vec!["--preset", "sane", "--harmonic-work", "1"],
        vec!["--preset", "until-40k"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg("analyze")
            .arg(&input)
            .args(["-r", "48000", "--harmonics"])
            .args(extra)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("ResourceLimit"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("harmonic sampled compliance"));
        assert!(!cache.exists());
    }
}

#[test]
fn analyze_continuous_certificate_proves_sane_and_reports_bounds() {
    let case = CaseDirectory::new("continuous-sane-certificate");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0]);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg("analyze")
        .arg(&input)
        .args([
            "-r",
            "14700",
            "--preset",
            "sane",
            "--certify",
            "--error-floor",
            "-110dB",
        ])
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&result.stdout);
    assert!(
        result.status.success(),
        "{report}\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(report.contains("continuous certificate: Certified; target -110dB"));
    assert!(report.contains("complete phases: 1/1"));
    assert!(report.contains("continuous passband deviation upper bound:"));
    assert!(report.contains("continuous stopband upper bound:"));
    assert!(report.contains("not design error, effects, PCM, or full expanded anti-imaging band"));
}

#[test]
fn analyze_continuous_certificate_proves_corrected_high_bigint_bank() {
    let case = CaseDirectory::new("continuous-high-certificate");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0]);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg("analyze")
        .arg(&input)
        .args([
            "-r",
            "22050",
            "--preset",
            "high",
            "--certify",
            "--certificate-taps",
            "2049",
            "--error-floor",
            "-160dB",
        ])
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&result.stdout);
    assert!(
        result.status.success(),
        "{report}\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(report.contains("continuous certificate: Certified; target -160dB"));
    assert!(report.contains("1 phases × 1109 taps"));
    assert!(report.contains("execution backend: gmp-bigint"));
    assert!(report.contains("continuous passband deviation upper bound:"));
    assert!(report.contains("continuous stopband upper bound:"));
}

#[test]
fn analyze_exhausted_certificate_budget_is_not_success() {
    let case = CaseDirectory::new("continuous-work-limit");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0]);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg("analyze")
        .arg(&input)
        .args([
            "-r",
            "14700",
            "--preset",
            "fast",
            "--certify",
            "--certificate-work",
            "1",
        ])
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        report.contains("continuous certificate: Inconclusive(Work)"),
        "{report}"
    );
    assert!(report.contains("certificate work:"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("error-floor compliance:"));
    assert!(!report.contains("continuous passband deviation upper bound:"));
    assert!(!report.contains("continuous stopband upper bound:"));
    assert!(String::from_utf8_lossy(&result.stderr).contains("no quality pass is claimed"));
}

#[test]
fn analyze_until_certificate_tap_limit_precedes_design_and_cache_writes() {
    let case = CaseDirectory::new("continuous-until-preflight");
    let input = case.join("input.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0]);
    let before = fs::read(&input).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg("analyze")
        .arg(&input)
        .args(["-r", "22050", "--preset", "until-40k", "--certify"])
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        report.contains("Inconclusive(Taps); no coefficients designed or loaded"),
        "{report}"
    );
    assert!(!cache.exists());
    assert_eq!(before, fs::read(&input).unwrap());
    assert!(!report.contains("continuous stopband upper bound:"));
}

fn run_captured(input: &Path, output: &Path, options: &[&str]) -> std::process::Output {
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg(input)
        .arg(output)
        .args(options)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

#[test]
fn cli_signal_precision_preserves_the_bit_that_decides_pcm_rounding() {
    let case = CaseDirectory::new("signal-precision-pcm-tie");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[1]);
    // 2^-23 * (128 + 2^-56) is a 16-bit PCM halfway value plus 2^-79.
    let gain = "9223372036854775809/72057594037927936";
    for (preset, precision, expected) in
        [("fast", "63", 0), ("fast", "128", 1), ("high", "auto", 1)]
    {
        let output = case.join(&format!("{preset}-{precision}.wav"));
        let report = run_captured(
            &input,
            &output,
            &[
                "--gain",
                gain,
                "--preset",
                preset,
                "--signal-precision",
                precision,
                "--bits",
                "16",
                "--dither",
                "none",
                "--clip",
                "error",
            ],
        );
        let samples = hound::WavReader::open(output)
            .unwrap()
            .into_samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            samples,
            [expected],
            "preset={preset}, precision={precision}"
        );
        if preset == "high" {
            assert!(String::from_utf8_lossy(&report.stderr).contains("signal: Q65.96"));
        }
    }
}

#[test]
fn analyze_accumulator_and_backend_match_actual_signal_execution() {
    let case = CaseDirectory::new("analyze-execution-accumulator");
    let input = case.join("input.wav");
    write_pcm(&input, 1, &[0, 131072, -262144, 524288, 0]);
    let cases = [
        ("fast", "63", "66150", vec![], "checked-i128/GMP-fallback"),
        ("fast", "4096", "66150", vec![], "gmp-bigint"),
        ("high", "96", "66150", vec![], "gmp-bigint"),
        (
            "fast",
            "96",
            "66150",
            vec!["--window", "hann"],
            "gmp-bigint",
        ),
        (
            "fast",
            "96",
            "29400",
            vec!["--designer", "global-ls"],
            "gmp-bigint",
        ),
    ];
    let mut native_widths = Vec::new();
    for (index, (preset, precision, rate, extra, backend)) in cases.into_iter().enumerate() {
        let cache = case.join(&format!("cache-{index}"));
        let output = case.join(&format!("output-{index}.wav"));
        let mut options = vec![
            "-r",
            rate,
            "--preset",
            preset,
            "--signal-precision",
            precision,
            "--grid",
            "9",
            "--dither",
            "none",
            "--clip",
            "error",
        ];
        options.extend(extra);
        let analyze = || {
            let result = Command::new(env!("CARGO_BIN_EXE_sex"))
                .env("SEX_COEFFICIENT_CACHE", &cache)
                .arg("analyze")
                .arg(&input)
                .args(&options)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap()
        };
        let report = analyze();
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg(&input)
            .arg(&output)
            .args(&options)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let conversion = String::from_utf8(result.stderr).unwrap();
        let accumulator_line = report
            .lines()
            .find(|line| line.starts_with("accumulator: ") && line.contains("bits required,"))
            .unwrap();
        let mut parts = accumulator_line
            .trim_start_matches("accumulator: ")
            .split(" bits required, ");
        let required: u32 = parts.next().unwrap().parse().unwrap();
        let headroom: u32 = parts
            .next()
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let execution_line = conversion
            .lines()
            .find(|line| line.starts_with("filter: "))
            .unwrap();
        let actual: u32 = execution_line
            .split("-bit minimum accumulator")
            .next()
            .unwrap()
            .rsplit(", ")
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let plan_line = conversion
            .lines()
            .find(|line| line.starts_with("plan: preset "))
            .unwrap();
        let planned: u32 = plan_line
            .rsplit(", ")
            .next()
            .unwrap()
            .trim_end_matches("-bit accumulator")
            .parse()
            .unwrap();
        assert_eq!(required, actual, "{report}\n{conversion}");
        assert_eq!(required + headroom, planned);
        assert!(report.contains(&format!("execution backend: {backend}")));
        assert!(execution_line.contains(&format!("{backend} execution")));
        assert!(report.contains(&format!("accumulator scope: full declared Q65.{precision} input range; {planned} bits planned; not measured audio peak")));
        assert!(conversion.contains("coefficient cache: hit"));
        let cached = analyze();
        assert!(cached.contains("coefficient cache: hit"));
        assert!(cached.lines().any(|line| line == accumulator_line));
        let l1_line = report
            .lines()
            .find(|line| line.starts_with("numerical FIR gain:"))
            .unwrap();
        assert!(conversion.lines().any(|line| line == l1_line));
        assert!(cached.lines().any(|line| line == l1_line));
        if index < 2 {
            native_widths.push((required, planned));
        }
    }
    // Same native coefficient bank; widening only the signal's fractional
    // part must increase both full-range MAC widths by exactly the same amount.
    assert_eq!(native_widths[1].0 - native_widths[0].0, 4096 - 63);
    assert_eq!(native_widths[1].1 - native_widths[0].1, 4096 - 63);
}

#[test]
fn cli_dynamic_effect_coefficients_do_not_reuse_q2_62_zeros() {
    let case = CaseDirectory::new("dynamic-effect-precision");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let samples = [6_291_456, -4_194_304, 1, -1];
    write_pcm(&input, 1, &samples);
    let report = run_captured(
        &input,
        &output,
        &[
            "--gain",
            "18446744073709551615",
            "--mix",
            "1/18446744073709551615",
            "--dither",
            "none",
            "--clip",
            "error",
        ],
    );
    let actual = hound::WavReader::open(output)
        .unwrap()
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(actual, samples);
    assert!(String::from_utf8_lossy(&report.stderr).contains("signal: Q66.86"));
}

#[test]
fn cli_4096_bit_signal_normalization_and_shaped_dither_are_block_invariant() {
    let case = CaseDirectory::new("4096-bit-cli-chain");
    let input = case.join("input.wav");
    let samples = (0..31)
        .flat_map(|n| {
            if n % 5 < 2 {
                [6_291_456, 2_097_152]
            } else {
                [-4_194_304, 0]
            }
        })
        .collect::<Vec<_>>();
    write_pcm(&input, 2, &samples);
    for mode in ["tpdf", "high-pass-tpdf", "noise-shaped-9"] {
        let mut outputs = Vec::new();
        for block in ["1", "7", "4096"] {
            let output = case.join(&format!("{mode}-{block}.wav"));
            let report = run_captured(
                &input,
                &output,
                &[
                    "--gain",
                    "4",
                    "--dc-remove",
                    "1/2",
                    "--mix",
                    "1,1",
                    "--convolve",
                    "1,1/2",
                    "-r",
                    "48000",
                    "--preset",
                    "fast",
                    "--signal-precision",
                    "4096",
                    "--bits",
                    "16",
                    "--clip",
                    "normalize",
                    "--dither",
                    mode,
                    "--seed",
                    "42",
                    "--block-frames",
                    block,
                ],
            );
            let report = String::from_utf8_lossy(&report.stderr);
            assert!(report.contains("signal: Q65.4096"));
            assert!(report.contains("Q2.4096 raw"));
            assert!(report.contains("gmp-bigint execution"));
            assert!(report.contains("saturated samples: 0"));
            outputs.push(fs::read(output).unwrap());
        }
        assert_eq!(outputs[0], outputs[1], "mode={mode}");
        assert_eq!(outputs[0], outputs[2], "mode={mode}");
        let reader = hound::WavReader::new(outputs[0].as_slice()).unwrap();
        assert_eq!(reader.duration(), 35); // round_even(32 * 160/147)
        let peak = reader
            .into_samples::<i16>()
            .map(|x| i32::from(x.unwrap()).abs())
            .max()
            .unwrap();
        assert!(peak > 30_000);
    }
}

#[test]
fn until_40k_plan_includes_big_signal_and_insufficient_precision_is_rejected() {
    let case = CaseDirectory::new("until-signal-plan");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_pcm(&input, 1, &[0]);
    let report = Command::new(env!("CARGO_BIN_EXE_sex"))
        .args(["plan"])
        .arg(&input)
        .args(["-r", "48000", "--preset", "until-40k"])
        .output()
        .unwrap();
    assert!(report.status.success());
    let text = String::from_utf8_lossy(&report.stdout);
    assert!(text.contains("signal: Q65.512"));
    assert!(text.contains("execution accumulator plan: 1537 bits"));
    fs::write(&output, b"preserve me").unwrap();
    let failure = Command::new(env!("CARGO_BIN_EXE_sex"))
        .arg(&input)
        .arg(&output)
        .args(["--preset", "high", "--signal-precision", "63"])
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("below the calculated minimum 96"));
    assert_eq!(fs::read(output).unwrap(), b"preserve me");
}

#[test]
#[ignore = "133733-tap MPFR qualification; run explicitly with --release --ignored"]
fn until_40k_real_filter_meets_floor_and_streams_reproducibly() {
    let case = CaseDirectory::new("until-real-qualification");
    let input = case.join("input.wav");
    let cache = case.join("cache");
    write_pcm(
        &input,
        1,
        &[
            0, 131072, 262144, 524288, 1048576, 524288, 262144, 131072, 0,
        ],
    );
    let analysis = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg("analyze")
        .arg(&input)
        .args([
            "-r",
            "22050",
            "--preset",
            "until-40k",
            "--error-floor",
            "-600dB",
            "--grid",
            "65",
        ])
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&analysis.stdout);
    println!("{report}");
    assert!(
        analysis.status.success(),
        "{}",
        String::from_utf8_lossy(&analysis.stderr)
    );
    assert!(report.contains("passband=true, stopband=true, coefficient-quantization=true"));
    assert!(report.contains("signal: Q65.512"));
    assert!(
        String::from_utf8_lossy(&analysis.stderr)
            .contains("quality feedback accepted 1 candidate(s)")
    );
    let mut outputs = Vec::new();
    for block in ["1", "7", "4096"] {
        let output = case.join(&format!("output-{block}.wav"));
        let conversion = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg(&input)
            .arg(&output)
            .args([
                "-r",
                "22050",
                "--preset",
                "until-40k",
                "--clip",
                "error",
                "--dither",
                "none",
                "--block-frames",
                block,
            ])
            .output()
            .unwrap();
        let report = String::from_utf8_lossy(&conversion.stderr);
        assert!(conversion.status.success(), "{report}");
        assert!(report.contains("133733 taps/phase"));
        assert!(report.contains("signal: Q65.512"));
        assert!(report.contains("gmp-bigint execution"));
        assert!(report.contains("saturated samples: 0"));
        assert!(report.contains("cache: hit"));
        assert!(report.contains("quality feedback accepted 1 candidate(s)"));
        outputs.push(fs::read(output).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
    let reader = hound::WavReader::new(outputs[0].as_slice()).unwrap();
    assert_eq!(reader.duration(), 4); // round_even(9 / 2)
    assert!(
        reader
            .into_samples::<i32>()
            .any(|value| value.unwrap() != 0)
    );
}

#[test]
fn near_unity_dc_radius_automatically_gets_enough_precision() {
    let case = CaseDirectory::new("near-unity-dc-plan");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_pcm(&input, 1, &[100, 100, 100, 0]);
    let report = run_captured(
        &input,
        &output,
        &[
            "--dc-remove",
            "18446744073709551614/18446744073709551615",
            "--dither",
            "none",
        ],
    );
    assert!(String::from_utf8_lossy(&report.stderr).contains("signal: Q65.86"));
    let samples = hound::WavReader::open(output)
        .unwrap()
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(samples, [100, 100, 100, 0]);
}

#[test]
fn amplified_cli_refines_fir_precision_and_uses_the_refined_cache_key() {
    let case = CaseDirectory::new("coupled-numerical-plan");
    let input = case.join("input.wav");
    let cache = case.join("cache");
    write_pcm(&input, 1, &[0, 1, 3, -2, 0, -1, 4, 2, 0]);
    let options = [
        "-r",
        "48000",
        "--preset",
        "fast",
        "--gain",
        "1152921504606846976",
        "--clip",
        "normalize",
        "--dither",
        "none",
    ];
    let planned = Command::new(env!("CARGO_BIN_EXE_sex"))
        .arg("plan")
        .arg(&input)
        .args(options)
        .output()
        .unwrap();
    let plan = String::from_utf8_lossy(&planned.stdout);
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    assert!(plan.contains("85 coefficient fractional bits"), "{plan}");
    assert!(plan.contains("numerical error bound:"));
    assert!(plan.contains("1 precision refinement(s)"));
    let mut outputs = Vec::new();
    for block in ["1", "7", "4096"] {
        let output = case.join(&format!("out-{block}.wav"));
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .env("SEX_COEFFICIENT_CACHE", &cache)
            .arg(&input)
            .arg(&output)
            .args(options)
            .args(["--block-frames", block])
            .output()
            .unwrap();
        let report = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{report}");
        assert!(
            report.contains("85 coefficient fractional bits"),
            "{report}"
        );
        assert!(report.contains("gmp-bigint execution"));
        assert!(report.contains("saturated samples: 0"));
        if block == "1" {
            assert!(report.contains("cache: miss"));
        } else {
            assert!(report.contains("cache: hit"));
        }
        outputs.push(fs::read(output).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
    assert_eq!(
        hound::WavReader::new(outputs[0].as_slice())
            .unwrap()
            .duration(),
        10
    );
    // The unamplified filter must NOT load the wider, coupled-plan bank.
    let baseline = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", &cache)
        .arg(&input)
        .arg(case.join("baseline.wav"))
        .args(["-r", "48000", "--preset", "fast", "--dither", "none"])
        .output()
        .unwrap();
    assert!(baseline.status.success());
    assert!(String::from_utf8_lossy(&baseline.stderr).contains("cache: miss"));
    assert_eq!(fs::read_dir(cache).unwrap().count(), 2);
}

#[test]
fn numerical_plan_failures_preserve_output_and_validate_source_layout() {
    let case = CaseDirectory::new("numerical-plan-rejection");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_pcm(&input, 1, &[1]);
    fs::write(&output, b"existing output").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg(&input)
        .arg(&output)
        .args([
            "-r",
            "48000",
            "--preset",
            "fast",
            "--gain",
            "1152921504606846976",
            "--clip",
            "normalize",
            "--precision",
            "128",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("below the calculated minimum 149"));
    assert_eq!(fs::read(&output).unwrap(), b"existing output");
    assert_eq!(fs::read_dir(&case.0).unwrap().count(), 2);
    for mode in ["plan", "analyze"] {
        let result = Command::new(env!("CARGO_BIN_EXE_sex"))
            .arg(mode)
            .arg(&input)
            .args(["-r", "48000", "--mix", "1,1"])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("matrix has 2 input column(s)"));
    }
}

fn write_pcm(path: &Path, channels: u16, samples: &[i32]) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels,
            sample_rate: 44_100,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for sample in samples {
        writer.write_sample(*sample).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn headroom_is_preserved_until_later_stages_restore_pcm_range() {
    let case = CaseDirectory::new("headroom-cancellation");
    let input = case.join("input.wav");
    let mono = case.join("mono.wav");
    let samples = [6_291_456, -4_194_304, 2_097_152, 0, -6_291_456];
    let stereo = samples
        .iter()
        .flat_map(|sample| [*sample, *sample])
        .collect::<Vec<_>>();
    write_pcm(&input, 2, &stereo);
    write_pcm(&mono, 1, &samples);
    for rate in ["44100", "48000"] {
        for preset in ["fast", "high"] {
            let reference = case.join(&format!("reference-{rate}-{preset}.wav"));
            let options = [
                "-r".to_owned(),
                rate.to_owned(),
                "--preset".to_owned(),
                preset.to_owned(),
                "--dither".to_owned(),
                "none".to_owned(),
                "--clip".to_owned(),
                "error".to_owned(),
            ];
            run_sex_with(&mono, &reference, &options);
            for block in ["1", "7", "4096"] {
                let output = case.join(&format!("output-{rate}-{preset}-{block}.wav"));
                let mut effects = options.to_vec();
                effects.extend(
                    [
                        "--gain",
                        "4",
                        "--mix",
                        "1,1",
                        "--convolve",
                        "1/8",
                        "--block-frames",
                        block,
                    ]
                    .map(str::to_owned),
                );
                run_sex_with(&input, &output, &effects);
                assert_eq!(
                    fs::read(&reference).unwrap(),
                    fs::read(output).unwrap(),
                    "rate={rate} preset={preset} block={block}"
                );
            }
        }
    }
}

#[test]
fn normalization_handles_real_positive_overshoot_at_the_pcm_endpoint() {
    let case = CaseDirectory::new("wide-normalization-endpoint");
    let input = case.join("input.wav");
    write_pcm(
        &input,
        2,
        &[6_291_456, 6_291_456, -4_194_304, -4_194_304, 0, 0],
    );
    let mut outputs = Vec::new();
    for block in ["1", "7"] {
        let output = case.join(&format!("output-{block}.wav"));
        run_sex_with(
            &input,
            &output,
            &[
                "--mix",
                "1,1",
                "--convolve",
                "1,1/2",
                "--clip",
                "normalize",
                "--dither",
                "none",
                "--block-frames",
                block,
            ]
            .map(str::to_owned),
        );
        outputs.push(fs::read(&output).unwrap());
        let samples = hound::WavReader::open(output)
            .unwrap()
            .into_samples::<i32>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(samples, [8_388_607, -1_398_101, -2_796_202, 0]);
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn dc_overshoot_is_clipped_only_at_final_pcm_and_counted() {
    let case = CaseDirectory::new("wide-dc-saturation");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_pcm(&input, 1, &[-8_388_608, 8_388_607]);
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg(&input)
        .arg(&output)
        .args(["--dc-remove", "0", "--dither", "none", "--clip", "saturate"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("saturated samples: 1"));
    let samples = hound::WavReader::open(output)
        .unwrap()
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(samples, [-8_388_608, 8_388_607]);
}

#[test]
fn amplified_stateful_pipeline_normalizes_after_rate_and_is_block_invariant() {
    let case = CaseDirectory::new("amplified-block-invariance");
    let input = case.join("input.wav");
    let samples = (0..101)
        .flat_map(|frame| {
            let left = if frame % 11 < 5 {
                6_291_456
            } else {
                -4_194_304
            };
            [left, left / 2]
        })
        .collect::<Vec<_>>();
    write_pcm(&input, 2, &samples);
    for preset in ["fast", "high"] {
        let mut outputs = Vec::new();
        for block in ["1", "7", "4096"] {
            let output = case.join(&format!("{preset}-{block}.wav"));
            run_sex_with(
                &input,
                &output,
                &[
                    "--gain",
                    "4",
                    "--dc-remove",
                    "1/2",
                    "--mix",
                    "1,1",
                    "--convolve",
                    "1,1/2",
                    "rate",
                    "48000",
                    "--preset",
                    preset,
                    "--bits",
                    "16",
                    "--dither",
                    "noise-shaped-5",
                    "--seed",
                    "42",
                    "--clip",
                    "normalize",
                    "--block-frames",
                    block,
                ]
                .map(str::to_owned),
            );
            outputs.push(fs::read(output).unwrap());
        }
        assert_eq!(outputs[0], outputs[1], "preset={preset}");
        assert_eq!(outputs[0], outputs[2], "preset={preset}");
        let reader = hound::WavReader::new(outputs[0].as_slice()).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.duration(), 111); // round_even((101 + 1) * 160/147)
        let peak = reader
            .into_samples::<i16>()
            .map(|sample| i32::from(sample.unwrap()).abs())
            .max()
            .unwrap();
        assert!(
            peak > 30_000,
            "normalization must exercise a non-silent peak"
        );
    }
}

struct CaseDirectory(PathBuf);

impl CaseDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_CASE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sex-{label}-{}-{sequence}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for CaseDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_stereo_fixture(path: &Path, frames: usize) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for frame in 0..frames {
        let index = i32::try_from(frame).unwrap();
        let left = ((index * 65_537 + 17) & 0x00ff_ffff) - 0x0080_0000;
        let right = -left - 1;
        writer.write_sample(left).unwrap();
        writer.write_sample(right).unwrap();
    }
    writer.finalize().unwrap();
}

fn run_sex(input: &Path, output: &Path, rate: u32) {
    run_sex_with(input, output, &["-r".to_owned(), rate.to_string()]);
}

fn run_sex_with(input: &Path, output: &Path, options: &[String]) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sex"));
    command.env("SEX_COEFFICIENT_CACHE", "off");
    command.arg(input);
    command.args(options);
    command.arg(output);
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "sex failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn rate_changing_cli_is_byte_reproducible() {
    let case = CaseDirectory::new("rate-reproducibility");
    let input = case.join("input.wav");
    let first = case.join("first.wav");
    let second = case.join("second.wav");
    write_stereo_fixture(&input, 1_001);

    run_sex(&input, &first, 48_000);
    run_sex(&input, &second, 48_000);

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let reader = hound::WavReader::open(first).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.spec().bits_per_sample, 24);
    assert_eq!(reader.duration(), 1_090);
}

#[test]
fn same_rate_cli_preserves_every_integer_sample() {
    let case = CaseDirectory::new("same-rate");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_stereo_fixture(&input, 257);

    run_sex(&input, &output, 44_100);

    let input_samples = hound::WavReader::open(input)
        .unwrap()
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let output_reader = hound::WavReader::open(output).unwrap();
    assert_eq!(output_reader.duration(), 257);
    let output_samples = output_reader
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(input_samples, output_samples);
}

#[test]
fn target_depth_dither_is_seeded_and_reproducible() {
    let case = CaseDirectory::new("dither-reproducibility");
    let input = case.join("input.wav");
    let first = case.join("first.wav");
    let second = case.join("second.wav");
    let different_seed = case.join("different-seed.wav");
    write_stereo_fixture(&input, 1_003);

    let options = [
        "--bits".to_owned(),
        "16".to_owned(),
        "--dither".to_owned(),
        "tpdf".to_owned(),
        "--seed".to_owned(),
        "42".to_owned(),
    ];
    run_sex_with(&input, &first, &options);
    run_sex_with(&input, &second, &options);
    let different_options = [
        "--bits".to_owned(),
        "16".to_owned(),
        "--dither".to_owned(),
        "tpdf".to_owned(),
        "--seed".to_owned(),
        "43".to_owned(),
    ];
    run_sex_with(&input, &different_seed, &different_options);

    let first_bytes = fs::read(&first).unwrap();
    assert_eq!(first_bytes, fs::read(&second).unwrap());
    assert_ne!(first_bytes, fs::read(&different_seed).unwrap());
    let reader = hound::WavReader::open(first).unwrap();
    assert_eq!(reader.spec().bits_per_sample, 16);
    assert_eq!(reader.duration(), 1_003);
}

#[test]
fn high_preset_executes_bigint_coefficients_and_order_five_shaping() {
    let case = CaseDirectory::new("high-bigint");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_stereo_fixture(&input, 33);

    run_sex_with(
        &input,
        &output,
        &[
            "-r".to_owned(),
            "48000".to_owned(),
            "--preset".to_owned(),
            "high".to_owned(),
        ],
    );

    let reader = hound::WavReader::open(output).unwrap();
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.duration(), 36);
}

#[test]
fn normalize_is_two_pass_and_byte_reproducible_after_rate_change() {
    let case = CaseDirectory::new("normalize");
    let input = case.join("input.wav");
    let first = case.join("first.wav");
    let second = case.join("second.wav");
    write_stereo_fixture(&input, 101);
    let options = [
        "-r".to_owned(),
        "48000".to_owned(),
        "--preset".to_owned(),
        "fast".to_owned(),
        "--dither".to_owned(),
        "none".to_owned(),
        "--clip".to_owned(),
        "normalize".to_owned(),
    ];

    run_sex_with(&input, &first, &options);
    run_sex_with(&input, &second, &options);

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let reader = hound::WavReader::open(first).unwrap();
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.duration(), 110);
}

#[test]
fn allow_headroom_is_a_real_wide_path_not_an_alias_for_saturation() {
    let case = CaseDirectory::new("allow-headroom");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_stereo_fixture(&input, 17);

    run_sex_with(
        &input,
        &output,
        &[
            "--dither".to_owned(),
            "none".to_owned(),
            "--clip".to_owned(),
            "allow-headroom".to_owned(),
        ],
    );

    assert_eq!(fs::read(input).unwrap(), fs::read(output).unwrap());
}

#[test]
fn flac_cli_preserves_integer_pcm_metadata_and_reproducible_bytes() {
    let case = CaseDirectory::new("flac-metadata");
    let input = case.join("input.flac");
    let first = case.join("first.flac");
    let second = case.join("second.flac");
    let library = SndFileLibrary::load().unwrap();
    let spec = AudioSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 24,
    };
    let metadata = AudioMetadata::new(vec![
        MetadataEntry {
            key: MetadataKey::Title,
            value: "Bit-exact FLAC fixture".to_owned(),
        },
        MetadataEntry {
            key: MetadataKey::Artist,
            value: "SeX integration test".to_owned(),
        },
    ]);
    let pcm = [-8_388_608, 8_388_607, -1, 0, 1, 1_234_567, -7_654_321, 42];
    let mut writer = library.create(&input, spec, &metadata).unwrap();
    writer.write_signed_pcm_frames(&pcm).unwrap();
    writer.finalize().unwrap();

    let first_run = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg(&input)
        .arg(&first)
        .output()
        .unwrap();
    assert!(
        first_run.status.success(),
        "sex failed: {}",
        String::from_utf8_lossy(&first_run.stderr)
    );
    let stderr = String::from_utf8(first_run.stderr).unwrap();
    let backend = format!("sexio-sndfile ({})", library.version().unwrap());
    assert!(stderr.contains(&format!("I/O backends: input {backend}; output {backend}")));
    run_sex_with(&input, &second, &[]);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

    let mut reader = library.open(&first).unwrap();
    assert_eq!(reader.spec(), spec);
    assert_eq!(
        reader.metadata().get(MetadataKey::Title),
        Some("Bit-exact FLAC fixture")
    );
    assert_eq!(
        reader.metadata().get(MetadataKey::Artist),
        Some("SeX integration test")
    );
    let output = reader.read_frames(32).unwrap();
    let expected = pcm
        .iter()
        .map(|sample| sexq::Q1_63::from_signed_pcm(*sample, 24).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(output.samples, expected);
}

#[test]
fn wave_info_metadata_is_probed_without_abandoning_integer_pcm() {
    let case = CaseDirectory::new("wave-info-metadata");
    let input = case.join("input.wav");
    let first = case.join("first.wav");
    let second = case.join("second.wav");
    let library = SndFileLibrary::load().unwrap();
    let spec = AudioSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 24,
    };
    let metadata = AudioMetadata::new(vec![
        MetadataEntry {
            key: MetadataKey::Title,
            value: "WAVE INFO fixture".to_owned(),
        },
        MetadataEntry {
            key: MetadataKey::Artist,
            value: "SeX integer path".to_owned(),
        },
    ]);
    let pcm = [-8_388_608, 8_388_607, -1, 0, 1, 1_234_567, -7_654_321, 42];
    let mut writer = library.create(&input, spec, &metadata).unwrap();
    writer.write_signed_pcm_frames(&pcm).unwrap();
    writer.finalize().unwrap();

    run_sex_with(&input, &first, &[]);
    run_sex_with(&input, &second, &[]);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

    let mut reader = library.open(&first).unwrap();
    assert_eq!(reader.spec(), spec);
    assert_eq!(
        reader.metadata().get(MetadataKey::Title),
        Some("WAVE INFO fixture")
    );
    assert_eq!(
        reader.metadata().get(MetadataKey::Artist),
        Some("SeX integer path")
    );
    let output = reader.read_frames(32).unwrap();
    let expected = pcm
        .iter()
        .map(|sample| sexq::Q1_63::from_signed_pcm(*sample, 24).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(output.samples, expected);
}

#[test]
fn rate_conversion_can_stream_to_aiff() {
    let case = CaseDirectory::new("aiff-rate");
    let input = case.join("input.wav");
    let output = case.join("output.aiff");
    write_stereo_fixture(&input, 101);
    run_sex_with(
        &input,
        &output,
        &[
            "-r".to_owned(),
            "48000".to_owned(),
            "--preset".to_owned(),
            "fast".to_owned(),
        ],
    );

    let library = SndFileLibrary::load().unwrap();
    let mut reader = library.open(&output).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.spec().bits_per_sample, 24);
    let mut frames = 0;
    loop {
        let chunk = reader.read_frames(17).unwrap();
        frames += chunk.frames;
        if chunk.frames == 0 {
            break;
        }
    }
    assert_eq!(frames, 110);
}

#[test]
fn unavailable_default_cache_falls_back_to_uncached_design() {
    let case = CaseDirectory::new("readonly-default-cache");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_stereo_fixture(&input, 17);

    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env_remove("SEX_COEFFICIENT_CACHE")
        .env("XDG_CACHE_HOME", "/sys")
        .arg(&input)
        .args(["-r", "48000", "--preset", "fast"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "sex failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("default coefficient cache is unavailable")
    );
    assert!(output.exists());
}

#[test]
fn one_sample_rate_changes_obey_exact_nearest_duration() {
    let case = CaseDirectory::new("one-sample-rates");

    for (input_rate, output_rate, expected_frames) in
        [(44_100_u32, 48_000_u32, 1_u32), (48_000, 16_000, 0)]
    {
        let input = case.join(&format!("input-{input_rate}.wav"));
        let output = case.join(&format!("output-{output_rate}.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: input_rate,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&input, spec).unwrap();
        writer.write_sample(1_234_567_i32).unwrap();
        writer.finalize().unwrap();

        run_sex_with(
            &input,
            &output,
            &[
                "-r".to_owned(),
                output_rate.to_string(),
                "--preset".to_owned(),
                "fast".to_owned(),
            ],
        );
        let reader = hound::WavReader::open(output).unwrap();
        assert_eq!(reader.spec().sample_rate, output_rate);
        assert_eq!(reader.duration(), expected_frames);
    }
}

#[test]
fn exact_linear_gain_runs_through_the_streaming_pipeline() {
    let case = CaseDirectory::new("exact-gain");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let samples = [-8_000_000_i32, 8_000_000, -2, 2, 0, 4_000_000];
    let mut writer = hound::WavWriter::create(&input, spec).unwrap();
    for sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();

    run_sex_with(
        &input,
        &output,
        &[
            "--gain".to_owned(),
            "1/2".to_owned(),
            "--dither".to_owned(),
            "none".to_owned(),
            "--clip".to_owned(),
            "error".to_owned(),
        ],
    );

    let output_samples = hound::WavReader::open(output)
        .unwrap()
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(output_samples, samples.map(|sample| sample / 2).to_vec());
}

#[test]
fn exact_dc_removal_is_exposed_by_the_streaming_cli() {
    let case = CaseDirectory::new("dc-removal");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&input, spec).unwrap();
    for _ in 0..8 {
        writer.write_sample(4_194_304_i32).unwrap();
    }
    writer.finalize().unwrap();

    run_sex_with(
        &input,
        &output,
        &[
            "--dc-remove".to_owned(),
            "1/2".to_owned(),
            "--dither".to_owned(),
            "none".to_owned(),
            "--clip".to_owned(),
            "error".to_owned(),
        ],
    );

    let output_samples = hound::WavReader::open(output)
        .unwrap()
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        output_samples,
        vec![
            4_194_304, 2_097_152, 1_048_576, 524_288, 262_144, 131_072, 65_536, 32_768
        ]
    );
}

#[test]
fn exact_channel_mix_changes_layout_before_rate_conversion_reproducibly() {
    let case = CaseDirectory::new("channel-mix-rate");
    let input = case.join("input.wav");
    let first = case.join("first.wav");
    let second = case.join("second.wav");
    write_stereo_fixture(&input, 101);
    let options = [
        "--mix".to_owned(),
        "1/2,1/2".to_owned(),
        "rate".to_owned(),
        "48000".to_owned(),
        "--preset".to_owned(),
        "fast".to_owned(),
    ];

    run_sex_with(&input, &first, &options);
    run_sex_with(&input, &second, &options);

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let reader = hound::WavReader::open(first).unwrap();
    assert_eq!(reader.spec().channels, 1);
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.duration(), 110);
}

#[test]
fn exact_channel_mix_rounds_each_output_row_once() {
    let case = CaseDirectory::new("channel-mix-exact");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&input, spec).unwrap();
    for sample in [8_000_000_i32, -4_000_000, 2, -2, -3_000_000, 1_000_000] {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();

    run_sex_with(
        &input,
        &output,
        &[
            "--mix".to_owned(),
            "1/2,1/2".to_owned(),
            "--dither".to_owned(),
            "none".to_owned(),
            "--clip".to_owned(),
            "error".to_owned(),
        ],
    );

    let reader = hound::WavReader::open(output).unwrap();
    assert_eq!(reader.spec().channels, 1);
    let samples = reader
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(samples, [2_000_000, 0, -1_000_000]);
}

#[test]
fn exact_convolution_emits_the_causal_tail() {
    let case = CaseDirectory::new("convolution-tail");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&input, spec).unwrap();
    writer.write_sample(2_097_152_i32).unwrap();
    writer.finalize().unwrap();

    run_sex_with(
        &input,
        &output,
        &[
            "--convolve".to_owned(),
            "1,1/2,-1/4".to_owned(),
            "--dither".to_owned(),
            "none".to_owned(),
            "--clip".to_owned(),
            "error".to_owned(),
        ],
    );

    let reader = hound::WavReader::open(output).unwrap();
    assert_eq!(reader.duration(), 3);
    let samples = reader
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(samples, [2_097_152, 1_048_576, -524_288]);
}

#[test]
fn convolution_tail_participates_in_rate_and_two_pass_normalization() {
    let case = CaseDirectory::new("convolution-rate-normalize");
    let input = case.join("input.wav");
    let first = case.join("first.wav");
    let second = case.join("second.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&input, spec).unwrap();
    writer.write_sample(2_097_152_i32).unwrap();
    writer.finalize().unwrap();
    let options = [
        "--convolve".to_owned(),
        "1,1/2,-1/4".to_owned(),
        "rate".to_owned(),
        "48000".to_owned(),
        "--preset".to_owned(),
        "fast".to_owned(),
        "--dither".to_owned(),
        "none".to_owned(),
        "--clip".to_owned(),
        "normalize".to_owned(),
    ];

    run_sex_with(&input, &first, &options);
    run_sex_with(&input, &second, &options);

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let reader = hound::WavReader::open(first).unwrap();
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.duration(), 3);
}

#[test]
fn complete_stateful_pipeline_is_invariant_to_streaming_block_size() {
    let case = CaseDirectory::new("block-invariance");
    let input = case.join("input.wav");
    write_stereo_fixture(&input, 101);
    let mut outputs = Vec::new();

    for block_frames in ["1", "7", "4096"] {
        let output = case.join(&format!("block-{block_frames}.wav"));
        run_sex_with(
            &input,
            &output,
            &[
                "--gain".to_owned(),
                "1/4".to_owned(),
                "--dc-remove".to_owned(),
                "1/2".to_owned(),
                "--mix".to_owned(),
                "1/2,1/2".to_owned(),
                "--convolve".to_owned(),
                "1,1/2,-1/4".to_owned(),
                "rate".to_owned(),
                "48000".to_owned(),
                "--preset".to_owned(),
                "fast".to_owned(),
                "--bits".to_owned(),
                "16".to_owned(),
                "--dither".to_owned(),
                "noise-shaped-5".to_owned(),
                "--seed".to_owned(),
                "42".to_owned(),
                "--clip".to_owned(),
                "normalize".to_owned(),
                "--block-frames".to_owned(),
                block_frames.to_owned(),
            ],
        );
        outputs.push(fs::read(output).unwrap());
    }

    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
    let reader = hound::WavReader::new(outputs[0].as_slice()).unwrap();
    assert_eq!(reader.spec().channels, 1);
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.spec().bits_per_sample, 16);
    assert_eq!(reader.duration(), 112);
}

#[test]
fn failed_processing_preserves_existing_output_and_removes_temporary_file() {
    let case = CaseDirectory::new("transaction-failure");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&input, spec).unwrap();
    writer.write_sample(8_388_607_i32).unwrap();
    writer.write_sample(-8_388_608_i32).unwrap();
    writer.finalize().unwrap();
    let sentinel = b"existing output must survive";
    fs::write(&output, sentinel).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg(&input)
        .args(["--dc-remove", "0", "--dither", "none", "--clip", "error"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read(&output).unwrap(), sentinel);
    let temporary_files = fs::read_dir(&case.0)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sex-output-")
        })
        .count();
    assert_eq!(temporary_files, 0);
}

#[test]
fn truncated_pcm_after_successful_chunks_does_not_publish_partial_output() {
    let case = CaseDirectory::new("truncated-pcm-transaction");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    let mut writer = hound::WavWriter::create(
        &input,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for code in [-123_i32, 456, 789] {
        writer.write_sample(code).unwrap();
    }
    writer.finalize().unwrap();
    let mut bytes = fs::read(&input).unwrap();
    bytes.pop(); // Keep the valid declared length; truncate the final PCM word.
    let mut probe = sexio::WaveReader::new(std::io::Cursor::new(bytes.clone())).unwrap();
    assert_eq!(probe.read_frames(1).unwrap().frames, 1);
    assert_eq!(probe.read_frames(1).unwrap().frames, 1);
    assert!(probe.read_frames(1).is_err());
    fs::write(&input, bytes).unwrap();
    let sentinel = b"do not replace with a partially decoded track";
    fs::write(&output, sentinel).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sex"))
        .env("SEX_COEFFICIENT_CACHE", "off")
        .arg(&input)
        .arg(&output)
        .args(["--block-frames", "1", "--dither", "none", "--clip", "error"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("wrote 3 frames"));
    assert_eq!(fs::read(&output).unwrap(), sentinel);
    for entry in fs::read_dir(&case.0).unwrap() {
        assert!(
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".sex-output-")
        );
    }
}

#[test]
fn successful_processing_publishes_over_existing_output() {
    let case = CaseDirectory::new("transaction-success");
    let input = case.join("input.wav");
    let output = case.join("output.wav");
    write_stereo_fixture(&input, 17);
    fs::write(&output, b"old output").unwrap();

    run_sex_with(&input, &output, &[]);

    let reader = hound::WavReader::open(&output).unwrap();
    assert_eq!(reader.duration(), 17);
    assert_eq!(reader.spec().channels, 2);
}

#[test]
fn exact_gain_precedes_rate_and_two_pass_normalization_reproducibly() {
    let case = CaseDirectory::new("gain-rate-normalize");
    let input = case.join("input.wav");
    let first = case.join("first.wav");
    let second = case.join("second.wav");
    write_stereo_fixture(&input, 101);
    let options = [
        "rate".to_owned(),
        "48000".to_owned(),
        "--preset".to_owned(),
        "fast".to_owned(),
        "--gain".to_owned(),
        "1/2".to_owned(),
        "--dither".to_owned(),
        "none".to_owned(),
        "--clip".to_owned(),
        "normalize".to_owned(),
    ];
    run_sex_with(&input, &first, &options);
    run_sex_with(&input, &second, &options);

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let reader = hound::WavReader::open(first).unwrap();
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.duration(), 110);
}
