//! Streaming audio format adapters for SeX.
//!
//! Milestone 1 intentionally supports integer PCM WAVE through the `hound`
//! adapter. Format parsing remains outside the DSP crates and can later be
//! replaced or supplemented by libsndfile without changing Q arithmetic.

use sexq::{ArithmeticError, OverflowPolicy, Q1_63, RoundingMode};
use std::fmt;
use std::io::{self, Read, Seek, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioSpec {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
}

impl AudioSpec {
    pub fn validate(self) -> Result<Self, AudioIoError> {
        if self.channels == 0 {
            return Err(AudioIoError::ZeroChannels);
        }
        if self.sample_rate == 0 {
            return Err(AudioIoError::ZeroSampleRate);
        }
        if !(1..=32).contains(&self.bits_per_sample) {
            return Err(AudioIoError::UnsupportedIntegerWidth(self.bits_per_sample));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MetadataKey {
    Title,
    Copyright,
    Software,
    Artist,
    Comment,
    Date,
    Album,
    License,
    TrackNumber,
    Genre,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataEntry {
    pub key: MetadataKey,
    pub value: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AudioMetadata {
    entries: Vec<MetadataEntry>,
}

impl AudioMetadata {
    pub fn new(entries: Vec<MetadataEntry>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[MetadataEntry] {
        &self.entries
    }

    pub fn get(&self, key: MetadataKey) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value.as_str())
    }
}

#[derive(Debug)]
pub enum AudioIoError {
    Wave(hound::Error),
    Arithmetic(ArithmeticError),
    FloatingPointWaveUnsupported,
    UnsupportedIntegerWidth(u16),
    ZeroChannels,
    ZeroSampleRate,
    FrameCountOverflow,
    BufferAllocation(std::collections::TryReserveError),
    ReaderFailed,
    WriterFailed,
    WaveLayoutOverflow,
    WaveSizeLimit { data_bytes: u64, maximum: u64 },
    PartialFrame { samples: usize, channels: u16 },
}

impl fmt::Display for AudioIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wave(error) => error.fmt(f),
            Self::Arithmetic(error) => error.fmt(f),
            Self::FloatingPointWaveUnsupported => {
                f.write_str("floating-point WAVE input is outside milestone 1")
            }
            Self::UnsupportedIntegerWidth(bits) => {
                write!(f, "integer PCM width must be in 1..=32, got {bits}")
            }
            Self::ZeroChannels => f.write_str("audio stream must have at least one channel"),
            Self::ZeroSampleRate => f.write_str("sample rate must be non-zero"),
            Self::FrameCountOverflow => f.write_str("frame/sample count overflowed usize"),
            Self::BufferAllocation(error) => {
                write!(f, "cannot allocate audio read buffer: {error}")
            }
            Self::ReaderFailed => {
                f.write_str("audio reader cannot continue after an earlier read error")
            }
            Self::WriterFailed => {
                f.write_str("audio writer cannot continue after an earlier write error")
            }
            Self::WaveLayoutOverflow => {
                f.write_str("WAVE block alignment or byte rate exceeds its header field")
            }
            Self::WaveSizeLimit {
                data_bytes,
                maximum,
            } => write!(
                f,
                "classic RIFF/WAVE data size {data_bytes} exceeds the {maximum}-byte limit; RF64 output is not supported by this adapter"
            ),
            Self::PartialFrame { samples, channels } => write!(
                f,
                "chunk has {samples} interleaved samples, not a multiple of {channels} channels"
            ),
        }
    }
}

impl std::error::Error for AudioIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wave(error) => Some(error),
            Self::Arithmetic(error) => Some(error),
            Self::BufferAllocation(error) => Some(error),
            _ => None,
        }
    }
}

impl From<hound::Error> for AudioIoError {
    fn from(value: hound::Error) -> Self {
        Self::Wave(value)
    }
}

impl From<ArithmeticError> for AudioIoError {
    fn from(value: ArithmeticError) -> Self {
        Self::Arithmetic(value)
    }
}

/// A frame-aligned interleaved Q1.63 chunk.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InterleavedChunk {
    pub samples: Vec<Q1_63>,
    pub frames: usize,
}

/// On-demand integer PCM WAVE reader.
pub struct WaveReader<R: Read> {
    inner: hound::WavReader<RetryInterrupted<R>>,
    spec: AudioSpec,
    eof: bool,
    failed: bool,
}

