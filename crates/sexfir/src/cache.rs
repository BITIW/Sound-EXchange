//! Versioned, validated, atomic on-disk coefficient cache.

mod optimized_cache;
pub use optimized_cache::{design_optimized_big_cached, refine_optimized_quantization_cached};

use super::*;
use rug::integer::Order;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAGIC: &[u8; 8] = b"SEXCFIR1";
// Kinds 1..=3 are legacy entries without whole-payload integrity. Keep the
// optimized kind 4 and its already-hashed format unchanged.
const NATIVE_KIND: u8 = 5;
const BIG_KIND: u8 = 6;
const WINDOWED_BIG_KIND: u8 = 7;
const MAX_CACHE_STRING_BYTES: usize = 4_096;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheStatus {
    Hit,
    DesignedAndStored,
}

#[derive(Debug)]
pub enum CacheError {
    Io(io::Error),
    Design(DesignError),
    Optimized(optimized::Error),
    Corrupt(&'static str),
    InvalidUtf8,
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Design(error) => error.fmt(f),
            Self::Optimized(error) => error.fmt(f),
            Self::Corrupt(reason) => write!(f, "coefficient cache entry is corrupt: {reason}"),
            Self::InvalidUtf8 => f.write_str("coefficient cache contains invalid UTF-8"),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Design(error) => Some(error),
            Self::Optimized(error) => Some(error),
            Self::Corrupt(_) | Self::InvalidUtf8 => None,
        }
    }
}

impl From<io::Error> for CacheError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<DesignError> for CacheError {
    fn from(value: DesignError) -> Self {
        Self::Design(value)
    }
}

pub fn design_kaiser_cached(
    directory: &Path,
    spec: &KaiserSpec,
) -> Result<(DesignedFilter, CacheStatus), CacheError> {
    spec.validate()?;
    let key = native_request_identity(spec);
    let path = cache_path(directory, "native", &key);
    if path.try_exists()? {
        return Ok((load_native(&path, spec, &key)?, CacheStatus::Hit));
    }
    let designed = design_kaiser(spec)?;
    store_native(&path, &key, &designed)?;
    Ok((designed, CacheStatus::DesignedAndStored))
}

pub fn design_kaiser_big_cached(
    directory: &Path,
    spec: &BigKaiserSpec,
) -> Result<(DesignedBigFilter, CacheStatus), CacheError> {
    spec.validate()?;
    let key = big_request_identity(spec);
    let path = cache_path(directory, "big", &key);
    if path.try_exists()? {
        return Ok((load_big(&path, spec, &key)?, CacheStatus::Hit));
    }
    let designed = design_kaiser_big(spec)?;
    store_big(&path, &key, &designed)?;
    Ok((designed, CacheStatus::DesignedAndStored))
}

fn cache_path(directory: &Path, backend: &str, key: &str) -> PathBuf {
    directory.join(format!("{backend}-{key}.sexfir"))
}

fn native_request_identity(spec: &KaiserSpec) -> String {
    request_identity(b"sex/sexfir/cache-request-native-v2\0", spec, 62, 128)
}

fn big_request_identity(spec: &BigKaiserSpec) -> String {
    request_identity(
        b"sex/sexfir/cache-request-big-v2\0",
        &spec.core,
        spec.coefficient_fractional_bits,
        spec.accumulator_bits,
    )
}

fn request_identity(
    domain: &[u8],
    spec: &KaiserSpec,
    coefficient_fractional_bits: u32,
    accumulator_bits: u32,
) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(spec.ratio.up().to_le_bytes());
    hash.update(spec.ratio.down().to_le_bytes());
    hash.update((spec.taps_per_phase as u64).to_le_bytes());
    hash.update(spec.rolloff.numerator.to_le_bytes());
    hash.update(spec.rolloff.denominator.to_le_bytes());
    hash.update(spec.beta.numerator.to_le_bytes());
    hash.update(spec.beta.denominator.to_le_bytes());
    hash.update(spec.working_precision_bits.to_le_bytes());
    hash.update([rounding_code(spec.quantization_rounding)]);
    hash.update(coefficient_fractional_bits.to_le_bytes());
    hash.update(accumulator_bits.to_le_bytes());
    encode_sha256(hash)
}

