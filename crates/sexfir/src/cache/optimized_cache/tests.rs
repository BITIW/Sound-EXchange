use super::*;

struct Directory(PathBuf);

#[test]
fn remez_cosine_cache_policy_does_not_change_persistent_cache_bytes() {
    for bits in [96, 4096] {
        let request = spec(true, bits);
        let cached_limits = optimized::Limits::default();
        let work = request.preflight(cached_limits).unwrap();
        let direct_limits = optimized::Limits {
            max_storage_bytes: work.storage_bytes,
            ..cached_limits
        };
        let mut payload = None;
        for cold_limits in [cached_limits, direct_limits] {
            let directory = Directory::new();
            let (bank, status) =
                design_optimized_big_cached(&directory.0, &request, cold_limits).unwrap();
            assert_eq!(status, CacheStatus::DesignedAndStored);
            let path = location(&directory, &request).0;
            let bytes = fs::read(&path).unwrap();
            for warm_limits in [cached_limits, direct_limits] {
                let (warm, status) =
                    design_optimized_big_cached(&directory.0, &request, warm_limits).unwrap();
                assert_eq!(status, CacheStatus::Hit);
                assert_eq!(bank.bank(), warm.bank());
                assert_eq!(bank.report().quantization, warm.report().quantization);
                assert_eq!(bank.report().solution, warm.report().solution);
                assert_eq!(warm.report().work, request.preflight(warm_limits).unwrap());
                assert_eq!(fs::read(&path).unwrap(), bytes);
            }
            if let Some(previous) = &payload {
                assert_eq!(&bytes, previous);
            }
            payload = Some(bytes);
        }
    }
}

impl Directory {
    fn new() -> Self {
        let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sex-optimized-cache-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn spec(equiripple: bool, bits: u32) -> optimized::Spec {
    let ratio = RateRatio::from_fraction(2, 3).unwrap();
    let rolloff = Fraction::new(9, 10).unwrap();
    let pass = Fraction::new(1, 1).unwrap();
    let stop = Fraction::new(10, 1).unwrap();
    let precision = (bits + 64).max(128);
    let method = if equiripple {
        optimized::Method::Equiripple(EquirippleSpec {
            ratio,
            taps_per_phase: 7,
            rolloff,
            passband_weight: pass,
            stopband_weight: stop,
            grid_density: 16,
            max_iterations: 64,
            working_precision_bits: precision,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        })
    } else {
        optimized::Method::GlobalLeastSquares(LeastSquaresSpec {
            ratio,
            taps_per_phase: 7,
            rolloff,
            passband_weight: pass,
            stopband_weight: stop,
            grid_points_per_band: 32,
            working_precision_bits: precision,
            quantization_rounding: RoundingMode::NearestTiesToEven,
        })
    };
    optimized::Spec {
        method,
        coefficient_fractional_bits: bits,
        accumulator_bits: (bits + 80).max(128),
    }
}

fn location(directory: &Directory, request: &optimized::Spec) -> (PathBuf, String) {
    let key = encode_sha256(optimized::spec_hash(
        request,
        b"sex/sexfir/optimized-cache-request-v1\0",
    ));
    (cache_path(&directory.0, "optimized-big", &key), key)
}

fn reseal(bytes: &mut [u8]) {
    let end = bytes.len() - 32;
    let digest = Sha256::digest(&bytes[..end]);
    bytes[end..].copy_from_slice(&digest);
}

#[test]
fn optimized_cache_preserves_both_solvers_and_arbitrary_widths() {
    let directory = Directory::new();
    for equiripple in [false, true] {
        for bits in [0, 8, 31, 62, 63, 64, 96, 4096] {
            let request = spec(equiripple, bits);
            let direct = optimized::design(&request, optimized::Limits::default()).unwrap();
            let (cold, status) =
                design_optimized_big_cached(&directory.0, &request, optimized::Limits::default())
                    .unwrap();
            assert_eq!(status, CacheStatus::DesignedAndStored);
            assert_eq!(cold, direct);
            let path = location(&directory, &request).0;
            let bytes = fs::read(&path).unwrap();
            let (warm, status) =
                design_optimized_big_cached(&directory.0, &request, optimized::Limits::default())
                    .unwrap();
            assert_eq!(status, CacheStatus::Hit);
            assert_eq!(cold, warm);
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn optimized_cache_checks_payload_footer_lengths_and_padding() {
    let directory = Directory::new();
    for equiripple in [false, true] {
        let request = spec(equiripple, 96);
        design_optimized_big_cached(&directory.0, &request, optimized::Limits::default()).unwrap();
        let path = location(&directory, &request).0;
        let original = fs::read(&path).unwrap();
        for length in [0, 8, 72, 80, original.len() - 33, original.len() - 1] {
            fs::write(&path, &original[..length]).unwrap();
            assert!(
                design_optimized_big_cached(&directory.0, &request, optimized::Limits::default())
                    .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), original[..length]);
        }
        let mut changed = original.clone();
        changed[85] = if changed[85] == b'0' { b'1' } else { b'0' };
        fs::write(&path, &changed).unwrap();
        assert!(matches!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default()),
            Err(CacheError::Corrupt("optimized payload checksum mismatch"))
        ));
        for (offset, value) in [(8, 3), (9, b'x')] {
            changed = original.clone();
            changed[offset] = value;
            reseal(&mut changed);
            fs::write(&path, &changed).unwrap();
            assert!(
                design_optimized_big_cached(&directory.0, &request, optimized::Limits::default())
                    .is_err()
            );
        }
        changed = original.clone();
        changed[73..81].copy_from_slice(&u64::MAX.to_le_bytes());
        reseal(&mut changed);
        fs::write(&path, &changed).unwrap();
        assert!(matches!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default()),
            Err(CacheError::Corrupt("coefficient count mismatch"))
        ));
        changed = original.clone();
        changed[81..85].copy_from_slice(&u32::MAX.to_le_bytes());
        reseal(&mut changed);
        fs::write(&path, &changed).unwrap();
        assert!(matches!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default()),
            Err(CacheError::Corrupt("optimized report string too long"))
        ));
        changed = original.clone();
        let coefficient_start = changed.len() - 32 - 14 * 13;
        changed[coefficient_start + 12] |= 0x80;
        reseal(&mut changed);
        fs::write(&path, &changed).unwrap();
        assert!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default())
                .is_err()
        );
        changed = original.clone();
        changed[coefficient_start] ^= 1;
        reseal(&mut changed);
        fs::write(&path, &changed).unwrap();
        assert!(matches!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default()),
            Err(CacheError::Corrupt("coefficient identity mismatch"))
        ));
        changed = original.clone();
        changed.push(0);
        fs::write(&path, &changed).unwrap();
        assert!(matches!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default()),
            Err(CacheError::Corrupt("trailing bytes"))
        ));
        fs::write(&path, original).unwrap();
        assert_eq!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default())
                .unwrap()
                .1,
            CacheStatus::Hit
        );
    }
}