// hound's byte-reading helpers do not retry Interrupted. Do that below its
// parser so an interrupted multi-byte field/sample retains its byte position.
struct RetryInterrupted<R>(R);

impl<R: Read> Read for RetryInterrupted<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.0.read(buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }
}

impl<R: Read> WaveReader<R> {
    pub fn new(reader: R) -> Result<Self, AudioIoError> {
        let inner = hound::WavReader::new(RetryInterrupted(reader))?;
        let wave_spec = inner.spec();
        if wave_spec.sample_format == hound::SampleFormat::Float {
            return Err(AudioIoError::FloatingPointWaveUnsupported);
        }
        let spec = AudioSpec {
            channels: wave_spec.channels,
            sample_rate: wave_spec.sample_rate,
            bits_per_sample: wave_spec.bits_per_sample,
        }
        .validate()?;
        Ok(Self {
            inner,
            spec,
            eof: false,
            failed: false,
        })
    }

    pub const fn spec(&self) -> AudioSpec {
        self.spec
    }

    pub const fn is_eof(&self) -> bool {
        self.eof
    }

    /// Read at most `max_frames`, preserving channel interleaving and frame
    /// boundaries. An empty chunk with `is_eof() == true` ends the stream.
    /// Storage grows with actual decoded samples, not the requested upper bound.
    /// Interrupted reads are retried. Other decode/allocation errors are terminal:
    /// the failed chunk is not returned and subsequent calls return `ReaderFailed`.
    /// A frame-count overflow is rejected before reading and can be retried.
    pub fn read_frames(&mut self, max_frames: usize) -> Result<InterleavedChunk, AudioIoError> {
        if self.failed {
            return Err(AudioIoError::ReaderFailed);
        }
        if max_frames == 0 || self.eof {
            return Ok(InterleavedChunk::default());
        }
        let channels = usize::from(self.spec.channels);
        let sample_limit = max_frames
            .checked_mul(channels)
            .ok_or(AudioIoError::FrameCountOverflow)?;
        let result = self.read_samples(sample_limit);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn read_samples(&mut self, sample_limit: usize) -> Result<InterleavedChunk, AudioIoError> {
        let channels = usize::from(self.spec.channels);
        let mut samples = Vec::new();
        let mut wave_samples = self.inner.samples::<i32>();
        for _ in 0..sample_limit {
            if samples.len() == samples.capacity() && wave_samples.len() != 0 {
                // Even an enormous declared data length only causes small,
                // fallible initial allocation. Grow only as bytes are decoded.
                let additional = (sample_limit - samples.len())
                    .min(wave_samples.len())
                    .min(8192);
                samples
                    .try_reserve(additional)
                    .map_err(AudioIoError::BufferAllocation)?;
            }
            match wave_samples.next() {
                Some(sample) => samples.push(Q1_63::from_signed_pcm(
                    i64::from(sample?),
                    u32::from(self.spec.bits_per_sample),
                )?),
                None => {
                    self.eof = true;
                    break;
                }
            }
        }
        if !samples.len().is_multiple_of(channels) {
            return Err(AudioIoError::PartialFrame {
                samples: samples.len(),
                channels: self.spec.channels,
            });
        }
        Ok(InterleavedChunk {
            frames: samples.len() / channels,
            samples,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WriteReport {
    pub frames: u64,
    pub saturated_samples: u64,
}

/// Streaming integer PCM WAVE writer with explicit Q1.63 quantization policy.
/// Any write error is terminal, including rejected frame layout or arithmetic.
/// `report()` counts only complete successful calls. A failed call can already
/// have written a prefix to the sink; only successful `finalize()` certifies
/// completion. Use a transactional destination when partial files must not be
/// visible (the CLI does). Underlying drop/close behavior is not transactional.
pub struct WaveWriter<W: Write + Seek> {
    inner: hound::WavWriter<W>,
    spec: AudioSpec,
    report: WriteReport,
    failed: bool,
}

fn validate_wave_output_size(spec: AudioSpec, frames: u64) -> Result<(), AudioIoError> {
    // hound 3.5.1 writes a 44-byte PCM or 68-byte extensible header.
    // RIFF's u32 size excludes its first eight bytes. Account for the header
    // before entering hound's otherwise unchecked u32 data/RIFF counters.
    let riff_header_bytes = if spec.channels > 2 || spec.bits_per_sample > 16 {
        60
    } else {
        36
    };
    let maximum = u64::from(u32::MAX) - riff_header_bytes;
    let data_bytes = frames
        .checked_mul(u64::from(spec.channels))
        .and_then(|samples| samples.checked_mul(u64::from(spec.bits_per_sample.div_ceil(8))))
        .ok_or(AudioIoError::FrameCountOverflow)?;
    if data_bytes > maximum {
        return Err(AudioIoError::WaveSizeLimit {
            data_bytes,
            maximum,
        });
    }
    Ok(())
}

impl<W: Write + Seek> WaveWriter<W> {
    pub fn new(writer: W, spec: AudioSpec) -> Result<Self, AudioIoError> {
        let spec = spec.validate()?;
        let block_align = spec
            .channels
            .checked_mul(spec.bits_per_sample.div_ceil(8))
            .ok_or(AudioIoError::WaveLayoutOverflow)?;
        spec.sample_rate
            .checked_mul(u32::from(block_align))
            .ok_or(AudioIoError::WaveLayoutOverflow)?;
        let inner = hound::WavWriter::new(
            writer,
            hound::WavSpec {
                channels: spec.channels,
                sample_rate: spec.sample_rate,
                bits_per_sample: spec.bits_per_sample,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        Ok(Self {
            inner,
            spec,
            report: WriteReport::default(),
            failed: false,
        })
    }

    pub const fn spec(&self) -> AudioSpec {
        self.spec
    }

    pub const fn report(&self) -> WriteReport {
        self.report
    }

    pub fn write_frames(
        &mut self,
        interleaved: &[Q1_63],
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<(), AudioIoError> {
        if self.failed {
            return Err(AudioIoError::WriterFailed);
        }
        let result = self.write_frames_inner(interleaved, rounding, overflow);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn write_frames_inner(
        &mut self,
        interleaved: &[Q1_63],
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<(), AudioIoError> {
        let channels = usize::from(self.spec.channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err(AudioIoError::PartialFrame {
                samples: interleaved.len(),
                channels: self.spec.channels,
            });
        }

        let frames = u64::try_from(interleaved.len() / channels)
            .map_err(|_| AudioIoError::FrameCountOverflow)?;
        let next_frames = self
            .report
            .frames
            .checked_add(frames)
            .ok_or(AudioIoError::FrameCountOverflow)?;
        validate_wave_output_size(self.spec, next_frames)?;
        let mut next_saturated_samples = self.report.saturated_samples;
        for &sample in interleaved {
            let quantized =
                sample.to_signed_pcm(u32::from(self.spec.bits_per_sample), rounding, overflow)?;
            let encoded = i32::try_from(quantized.value)
                .expect("a validated 1..=32-bit PCM value always fits i32");
            self.inner.write_sample(encoded)?;
            if quantized.saturated {
                next_saturated_samples = next_saturated_samples
                    .checked_add(1)
                    .ok_or(AudioIoError::FrameCountOverflow)?;
            }
        }
        self.report.frames = next_frames;
        self.report.saturated_samples = next_saturated_samples;
        Ok(())
    }

    /// Write already quantized signed PCM. This is the boundary used by the
    /// dither subsystem; values are still range-checked against the declared
    /// stream width before reaching the container library.
    pub fn write_signed_pcm_frames(&mut self, interleaved: &[i64]) -> Result<(), AudioIoError> {
        if self.failed {
            return Err(AudioIoError::WriterFailed);
        }
        let result = self.write_signed_pcm_frames_inner(interleaved);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn write_signed_pcm_frames_inner(&mut self, interleaved: &[i64]) -> Result<(), AudioIoError> {
        let channels = usize::from(self.spec.channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err(AudioIoError::PartialFrame {
                samples: interleaved.len(),
                channels: self.spec.channels,
            });
        }
        let frames = u64::try_from(interleaved.len() / channels)
            .map_err(|_| AudioIoError::FrameCountOverflow)?;
        let next_frames = self
            .report
            .frames
            .checked_add(frames)
            .ok_or(AudioIoError::FrameCountOverflow)?;
        validate_wave_output_size(self.spec, next_frames)?;
        for &sample in interleaved {
            Q1_63::from_signed_pcm(sample, u32::from(self.spec.bits_per_sample))?;
            let encoded =
                i32::try_from(sample).expect("a validated 1..=32-bit PCM value always fits i32");
            self.inner.write_sample(encoded)?;
        }
        self.report.frames = next_frames;
        Ok(())
    }

    /// Finalize the WAVE header and surface seek/write/flush errors. An earlier
    /// write failure returns `WriterFailed`, never a successful report.
    pub fn finalize(self) -> Result<WriteReport, AudioIoError> {
        if self.failed {
            return Err(AudioIoError::WriterFailed);
        }
        let report = self.report;
        self.inner.finalize()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Cursor;
    use std::rc::Rc;

    #[derive(Default)]
    struct WriteFaults {
        remaining: Cell<Option<usize>>,
        calls: Cell<usize>,
        seek: Cell<bool>,
        flush: Cell<bool>,
    }

    struct FaultWriter {
        inner: Cursor<Vec<u8>>,
        faults: Rc<WriteFaults>,
    }

    impl Write for FaultWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.faults.calls.set(self.faults.calls.get() + 1);
            if let Some(remaining) = self.faults.remaining.get() {
                if remaining == 0 {
                    self.faults.remaining.set(None);
                    return Err(io::Error::other("injected sample write failure"));
                }
                self.faults.remaining.set(Some(remaining - 1));
            }
            self.inner.write(&bytes[..bytes.len().min(1)])
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.faults.flush.get() {
                Err(io::Error::other("injected flush failure"))
            } else {
                Ok(())
            }
        }
    }

    impl Seek for FaultWriter {
        fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
            if self.faults.seek.get() {
                Err(io::Error::other("injected seek failure"))
            } else {
                self.inner.seek(position)
            }
        }
    }

    fn fault_writer(bits: u16) -> (WaveWriter<FaultWriter>, Rc<WriteFaults>) {
        let faults = Rc::new(WriteFaults::default());
        let sink = FaultWriter {
            inner: Cursor::new(Vec::new()),
            faults: Rc::clone(&faults),
        };
        let writer = WaveWriter::new(
            sink,
            AudioSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: bits,
            },
        )
        .unwrap();
        (writer, faults)
    }

    #[test]
    fn wave_writer_fault_at_each_payload_byte_poisoning_and_reports_are_consistent() {
        for bits in [8, 16, 24, 32] {
            for prequantized in [false, true] {
                for offset in 0..usize::from(bits / 8) * 4 {
                    let (mut writer, faults) = fault_writer(bits);
                    writer.write_signed_pcm_frames(&[0, 1]).unwrap();
                    let prior = writer.report();
                    faults.remaining.set(Some(offset));
                    let result = if prequantized {
                        writer.write_signed_pcm_frames(&[-1, 0, 1, 127])
                    } else {
                        writer.write_frames(
                            &[Q1_63::MAX, Q1_63::ZERO, Q1_63::MIN, Q1_63::ZERO],
                            RoundingMode::NearestTiesToEven,
                            OverflowPolicy::Saturate,
                        )
                    };
                    assert!(matches!(
                        result,
                        Err(AudioIoError::Wave(hound::Error::IoError(_)))
                    ));
                    assert_eq!(writer.report(), prior);
                    let before = faults.calls.get();
                    assert!(matches!(
                        writer.write_signed_pcm_frames(&[]),
                        Err(AudioIoError::WriterFailed)
                    ));
                    assert!(matches!(
                        writer.write_frames(
                            &[Q1_63::ZERO; 2],
                            RoundingMode::Floor,
                            OverflowPolicy::Error
                        ),
                        Err(AudioIoError::WriterFailed)
                    ));
                    assert_eq!(faults.calls.get(), before);
                    assert!(matches!(writer.finalize(), Err(AudioIoError::WriterFailed)));
                }
            }
        }
    }

    #[test]
    fn wave_writer_arithmetic_and_layout_errors_cannot_finalize_successfully() {
        for mode in 0..3 {
            let (mut writer, _) = fault_writer(16);
            writer.write_signed_pcm_frames(&[1, -1]).unwrap();
            let prior = writer.report();
            let result = match mode {
                0 => writer.write_signed_pcm_frames(&[0]),
                1 => writer.write_signed_pcm_frames(&[0, 32768]),
                _ => writer.write_frames(
                    &[Q1_63::ZERO, Q1_63::MAX],
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error,
                ),
            };
            assert!(result.is_err());
            assert_eq!(writer.report(), prior);
            assert!(matches!(writer.finalize(), Err(AudioIoError::WriterFailed)));
        }
    }

    #[test]
    fn wave_writer_surfaces_final_header_seek_write_and_flush_failures() {
        for mode in 0..3 {
            let (mut writer, faults) = fault_writer(24);
            writer.write_signed_pcm_frames(&[1, -1]).unwrap();
            match mode {
                0 => faults.seek.set(true),
                1 => faults.remaining.set(Some(0)),
                _ => faults.flush.set(true),
            }
            assert!(matches!(
                writer.finalize(),
                Err(AudioIoError::Wave(hound::Error::IoError(_)))
            ));
        }
    }

    #[test]
    fn wave_writer_rejects_header_field_overflow_before_writing() {
        for spec in [
            AudioSpec {
                channels: u16::MAX,
                sample_rate: 48_000,
                bits_per_sample: 32,
            },
            AudioSpec {
                channels: 2,
                sample_rate: u32::MAX,
                bits_per_sample: 16,
            },
        ] {
            let faults = Rc::new(WriteFaults::default());
            let sink = FaultWriter {
                inner: Cursor::new(Vec::new()),
                faults: Rc::clone(&faults),
            };
            assert!(matches!(
                WaveWriter::new(sink, spec),
                Err(AudioIoError::WaveLayoutOverflow)
            ));
            assert_eq!(faults.calls.get(), 0);
        }
    }

    #[test]
    fn wave_writer_riff_size_boundary_is_checked_without_large_fixtures() {
        for bits in [8, 16, 24, 32] {
            for channels in [1, 2, 3] {
                let spec = AudioSpec {
                    channels,
                    sample_rate: 48_000,
                    bits_per_sample: bits,
                };
                let mut cursor = Cursor::new(Vec::new());
                WaveWriter::new(&mut cursor, spec)
                    .unwrap()
                    .finalize()
                    .unwrap();
                let header = cursor.get_ref().len() as u64 - 8;
                let frame_bytes = u64::from(channels) * u64::from(bits / 8);
                let last_frame = (u64::from(u32::MAX) - header) / frame_bytes;
                validate_wave_output_size(spec, last_frame).unwrap();
                assert!(matches!(validate_wave_output_size(spec, last_frame + 1),
                    Err(AudioIoError::WaveSizeLimit { maximum, .. }) if maximum == u64::from(u32::MAX) - header));
                let (mut writer, faults) = fault_writer(bits);
                // Model the boundary through the writer's completed-frame count;
                // the underlying fixture remains just a small header. A rejected
                // next chunk must not reach hound's narrow byte counter at all.
                let own_frame_bytes = 2 * u64::from(bits / 8);
                let own_header = if bits > 16 { 60 } else { 36 };
                writer.report.frames = (u64::from(u32::MAX) - own_header) / own_frame_bytes;
                let before = faults.calls.get();
                assert!(matches!(
                    writer.write_signed_pcm_frames(&[0, 0]),
                    Err(AudioIoError::WaveSizeLimit { .. })
                ));
                assert_eq!(faults.calls.get(), before);
                assert!(matches!(writer.finalize(), Err(AudioIoError::WriterFailed)));
            }
        }
    }

    struct FaultReader<'a> {
        bytes: &'a [u8],
        position: usize,
        fault_at: usize,
        fault_kind: io::ErrorKind,
        fired: bool,
        calls: &'a Cell<usize>,
    }

    impl Read for FaultReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.calls.set(self.calls.get() + 1);
            if self.position == self.fault_at && !self.fired {
                self.fired = true;
                return Err(io::Error::new(self.fault_kind, "injected read failure"));
            }
            if buffer.is_empty() || self.position == self.bytes.len() {
                return Ok(0);
            }
            buffer[0] = self.bytes[self.position];
            self.position += 1;
            Ok(1)
        }
    }

    #[test]
    fn interrupted_reads_at_every_header_and_sample_byte_are_retried_exactly() {
        for bits in [8, 16, 24, 32] {
            let spec = AudioSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: bits,
            };
            let pcm = [-1, 0, 1, 127];
            let bytes = make_integer_wave(spec, &pcm);
            for fault_at in 0..bytes.len() {
                let calls = Cell::new(0);
                let source = FaultReader {
                    bytes: &bytes,
                    position: 0,
                    fault_at,
                    fault_kind: io::ErrorKind::Interrupted,
                    fired: false,
                    calls: &calls,
                };
                let mut reader = WaveReader::new(source).unwrap();
                let mut decoded = Vec::new();
                while !reader.is_eof() {
                    decoded.extend(reader.read_frames(1).unwrap().samples);
                }
                assert_eq!(
                    decoded,
                    pcm.iter()
                        .map(|&code| Q1_63::from_raw(i64::from(code) << (64 - bits)))
                        .collect::<Vec<_>>()
                );
                assert_eq!(calls.get(), bytes.len() + 1, "bits={bits}, byte={fault_at}");
            }
        }
    }

    #[test]
    fn noninterrupted_errors_at_every_byte_fail_closed_without_resuming() {
        for bits in [8, 16, 24, 32] {
            let spec = AudioSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: bits,
            };
            let bytes = make_integer_wave(spec, &[-1, 0, 1, 127]);
            for fault_kind in [
                io::ErrorKind::Other,
                io::ErrorKind::UnexpectedEof,
                io::ErrorKind::WouldBlock,
            ] {
                for fault_at in 0..bytes.len() {
                    let calls = Cell::new(0);
                    let source = FaultReader {
                        bytes: &bytes,
                        position: 0,
                        fault_at,
                        fault_kind,
                        fired: false,
                        calls: &calls,
                    };
                    match WaveReader::new(source) {
                        Err(AudioIoError::Wave(hound::Error::IoError(error))) => {
                            assert_eq!(error.kind(), fault_kind)
                        }
                        Err(error) => panic!("lost original header I/O error: {error}"),
                        Ok(mut reader) => {
                            assert!(matches!(reader.read_frames(2),
                                Err(AudioIoError::Wave(hound::Error::IoError(error)))
                                if error.kind() == fault_kind));
                            assert!(!reader.is_eof());
                            let calls_after_failure = calls.get();
                            for frames in [0, 1, usize::MAX] {
                                assert!(matches!(
                                    reader.read_frames(frames),
                                    Err(AudioIoError::ReaderFailed)
                                ));
                                assert_eq!(calls.get(), calls_after_failure);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn oversized_requests_do_not_allocate_the_requested_upper_bound() {
        for channels in [1, 2, 4] {
            let spec = AudioSpec {
                channels,
                sample_rate: 48_000,
                bits_per_sample: 16,
            };
            let pcm = vec![123; usize::from(channels)];
            let bytes = make_integer_wave(spec, &pcm);
            let mut reader = WaveReader::new(Cursor::new(bytes)).unwrap();
            let chunk = reader
                .read_frames(usize::MAX / usize::from(channels))
                .unwrap();
            assert_eq!(chunk.frames, 1);
            assert_eq!(chunk.samples.len(), usize::from(channels));
            assert!(chunk.samples.capacity() <= 8192);
            assert!(reader.is_eof());
        }
    }

    #[test]
    fn frame_count_overflow_is_rejected_before_reading_and_is_retryable() {
        let spec = AudioSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
        };
        let bytes = make_integer_wave(spec, &[-1, 1]);
        let calls = Cell::new(0);
        let source = FaultReader {
            bytes: &bytes,
            position: 0,
            fault_at: usize::MAX,
            fault_kind: io::ErrorKind::Other,
            fired: false,
            calls: &calls,
        };
        let mut reader = WaveReader::new(source).unwrap();
        let before = calls.get();
        assert!(matches!(
            reader.read_frames(usize::MAX),
            Err(AudioIoError::FrameCountOverflow)
        ));
        assert_eq!(calls.get(), before);
        assert_eq!(reader.read_frames(1).unwrap().frames, 1);
    }

    #[test]
    fn huge_declared_payload_with_huge_request_reports_truncation_without_bulk_allocation() {
        let spec = AudioSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
        };
        let mut bytes = make_integer_wave(spec, &[123]);
        let data = bytes
            .windows(4)
            .position(|window| window == b"data")
            .unwrap();
        bytes[data + 4..data + 8].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        let mut reader = WaveReader::new(Cursor::new(bytes)).unwrap();
        assert!(matches!(
            reader.read_frames(usize::MAX),
            Err(AudioIoError::Wave(_))
        ));
        assert!(matches!(
            reader.read_frames(1),
            Err(AudioIoError::ReaderFailed)
        ));
    }

    fn make_integer_wave(spec: AudioSpec, samples: &[i32]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut cursor,
                hound::WavSpec {
                    channels: spec.channels,
                    sample_rate: spec.sample_rate,
                    bits_per_sample: spec.bits_per_sample,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for &sample in samples {
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn reader_is_streaming_and_frame_aligned() {
        let spec = AudioSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
        };
        let pcm = [-32_768, 32_767, -1, 0, 1, 1234];
        let bytes = make_integer_wave(spec, &pcm);
        let mut reader = WaveReader::new(Cursor::new(bytes)).unwrap();

        assert_eq!(reader.spec(), spec);
        let first = reader.read_frames(2).unwrap();
        assert_eq!(first.frames, 2);
        assert_eq!(
            first.samples,
            pcm[..4]
                .iter()
                .map(|&value| Q1_63::from_signed_pcm(value.into(), 16).unwrap())
                .collect::<Vec<_>>()
        );
        let second = reader.read_frames(2).unwrap();
        assert_eq!(second.frames, 1);
        assert_eq!(reader.read_frames(2).unwrap().frames, 0);
        assert!(reader.is_eof());
    }

    #[test]
    fn writer_and_reader_round_trip_integer_pcm_bit_exactly() {
        let spec = AudioSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 24,
        };
        let pcm = [-8_388_608, -1, 0, 1, 8_388_607];
        let samples = pcm
            .iter()
            .map(|&value| Q1_63::from_signed_pcm(value, 24).unwrap())
            .collect::<Vec<_>>();
        let mut cursor = Cursor::new(Vec::new());
        let report = {
            let mut writer = WaveWriter::new(&mut cursor, spec).unwrap();
            writer
                .write_frames(
                    &samples,
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Error,
                )
                .unwrap();
            writer.finalize().unwrap()
        };
        assert_eq!(report.frames, 5);
        assert_eq!(report.saturated_samples, 0);

        cursor.set_position(0);
        let mut reader = WaveReader::new(cursor).unwrap();
        assert_eq!(reader.read_frames(16).unwrap().samples, samples);
    }

    #[test]
    fn one_sample_file_is_supported() {
        let spec = AudioSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 8,
        };
        let bytes = make_integer_wave(spec, &[-128]);
        let mut reader = WaveReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.read_frames(4).unwrap().samples, vec![Q1_63::MIN]);
    }

    #[test]
    fn float_wave_is_rejected_before_entering_the_signal_path() {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut cursor,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 48_000,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                },
            )
            .unwrap();
            writer.write_sample(0.5_f32).unwrap();
            writer.finalize().unwrap();
        }
        cursor.set_position(0);
        assert!(matches!(
            WaveReader::new(cursor),
            Err(AudioIoError::FloatingPointWaveUnsupported)
        ));
    }

    #[test]
    fn writer_rejects_partial_multichannel_frames() {
        let spec = AudioSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
        };
        let mut cursor = Cursor::new(Vec::new());
        let mut writer = WaveWriter::new(&mut cursor, spec).unwrap();
        assert!(matches!(
            writer.write_frames(
                &[Q1_63::ZERO],
                RoundingMode::NearestTiesToEven,
                OverflowPolicy::Error
            ),
            Err(AudioIoError::PartialFrame {
                samples: 1,
                channels: 2
            })
        ));
    }

    #[test]
    fn output_saturation_is_reported() {
        let spec = AudioSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
        };
        let mut cursor = Cursor::new(Vec::new());
        let report = {
            let mut writer = WaveWriter::new(&mut cursor, spec).unwrap();
            writer
                .write_frames(
                    &[Q1_63::MAX],
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Saturate,
                )
                .unwrap();
            writer.finalize().unwrap()
        };
        assert_eq!(report.saturated_samples, 1);
    }

    #[test]
    fn prequantized_pcm_is_range_and_frame_checked() {
        let spec = AudioSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
        };
        let mut cursor = Cursor::new(Vec::new());
        let mut writer = WaveWriter::new(&mut cursor, spec).unwrap();
        writer
            .write_signed_pcm_frames(&[-32_768, 32_767, -1, 1])
            .unwrap();
        assert!(writer.write_signed_pcm_frames(&[0]).is_err());
        assert!(writer.write_signed_pcm_frames(&[32_768, 0]).is_err());
    }
}