const fn rounding_code(mode: RoundingMode) -> u8 {
    match mode {
        RoundingMode::TowardZero => 0,
        RoundingMode::Floor => 1,
        RoundingMode::Ceiling => 2,
        RoundingMode::NearestTiesToEven => 3,
        RoundingMode::NearestTiesAwayFromZero => 4,
    }
}

fn store_native(path: &Path, key: &str, designed: &DesignedFilter) -> Result<(), CacheError> {
    atomic_write_hashed(path, |writer| {
        write_header(writer, NATIVE_KIND, key)?;
        write_u64(writer, designed.report.coefficient_count as u64)?;
        write_u64(writer, designed.report.max_dc_correction_raw)?;
        write_string(writer, &designed.report.max_abs_coefficient_error_decimal)?;
        write_string(writer, &designed.report.max_abs_coefficient_error_db)?;
        write_string(writer, &designed.report.max_phase_l1_error_decimal)?;
        write_string(writer, &designed.report.max_phase_l1_error_db)?;
        write_u16(writer, designed.report.required_accumulator_bits)?;
        write_string(writer, &designed.report.coefficient_sha256)?;
        for phase in 0..designed.bank.phase_count() {
            for coefficient in designed
                .bank
                .phase(phase as u64)
                .map_err(DesignError::from)?
            {
                writer.write_all(&coefficient.raw().to_le_bytes())?;
            }
        }
        Ok(())
    })
}

fn load_native(path: &Path, spec: &KaiserSpec, key: &str) -> Result<DesignedFilter, CacheError> {
    let file = File::open(path)?;
    let mut reader = Hashed::new(BufReader::new(file));
    read_header(&mut reader, NATIVE_KIND, key)?;
    let coefficient_count = expected_coefficient_count(spec.ratio, spec.taps_per_phase)?;
    if read_u64(&mut reader)? != coefficient_count as u64 {
        return Err(CacheError::Corrupt("coefficient count mismatch"));
    }
    let max_dc_correction_raw = read_u64(&mut reader)?;
    let max_abs_coefficient_error_decimal = read_string(&mut reader)?;
    let max_abs_coefficient_error_db = read_string(&mut reader)?;
    let max_phase_l1_error_decimal = read_string(&mut reader)?;
    let max_phase_l1_error_db = read_string(&mut reader)?;
    let stored_accumulator_bits = read_u16(&mut reader)?;
    let stored_sha256 = read_string(&mut reader)?;
    let mut coefficients = Vec::with_capacity(coefficient_count);
    for _ in 0..coefficient_count {
        let mut raw = [0_u8; 8];
        reader.read_exact(&mut raw)?;
        coefficients.push(Q2_62::from_raw(i64::from_le_bytes(raw)));
    }
    verify_payload(reader)?;
    let delay = u64::try_from((spec.taps_per_phase - 1) / 2)
        .map_err(|_| CacheError::Corrupt("delay does not fit u64"))?;
    let calculated_sha256 = coefficient_identity(spec, delay, &coefficients);
    if stored_sha256 != calculated_sha256 {
        return Err(CacheError::Corrupt("coefficient identity mismatch"));
    }
    let bank = PolyphaseFirQ63::for_ratio(spec.ratio, spec.taps_per_phase, coefficients)
        .map_err(DesignError::from)?;
    if bank.required_accumulator_bits() != stored_accumulator_bits {
        return Err(CacheError::Corrupt("accumulator bound mismatch"));
    }
    Ok(DesignedFilter {
        spec: spec.clone(),
        ratio: spec.ratio,
        input_delay_frames: delay,
        bank,
        report: QuantizationReport {
            algorithm: "kaiser-windowed-sinc-v1",
            working_precision_bits: spec.working_precision_bits,
            coefficient_fractional_bits: Q2_62::FRACTIONAL_BITS,
            coefficient_count,
            max_dc_correction_raw,
            max_abs_coefficient_error_decimal,
            max_abs_coefficient_error_db,
            max_phase_l1_error_decimal,
            max_phase_l1_error_db,
            required_accumulator_bits: stored_accumulator_bits,
            coefficient_sha256: stored_sha256,
        },
    })
}

fn store_big(path: &Path, key: &str, designed: &DesignedBigFilter) -> Result<(), CacheError> {
    store_big_bank(path, key, BIG_KIND, &designed.bank, &designed.report)
}

