use super::*;

struct Case(PathBuf);

impl Case {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = env::temp_dir().join(format!(
                "sex-normalization-replay-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("{error}"),
            }
        }
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn fixture(path: &Path, channels: u16, rate: u32, bits: u16, samples: &[i32]) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn normalization_input_digest_is_decoded_pcm_and_chunk_independent() {
    let case = Case::new();
    let input = case.0.join("input.wav");
    let format = BigQFormat::new(65, 63).unwrap();
    for channels in [1, 2] {
        for samples in [vec![], vec![i32::MIN, i32::MAX, -1, 0, 1, 1_234_567]] {
            fixture(&input, channels, 44100, 32, &samples);
            let pipeline = BigPipeline::new(channels, format, vec![]).unwrap();
            // Independent PCM32 -> canonical signed Q1.63 byte oracle.
            let bytes: Vec<u8> = samples
                .iter()
                .flat_map(|&sample| (i64::from(sample) * (1_i64 << 32)).to_le_bytes())
                .collect();
            let expected: [u8; 32] = Sha256::digest(bytes).into();
            let mut previous = None;
            for block in [1, 2, 7, 4096] {
                let report = run_pass(
                    &mut AudioReader::open(&input).unwrap(),
                    &pipeline,
                    None,
                    RateRatio::from_fraction(1, 1).unwrap(),
                    block,
                    true,
                    |_| Ok(()),
                )
                .unwrap();
                assert_eq!(report.input_pcm_sha256, Some(expected));
                assert_eq!(
                    report.input_frames,
                    (samples.len() / usize::from(channels)) as u64
                );
                assert_eq!(report.output_frames, report.input_frames);
                if let Some(previous) = &previous {
                    report.verify_normalization_replay(previous).unwrap();
                }
                previous = Some(report);
            }
            let unhashed = run_pass(
                &mut AudioReader::open(&input).unwrap(),
                &pipeline,
                None,
                RateRatio::from_fraction(1, 1).unwrap(),
                1,
                false,
                |_| Ok(()),
            )
            .unwrap();
            assert_eq!(unhashed.input_pcm_sha256, None);
            assert!(unhashed.verify_normalization_replay(&unhashed).is_err());
        }
    }
}

#[test]
fn normalization_detects_input_length_even_when_rate_rounds_to_same_output_length() {
    let case = Case::new();
    let input = case.0.join("input.wav");
    let format = BigQFormat::new(65, 63).unwrap();
    let pipeline = BigPipeline::new(1, format, vec![]).unwrap();
    let bank = SignalBank::Native {
        bank: Arc::new(PolyphaseFirQ63::new(1, 1, vec![Q2_62::from_raw(1_i64 << 62)]).unwrap()),
        delay: 0,
    };
    let mut reports = Vec::new();
    for frames in [3, 4] {
        fixture(&input, 1, 44100, 16, &vec![0; frames]);
        reports.push(
            run_pass(
                &mut AudioReader::open(&input).unwrap(),
                &pipeline,
                Some(&bank),
                RateRatio::from_fraction(1, 3).unwrap(),
                1,
                true,
                |_| Ok(()),
            )
            .unwrap(),
        );
    }
    assert_eq!(reports[0].output_frames, reports[1].output_frames);
    assert_eq!(reports[0].output_frames, 1);
    assert_eq!(
        reports[0]
            .verify_normalization_replay(&reports[1])
            .unwrap_err()
            .to_string(),
        "input frame count changed between normalization passes"
    );
}

#[test]
fn normalization_changed_input_never_publishes_even_if_effect_erases_pcm_change() {
    let case = Case::new();
    let input = case.0.join("input.wav");
    let output = case.0.join("output.wav");
    for change in ["pcm", "length", "channels", "rate", "depth", "metadata"] {
        for bits in [8, 16, 24, 32] {
            fixture(&input, 1, 44100, bits, &[0, 1, -1, 2]);
            fs::write(&output, b"existing destination must survive").unwrap();
            let transaction = OutputTransaction::reserve(&output).unwrap();
            let temporary = transaction.temporary_path().to_owned();
            let quality = QualityOptions {
                clip_policy: ClipPolicy::Normalize,
                dither_mode: Some(DitherMode::None),
                // Both passes produce silence even when input PCM changes.
                gain: Some(LinearGain::parse("0").unwrap()),
                block_frames: Some(1),
                ..QualityOptions::default()
            };
            let result =
                process_audio_with_reopen(&input, &temporary, None, None, quality, |path| {
                    match change {
                        "pcm" => fixture(path, 1, 44100, bits, &[0, 1, -1, 3]),
                        "length" => fixture(path, 1, 44100, bits, &[0, 1, -1]),
                        "channels" => fixture(path, 2, 44100, bits, &[0, 1, -1, 2]),
                        "rate" => fixture(path, 1, 48000, bits, &[0, 1, -1, 2]),
                        "depth" => fixture(
                            path,
                            1,
                            44100,
                            if bits == 16 { 24 } else { 16 },
                            &[0, 1, -1, 2],
                        ),
                        "metadata" => {}
                        _ => unreachable!(),
                    }
                    let mut reader = AudioReader::open(path)?;
                    if change == "metadata" {
                        // Inject the adapter's typed metadata result, without
                        // making this failure test depend on native tag writing.
                        let AudioReader::Wave { metadata, .. } = &mut reader else {
                            unreachable!()
                        };
                        *metadata = AudioMetadata::new(vec![sexio::MetadataEntry {
                            key: sexio::MetadataKey::Title,
                            value: "changed title".into(),
                        }]);
                    }
                    Ok(reader)
                });
            let error = match result {
                Err(error) => error.to_string(),
                Ok(_) => panic!("accepted changed {change} at PCM{bits}"),
            };
            let field = match change {
                "pcm" => "PCM",
                "length" => "frame count",
                "metadata" => "metadata",
                _ => "audio format",
            };
            assert_eq!(
                error,
                format!("input {field} changed between normalization passes")
            );
            drop(transaction);
            assert!(!temporary.exists());
            assert_eq!(
                fs::read(&output).unwrap(),
                b"existing destination must survive"
            );
            assert_eq!(fs::read_dir(&case.0).unwrap().count(), 2);
        }
    }
}
