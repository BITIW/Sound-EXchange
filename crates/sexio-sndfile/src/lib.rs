//! Runtime-loaded libsndfile adapter for SeX.
//!
//! The dynamic/unsafe boundary is confined to `ffi`. Public processing uses
//! Q1.63 samples and accepts only integer source encodings.

use sexio::{
    AudioIoError, AudioMetadata, AudioSpec, InterleavedChunk, MetadataEntry, MetadataKey,
    WriteReport,
};
use sexq::{ArithmeticError, OverflowPolicy, Q1_63, RoundingMode};
use std::collections::HashSet;
use std::ffi::CString;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerFormat {
    Wave,
    Aiff,
    Flac,
}

impl ContainerFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("wav") || extension.eq_ignore_ascii_case("wave") {
            Some(Self::Wave)
        } else if extension.eq_ignore_ascii_case("aif") || extension.eq_ignore_ascii_case("aiff") {
            Some(Self::Aiff)
        } else if extension.eq_ignore_ascii_case("flac") {
            Some(Self::Flac)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub enum SndFileError {
    LibraryUnavailable(String),
    MissingSymbol {
        name: &'static str,
        detail: String,
    },
    PathContainsNul,
    MetadataContainsNul(MetadataKey),
    MetadataIsNotUtf8(MetadataKey),
    VersionStringUnavailable,
    VersionStringIsNotUtf8,
    DuplicateMetadataKey(MetadataKey),
    UnsupportedContainerExtension,
    UnsupportedSourceEncoding(i32),
    UnsupportedOutputWidth {
        container: ContainerFormat,
        bits: u16,
    },
    InvalidContainerEncoding,
    Open(String),
    Read(String),
    Write(String),
    MetadataWrite {
        key: MetadataKey,
        detail: String,
    },
    Close(String),
    FrameCountOverflow,
    PartialFrame {
        samples: usize,
        channels: u16,
    },
    Poisoned,
    Audio(AudioIoError),
    Arithmetic(ArithmeticError),
}

impl fmt::Display for SndFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibraryUnavailable(detail) => {
                write!(f, "libsndfile runtime is unavailable: {detail}")
            }
            Self::MissingSymbol { name, detail } => {
                write!(f, "libsndfile symbol {name} is unavailable: {detail}")
            }
            Self::PathContainsNul => f.write_str("audio path contains an embedded NUL byte"),
            Self::MetadataContainsNul(key) => {
                write!(
                    f,
                    "metadata value for {key:?} contains an embedded NUL byte"
                )
            }
            Self::MetadataIsNotUtf8(key) => {
                write!(f, "libsndfile returned non-UTF-8 metadata for {key:?}")
            }
            Self::VersionStringUnavailable => {
                f.write_str("libsndfile returned a null version string")
            }
            Self::VersionStringIsNotUtf8 => {
                f.write_str("libsndfile returned a non-UTF-8 version string")
            }
            Self::DuplicateMetadataKey(key) => {
                write!(f, "metadata contains duplicate {key:?} entries")
            }
            Self::UnsupportedContainerExtension => {
                f.write_str("output extension must be .wav, .aif/.aiff, or .flac")
            }
            Self::UnsupportedSourceEncoding(format) => write!(
                f,
                "libsndfile source subtype 0x{format:04x} is not integer PCM"
            ),
            Self::UnsupportedOutputWidth { container, bits } => write!(
                f,
                "{container:?} output does not support {bits}-bit integer PCM in this backend"
            ),
            Self::InvalidContainerEncoding => {
                f.write_str("libsndfile rejected the requested container/PCM combination")
            }
            Self::Open(detail) => write!(f, "libsndfile could not open audio: {detail}"),
            Self::Read(detail) => write!(f, "libsndfile read failed: {detail}"),
            Self::Write(detail) => write!(f, "libsndfile write failed: {detail}"),
            Self::MetadataWrite { key, detail } => {
                write!(f, "could not write {key:?} metadata: {detail}")
            }
            Self::Close(detail) => write!(f, "libsndfile finalization failed: {detail}"),
            Self::FrameCountOverflow => f.write_str("audio frame/sample count overflow"),
            Self::PartialFrame { samples, channels } => write!(
                f,
                "chunk has {samples} samples, not a multiple of {channels} channels"
            ),
            Self::Poisoned => f.write_str("audio writer cannot continue after an earlier error"),
            Self::Audio(error) => error.fmt(f),
            Self::Arithmetic(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SndFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Audio(error) => Some(error),
            Self::Arithmetic(error) => Some(error),
            _ => None,
        }
    }
}

impl From<AudioIoError> for SndFileError {
    fn from(value: AudioIoError) -> Self {
        Self::Audio(value)
    }
}

impl From<ArithmeticError> for SndFileError {
    fn from(value: ArithmeticError) -> Self {
        Self::Arithmetic(value)
    }
}

pub struct SndFileLibrary {
    api: Arc<ffi::Api>,
}

impl SndFileLibrary {
    pub fn load() -> Result<Self, SndFileError> {
        Ok(Self {
            api: Arc::new(ffi::Api::load()?),
        })
    }

    pub fn open(&self, path: &Path) -> Result<SndFileReader, SndFileError> {
        SndFileReader::open_with_api(Arc::clone(&self.api), path)
    }

    pub fn version(&self) -> Result<String, SndFileError> {
        self.api.version()
    }

    pub fn create(
        &self,
        path: &Path,
        spec: AudioSpec,
        metadata: &AudioMetadata,
    ) -> Result<SndFileWriter, SndFileError> {
        let container =
            ContainerFormat::from_path(path).ok_or(SndFileError::UnsupportedContainerExtension)?;
        SndFileWriter::create_with_api(Arc::clone(&self.api), path, spec, container, metadata)
    }

    pub fn create_as(
        &self,
        path: &Path,
        spec: AudioSpec,
        container: ContainerFormat,
        metadata: &AudioMetadata,
    ) -> Result<SndFileWriter, SndFileError> {
        SndFileWriter::create_with_api(Arc::clone(&self.api), path, spec, container, metadata)
    }
}

