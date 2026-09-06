use super::*;

#[cfg(test)]
mod tests;

const OPTIMIZED_KIND: u8 = 4;
const REPORT_BYTES: usize = 128;

pub fn refine_optimized_quantization_cached(
    directory: &Path,
    initial: &optimized::Spec,
    target: ErrorFloor,
    auto_working_precision: bool,
    limits: optimized::SearchLimits,
) -> Result<optimized::PrecisionSearch, optimized::SearchError> {
    optimized::refine_quantization_with(
        initial,
        target,
        auto_working_precision,
        limits,
        |spec, limits| {
            design_optimized_big_cached(directory, spec, limits)
                .map(|(bank, _)| bank)
                .map_err(|error| error.to_string())
        },
    )
}

pub fn design_optimized_big_cached(
    directory: &Path,
    spec: &optimized::Spec,
    limits: optimized::Limits,
) -> Result<(optimized::Designed, CacheStatus), CacheError> {
    let work = spec.preflight(limits).map_err(CacheError::Optimized)?;
    let key = encode_sha256(optimized::spec_hash(
        spec,
        b"sex/sexfir/optimized-cache-request-v1\0",
    ));
    let path = cache_path(directory, "optimized-big", &key);
    if path.try_exists()? {
        return load(&path, spec, work, &key).map(|bank| (bank, CacheStatus::Hit));
    }
    let designed = optimized::design(spec, limits).map_err(CacheError::Optimized)?;
    store(&path, &key, &designed)?;
    Ok((designed, CacheStatus::DesignedAndStored))
}

fn store(path: &Path, key: &str, designed: &optimized::Designed) -> Result<(), CacheError> {
    atomic_write(path, |output| {
        let mut writer = Hashed::new(output);
        write_header(&mut writer, OPTIMIZED_KIND, key)?;
        let report = &designed.report.quantization;
        write_u64(&mut writer, report.coefficient_count as u64)?;
        for value in [
            &report.max_dc_correction_raw,
            &report.max_abs_coefficient_error_decimal,
            &report.max_abs_coefficient_error_db,
            &report.max_phase_l1_error_decimal,
            &report.max_phase_l1_error_db,
            &report.coefficient_sha256,
        ] {
            write_string(&mut writer, value)?;
        }
        write_u32(&mut writer, report.required_accumulator_bits)?;
        match &designed.report.solution {
            optimized::Solution::LeastSquares {
                max_normal_residual_decimal,
            } => {
                writer.write_all(&[0])?;
                write_string(&mut writer, max_normal_residual_decimal)?;
            }
            optimized::Solution::Equiripple {
                iterations,
                extremal_frequencies_decimal,
                prototype_weighted_error_decimal,
            } => {
                writer.write_all(&[1])?;
                write_u32(&mut writer, *iterations)?;
                write_u64(&mut writer, extremal_frequencies_decimal.len() as u64)?;
                for frequency in extremal_frequencies_decimal {
                    write_string(&mut writer, frequency)?;
                }
                write_string(&mut writer, prototype_weighted_error_decimal)?;
            }
        }
        let width = (designed.spec.coefficient_fractional_bits + 2).div_ceil(8) as usize;
        write_u32(&mut writer, width as u32)?;
        for phase in 0..designed.ratio().up() {
            for coefficient in designed.bank.phase(phase).map_err(DesignError::from)? {
                let mut bytes = coefficient
                    .to_twos_complement_bits()
                    .to_digits::<u8>(Order::Lsf);
                bytes.resize(width, 0);
                writer.write_all(&bytes)?;
            }
        }
        let (output, digest) = writer.finish();
        output.write_all(&digest)?;
        Ok(())
    })
}