#[test]
fn optimized_cache_rejects_resealed_invalid_reports_and_dc() {
    let directory = Directory::new();
    for equiripple in [false, true] {
        let request = spec(equiripple, 96);
        let bank = optimized::design(&request, optimized::Limits::default()).unwrap();
        let (path, key) = location(&directory, &request);
        for variant in 0..7 {
            let mut changed = bank.clone();
            match variant {
                0 => changed.report.quantization.max_phase_l1_error_decimal = "NaN".into(),
                1 => {
                    changed
                        .report
                        .quantization
                        .max_abs_coefficient_error_decimal = "-1".into()
                }
                2 => changed.report.quantization.max_dc_correction_raw = "999".into(),
                3 => changed.report.quantization.required_accumulator_bits += 1,
                4 => changed.report.quantization.max_abs_coefficient_error_db = "inf".into(),
                5 => {
                    changed.report.solution = match changed.report.solution {
                        optimized::Solution::LeastSquares { .. } => {
                            optimized::Solution::Equiripple {
                                iterations: 1,
                                extremal_frequencies_decimal: vec![],
                                prototype_weighted_error_decimal: "1".into(),
                            }
                        }
                        optimized::Solution::Equiripple { .. } => {
                            optimized::Solution::LeastSquares {
                                max_normal_residual_decimal: "0".into(),
                            }
                        }
                    }
                }
                _ => match &mut changed.report.solution {
                    optimized::Solution::LeastSquares {
                        max_normal_residual_decimal,
                    } => *max_normal_residual_decimal = "-1".into(),
                    optimized::Solution::Equiripple { iterations, .. } => *iterations = 0,
                },
            }
            store(&path, &key, &changed).unwrap();
            assert!(
                design_optimized_big_cached(&directory.0, &request, optimized::Limits::default())
                    .is_err()
            );
        }
        let mut changed = bank.clone();
        let mut coefficients = (0..2)
            .flat_map(|phase| bank.bank.phase(phase).unwrap().to_vec())
            .collect::<Vec<_>>();
        let format = coefficients[0].format();
        coefficients[0] = BigQ::from_raw(Integer::from(coefficients[0].raw() + 1), format).unwrap();
        changed.report.quantization.coefficient_sha256 =
            optimized::identity(&request, bank.input_delay_frames(), &coefficients);
        changed.bank = PolyphaseFirBigQ63::for_ratio(
            request.method.ratio(),
            7,
            format,
            request.accumulator_bits,
            coefficients,
        )
        .unwrap();
        changed.report.quantization.required_accumulator_bits =
            changed.bank.required_accumulator_bits();
        store(&path, &key, &changed).unwrap();
        assert!(matches!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default()),
            Err(CacheError::Corrupt("phase DC sum mismatch"))
        ));
    }
}

