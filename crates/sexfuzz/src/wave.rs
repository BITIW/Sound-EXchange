//! Bounded, deterministic container properties; not coverage-guided fuzzing.
use super::SplitMix64;
use sexio::{AudioSpec, WaveReader, WaveWriter};
use sexq::{OverflowPolicy, Q1_63, RoundingMode};
use std::cell::Cell;
use std::error::Error;
use std::io::{self, Cursor, Read};

const MAX_READ_CALLS: usize = 65_536;
const MAX_DECODED_SAMPLES: usize = 4096;

struct ShortReader<'a> {
    bytes: &'a [u8],
    max_read: usize,
    calls: &'a Cell<usize>,
}

impl Read for ShortReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.calls.set(self.calls.get() + 1);
        if self.calls.get() > MAX_READ_CALLS {
            return Err(io::Error::other("property-driver read budget exceeded"));
        }
        let count = buffer.len().min(self.max_read).min(self.bytes.len());
        buffer[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Outcome {
    Rejected,
    Decoded(AudioSpec, Vec<Q1_63>),
}

fn decode(bytes: &[u8], max_read: usize, frames: usize) -> Result<Outcome, Box<dyn Error>> {
    let calls = Cell::new(0);
    let source = ShortReader {
        bytes,
        max_read,
        calls: &calls,
    };
    let outcome = (|| -> Result<Outcome, Box<dyn Error>> {
        let mut reader = match WaveReader::new(source) {
            Ok(reader) => reader,
            Err(_) => return Ok(Outcome::Rejected),
        };
        let spec = reader.spec();
        // Even a mutated 65535-channel header cannot request an unbounded
        // sample allocation. Ordinary generated streams have at most 4 channels.
        let frames = if spec.channels > 4 { 1 } else { frames };
        let mut samples = Vec::new();
        loop {
            let before = calls.get();
            let zero = reader.read_frames(0)?;
            if !zero.samples.is_empty() || zero.frames != 0 || calls.get() != before {
                return Err("zero-frame request consumed input or produced samples".into());
            }
            let chunk = match reader.read_frames(frames) {
                Ok(chunk) => chunk,
                Err(_) => return Ok(Outcome::Rejected),
            };
            if chunk.frames > frames
                || chunk.samples.len() != chunk.frames * usize::from(spec.channels)
            {
                return Err("reader violated requested size or frame alignment".into());
            }
            samples.extend(chunk.samples);
            if samples.len() > MAX_DECODED_SAMPLES {
                return Err("property-driver decoded-sample budget exceeded".into());
            }
            if reader.is_eof() {
                let before = calls.get();
                let after_eof = reader.read_frames(frames)?;
                if !after_eof.samples.is_empty() || after_eof.frames != 0 || calls.get() != before {
                    return Err("EOF was not stable".into());
                }
                return Ok(Outcome::Decoded(spec, samples));
            }
            if chunk.frames == 0 {
                return Err("empty read without EOF would stall a streaming consumer".into());
            }
        }
    })();
    // A budget-induced I/O error must fail the property, never count as a
    // safely rejected malformed input. Parser panics also propagate as failures.
    if calls.get() > MAX_READ_CALLS {
        return Err("parser exhausted property-driver read budget".into());
    }
    outcome
}

// Independent minimal PCM fixture encoder, intentionally test-only. Production
// WAVE parsing/writing remains delegated to hound rather than this fixture code.
fn fixture(spec: AudioSpec, signed: &[i64]) -> Vec<u8> {
    let bytes_per_sample = usize::from(spec.bits_per_sample / 8);
    let data_len = signed.len() * bytes_per_sample;
    let block_align = spec.channels * (spec.bits_per_sample / 8);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len as u32 + (data_len % 2) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&spec.channels.to_le_bytes());
    bytes.extend_from_slice(&spec.sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(spec.sample_rate * u32::from(block_align)).to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&spec.bits_per_sample.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &sample in signed {
        if spec.bits_per_sample == 8 {
            bytes.push((sample + 128) as u8);
        } else {
            bytes.extend_from_slice(&sample.to_le_bytes()[..bytes_per_sample]);
        }
    }
    if data_len % 2 == 1 {
        bytes.push(0);
    }
    bytes
}

fn compare_reads(bytes: &[u8], random: &mut SplitMix64) -> Result<Outcome, Box<dyn Error>> {
    let whole = decode(bytes, usize::MAX, 131)?;
    let short = decode(
        bytes,
        1 + random.range(17) as usize,
        1 + random.range(31) as usize,
    )?;
    if whole != short {
        return Err("WAVE acceptance, spec, or decoded PCM depends on chunk boundaries".into());
    }
    Ok(whole)
}

fn fingerprint(outcome: &Outcome) -> u64 {
    match outcome {
        Outcome::Rejected => 0x6a09_e667_f3bc_c909,
        Outcome::Decoded(spec, samples) => samples.iter().fold(
            u64::from(spec.sample_rate)
                ^ (u64::from(spec.channels) << 32)
                ^ (u64::from(spec.bits_per_sample) << 48),
            |hash, sample| hash.rotate_left(7) ^ sample.raw() as u64,
        ),
    }
}

pub(super) fn run_case(seed: u64) -> Result<u64, Box<dyn Error>> {
    let mut random = SplitMix64::new(seed);
    let bits = [8, 16, 24, 32][random.range(4) as usize];
    let spec = AudioSpec {
        channels: 1 + random.range(4) as u16,
        sample_rate: 1 + random.range(192_000) as u32,
        bits_per_sample: bits,
    };
    // Includes empty and one-frame files, with both signed endpoints, zero,
    // alternating full scale, and randomized payloads across the sequence.
    let frames = random.range(129) as usize;
    let low = -(1_i64 << (bits - 1));
    let high = -low - 1;
    let signed = (0..frames * usize::from(spec.channels))
        .map(|index| match index % 8 {
            0 => low,
            1 => high,
            2 => 0,
            _ => ((random.next() & ((1_u64 << bits) - 1)) as i64) + low,
        })
        .collect::<Vec<_>>();
    // Independent arithmetic oracle, not the PCM import method under test.
    let expected = Outcome::Decoded(
        spec,
        signed
            .iter()
            .map(|sample| Q1_63::from_raw(sample << (64 - bits)))
            .collect(),
    );
    let bytes = fixture(spec, &signed);
    let actual = compare_reads(&bytes, &mut random)?;
    if actual != expected {
        return Err("independent WAVE fixture disagrees with signed-PCM oracle".into());
    }

    // Exercise the production writer with random frame-aligned chunks as well.
    let mut encoded = Cursor::new(Vec::new());
    let mut writer = WaveWriter::new(&mut encoded, spec)?;
    let Outcome::Decoded(_, samples) = &expected else {
        unreachable!()
    };
    let mut offset = 0;
    while offset < samples.len() {
        let count = ((1 + random.range(17) as usize) * usize::from(spec.channels))
            .min(samples.len() - offset);
        writer.write_frames(
            &samples[offset..offset + count],
            RoundingMode::NearestTiesToEven,
            OverflowPolicy::Error,
        )?;
        offset += count;
    }
    let report = writer.finalize()?;
    if report.frames != frames as u64
        || report.saturated_samples != 0
        || compare_reads(encoded.get_ref(), &mut random)? != expected
    {
        return Err("WAVE writer round trip or accounting mismatch".into());
    }
    let mut whole_encoded = Cursor::new(Vec::new());
    let mut whole_writer = WaveWriter::new(&mut whole_encoded, spec)?;
    whole_writer.write_signed_pcm_frames(&signed)?;
    if whole_writer.finalize()? != report || whole_encoded.get_ref() != encoded.get_ref() {
        return Err("WAVE bytes depend on writer chunks or exact PCM entry point".into());
    }

    let mut checksum = fingerprint(&actual);
    let mut check = |candidate: &[u8], random: &mut SplitMix64| -> Result<(), Box<dyn Error>> {
        checksum = checksum.rotate_left(11) ^ fingerprint(&compare_reads(candidate, random)?);
        Ok(())
    };
    // Every incomplete minimal-header prefix, then a random payload truncation.
    for end in 0..44 {
        let prefix = &bytes[..end];
        if compare_reads(prefix, &mut random)? != Outcome::Rejected {
            return Err(format!("incomplete WAVE header accepted at byte {end}").into());
        }
        check(prefix, &mut random)?;
    }
    let end = random.range(bytes.len() as u64 + 1) as usize;
    check(&bytes[..end], &mut random)?;
    // Corrupt each structural field; large declared chunks never allocate a
    // corresponding file. Also mutate arbitrary payload/header bits. Acceptance
    // of a mutation is allowed, but a successful decode must remain identical.
    for (offset, width) in [
        (0, 4),
        (4, 4),
        (8, 4),
        (12, 4),
        (16, 4),
        (20, 2),
        (22, 2),
        (24, 4),
        (28, 4),
        (32, 2),
        (34, 2),
        (36, 4),
        (40, 4),
    ] {
        for byte in [0, 0xff] {
            let mut changed = bytes.clone();
            changed[offset..offset + width].fill(byte);
            check(&changed, &mut random)?;
        }
    }
    for _ in 0..8 {
        let mut changed = bytes.clone();
        let index = random.range(changed.len() as u64) as usize;
        changed[index] ^= 1 << random.range(8);
        check(&changed, &mut random)?;
    }
    Ok(checksum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_wave_property_batch() {
        let mut random = SplitMix64::new(0x1319_8a2e_0370_7344);
        for _ in 0..256 {
            let seed = random.next();
            run_case(seed).unwrap_or_else(|error| panic!("--suite io --case-seed {seed}: {error}"));
        }
    }

    #[test]
    fn declared_four_gigabyte_stream_reads_only_requested_prefix() {
        let spec = AudioSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
        };
        let mut bytes = fixture(spec, &[i64::from(i32::MIN), i64::from(i32::MAX)]);
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[40..44].copy_from_slice(&(u32::MAX - 7).to_le_bytes());
        let calls = Cell::new(0);
        let mut source = ShortReader {
            bytes: &bytes,
            max_read: 1,
            calls: &calls,
        };
        let mut reader = WaveReader::new(&mut source).unwrap();
        assert_eq!(calls.get(), 44);
        let chunk = reader.read_frames(1).unwrap();
        assert_eq!(chunk.frames, 1);
        assert_eq!(chunk.samples[0].raw(), i64::MIN);
        assert_eq!(chunk.samples[1].raw(), i64::from(i32::MAX) << 32);
        assert_eq!(calls.get(), 52);
        assert!(!reader.is_eof());
        assert!(reader.read_frames(1).is_err());
        assert_eq!(calls.get(), 53);
    }

    #[test]
    fn exhausted_reader_budget_is_a_failure_not_a_rejected_case() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&70_012_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEJUNK");
        bytes.extend_from_slice(&70_000_u32.to_le_bytes());
        bytes.resize(70_020, 0);
        let error = decode(&bytes, 1, 1).unwrap_err();
        assert_eq!(
            error.to_string(),
            "parser exhausted property-driver read budget"
        );
    }
}
