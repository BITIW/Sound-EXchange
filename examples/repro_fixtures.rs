//! Small, integer-only corpus shared by architecture qualification runs.
use sexio::{AudioMetadata, AudioSpec, MetadataEntry, MetadataKey};
use sexio_sndfile::SndFileLibrary;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::BufWriter;
use std::path::Path;

fn sample(index: usize, bits: u16, random: &mut u32) -> i32 {
    // Wrapping is confined to this fixture PRNG, never signal arithmetic.
    *random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    let half = 1_i64 << (bits - 1);
    let value = match index % 16 {
        0 | 7 => 0,
        1 => half - 1,
        2 => -half,
        3 => 1,
        4 => -1,
        _ => i64::from(*random >> (32 - bits)) - half,
    };
    i32::try_from(value).unwrap()
}

fn write(
    path: &Path,
    channels: u16,
    rate: u32,
    bits: u16,
    frames: usize,
) -> Result<(), Box<dyn Error>> {
    let file = OpenOptions::new().create_new(true).write(true).open(path)?;
    let mut writer = hound::WavWriter::new(
        BufWriter::new(file),
        hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    let mut random = 0x243f_6a88;
    for index in 0..frames * usize::from(channels) {
        writer.write_sample(sample(
            if frames == 1 { 1 } else { index },
            bits,
            &mut random,
        ))?;
    }
    writer.finalize()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = args.next().ok_or("usage: repro_fixtures NEW_DIRECTORY")?;
    if args.next().is_some() {
        return Err("expected one output directory".into());
    }
    let directory = Path::new(&directory);
    fs::create_dir(directory)?;
    write(&directory.join("stereo24.wav"), 2, 44100, 24, 257)?;
    write(&directory.join("mono24.wav"), 1, 44100, 24, 73)?;
    write(&directory.join("eight32.wav"), 8, 48000, 32, 97)?;
    write(&directory.join("one16.wav"), 1, 44100, 16, 1)?;
    write(&directory.join("empty24.wav"), 2, 44100, 24, 0)?;
    write(&directory.join("tiny24.wav"), 1, 48000, 24, 9)?;
    if std::env::var("SEX_REPRO_CODECS").as_deref() == Ok("1") {
        let library = SndFileLibrary::load()?;
        let metadata = AudioMetadata::new(vec![
            MetadataEntry {
                key: MetadataKey::Title,
                value: "SeX: бит-в-бит".to_owned(),
            },
            MetadataEntry {
                key: MetadataKey::Artist,
                value: "Cross-architecture corpus".to_owned(),
            },
        ]);
        for extension in ["wav", "aiff", "flac"] {
            let mut writer = library.create(
                &directory.join(format!("tagged.{extension}")),
                AudioSpec {
                    channels: 2,
                    sample_rate: 44100,
                    bits_per_sample: 24,
                },
                &metadata,
            )?;
            let mut random = 0x243f_6a88;
            let codes = (0..146)
                .map(|i| i64::from(sample(i, 24, &mut random)))
                .collect::<Vec<_>>();
            writer.write_signed_pcm_frames(&codes)?;
            writer.finalize()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_codes_are_bounded_replayable_and_include_endpoints() {
        for bits in [16, 24, 32] {
            let generate = || {
                let mut state = 0x243f_6a88;
                (0..1024)
                    .map(|i| sample(i, bits, &mut state))
                    .collect::<Vec<_>>()
            };
            let a = generate();
            assert_eq!(a, generate());
            let half = 1_i64 << (bits - 1);
            assert_eq!(i64::from(*a.iter().min().unwrap()), -half);
            assert_eq!(i64::from(*a.iter().max().unwrap()), half - 1);
        }
    }
}