fn report_string(reader: &mut impl Read) -> Result<String, CacheError> {
    let length = read_u32(reader)? as usize;
    if length > REPORT_BYTES {
        return Err(CacheError::Corrupt("optimized report string too long"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| CacheError::InvalidUtf8)
}

fn measurement(value: &str, precision: u32, decibels: bool) -> Result<Float, CacheError> {
    if decibels && value == "-inf" {
        return Ok(Float::with_val(precision, rug::float::Special::NegInfinity));
    }
    let parsed =
        Float::parse(value).map_err(|_| CacheError::Corrupt("invalid optimized measurement"))?;
    let number = Float::with_val(precision, parsed);
    if !number.is_finite() || (!decibels && number < 0) {
        return Err(CacheError::Corrupt(
            "nonfinite or negative optimized measurement",
        ));
    }
    Ok(number)
}

fn load(
    path: &Path,
    spec: &optimized::Spec,
    work: optimized::Work,
    key: &str,
) -> Result<optimized::Designed, CacheError> {
    let mut reader = Hashed::new(BufReader::new(File::open(path)?));
    read_header(&mut reader, OPTIMIZED_KIND, key)?;
    if read_u64(&mut reader)? != work.coefficient_count {
        return Err(CacheError::Corrupt("coefficient count mismatch"));
    }
    let max_dc_correction_raw = report_string(&mut reader)?;
    let max_abs_coefficient_error_decimal = report_string(&mut reader)?;
    let max_abs_coefficient_error_db = report_string(&mut reader)?;
    let max_phase_l1_error_decimal = report_string(&mut reader)?;
    let max_phase_l1_error_db = report_string(&mut reader)?;
    let coefficient_sha256 = report_string(&mut reader)?;
    let required_accumulator_bits = read_u32(&mut reader)?;
    let precision = spec.method.precision();
    let correction = Integer::from_str_radix(&max_dc_correction_raw, 10)
        .map_err(|_| CacheError::Corrupt("invalid DC correction"))?;
    if correction < 0 || correction > spec.method.taps() {
        return Err(CacheError::Corrupt("DC correction outside rounding bound"));
    }
    let maximum_error = measurement(&max_abs_coefficient_error_decimal, precision, false)?;
    let phase_error = measurement(&max_phase_l1_error_decimal, precision, false)?;
    if maximum_error > phase_error {
        return Err(CacheError::Corrupt("inconsistent quantization errors"));
    }
    measurement(&max_abs_coefficient_error_db, precision, true)?;
    measurement(&max_phase_l1_error_db, precision, true)?;
    let mut kind = [0];
    reader.read_exact(&mut kind)?;
    let solution = match (&spec.method, kind[0]) {
        (optimized::Method::GlobalLeastSquares(_), 0) => {
            let residual = report_string(&mut reader)?;
            measurement(&residual, precision, false)?;
            optimized::Solution::LeastSquares {
                max_normal_residual_decimal: residual,
            }
        }
        (optimized::Method::Equiripple(core), 1) => {
            let iterations = read_u32(&mut reader)?;
            if iterations == 0 || iterations > core.max_iterations {
                return Err(CacheError::Corrupt("invalid exchange iteration count"));
            }
            if read_u64(&mut reader)? != work.solver_dimension {
                return Err(CacheError::Corrupt("invalid extrema count"));
            }
            let mut frequencies = Vec::with_capacity(work.solver_dimension as usize);
            let mut previous = None;
            let nyquist = Float::with_val(precision, 1) / 2;
            for _ in 0..work.solver_dimension {
                let frequency = report_string(&mut reader)?;
                let value = measurement(&frequency, precision, false)?;
                if value > nyquist || previous.as_ref().is_some_and(|prior| value <= *prior) {
                    return Err(CacheError::Corrupt("unordered or out-of-range extrema"));
                }
                previous = Some(value);
                frequencies.push(frequency);
            }
            let weighted_error = report_string(&mut reader)?;
            if measurement(&weighted_error, precision, false)? <= 0 {
                return Err(CacheError::Corrupt("nonpositive Remez ripple"));
            }
            optimized::Solution::Equiripple {
                iterations,
                extremal_frequencies_decimal: frequencies,
                prototype_weighted_error_decimal: weighted_error,
            }
        }
        _ => return Err(CacheError::Corrupt("optimized solution kind mismatch")),
    };
    let format = BigQFormat::new(2, spec.coefficient_fractional_bits).map_err(DesignError::from)?;
    let width = format.total_bits().div_ceil(8) as usize;
    if read_u32(&mut reader)? as usize != width {
        return Err(CacheError::Corrupt("coefficient storage width mismatch"));
    }
    let mut bytes = vec![0; width];
    let mut coefficients = Vec::with_capacity(work.coefficient_count as usize);
    for _ in 0..work.coefficient_count {
        reader.read_exact(&mut bytes)?;
        coefficients.push(
            BigQ::from_twos_complement_bits(Integer::from_digits(&bytes, Order::Lsf), format)
                .map_err(DesignError::from)?,
        );
    }
    let (mut reader, digest) = reader.finish();
    let mut stored_digest = [0; 32];
    reader.read_exact(&mut stored_digest)?;
    if digest != stored_digest {
        return Err(CacheError::Corrupt("optimized payload checksum mismatch"));
    }
    ensure_eof(&mut reader)?;
    let delay = (spec.method.taps() as u64 - 1) / 2;
    if optimized::identity(spec, delay, &coefficients) != coefficient_sha256 {
        return Err(CacheError::Corrupt("coefficient identity mismatch"));
    }
    let unity = Integer::from(1) << spec.coefficient_fractional_bits;
    for phase in coefficients.chunks_exact(spec.method.taps()) {
        if Integer::from(Integer::sum(phase.iter().map(BigQ::raw))) != unity {
            return Err(CacheError::Corrupt("phase DC sum mismatch"));
        }
    }
    let bank = PolyphaseFirBigQ63::for_ratio(
        spec.method.ratio(),
        spec.method.taps(),
        format,
        spec.accumulator_bits,
        coefficients,
    )
    .map_err(DesignError::from)?;
    if bank.required_accumulator_bits() != required_accumulator_bits {
        return Err(CacheError::Corrupt("accumulator bound mismatch"));
    }
    Ok(optimized::Designed {
        spec: spec.clone(),
        input_delay_frames: delay,
        bank,
        report: optimized::Report {
            work,
            solution,
            quantization: BigQuantizationReport {
                algorithm: spec.method.algorithm(),
                working_precision_bits: precision,
                coefficient_fractional_bits: spec.coefficient_fractional_bits,
                coefficient_count: work.coefficient_count as usize,
                max_dc_correction_raw,
                max_abs_coefficient_error_decimal,
                max_abs_coefficient_error_db,
                max_phase_l1_error_decimal,
                max_phase_l1_error_db,
                configured_accumulator_bits: spec.accumulator_bits,
                required_accumulator_bits,
                coefficient_sha256,
            },
        },
    })
}