pub struct SndFileReader {
    api: Arc<ffi::Api>,
    handle: Option<ffi::Handle>,
    spec: AudioSpec,
    metadata: AudioMetadata,
    state: ReaderState,
}

impl SndFileReader {
    fn open_with_api(api: Arc<ffi::Api>, path: &Path) -> Result<Self, SndFileError> {
        let (handle, info) = open_audio_path(&api, path, ffi::SFM_READ, ffi::SfInfo::default())?;
        let subtype = info.format & ffi::SF_FORMAT_SUBMASK;
        let bits_per_sample = match subtype {
            ffi::SF_FORMAT_PCM_S8 | ffi::SF_FORMAT_PCM_U8 => 8,
            ffi::SF_FORMAT_PCM_16 => 16,
            ffi::SF_FORMAT_PCM_24 => 24,
            ffi::SF_FORMAT_PCM_32 => 32,
            _ => {
                let detail = subtype;
                api.close(handle).ok();
                return Err(SndFileError::UnsupportedSourceEncoding(detail));
            }
        };
        let spec_result = (|| {
            Ok(AudioSpec {
                channels: u16::try_from(info.channels)
                    .map_err(|_| SndFileError::FrameCountOverflow)?,
                sample_rate: u32::try_from(info.sample_rate)
                    .map_err(|_| SndFileError::FrameCountOverflow)?,
                bits_per_sample,
            }
            .validate()?)
        })();
        let spec = match spec_result {
            Ok(spec) => spec,
            Err(error) => {
                api.close(handle).ok();
                return Err(error);
            }
        };
        let metadata = match read_metadata(&api, handle) {
            Ok(metadata) => metadata,
            Err(error) => {
                api.close(handle).ok();
                return Err(error);
            }
        };
        Ok(Self {
            api,
            handle: Some(handle),
            spec,
            metadata,
            state: ReaderState::default(),
        })
    }

    pub const fn spec(&self) -> AudioSpec {
        self.spec
    }

    pub const fn metadata(&self) -> &AudioMetadata {
        &self.metadata
    }

    pub fn library_version(&self) -> Result<String, SndFileError> {
        self.api.version()
    }

    pub const fn is_eof(&self) -> bool {
        self.state.eof
    }

    /// Read at most `max_frames` using bounded native scratch space. Returned
    /// storage grows with decoded audio. Errors after reading starts are terminal.
    pub fn read_frames(&mut self, max_frames: usize) -> Result<InterleavedChunk, SndFileError> {
        let handle = self.handle.expect("reader handle lives until Drop");
        self.state
            .read(max_frames, self.spec.channels, |output, frames| {
                self.api.read_frames_i32(handle, output, frames)
            })
    }
}

#[derive(Default)]
struct ReaderState {
    eof: bool,
    failed: bool,
}

