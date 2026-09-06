//! Complete input-residue coverage in bounded batches, separate from tones.
use super::*;

fn batch(from: u32, to: u32, start: u32, count: u16) -> Result<(u32, Vec<Signal>), Box<dyn Error>> {
    let (_, denominator) = reduced_ratio(from, to);
    if count == 0
        || count > 32
        || start >= denominator
        || u64::from(start) + u64::from(count) > u64::from(denominator)
    {
        return Err(
            "phase batch must select 1..32 distinct offsets within the reduced input period".into(),
        );
    }
    // Extend the original one-second fixture by a complete input period. Even
    // a coprime ratio with M near the input rate keeps all impulses centered
    // with at least half a second of context on either side.
    let frames = from
        .checked_add(denominator)
        .and_then(|n| n.checked_add(7))
        .ok_or("phase fixture duration overflow")?;
    let signals = (start..start + u32::from(count))
        .map(|offset| Signal {
            name: format!("impulse-offset-{offset}"),
            kind: Kind::Impulse {
                input_frame: from / 2 + offset,
            },
        })
        .collect();
    Ok((frames, signals))
}

pub(super) fn generate(
    path: &Path,
    from: u32,
    to: u32,
    start: u32,
    count: u16,
) -> Result<(), Box<dyn Error>> {
    let (frames, signals) = batch(from, to, start, count)?;
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = hound::WavWriter::new(
        BufWriter::new(file),
        hound::WavSpec {
            channels: count,
            sample_rate: from,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for frame in 0..frames {
        for signal in &signals {
            writer.write_sample(fixture_code(signal, frame as usize, from))?;
        }
    }
    writer.finalize()?;
    let mut manifest = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.with_extension("channels.tsv"))?,
    );
    writeln!(
        manifest,
        "channel\tname\tinput_frame\tfractional_phase_numerator\tdenominator"
    )?;
    let (l, m) = reduced_ratio(from, to);
    for (index, signal) in signals.iter().enumerate() {
        let Kind::Impulse { input_frame } = signal.kind else {
            unreachable!()
        };
        writeln!(
            manifest,
            "{index}\t{}\t{input_frame}\t{}\t{m}",
            signal.name,
            u64::from(input_frame) * u64::from(l) % u64::from(m)
        )?;
    }
    manifest.flush()?;
    println!("{frames}\t{count}");
    Ok(())
}

pub(super) fn analyze(
    path: &Path,
    from: u32,
    to: u32,
    start: u32,
    count: u16,
    directory: &Path,
    strict: bool,
) -> Result<(), Box<dyn Error>> {
    let (input_frames, signals) = batch(from, to, start, count)?;
    let Audio { rate, channels } = read_wave(path)?;
    if rate != to || channels.len() != signals.len() {
        return Err("phase batch output rate/channel mismatch".into());
    }
    let expected = rounded_frames(u64::from(input_frames), from, to);
    let frames = channels[0].len();
    let difference = frames as i64 - expected as i64;
    if difference.abs() > i64::from(!strict) {
        return Err("phase batch output duration mismatch".into());
    }
    fs::create_dir(directory)?;
    for (signal, values) in signals.iter().zip(&channels) {
        let Kind::Impulse { input_frame } = signal.kind else {
            unreachable!()
        };
        impulse_response(values, to, from, input_frame, &signal.name, directory)?;
    }
    let mut summary = output_file(directory, "summary.tsv")?;
    writeln!(
        summary,
        "first_offset\tcount\tperiod\tframes\texpected_frames"
    )?;
    writeln!(
        summary,
        "{start}\t{count}\t{}\t{frames}\t{expected}",
        reduced_ratio(from, to).1
    )?;
    summary.flush()?;
    println!(
        "{}: input offsets {start}..{}, {frames} frames; all impulse gates passed",
        path.display(),
        start + u32::from(count)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_phase_batches_cover_every_residue_once_with_context() {
        for (from, to) in [
            (48000, 16000),
            (44100, 48000),
            (48000, 44100),
            (44100, 47900),
            (48000, 1000),
            (1001, 1000),
            (1000, 192000),
            (44100, 44100),
        ] {
            let (l, m) = reduced_ratio(from, to);
            let mut residues = std::collections::BTreeSet::new();
            for start in (0..m).step_by(32) {
                let count = (m - start).min(32) as u16;
                let (frames, signals) = batch(from, to, start, count).unwrap();
                assert!(signals.len() <= 32);
                for signal in signals {
                    let Kind::Impulse { input_frame } = signal.kind else {
                        unreachable!()
                    };
                    assert!(input_frame >= from / 2 && frames - input_frame >= from / 2);
                    assert!(residues.insert(u64::from(input_frame) * u64::from(l) % u64::from(m)));
                    for frame in input_frame - 1..=input_frame + 1 {
                        assert_eq!(
                            fixture_code(&signal, frame as usize, from),
                            if frame == input_frame { 1 << 30 } else { 0 }
                        );
                    }
                }
            }
            assert_eq!(residues, (0..u64::from(m)).collect());
            for (start, count) in [(0, 0), (0, 33), (m, 1), (m - 1, 2)] {
                assert!(batch(from, to, start, count).is_err());
            }
        }
    }
}
