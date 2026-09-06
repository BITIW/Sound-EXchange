//! MPFR is used only to display completed exact measurement aggregates in dB.
use rug::{Float, Integer};
use sexdsp::pcm_error::PcmErrorStats;

fn db(raw: &Integer, fractional_bits: u32, divisor: u64, factor: u32) -> String {
    if raw == &0 {
        return "-inf".into();
    }
    let mut value = Float::with_val(256, raw);
    value /= divisor;
    value >>= fractional_bits;
    value.log10_mut();
    value *= factor;
    value.to_string_radix(10, Some(16))
}

pub(super) fn report(stats: &PcmErrorStats) -> String {
    let (rms, peak) = if stats.samples == 0 {
        ("n/a (empty)".into(), "n/a (empty)".into())
    } else {
        (
            db(
                &stats.sum_squared_raw,
                stats.fractional_bits * 2,
                stats.samples,
                10,
            ),
            db(&stats.peak_raw, stats.fractional_bits, 1, 20),
        )
    };
    format!(
        "measured final PCM error: RMS {rms} dBFS; peak {peak} dBFS; {} interleaved samples\n\
         PCM error scope: output minus immediate pre-PCM signal after normalization; includes dither/noise shaping/clipping; exact integer aggregates, approximate dB; full-band file measurement, not a noise floor or pre-PCM error bound\n",
        stats.samples
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_error_db_distinguishes_empty_exact_and_nonzero_measurements() {
        let mut stats = PcmErrorStats {
            samples: 0,
            fractional_bits: 4096,
            peak_raw: Integer::new(),
            sum_squared_raw: Integer::new(),
        };
        assert!(report(&stats).contains("RMS n/a (empty) dBFS; peak n/a (empty) dBFS"));
        stats.samples = 1;
        assert!(report(&stats).contains("RMS -inf dBFS; peak -inf dBFS"));
        // One error of exactly 1/2 FS. Fractional width must cancel exactly.
        stats.peak_raw = Integer::from(1) << 4095;
        stats.sum_squared_raw = Integer::from(1) << 8190;
        assert!(
            report(&stats).contains("RMS -6.020599913279624 dBFS; peak -6.020599913279624 dBFS")
        );
        // Add three zero-error samples: RMS halves, peak is unchanged.
        stats.samples = 4;
        assert!(
            report(&stats).contains("RMS -12.04119982655925 dBFS; peak -6.020599913279624 dBFS")
        );
    }
}