impl ReaderState {
    fn read(
        &mut self,
        max_frames: usize,
        channels: u16,
        mut read: impl FnMut(&mut [i32], i64) -> Result<i64, SndFileError>,
    ) -> Result<InterleavedChunk, SndFileError> {
        if self.failed {
            return Err(AudioIoError::ReaderFailed.into());
        }
        if max_frames == 0 || self.eof {
            return Ok(InterleavedChunk::default());
        }
        let channels = usize::from(channels);
        max_frames
            .checked_mul(channels)
            .ok_or(SndFileError::FrameCountOverflow)?;
        let result = (|| {
            // At most 8192 samples, or one complete very-high-channel frame.
            let scratch_frames = max_frames.min((8192 / channels).max(1));
            let mut native = Vec::new();
            native
                .try_reserve_exact(scratch_frames * channels)
                .map_err(AudioIoError::BufferAllocation)?;
            native.resize(scratch_frames * channels, 0_i32);
            let mut samples = Vec::new();
            let mut frames = 0;
            while frames < max_frames {
                let requested = scratch_frames.min(max_frames - frames);
                let count = read(&mut native[..requested * channels], requested as i64)?;
                if count < 0 || count > requested as i64 {
                    return Err(SndFileError::Read(
                        "invalid returned frame count".to_owned(),
                    ));
                }
                let count = count as usize;
                let sample_count = count * channels;
                samples
                    .try_reserve(sample_count)
                    .map_err(AudioIoError::BufferAllocation)?;
                samples.extend(
                    native[..sample_count]
                        .iter()
                        .map(|&sample| Q1_63::from_raw(i64::from(sample) << 32)),
                );
                frames += count;
                if count < requested {
                    self.eof = true;
                    break;
                }
            }
            Ok(InterleavedChunk { samples, frames })
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl Drop for SndFileReader {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.api.close(handle).ok();
        }
    }
}

pub struct SndFileWriter {
    api: Arc<ffi::Api>,
    handle: Option<ffi::Handle>,
    spec: AudioSpec,
    report: WriteReport,
    poisoned: bool,
}

impl SndFileWriter {
    fn create_with_api(
        api: Arc<ffi::Api>,
        path: &Path,
        spec: AudioSpec,
        container: ContainerFormat,
        metadata: &AudioMetadata,
    ) -> Result<Self, SndFileError> {
        let spec = spec.validate()?;
        let format = output_format(container, spec.bits_per_sample)?;
        let info = ffi::SfInfo {
            frames: 0,
            sample_rate: i32::try_from(spec.sample_rate)
                .map_err(|_| SndFileError::FrameCountOverflow)?,
            channels: i32::from(spec.channels),
            format,
            sections: 0,
            seekable: 0,
        };
        if !api.format_check(&info) {
            return Err(SndFileError::InvalidContainerEncoding);
        }
        validate_metadata(metadata)?;
        let (handle, _) = open_audio_path(&api, path, ffi::SFM_WRITE, info)?;
        if let Err(error) = write_metadata(&api, handle, metadata) {
            api.close(handle).ok();
            return Err(error);
        }
        Ok(Self {
            api,
            handle: Some(handle),
            spec,
            report: WriteReport::default(),
            poisoned: false,
        })
    }

    pub const fn spec(&self) -> AudioSpec {
        self.spec
    }

    pub const fn report(&self) -> WriteReport {
        self.report
    }

    pub fn library_version(&self) -> Result<String, SndFileError> {
        self.api.version()
    }

    pub fn write_frames(
        &mut self,
        interleaved: &[Q1_63],
        rounding: RoundingMode,
        overflow: OverflowPolicy,
    ) -> Result<(), SndFileError> {
        if self.poisoned {
            return Err(SndFileError::Poisoned);
        }
        let result = (|| {
            validate_frame_alignment(interleaved.len(), self.spec.channels)?;
            let mut native = Vec::new();
            native
                .try_reserve(interleaved.len())
                .map_err(AudioIoError::BufferAllocation)?;
            let mut saturated_samples = 0_u64;
            for sample in interleaved {
                let outcome = sample.to_signed_pcm(
                    u32::from(self.spec.bits_per_sample),
                    rounding,
                    overflow,
                )?;
                native.push(left_align_i32(outcome.value, self.spec.bits_per_sample)?);
                saturated_samples = saturated_samples
                    .checked_add(u64::from(outcome.saturated))
                    .ok_or(SndFileError::FrameCountOverflow)?;
            }
            self.write_native_frames(&native, saturated_samples)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    pub fn write_signed_pcm_frames(&mut self, interleaved: &[i64]) -> Result<(), SndFileError> {
        if self.poisoned {
            return Err(SndFileError::Poisoned);
        }
        let result = (|| {
            validate_frame_alignment(interleaved.len(), self.spec.channels)?;
            let mut native = Vec::new();
            native
                .try_reserve(interleaved.len())
                .map_err(AudioIoError::BufferAllocation)?;
            for sample in interleaved {
                Q1_63::from_signed_pcm(*sample, u32::from(self.spec.bits_per_sample))?;
                native.push(left_align_i32(*sample, self.spec.bits_per_sample)?);
            }
            self.write_native_frames(&native, 0)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn write_native_frames(
        &mut self,
        native: &[i32],
        saturated_samples: u64,
    ) -> Result<(), SndFileError> {
        let channels = usize::from(self.spec.channels);
        let frames = native.len() / channels;
        let requested = i64::try_from(frames).map_err(|_| SndFileError::FrameCountOverflow)?;
        // Validate both counters before submitting a block, then commit them
        // together only after a complete successful native write.
        let next_report = WriteReport {
            frames: self
                .report
                .frames
                .checked_add(u64::try_from(frames).map_err(|_| SndFileError::FrameCountOverflow)?)
                .ok_or(SndFileError::FrameCountOverflow)?,
            saturated_samples: self
                .report
                .saturated_samples
                .checked_add(saturated_samples)
                .ok_or(SndFileError::FrameCountOverflow)?,
        };
        let handle = self.handle.expect("writer handle lives until finalize");
        let written = self.api.write_frames_i32(handle, native, requested)?;
        if written != requested {
            return Err(SndFileError::Write(self.api.error(handle)));
        }
        self.report = next_report;
        Ok(())
    }

    pub fn finalize(mut self) -> Result<WriteReport, SndFileError> {
        if self.poisoned {
            return Err(SndFileError::Poisoned);
        }
        let handle = self.handle.take().expect("writer is finalized once");
        self.api.close(handle)?;
        Ok(self.report)
    }
}

impl Drop for SndFileWriter {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.api.close(handle).ok();
        }
    }
}

fn output_format(container: ContainerFormat, bits: u16) -> Result<i32, SndFileError> {
    let subtype = match (container, bits) {
        (ContainerFormat::Wave, 8) => ffi::SF_FORMAT_PCM_U8,
        (ContainerFormat::Aiff | ContainerFormat::Flac, 8) => ffi::SF_FORMAT_PCM_S8,
        (_, 16) => ffi::SF_FORMAT_PCM_16,
        (_, 24) => ffi::SF_FORMAT_PCM_24,
        (ContainerFormat::Wave | ContainerFormat::Aiff, 32) => ffi::SF_FORMAT_PCM_32,
        _ => return Err(SndFileError::UnsupportedOutputWidth { container, bits }),
    };
    let major = match container {
        ContainerFormat::Wave => ffi::SF_FORMAT_WAV,
        ContainerFormat::Aiff => ffi::SF_FORMAT_AIFF,
        ContainerFormat::Flac => ffi::SF_FORMAT_FLAC,
    };
    Ok(major | subtype)
}

#[cfg(not(windows))]
fn open_audio_path(
    api: &ffi::Api,
    path: &Path,
    mode: i32,
    info: ffi::SfInfo,
) -> Result<(ffi::Handle, ffi::SfInfo), SndFileError> {
    let path = path_to_c_string(path)?;
    api.open(&path, mode, info)
}

#[cfg(windows)]
fn open_audio_path(
    api: &ffi::Api,
    path: &Path,
    mode: i32,
    info: ffi::SfInfo,
) -> Result<(ffi::Handle, ffi::SfInfo), SndFileError> {
    use std::os::windows::ffi::OsStrExt;

    let mut path = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if path.contains(&0) {
        return Err(SndFileError::PathContainsNul);
    }
    path.push(0);
    api.open_wide(&path, mode, info)
}

#[cfg(not(windows))]
fn path_to_c_string(path: &Path) -> Result<CString, SndFileError> {
    CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| SndFileError::PathContainsNul)
}

fn left_align_i32(sample: i64, bits: u16) -> Result<i32, SndFileError> {
    let shifted = i128::from(sample) << (32 - u32::from(bits));
    i32::try_from(shifted).map_err(|_| SndFileError::FrameCountOverflow)
}

fn validate_frame_alignment(samples: usize, channels: u16) -> Result<(), SndFileError> {
    if !samples.is_multiple_of(usize::from(channels)) {
        return Err(SndFileError::PartialFrame { samples, channels });
    }
    Ok(())
}

const METADATA_KEYS: [(MetadataKey, i32); 10] = [
    (MetadataKey::Title, ffi::SF_STR_TITLE),
    (MetadataKey::Copyright, ffi::SF_STR_COPYRIGHT),
    (MetadataKey::Software, ffi::SF_STR_SOFTWARE),
    (MetadataKey::Artist, ffi::SF_STR_ARTIST),
    (MetadataKey::Comment, ffi::SF_STR_COMMENT),
    (MetadataKey::Date, ffi::SF_STR_DATE),
    (MetadataKey::Album, ffi::SF_STR_ALBUM),
    (MetadataKey::License, ffi::SF_STR_LICENSE),
    (MetadataKey::TrackNumber, ffi::SF_STR_TRACKNUMBER),
    (MetadataKey::Genre, ffi::SF_STR_GENRE),
];

fn read_metadata(api: &ffi::Api, handle: ffi::Handle) -> Result<AudioMetadata, SndFileError> {
    let mut entries = Vec::new();
    for (key, raw_key) in METADATA_KEYS {
        if let Some(value) = api.get_string(handle, raw_key) {
            let value = value
                .to_str()
                .map_err(|_| SndFileError::MetadataIsNotUtf8(key))?
                .to_owned();
            entries.push(MetadataEntry { key, value });
        }
    }
    Ok(AudioMetadata::new(entries))
}

fn validate_metadata(metadata: &AudioMetadata) -> Result<(), SndFileError> {
    let mut seen = HashSet::new();
    for entry in metadata.entries() {
        if !seen.insert(entry.key) {
            return Err(SndFileError::DuplicateMetadataKey(entry.key));
        }
        if entry.value.as_bytes().contains(&0) {
            return Err(SndFileError::MetadataContainsNul(entry.key));
        }
    }
    Ok(())
}

fn write_metadata(
    api: &ffi::Api,
    handle: ffi::Handle,
    metadata: &AudioMetadata,
) -> Result<(), SndFileError> {
    for entry in metadata.entries() {
        let raw_key = METADATA_KEYS
            .iter()
            .find(|(key, _)| *key == entry.key)
            .map(|(_, value)| *value)
            .expect("all MetadataKey variants have libsndfile mappings");
        let value = CString::new(entry.value.as_bytes())
            .map_err(|_| SndFileError::MetadataContainsNul(entry.key))?;
        if !api.set_string(handle, raw_key, &value) {
            return Err(SndFileError::MetadataWrite {
                key: entry.key,
                detail: api.error(handle),
            });
        }
    }
    Ok(())
}

#[allow(unsafe_code)]
mod ffi {
    use super::SndFileError;
    use libloading::Library;
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::ptr::NonNull;

    pub const SFM_READ: c_int = 0x10;
    pub const SFM_WRITE: c_int = 0x20;
    pub const SF_FORMAT_SUBMASK: c_int = 0x0000_ffff;
    pub const SF_FORMAT_WAV: c_int = 0x0001_0000;
    pub const SF_FORMAT_AIFF: c_int = 0x0002_0000;
    pub const SF_FORMAT_FLAC: c_int = 0x0017_0000;
    pub const SF_FORMAT_PCM_S8: c_int = 0x0001;
    pub const SF_FORMAT_PCM_16: c_int = 0x0002;
    pub const SF_FORMAT_PCM_24: c_int = 0x0003;
    pub const SF_FORMAT_PCM_32: c_int = 0x0004;
    pub const SF_FORMAT_PCM_U8: c_int = 0x0005;
    pub const SF_STR_TITLE: c_int = 0x01;
    pub const SF_STR_COPYRIGHT: c_int = 0x02;
    pub const SF_STR_SOFTWARE: c_int = 0x03;
    pub const SF_STR_ARTIST: c_int = 0x04;
    pub const SF_STR_COMMENT: c_int = 0x05;
    pub const SF_STR_DATE: c_int = 0x06;
    pub const SF_STR_ALBUM: c_int = 0x07;
    pub const SF_STR_LICENSE: c_int = 0x08;
    pub const SF_STR_TRACKNUMBER: c_int = 0x09;
    pub const SF_STR_GENRE: c_int = 0x10;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct SfInfo {
        pub frames: i64,
        pub sample_rate: c_int,
        pub channels: c_int,
        pub format: c_int,
        pub sections: c_int,
        pub seekable: c_int,
    }

    #[derive(Clone, Copy)]
    pub struct Handle(NonNull<c_void>);

    #[cfg(not(windows))]
    type SfOpen = unsafe extern "C" fn(*const c_char, c_int, *mut SfInfo) -> *mut c_void;
    #[cfg(windows)]
    type SfWcharOpen = unsafe extern "C" fn(*const u16, c_int, *mut SfInfo) -> *mut c_void;
    type SfClose = unsafe extern "C" fn(*mut c_void) -> c_int;
    type SfStrError = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    type SfError = unsafe extern "C" fn(*mut c_void) -> c_int;
    type SfErrorNumber = unsafe extern "C" fn(c_int) -> *const c_char;
    type SfReadfInt = unsafe extern "C" fn(*mut c_void, *mut c_int, i64) -> i64;
    type SfWritefInt = unsafe extern "C" fn(*mut c_void, *const c_int, i64) -> i64;
    type SfGetString = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;
    type SfSetString = unsafe extern "C" fn(*mut c_void, c_int, *const c_char) -> c_int;
    type SfFormatCheck = unsafe extern "C" fn(*const SfInfo) -> c_int;
    type SfVersionString = unsafe extern "C" fn() -> *const c_char;

    pub struct Api {
        _library: Library,
        #[cfg(not(windows))]
        open: SfOpen,
        #[cfg(windows)]
        wchar_open: SfWcharOpen,
        close: SfClose,
        strerror: SfStrError,
        error_code: SfError,
        error_number: SfErrorNumber,
        readf_int: SfReadfInt,
        writef_int: SfWritefInt,
        get_string: SfGetString,
        set_string: SfSetString,
        format_check: SfFormatCheck,
        version_string: SfVersionString,
    }

    impl Api {
        pub fn load() -> Result<Self, SndFileError> {
            let mut details = Vec::new();
            for candidate in library_candidates() {
                // SAFETY: loading a library is unsafe because its initializers
                // may run. These are canonical platform-specific libsndfile
                // names selected by this backend, not user-provided paths.
                match unsafe { Library::new(candidate) } {
                    Ok(library) => return Self::from_library(library),
                    Err(error) => details.push(format!("{candidate}: {error}")),
                }
            }
            Err(SndFileError::LibraryUnavailable(details.join("; ")))
        }

        fn from_library(library: Library) -> Result<Self, SndFileError> {
            macro_rules! symbol {
                ($name:literal, $ty:ty) => {{
                    // SAFETY: each requested symbol name and signature matches
                    // the stable libsndfile 1.x C API. Function pointers are
                    // copied while `library` remains owned by `Api`.
                    unsafe { library.get::<$ty>(concat!($name, "\0").as_bytes()) }
                        .map(|symbol| *symbol)
                        .map_err(|error| SndFileError::MissingSymbol {
                            name: $name,
                            detail: error.to_string(),
                        })?
                }};
            }
            #[cfg(not(windows))]
            let open = symbol!("sf_open", SfOpen);
            #[cfg(windows)]
            let wchar_open = symbol!("sf_wchar_open", SfWcharOpen);
            let close = symbol!("sf_close", SfClose);
            let strerror = symbol!("sf_strerror", SfStrError);
            let error_code = symbol!("sf_error", SfError);
            let error_number = symbol!("sf_error_number", SfErrorNumber);
            let readf_int = symbol!("sf_readf_int", SfReadfInt);
            let writef_int = symbol!("sf_writef_int", SfWritefInt);
            let get_string = symbol!("sf_get_string", SfGetString);
            let set_string = symbol!("sf_set_string", SfSetString);
            let format_check = symbol!("sf_format_check", SfFormatCheck);
            let version_string = symbol!("sf_version_string", SfVersionString);
            Ok(Self {
                _library: library,
                #[cfg(not(windows))]
                open,
                #[cfg(windows)]
                wchar_open,
                close,
                strerror,
                error_code,
                error_number,
                readf_int,
                writef_int,
                get_string,
                set_string,
                format_check,
                version_string,
            })
        }

        #[cfg(not(windows))]
        pub fn open(
            &self,
            path: &CStr,
            mode: c_int,
            mut info: SfInfo,
        ) -> Result<(Handle, SfInfo), SndFileError> {
            // SAFETY: `path` is NUL-terminated, `info` is a valid writable C
            // layout, and the function pointer is loaded from libsndfile.
            let raw = unsafe { (self.open)(path.as_ptr(), mode, &mut info) };
            let handle = NonNull::new(raw)
                .map(Handle)
                .ok_or_else(|| SndFileError::Open(self.error_raw(raw)))?;
            Ok((handle, info))
        }

        #[cfg(windows)]
        pub fn open_wide(
            &self,
            path: &[u16],
            mode: c_int,
            mut info: SfInfo,
        ) -> Result<(Handle, SfInfo), SndFileError> {
            debug_assert_eq!(path.last(), Some(&0));
            // SAFETY: `path` is NUL-terminated UTF-16, `info` has the required
            // writable C layout, and this Windows-only symbol was loaded from
            // libsndfile before `Api` construction completed.
            let raw = unsafe { (self.wchar_open)(path.as_ptr(), mode, &mut info) };
            let handle = NonNull::new(raw)
                .map(Handle)
                .ok_or_else(|| SndFileError::Open(self.error_raw(raw)))?;
            Ok((handle, info))
        }

        pub fn close(&self, handle: Handle) -> Result<(), SndFileError> {
            // SAFETY: `handle` originated from a successful `sf_open` and is
            // consumed here, so this call closes it at most once.
            let result = unsafe { (self.close)(handle.0.as_ptr()) };
            if result == 0 {
                Ok(())
            } else {
                // The handle is closed; do not inspect it or report its stale
                // pre-close status. Resolve the actual returned error code.
                Err(SndFileError::Close(self.error_for_code(result)))
            }
        }

        pub fn read_frames_i32(
            &self,
            handle: Handle,
            output: &mut [i32],
            frames: i64,
        ) -> Result<i64, SndFileError> {
            if frames < 0 {
                return Err(SndFileError::FrameCountOverflow);
            }
            // SAFETY: the outer layer sizes `output` for frames*channels;
            // libsndfile writes no more than the requested frame count.
            let count = unsafe { (self.readf_int)(handle.0.as_ptr(), output.as_mut_ptr(), frames) };
            // SAFETY: querying the error on the same still-open handle is part
            // of the stable C API. A short/zero read alone does not prove EOF.
            let error = unsafe { (self.error_code)(handle.0.as_ptr()) };
            if error != 0 {
                Err(SndFileError::Read(self.error(handle)))
            } else {
                Ok(count)
            }
        }

        pub fn write_frames_i32(
            &self,
            handle: Handle,
            input: &[i32],
            frames: i64,
        ) -> Result<i64, SndFileError> {
            if frames < 0 {
                return Err(SndFileError::FrameCountOverflow);
            }
            // SAFETY: the outer layer provides frames*channels initialized
            // samples and libsndfile reads no more than that count.
            let count = unsafe { (self.writef_int)(handle.0.as_ptr(), input.as_ptr(), frames) };
            // SAFETY: the same live handle remains owned by the writer.
            let error = unsafe { (self.error_code)(handle.0.as_ptr()) };
            if error != 0 {
                Err(SndFileError::Write(self.error(handle)))
            } else {
                Ok(count)
            }
        }

        pub fn get_string(&self, handle: Handle, key: c_int) -> Option<&CStr> {
            // SAFETY: libsndfile owns the returned NUL-terminated string for
            // the lifetime of the still-open handle.
            let raw = unsafe { (self.get_string)(handle.0.as_ptr(), key) };
            (!raw.is_null()).then(|| unsafe { CStr::from_ptr(raw) })
        }

        pub fn set_string(&self, handle: Handle, key: c_int, value: &CStr) -> bool {
            // SAFETY: handle is open and value is a valid NUL-terminated C
            // string. libsndfile copies metadata into its own state.
            unsafe { (self.set_string)(handle.0.as_ptr(), key, value.as_ptr()) == 0 }
        }

        pub fn format_check(&self, info: &SfInfo) -> bool {
            // SAFETY: `info` has the exact C layout required by libsndfile.
            unsafe { (self.format_check)(info) != 0 }
        }

        pub fn version(&self) -> Result<String, SndFileError> {
            // SAFETY: this function has no arguments and returns a
            // library-owned, immutable C string for the loaded library's
            // lifetime.
            let raw = unsafe { (self.version_string)() };
            if raw.is_null() {
                return Err(SndFileError::VersionStringUnavailable);
            }
            // SAFETY: a non-null sf_version_string result is NUL-terminated
            // and owned by the still-loaded library.
            unsafe { CStr::from_ptr(raw) }
                .to_str()
                .map(str::to_owned)
                .map_err(|_| SndFileError::VersionStringIsNotUtf8)
        }

        pub fn error(&self, handle: Handle) -> String {
            self.error_raw(handle.0.as_ptr())
        }

        pub fn error_for_code(&self, code: c_int) -> String {
            // SAFETY: sf_error_number accepts a code without a live handle and
            // returns a library-owned C string (or a null pointer on failure).
            let raw = unsafe { (self.error_number)(code) };
            if raw.is_null() {
                format!("libsndfile error {code}")
            } else {
                // SAFETY: a non-null sf_error_number result is a C string.
                unsafe { CStr::from_ptr(raw) }
                    .to_string_lossy()
                    .into_owned()
            }
        }

        fn error_raw(&self, handle: *mut c_void) -> String {
            // SAFETY: sf_strerror accepts either an open handle or NULL and
            // returns a library-owned NUL-terminated diagnostic.
            let raw = unsafe { (self.strerror)(handle) };
            if raw.is_null() {
                "unknown libsndfile error".to_owned()
            } else {
                // SAFETY: non-null sf_strerror results are C strings.
                unsafe { CStr::from_ptr(raw) }
                    .to_string_lossy()
                    .into_owned()
            }
        }
    }

    #[cfg(windows)]
    pub const fn library_candidates() -> &'static [&'static str] {
        &["sndfile.dll", "libsndfile-1.dll"]
    }

    #[cfg(target_os = "macos")]
    pub const fn library_candidates() -> &'static [&'static str] {
        &["libsndfile.1.dylib", "libsndfile.dylib"]
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    pub const fn library_candidates() -> &'static [&'static str] {
        &["libsndfile.so.1", "libsndfile.so"]
    }

    #[cfg(test)]
    mod status_tests {
        use super::*;
        use std::sync::Arc;

        unsafe extern "C" fn full_write(_: *mut c_void, _: *const c_int, frames: i64) -> i64 {
            frames
        }
        unsafe extern "C" fn failure(_: *mut c_void) -> c_int {
            2
        }
        unsafe extern "C" fn success(_: *mut c_void) -> c_int {
            0
        }
        unsafe extern "C" fn stale_error(_: *mut c_void) -> *const c_char {
            c"pre-close status".as_ptr()
        }

        fn test_api() -> Option<Api> {
            let library = crate::tests::loaded_library()?;
            Some(
                Arc::try_unwrap(library.api)
                    .ok()
                    .expect("test owns the only API reference"),
            )
        }

        #[test]
        fn full_write_count_cannot_override_nonzero_error_status() {
            let Some(mut api) = test_api() else {
                return;
            };
            api.writef_int = full_write;
            api.error_code = failure;
            api.strerror = stale_error;
            // These replaced test callbacks never dereference the handle. It
            // must not be passed to any real handle-taking library function.
            let handle = Handle(NonNull::dangling());
            for frames in [0, 1, 7] {
                assert!(matches!(api.write_frames_i32(handle, &[0; 7], frames),
                    Err(SndFileError::Write(detail)) if detail == "pre-close status"));
            }
            api.error_code = success;
            assert_eq!(api.write_frames_i32(handle, &[0; 7], 7).unwrap(), 7);
        }

        #[test]
        fn close_error_uses_returned_code_not_stale_handle_status() {
            let Some(mut api) = test_api() else {
                return;
            };
            api.close = failure;
            api.strerror = stale_error;
            // Close is a non-dereferencing callback; error_number is the real
            // handle-independent libsndfile function, queried with SF_ERR_SYSTEM.
            let handle = Handle(NonNull::dangling());
            let expected = api.error_for_code(2);
            assert!(!expected.is_empty());
            assert_ne!(expected, "pre-close status");
            assert!(
                matches!(api.close(handle), Err(SndFileError::Close(detail)) if detail == expected)
            );
            api.close = success;
            api.close(handle).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn temporary_audio_path(extension: &str) -> PathBuf {
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "sexio-sndfile-{}-{sequence}.{extension}",
            std::process::id()
        ))
    }

    pub(super) fn loaded_library() -> Option<SndFileLibrary> {
        match SndFileLibrary::load() {
            Ok(library) => Some(library),
            Err(SndFileError::LibraryUnavailable(detail)) => {
                assert!(
                    std::env::var("SEX_REQUIRE_SNDFILE_TESTS").as_deref() != Ok("1"),
                    "required libsndfile test runtime is unavailable: {detail}"
                );
                eprintln!(
                    "SKIPPED runtime-dependent libsndfile assertions: library unavailable; set SEX_REQUIRE_SNDFILE_TESTS=1 to require them"
                );
                None
            }
            Err(error) => panic!("unexpected libsndfile loading error: {error}"),
        }
    }

    #[test]
    fn runtime_library_candidates_match_the_target_family() {
        let candidates = ffi::library_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|candidate| !candidate.is_empty()));
        #[cfg(windows)]
        assert_eq!(candidates, ["sndfile.dll", "libsndfile-1.dll"]);
        #[cfg(target_os = "macos")]
        assert_eq!(candidates, ["libsndfile.1.dylib", "libsndfile.dylib"]);
        #[cfg(all(not(windows), not(target_os = "macos")))]
        assert_eq!(candidates, ["libsndfile.so.1", "libsndfile.so"]);

        if let Some(library) = loaded_library() {
            let version = library.version().unwrap();
            assert!(!version.is_empty());
            assert!(version.to_ascii_lowercase().contains("libsndfile"));
        }
    }

    fn round_trip(container: ContainerFormat, extension: &str, bits: u16) {
        let Some(library) = loaded_library() else {
            return;
        };
        let path = temporary_audio_path(extension);
        let spec = AudioSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: bits,
        };
        let maximum = (1_i64 << (bits - 1)) - 1;
        let minimum = -(1_i64 << (bits - 1));
        let pcm = [minimum, maximum, -1, 0, 1, maximum / 3];
        let report = {
            let mut writer = library
                .create_as(&path, spec, container, &AudioMetadata::default())
                .unwrap();
            writer.write_signed_pcm_frames(&pcm).unwrap();
            writer.finalize().unwrap()
        };
        assert_eq!(report.frames, 3);
        let mut reader = library.open(&path).unwrap();
        assert_eq!(reader.spec(), spec);
        let first = reader.read_frames(1).unwrap();
        let second = reader.read_frames(99).unwrap();
        assert!(reader.is_eof());
        let output = first
            .samples
            .into_iter()
            .chain(second.samples)
            .collect::<Vec<_>>();
        let expected = pcm
            .iter()
            .map(|sample| Q1_63::from_signed_pcm(*sample, u32::from(bits)).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(output, expected);
        drop(reader);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn wave_aiff_and_flac_integer_pcm_round_trip_exactly() {
        round_trip(ContainerFormat::Wave, "wav", 32);
        round_trip(ContainerFormat::Aiff, "aiff", 16);
        round_trip(ContainerFormat::Flac, "flac", 24);
    }

    #[test]
    fn oversized_read_of_tiny_wave_aiff_and_flac_is_bounded_and_exact() {
        let Some(library) = loaded_library() else {
            return;
        };
        for (container, extension, bits) in [
            (ContainerFormat::Wave, "wav", 32),
            (ContainerFormat::Aiff, "aiff", 16),
            (ContainerFormat::Flac, "flac", 24),
        ] {
            let path = temporary_audio_path(extension);
            let spec = AudioSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: bits,
            };
            let mut writer = library
                .create_as(&path, spec, container, &AudioMetadata::default())
                .unwrap();
            writer.write_signed_pcm_frames(&[-1, 1]).unwrap();
            writer.finalize().unwrap();
            let mut reader = library.open(&path).unwrap();
            let chunk = reader.read_frames(usize::MAX / 2).unwrap();
            assert_eq!(chunk.frames, 1);
            assert_eq!(
                chunk.samples,
                [-1, 1].map(|code| Q1_63::from_raw(code << (64 - bits)))
            );
            assert!(reader.is_eof());
            assert!(chunk.samples.capacity() <= 8192);
            drop(reader);
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn bounded_native_scratch_preserves_multiblock_pcm_and_eof() {
        for channels in [1_u16, 3, 9, u16::MAX] {
            let mut state = ReaderState::default();
            let mut remaining = 20_003;
            let mut sample_index = 0_i64;
            let mut calls = 0;
            // For the huge-channel case use two actual frames, not a huge fixture.
            if channels == u16::MAX {
                remaining = 2;
            }
            let total_frames = remaining;
            let chunk = state
                .read(
                    usize::MAX / usize::from(channels),
                    channels,
                    |output, frames| {
                        calls += 1;
                        assert!(output.len() <= 8192.max(usize::from(channels)));
                        assert_eq!(output.len(), frames as usize * usize::from(channels));
                        let count = remaining.min(frames as usize);
                        for sample in &mut output[..count * usize::from(channels)] {
                            *sample = sample_index as i32 - 90_000;
                            sample_index += 1;
                        }
                        remaining -= count;
                        Ok(count as i64)
                    },
                )
                .unwrap();
            assert!(calls > 1);
            assert_eq!(chunk.frames, total_frames);
            for (index, sample) in chunk.samples.iter().enumerate() {
                assert_eq!(sample.raw(), (index as i64 - 90_000) << 32);
            }
            assert!(state.eof);
            assert_eq!(
                state
                    .read(1, channels, |_, _| panic!("read after EOF"))
                    .unwrap()
                    .frames,
                0
            );
        }
    }

    #[test]
    fn decoder_errors_discard_partial_chunks_and_poison_reader_state() {
        for error_count in [-1, 8193] {
            let mut state = ReaderState::default();
            let mut calls = 0;
            let result = state.read(20_000, 1, |_, frames| {
                calls += 1;
                if calls == 1 {
                    Ok(frames)
                } else {
                    Ok(error_count)
                }
            });
            assert!(matches!(result, Err(SndFileError::Read(_))));
            assert_eq!(calls, 2);
            assert!(!state.eof);
            for frames in [0, 1, usize::MAX] {
                assert!(matches!(
                    state.read(frames, 1, |_, _| panic!("continued failed decoder")),
                    Err(SndFileError::Audio(AudioIoError::ReaderFailed))
                ));
            }
        }
        let mut state = ReaderState::default();
        assert!(
            matches!(state.read(1, 1, |_, _| Err(SndFileError::Read("decoder error".to_owned()))),
            Err(SndFileError::Read(error)) if error == "decoder error")
        );
        assert!(state.failed);
        assert!(!state.eof);
    }

    #[test]
    fn reader_preflight_overflow_and_zero_requests_do_not_consume_or_poison() {
        let mut state = ReaderState::default();
        assert_eq!(
            state
                .read(0, 2, |_, _| panic!("read for zero request"))
                .unwrap()
                .frames,
            0
        );
        assert!(matches!(
            state.read(usize::MAX, 2, |_, _| panic!(
                "read before overflow rejection"
            )),
            Err(SndFileError::FrameCountOverflow)
        ));
        assert!(!state.failed);
        assert!(!state.eof);
        assert_eq!(state.read(1, 2, |_, frames| Ok(frames)).unwrap().frames, 1);
    }

    #[test]
    fn libsndfile_error_status_is_not_mistaken_for_successful_eof() {
        let Some(library) = loaded_library() else {
            return;
        };
        let path = temporary_audio_path("wav");
        let spec = AudioSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
        };
        let mut writer = library
            .create_as(
                &path,
                spec,
                ContainerFormat::Wave,
                &AudioMetadata::default(),
            )
            .unwrap();
        // Transfer a real write-only handle into the private reader test setup.
        // The first read must surface libsndfile's mode error, not report EOF.
        // Ownership is transferred, never duplicated; only the reader closes it.
        let mut reader = SndFileReader {
            api: Arc::clone(&writer.api),
            handle: writer.handle.take(),
            spec,
            metadata: AudioMetadata::default(),
            state: ReaderState::default(),
        };
        assert!(matches!(reader.read_frames(1), Err(SndFileError::Read(_))));
        assert!(!reader.is_eof());
        assert!(matches!(
            reader.read_frames(1),
            Err(SndFileError::Audio(AudioIoError::ReaderFailed))
        ));
        drop(reader);
        drop(writer);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn flac_metadata_round_trips_as_typed_utf8() {
        let Some(library) = loaded_library() else {
            return;
        };
        let path = temporary_audio_path("flac");
        let metadata = AudioMetadata::new(vec![
            MetadataEntry {
                key: MetadataKey::Title,
                value: "Deterministic test".to_owned(),
            },
            MetadataEntry {
                key: MetadataKey::Artist,
                value: "SeX".to_owned(),
            },
        ]);
        let mut writer = library
            .create(
                &path,
                AudioSpec {
                    channels: 1,
                    sample_rate: 44_100,
                    bits_per_sample: 24,
                },
                &metadata,
            )
            .unwrap();
        writer.write_signed_pcm_frames(&[0]).unwrap();
        writer.finalize().unwrap();

        let reader = library.open(&path).unwrap();
        assert_eq!(
            reader.metadata().get(MetadataKey::Title),
            Some("Deterministic test")
        );
        assert_eq!(reader.metadata().get(MetadataKey::Artist), Some("SeX"));
        drop(reader);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_output_contracts_fail_before_writing_samples() {
        assert_eq!(ContainerFormat::from_path(Path::new("track.unknown")), None);
        assert!(matches!(
            output_format(ContainerFormat::Flac, 32),
            Err(SndFileError::UnsupportedOutputWidth {
                container: ContainerFormat::Flac,
                bits: 32
            })
        ));
        let duplicate = AudioMetadata::new(vec![
            MetadataEntry {
                key: MetadataKey::Title,
                value: "one".to_owned(),
            },
            MetadataEntry {
                key: MetadataKey::Title,
                value: "two".to_owned(),
            },
        ]);
        assert!(matches!(
            validate_metadata(&duplicate),
            Err(SndFileError::DuplicateMetadataKey(MetadataKey::Title))
        ));
    }

    #[test]
    fn writer_poisoning_prevents_continuation_after_partial_frame() {
        let Some(library) = loaded_library() else {
            return;
        };
        let path = temporary_audio_path("wav");
        let mut writer = library
            .create(
                &path,
                AudioSpec {
                    channels: 2,
                    sample_rate: 48_000,
                    bits_per_sample: 16,
                },
                &AudioMetadata::default(),
            )
            .unwrap();
        assert!(matches!(
            writer.write_signed_pcm_frames(&[0]),
            Err(SndFileError::PartialFrame { .. })
        ));
        assert!(matches!(
            writer.write_signed_pcm_frames(&[0, 0]),
            Err(SndFileError::Poisoned)
        ));
        drop(writer);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn writer_counter_overflow_precedes_native_write_and_preserves_entire_report() {
        let Some(library) = loaded_library() else {
            return;
        };
        for saturated in [false, true] {
            let path = temporary_audio_path("wav");
            let spec = AudioSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 16,
            };
            let mut writer = library
                .create(&path, spec, &AudioMetadata::default())
                .unwrap();
            writer.write_signed_pcm_frames(&[1, -1]).unwrap();
            if saturated {
                writer.report.saturated_samples = u64::MAX;
            } else {
                writer.report.frames = u64::MAX;
            }
            let before = writer.report();
            let result = if saturated {
                writer.write_frames(
                    &[Q1_63::MAX, Q1_63::ZERO],
                    RoundingMode::NearestTiesToEven,
                    OverflowPolicy::Saturate,
                )
            } else {
                writer.write_signed_pcm_frames(&[2, -2])
            };
            assert!(matches!(result, Err(SndFileError::FrameCountOverflow)));
            assert_eq!(writer.report(), before);
            assert!(matches!(
                writer.write_signed_pcm_frames(&[]),
                Err(SndFileError::Poisoned)
            ));
            assert!(matches!(writer.finalize(), Err(SndFileError::Poisoned)));
            let mut reader = library.open(&path).unwrap();
            let chunk = reader.read_frames(8).unwrap();
            assert_eq!(chunk.frames, 1);
            assert_eq!(
                chunk.samples,
                [1, -1].map(|sample| Q1_63::from_raw(sample << 48))
            );
            drop(reader);
            fs::remove_file(path).unwrap();
        }
    }
}