fn store_big_bank(
    path: &Path,
    key: &str,
    kind: u8,
    bank: &PolyphaseFirBigQ63,
    report: &BigQuantizationReport,
) -> Result<(), CacheError> {
    atomic_write_hashed(path, |writer| {
        write_header(writer, kind, key)?;
        write_u64(writer, report.coefficient_count as u64)?;
        write_string(writer, &report.max_dc_correction_raw)?;
        write_string(writer, &report.max_abs_coefficient_error_decimal)?;
        write_string(writer, &report.max_abs_coefficient_error_db)?;
        write_string(writer, &report.max_phase_l1_error_decimal)?;
        write_string(writer, &report.max_phase_l1_error_db)?;
        write_u32(writer, report.required_accumulator_bits)?;
        write_string(writer, &report.coefficient_sha256)?;
        let bytes_per_coefficient =
            usize::try_from(bank.coefficient_format().total_bits().div_ceil(8))
                .map_err(|_| CacheError::Corrupt("coefficient storage width is too large"))?;
        write_u32(
            writer,
            u32::try_from(bytes_per_coefficient)
                .map_err(|_| CacheError::Corrupt("coefficient storage width is too large"))?,
        )?;
        for phase in 0..bank.phase_count() {
            for coefficient in bank.phase(phase as u64).map_err(DesignError::from)? {
                let mut bytes = coefficient
                    .to_twos_complement_bits()
                    .to_digits::<u8>(Order::Lsf);
                bytes.resize(bytes_per_coefficient, 0);
                writer.write_all(&bytes)?;
            }
        }
        Ok(())
    })
}

/// Window and width have separate versioned identities and full payload integrity.
pub fn design_windowed_big_cached(
    directory: &Path,
    spec: &windowed::Spec,
) -> Result<(windowed::Designed, CacheStatus), CacheError> {
    spec.preflight()?;
    let key = encode_sha256(windowed::spec_hash(
        spec,
        b"sex/sexfir/windowed-big-request-v2\0",
    ));
    let path = cache_path(directory, "windowed-big", &key);
    if path.try_exists()? {
        return load_windowed_big(&path, spec, &key).map(|bank| (bank, CacheStatus::Hit));
    }
    let designed = windowed::design(spec)?;
    store_big_bank(
        &path,
        &key,
        WINDOWED_BIG_KIND,
        &designed.bank,
        &designed.report,
    )?;
    Ok((designed, CacheStatus::DesignedAndStored))
}

fn load_windowed_big(
    path: &Path,
    spec: &windowed::Spec,
    key: &str,
) -> Result<windowed::Designed, CacheError> {
    let count = spec.preflight()?;
    let mut reader = Hashed::new(BufReader::new(File::open(path)?));
    read_header(&mut reader, WINDOWED_BIG_KIND, key)?;
    if read_u64(&mut reader)? != count as u64 {
        return Err(CacheError::Corrupt("coefficient count mismatch"));
    }
    let max_dc_correction_raw = read_string(&mut reader)?;
    let max_abs_coefficient_error_decimal = read_string(&mut reader)?;
    let max_abs_coefficient_error_db = read_string(&mut reader)?;
    let max_phase_l1_error_decimal = read_string(&mut reader)?;
    let max_phase_l1_error_db = read_string(&mut reader)?;
    let required_accumulator_bits = read_u32(&mut reader)?;
    let coefficient_sha256 = read_string(&mut reader)?;
    let format = BigQFormat::new(2, spec.coefficient_fractional_bits).map_err(DesignError::from)?;
    let width = format.total_bits().div_ceil(8) as usize;
    if read_u32(&mut reader)? as usize != width {
        return Err(CacheError::Corrupt("coefficient storage width mismatch"));
    }
    let mut bytes = vec![0; width];
    let mut coefficients = Vec::with_capacity(count);
    for _ in 0..count {
        reader.read_exact(&mut bytes)?;
        let bits = Integer::from_digits(&bytes, Order::Lsf);
        coefficients
            .push(BigQ::from_twos_complement_bits(bits, format).map_err(DesignError::from)?);
    }
    verify_payload(reader)?;
    let delay = (spec.core.taps_per_phase as u64 - 1) / 2;
    if windowed::identity(spec, delay, &coefficients) != coefficient_sha256 {
        return Err(CacheError::Corrupt("coefficient identity mismatch"));
    }
    let unity = Integer::from(1) << spec.coefficient_fractional_bits;
    for phase in coefficients.chunks_exact(spec.core.taps_per_phase) {
        let sum = Integer::from(Integer::sum(phase.iter().map(BigQ::raw)));
        if sum != unity {
            return Err(CacheError::Corrupt("phase DC sum mismatch"));
        }
    }
    let bank = PolyphaseFirBigQ63::for_ratio(
        spec.core.ratio,
        spec.core.taps_per_phase,
        format,
        spec.accumulator_bits,
        coefficients,
    )
    .map_err(DesignError::from)?;
    if bank.required_accumulator_bits() != required_accumulator_bits {
        return Err(CacheError::Corrupt("accumulator bound mismatch"));
    }
    Ok(windowed::Designed {
        spec: spec.clone(),
        input_delay_frames: delay,
        bank,
        report: BigQuantizationReport {
            algorithm: windowed::algorithm(spec.core.window),
            working_precision_bits: spec.core.working_precision_bits,
            coefficient_fractional_bits: spec.coefficient_fractional_bits,
            coefficient_count: count,
            max_dc_correction_raw,
            max_abs_coefficient_error_decimal,
            max_abs_coefficient_error_db,
            max_phase_l1_error_decimal,
            max_phase_l1_error_db,
            configured_accumulator_bits: spec.accumulator_bits,
            required_accumulator_bits,
            coefficient_sha256,
        },
    })
}