#[test]
fn optimized_cache_rejects_malformed_extrema_and_preserves_failed_publications() {
    let directory = Directory::new();
    let request = spec(true, 96);
    let (bank, _) =
        design_optimized_big_cached(&directory.0, &request, optimized::Limits::default()).unwrap();
    let (path, key) = location(&directory, &request);
    let original = fs::read(&path).unwrap();
    for variant in 0..7 {
        let mut changed = bank.clone();
        let optimized::Solution::Equiripple {
            iterations,
            extremal_frequencies_decimal,
            prototype_weighted_error_decimal,
        } = &mut changed.report.solution
        else {
            unreachable!()
        };
        match variant {
            0 => *iterations = 65,
            1 => {
                extremal_frequencies_decimal.pop();
            }
            2 => extremal_frequencies_decimal.swap(0, 1),
            3 => extremal_frequencies_decimal[0] = "NaN".into(),
            4 => *extremal_frequencies_decimal.last_mut().unwrap() = "1".into(),
            5 => *prototype_weighted_error_decimal = "0".into(),
            _ => *prototype_weighted_error_decimal = "0".repeat(REPORT_BYTES + 1),
        }
        store(&path, &key, &changed).unwrap();
        assert!(
            design_optimized_big_cached(&directory.0, &request, optimized::Limits::default())
                .is_err()
        );
    }
    fs::write(&path, &original).unwrap();
    let mut changed = bank.clone();
    changed.report.quantization.max_dc_correction_raw = "0".repeat(MAX_CACHE_STRING_BYTES + 1);
    assert!(store(&path, &key, &changed).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    let mut failing = request;
    let optimized::Method::Equiripple(core) = &mut failing.method else {
        unreachable!()
    };
    core.max_iterations = 1;
    let absent = directory.0.join("never-created");
    assert!(matches!(
        design_optimized_big_cached(&absent, &failing, optimized::Limits::default()),
        Err(CacheError::Optimized(optimized::Error::Design(
            DesignError::RemezDidNotConverge { iterations: 1 }
        )))
    ));
    assert!(!absent.exists());
}

#[test]
fn optimized_cache_search_is_bounded_even_on_hits() {
    let directory = Directory::new();
    for equiripple in [false, true] {
        let request = spec(equiripple, 8);
        let target = ErrorFloor::new(80).unwrap();
        let limits = optimized::SearchLimits::default();
        let uncached = optimized::refine_quantization(&request, target, false, limits).unwrap();
        let cold =
            refine_optimized_quantization_cached(&directory.0, &request, target, false, limits)
                .unwrap();
        let warm =
            refine_optimized_quantization_cached(&directory.0, &request, target, false, limits)
                .unwrap();
        assert_eq!(uncached, cold);
        assert_eq!(cold, warm);
        assert_eq!(cold.attempts.len(), 2);
        let bounded = optimized::SearchLimits {
            max_total_coefficients: 14,
            ..limits
        };
        assert_eq!(
            refine_optimized_quantization_cached(&directory.0, &request, target, false, bounded)
                .unwrap_err()
                .attempts
                .len(),
            1
        );
        let work = request.preflight(limits.design).unwrap();
        let bounded = optimized::Limits {
            max_work: work.terms - 1,
            ..limits.design
        };
        assert!(matches!(
            design_optimized_big_cached(&directory.0, &request, bounded),
            Err(CacheError::Optimized(optimized::Error::Limit { .. }))
        ));
        let other = optimized::design(&spec(!equiripple, 8), limits.design).unwrap();
        assert!(
            optimized::refine_quantization_with(&request, target, false, limits, |_, _| Ok(
                other.clone()
            ))
            .unwrap_err()
            .reason
            .contains("different request")
        );
    }
}

#[test]
fn optimized_cache_identity_and_concurrent_publication_are_deterministic() {
    let directory = Directory::new();
    let request = spec(true, 96);
    let base = location(&directory, &request).1;
    for variant in 0..9 {
        let mut changed = request.clone();
        let optimized::Method::Equiripple(core) = &mut changed.method else {
            unreachable!()
        };
        match variant {
            0 => core.ratio = RateRatio::from_fraction(3, 2).unwrap(),
            1 => core.taps_per_phase += 2,
            2 => core.rolloff = Fraction::new(3, 4).unwrap(),
            3 => core.passband_weight = Fraction::new(2, 1).unwrap(),
            4 => core.stopband_weight = Fraction::new(20, 1).unwrap(),
            5 => core.grid_density += 1,
            6 => core.max_iterations += 1,
            7 => core.working_precision_bits += 32,
            _ => changed.coefficient_fractional_bits += 1,
        }
        assert_ne!(base, location(&directory, &changed).1);
    }
    let mut changed = request.clone();
    changed.accumulator_bits += 1;
    assert_ne!(base, location(&directory, &changed).1);
    assert_ne!(base, location(&directory, &spec(false, 96)).1);
    std::thread::scope(|scope| {
        let handles = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    design_optimized_big_cached(
                        &directory.0,
                        &request,
                        optimized::Limits::default(),
                    )
                    .unwrap()
                    .0
                })
            })
            .collect::<Vec<_>>();
        let banks = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert!(banks.iter().all(|bank| bank == &banks[0]));
    });
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
}