fn load_big(path: &Path, spec: &BigKaiserSpec, key: &str) -> Result<DesignedBigFilter, CacheError> {
    let file = File::open(path)?;
    let mut reader = Hashed::new(BufReader::new(file));
    read_header(&mut reader, BIG_KIND, key)?;
    let coefficient_count = expected_coefficient_count(spec.core.ratio, spec.core.taps_per_phase)?;
    if read_u64(&mut reader)? != coefficient_count as u64 {
        return Err(CacheError::Corrupt("coefficient count mismatch"));
    }
    let max_dc_correction_raw = read_string(&mut reader)?;
    let max_abs_coefficient_error_decimal = read_string(&mut reader)?;
    let max_abs_coefficient_error_db = read_string(&mut reader)?;
    let max_phase_l1_error_decimal = read_string(&mut reader)?;
    let max_phase_l1_error_db = read_string(&mut reader)?;
    let stored_accumulator_bits = read_u32(&mut reader)?;
    let stored_sha256 = read_string(&mut reader)?;
    let coefficient_format =
        BigQFormat::new(2, spec.coefficient_fractional_bits).map_err(DesignError::from)?;
    let expected_bytes = usize::try_from(coefficient_format.total_bits().div_ceil(8))
        .map_err(|_| CacheError::Corrupt("coefficient storage width is too large"))?;
    let stored_bytes = usize::try_from(read_u32(&mut reader)?)
        .map_err(|_| CacheError::Corrupt("coefficient storage width is too large"))?;
    if stored_bytes != expected_bytes {
        return Err(CacheError::Corrupt("coefficient storage width mismatch"));
    }
    let mut coefficients = Vec::with_capacity(coefficient_count);
    let mut bytes = vec![0_u8; stored_bytes];
    for _ in 0..coefficient_count {
        reader.read_exact(&mut bytes)?;
        let bits = Integer::from_digits(&bytes, Order::Lsf);
        coefficients.push(
            BigQ::from_twos_complement_bits(bits, coefficient_format).map_err(DesignError::from)?,
        );
    }
    verify_payload(reader)?;
    let delay = u64::try_from((spec.core.taps_per_phase - 1) / 2)
        .map_err(|_| CacheError::Corrupt("delay does not fit u64"))?;
    let calculated_sha256 = coefficient_identity_big(spec, delay, &coefficients);
    if stored_sha256 != calculated_sha256 {
        return Err(CacheError::Corrupt("coefficient identity mismatch"));
    }
    let bank = PolyphaseFirBigQ63::for_ratio(
        spec.core.ratio,
        spec.core.taps_per_phase,
        coefficient_format,
        spec.accumulator_bits,
        coefficients,
    )
    .map_err(DesignError::from)?;
    if bank.required_accumulator_bits() != stored_accumulator_bits {
        return Err(CacheError::Corrupt("accumulator bound mismatch"));
    }
    Ok(DesignedBigFilter {
        spec: spec.clone(),
        input_delay_frames: delay,
        bank,
        report: BigQuantizationReport {
            algorithm: "kaiser-windowed-sinc-big-v1",
            working_precision_bits: spec.core.working_precision_bits,
            coefficient_fractional_bits: spec.coefficient_fractional_bits,
            coefficient_count,
            max_dc_correction_raw,
            max_abs_coefficient_error_decimal,
            max_abs_coefficient_error_db,
            max_phase_l1_error_decimal,
            max_phase_l1_error_db,
            configured_accumulator_bits: spec.accumulator_bits,
            required_accumulator_bits: stored_accumulator_bits,
            coefficient_sha256: stored_sha256,
        },
    })
}

fn expected_coefficient_count(
    ratio: RateRatio,
    taps_per_phase: usize,
) -> Result<usize, CacheError> {
    let phase_count =
        usize::try_from(ratio.up()).map_err(|_| CacheError::Corrupt("phase count is too large"))?;
    phase_count
        .checked_mul(taps_per_phase)
        .ok_or(CacheError::Corrupt("coefficient count overflow"))
}

fn atomic_write<F>(path: &Path, write: F) -> Result<(), CacheError>
where
    F: FnOnce(&mut BufWriter<File>) -> Result<(), CacheError>,
{
    let directory = path
        .parent()
        .ok_or(CacheError::Corrupt("cache path has no parent"))?;
    fs::create_dir_all(directory)?;
    let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(".sex-cache-{}-{sequence}.tmp", std::process::id()));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        write(&mut writer)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        match fs::rename(&temporary, path) {
            Ok(()) => Ok(()),
            Err(_error) if path.exists() => {
                fs::remove_file(&temporary)?;
                Ok(())
            }
            Err(error) => Err(CacheError::Io(error)),
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct Hashed<Stream> {
    inner: Stream,
    digest: Sha256,
}

impl<Stream> Hashed<Stream> {
    fn new(inner: Stream) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
        }
    }
    fn finish(self) -> (Stream, [u8; 32]) {
        (self.inner, self.digest.finalize().into())
    }
}

impl<Stream: Read> Read for Hashed<Stream> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(bytes)?;
        self.digest.update(&bytes[..count]);
        Ok(count)
    }
}

impl<Stream: Write> Write for Hashed<Stream> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.digest.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn atomic_write_hashed<F>(path: &Path, write: F) -> Result<(), CacheError>
where
    F: FnOnce(&mut Hashed<&mut BufWriter<File>>) -> Result<(), CacheError>,
{
    atomic_write(path, |output| {
        let mut writer = Hashed::new(output);
        write(&mut writer)?;
        let (output, digest) = writer.finish();
        output.write_all(&digest)?;
        Ok(())
    })
}

fn verify_payload(reader: Hashed<impl Read>) -> Result<(), CacheError> {
    let (mut reader, digest) = reader.finish();
    let mut stored = [0; 32];
    reader.read_exact(&mut stored)?;
    if digest != stored {
        return Err(CacheError::Corrupt("payload checksum mismatch"));
    }
    ensure_eof(&mut reader)
}

fn write_header(writer: &mut impl Write, kind: u8, key: &str) -> Result<(), CacheError> {
    writer.write_all(MAGIC)?;
    writer.write_all(&[kind])?;
    if key.len() != 64 {
        return Err(CacheError::Corrupt("internal cache key length"));
    }
    writer.write_all(key.as_bytes())?;
    Ok(())
}

fn read_header(reader: &mut impl Read, expected_kind: u8, key: &str) -> Result<(), CacheError> {
    let mut magic = [0_u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(CacheError::Corrupt("bad magic or version"));
    }
    let mut kind = [0_u8; 1];
    reader.read_exact(&mut kind)?;
    if kind[0] != expected_kind {
        return Err(CacheError::Corrupt("backend kind mismatch"));
    }
    let mut stored_key = [0_u8; 64];
    reader.read_exact(&mut stored_key)?;
    if stored_key != key.as_bytes() {
        return Err(CacheError::Corrupt("request identity mismatch"));
    }
    Ok(())
}

fn write_string(writer: &mut impl Write, value: &str) -> Result<(), CacheError> {
    if value.len() > MAX_CACHE_STRING_BYTES {
        return Err(CacheError::Corrupt("report string is too large"));
    }
    write_u32(
        writer,
        u32::try_from(value.len()).map_err(|_| CacheError::Corrupt("string length overflow"))?,
    )?;
    writer.write_all(value.as_bytes())?;
    Ok(())
}

fn read_string(reader: &mut impl Read) -> Result<String, CacheError> {
    let length = usize::try_from(read_u32(reader)?)
        .map_err(|_| CacheError::Corrupt("string length overflow"))?;
    if length > MAX_CACHE_STRING_BYTES {
        return Err(CacheError::Corrupt("report string is too large"));
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| CacheError::InvalidUtf8)
}

fn ensure_eof(reader: &mut impl Read) -> Result<(), CacheError> {
    let mut byte = [0_u8; 1];
    if reader.read(&mut byte)? == 0 {
        Ok(())
    } else {
        Err(CacheError::Corrupt("trailing bytes"))
    }
}

fn write_u16(writer: &mut impl Write, value: u16) -> Result<(), CacheError> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u32(writer: &mut impl Write, value: u32) -> Result<(), CacheError> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u64(writer: &mut impl Write, value: u64) -> Result<(), CacheError> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn read_u16(reader: &mut impl Read) -> Result<u16, CacheError> {
    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(reader: &mut impl Read) -> Result<u32, CacheError> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> Result<u64, CacheError> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "sex-cache-test-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn native_spec() -> KaiserSpec {
        KaiserSpec {
            ratio: RateRatio::from_fraction(3, 2).unwrap(),
            taps_per_phase: 7,
            rolloff: Fraction::new(9, 10).unwrap(),
            beta: Fraction::new(5, 1).unwrap(),
            working_precision_bits: 128,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        }
    }

    #[test]
    fn exact_admission_cache_roundtrip_retains_legacy_report_and_minimum_width() {
        let directory = TestDirectory::new("exact-admission");
        let core = KaiserSpec {
            ratio: RateRatio::from_fraction(1, 1).unwrap(),
            taps_per_phase: 3,
            rolloff: Fraction::new(1, 1).unwrap(),
            ..native_spec()
        };
        let (first, status) = design_kaiser_cached(&directory.0, &core).unwrap();
        assert_eq!(status, CacheStatus::DesignedAndStored);
        assert_eq!(first.bank().minimum_accumulator_bits(), 126);
        assert_eq!(first.report().required_accumulator_bits, 127);
        let (loaded, status) = design_kaiser_cached(&directory.0, &core).unwrap();
        assert_eq!(status, CacheStatus::Hit);
        assert_eq!(first, loaded);

        let spec = BigKaiserSpec {
            core,
            coefficient_fractional_bits: 96,
            accumulator_bits: 160,
        };
        let (first, status) = design_kaiser_big_cached(&directory.0, &spec).unwrap();
        assert_eq!(status, CacheStatus::DesignedAndStored);
        assert_eq!(first.bank().accumulator_bits(), 160);
        assert_eq!(first.bank().minimum_accumulator_bits(), 160);
        assert_eq!(first.bank().minimum_wide_accumulator_bits(), 224);
        assert_eq!(first.report().required_accumulator_bits, 161);
        let (loaded, status) = design_kaiser_big_cached(&directory.0, &spec).unwrap();
        assert_eq!(status, CacheStatus::Hit);
        assert_eq!(first, loaded);
    }

    #[test]
    fn native_cache_round_trips_and_reports_hits() {
        let directory = TestDirectory::new("native");
        let spec = native_spec();
        let (first, status) = design_kaiser_cached(&directory.0, &spec).unwrap();
        assert_eq!(status, CacheStatus::DesignedAndStored);
        let (second, status) = design_kaiser_cached(&directory.0, &spec).unwrap();
        assert_eq!(status, CacheStatus::Hit);
        assert_eq!(first, second);
    }

    #[test]
    fn bigint_cache_preserves_fixed_width_twos_complement_coefficients() {
        let directory = TestDirectory::new("big");
        let spec = BigKaiserSpec {
            core: native_spec(),
            coefficient_fractional_bits: 96,
            accumulator_bits: 192,
        };
        let (first, status) = design_kaiser_big_cached(&directory.0, &spec).unwrap();
        assert_eq!(status, CacheStatus::DesignedAndStored);
        let (second, status) = design_kaiser_big_cached(&directory.0, &spec).unwrap();
        assert_eq!(status, CacheStatus::Hit);
        assert_eq!(first, second);
    }

    #[test]
    fn corrupt_entry_is_rejected_instead_of_partially_loaded() {
        let directory = TestDirectory::new("corrupt");
        let spec = native_spec();
        design_kaiser_cached(&directory.0, &spec).unwrap();
        let key = native_request_identity(&spec);
        let path = cache_path(&directory.0, "native", &key);
        fs::write(path, b"truncated").unwrap();
        assert!(matches!(
            design_kaiser_cached(&directory.0, &spec),
            Err(CacheError::Corrupt(_)) | Err(CacheError::Io(_))
        ));
    }

    #[test]
    fn complete_cache_payload_including_report_strings_is_integrity_checked() {
        let directory = TestDirectory::new("full-payload-integrity");
        let native = native_spec();
        let big = BigKaiserSpec {
            core: native.clone(),
            coefficient_fractional_bits: 96,
            accumulator_bits: 192,
        };
        let window = window_spec(96);
        design_kaiser_cached(&directory.0, &native).unwrap();
        design_kaiser_big_cached(&directory.0, &big).unwrap();
        design_windowed_big_cached(&directory.0, &window).unwrap();
        for path in fs::read_dir(&directory.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
        {
            let original = fs::read(&path).unwrap();
            let kind = original[8];
            let load = || match kind {
                NATIVE_KIND => design_kaiser_cached(&directory.0, &native).map(|_| ()),
                BIG_KIND => design_kaiser_big_cached(&directory.0, &big).map(|_| ()),
                WINDOWED_BIG_KIND => design_windowed_big_cached(&directory.0, &window).map(|_| ()),
                _ => panic!("unexpected kind"),
            };
            let payload_end = original.len() - 32;
            assert_eq!(
                Sha256::digest(&original[..payload_end]).as_slice(),
                &original[payload_end..]
            );
            // First report value: u64 DC correction for native; first byte of
            // the same decimal string for bigint/windowed. Coefficients and
            // their identity remain intact, so the whole-payload check matters.
            let report_byte = if kind == NATIVE_KIND { 81 } else { 85 };
            let mut altered = original.clone();
            altered[report_byte] = if altered[report_byte] == b'9' {
                b'8'
            } else {
                b'9'
            };
            fs::write(&path, altered).unwrap();
            assert!(matches!(
                load(),
                Err(CacheError::Corrupt("payload checksum mismatch"))
            ));
            // Include every serialized byte: request, report, coefficient, digest.
            for offset in 0..original.len() {
                let mut altered = original.clone();
                altered[offset] ^= 1;
                fs::write(&path, altered).unwrap();
                assert!(
                    load().is_err(),
                    "accepted changed kind {kind}, byte {offset}"
                );
            }
            for cut in [0, 8, 73, payload_end, original.len() - 1] {
                fs::write(&path, &original[..cut]).unwrap();
                assert!(load().is_err());
            }
            let mut trailing = original.clone();
            trailing.push(0);
            fs::write(&path, trailing).unwrap();
            assert!(load().is_err());
            fs::write(&path, original).unwrap();
            load().unwrap();
        }
    }

    #[test]
    fn legacy_unchecked_native_entry_is_preserved_but_not_reused_or_resealed() {
        let directory = TestDirectory::new("legacy-integrity");
        let spec = native_spec();
        let (first, _) = design_kaiser_cached(&directory.0, &spec).unwrap();
        let key = native_request_identity(&spec);
        let path = cache_path(&directory.0, "native", &key);
        let old_key = request_identity(b"sex/sexfir/cache-request-native-v1\0", &spec, 62, 128);
        let old_path = cache_path(&directory.0, "native", &old_key);
        let mut old = fs::read(&path).unwrap();
        old.truncate(old.len() - 32);
        old[8] = 1;
        old[9..73].copy_from_slice(old_key.as_bytes());
        fs::rename(&path, &old_path).unwrap();
        fs::write(&old_path, &old).unwrap();
        let (second, status) = design_kaiser_cached(&directory.0, &spec).unwrap();
        assert_eq!(status, CacheStatus::DesignedAndStored);
        assert_eq!(first, second);
        assert_eq!(fs::read(&old_path).unwrap(), old);
        assert!(matches!(
            load_native(&old_path, &spec, &key),
            Err(CacheError::Corrupt("backend kind mismatch"))
        ));
        assert_eq!(
            design_kaiser_cached(&directory.0, &spec).unwrap().1,
            CacheStatus::Hit
        );
    }

    fn window_spec(bits: u32) -> windowed::Spec {
        windowed::Spec {
            core: WindowedSincSpec {
                ratio: RateRatio::from_fraction(3, 2).unwrap(),
                taps_per_phase: 7,
                rolloff: Fraction::new(9, 10).unwrap(),
                window: WindowFunction::Blackman,
                working_precision_bits: (bits + 64).max(128),
                quantization_rounding: RoundingMode::NearestTiesToEven,
            },
            coefficient_fractional_bits: bits,
            accumulator_bits: bits + 80,
        }
    }

    #[test]
    fn windowed_cache_round_trips_all_windows_and_arbitrary_widths() {
        let directory = TestDirectory::new("windowed");
        for bits in [64, 96, 4096] {
            for window in [
                WindowFunction::Rectangular,
                WindowFunction::Hann,
                WindowFunction::Blackman,
                WindowFunction::DolphChebyshev {
                    attenuation_db: 100,
                },
            ] {
                let mut spec = window_spec(bits);
                spec.core.window = window;
                let (first, status) = design_windowed_big_cached(&directory.0, &spec).unwrap();
                assert_eq!(status, CacheStatus::DesignedAndStored);
                let (second, status) = design_windowed_big_cached(&directory.0, &spec).unwrap();
                assert_eq!(status, CacheStatus::Hit);
                assert_eq!(first, second);
            }
        }
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 12);
    }

    #[test]
    fn windowed_cache_rejects_corruption_and_noncanonical_padding() {
        let directory = TestDirectory::new("windowed-corrupt");
        let spec = window_spec(64);
        design_windowed_big_cached(&directory.0, &spec).unwrap();
        let path = fs::read_dir(&directory.0)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let original = fs::read(&path).unwrap();
        let mut padding = original.clone();
        let last_coefficient_byte = padding.len() - 33;
        padding[last_coefficient_byte] |= 0x80; // Only two bits are valid; exclude digest footer.
        let mut coefficient = original.clone();
        let last_code = coefficient.len() - 32 - 9;
        coefficient[last_code] ^= 1;
        let mut trailing = original.clone();
        trailing.push(0);
        for invalid in [
            padding,
            coefficient,
            trailing,
            original[..original.len() - 1].to_vec(),
        ] {
            fs::write(&path, invalid).unwrap();
            assert!(design_windowed_big_cached(&directory.0, &spec).is_err());
        }
        fs::write(&path, &original).unwrap();
        assert_eq!(
            design_windowed_big_cached(&directory.0, &spec).unwrap().1,
            CacheStatus::Hit
        );
    }

    #[test]
    fn windowed_cache_preflight_and_request_identity_cover_design_parameters() {
        let directory = TestDirectory::new("windowed-spec");
        let original = window_spec(64);
        let key = |s: &windowed::Spec| encode_sha256(windowed::spec_hash(s, b"test"));
        for field in 0..8 {
            let mut changed = original.clone();
            match field {
                0 => changed.core.ratio = RateRatio::from_fraction(4, 3).unwrap(),
                1 => changed.core.taps_per_phase += 2,
                2 => changed.core.rolloff = Fraction::new(8, 9).unwrap(),
                3 => changed.core.window = WindowFunction::Hann,
                4 => changed.coefficient_fractional_bits += 1,
                5 => changed.core.working_precision_bits += 32,
                6 => changed.accumulator_bits += 1,
                _ => {
                    changed.core.window = WindowFunction::DolphChebyshev {
                        attenuation_db: 100,
                    }
                }
            }
            assert_ne!(key(&original), key(&changed));
        }
        let mut expensive = original;
        expensive.core.window = WindowFunction::DolphChebyshev {
            attenuation_db: 600,
        };
        expensive.core.taps_per_phase = 66867;
        assert!(matches!(
            design_windowed_big_cached(&directory.0, &expensive),
            Err(CacheError::Design(
                DesignError::DolphSeriesBudgetExceeded { .. }
            ))
        ));
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }
}
