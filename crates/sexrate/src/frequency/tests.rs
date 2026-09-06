use super::*;

#[test]
fn equivalent_rate_spellings_have_one_exact_identity() {
    for text in [
        "48000",
        "48000.000",
        "48k",
        "48K",
        ".048M",
        "4.8e4",
        "4.8E+4",
        "480000e-1",
        "96000/2",
        "00048000",
    ] {
        let rate: SampleRate = text.parse().unwrap();
        assert_eq!(rate, SampleRate::from_hz(48000).unwrap(), "{text}");
        assert_eq!(rate.to_string(), "48000");
        assert_eq!(rate.container_hz().unwrap(), 48000);
    }
    for text in ["16537.5", "33075/2", "16.5375k", "1.65375e4"] {
        assert_eq!(
            text.parse::<SampleRate>().unwrap(),
            SampleRate::from_fraction(33075, 2).unwrap()
        );
    }
}

#[test]
fn fractional_frequency_produces_exact_polyphase_ratio() {
    let input = SampleRate::from_hz(44100).unwrap();
    assert_eq!(
        "16537.5"
            .parse::<SampleRate>()
            .unwrap()
            .ratio_from(input)
            .unwrap(),
        RateRatio::from_fraction(3, 8).unwrap()
    );
    let exotic: SampleRate = "141421.356237".parse().unwrap();
    assert_eq!(
        exotic,
        SampleRate::from_fraction(141421356237, 1000000).unwrap()
    );
    assert_eq!(
        exotic.ratio_from(input).unwrap(),
        RateRatio::from_fraction(141421356237, 44100000000).unwrap()
    );
    assert_eq!(
        exotic.container_hz(),
        Err(ParseRateError::NonIntegralContainerRate)
    );
}

#[test]
fn cancellation_happens_before_native_width_limits() {
    let big = "12345678901234567890123456789012345678901234567890";
    assert_eq!(
        format!("{big}/{big}").parse::<SampleRate>().unwrap(),
        SampleRate::from_hz(1).unwrap()
    );
    assert_eq!(
        "18446744073709551616/2".parse::<SampleRate>().unwrap(),
        SampleRate::from_hz(1_u64 << 63).unwrap()
    );
    let fraction = Integer::from(5).pow(63).to_string();
    let decimal = format!("0.{fraction:0>63}");
    assert_eq!(
        decimal.parse::<SampleRate>().unwrap(),
        SampleRate::from_fraction(1, 1_u64 << 63).unwrap()
    );
    let first = SampleRate::from_fraction(u64::MAX, 2).unwrap();
    let second = SampleRate::from_fraction(u64::MAX, 7).unwrap();
    assert_eq!(
        second.ratio_from(first).unwrap(),
        RateRatio::from_fraction(2, 7).unwrap()
    );
}

#[test]
fn decimal_and_exponent_parsing_matches_integer_fraction_oracle() {
    for value in (1..10000_u64).step_by(97) {
        for digits in 0..=18 {
            let denominator = 10_u64.pow(digits);
            let expected = SampleRate::from_fraction(value, denominator).unwrap();
            assert_eq!(
                format!("{value}e-{digits}").parse::<SampleRate>().unwrap(),
                expected
            );
            if digits != 0 {
                let whole = value / denominator;
                let remainder = value % denominator;
                let decimal = format!("{whole}.{remainder:0>width$}", width = digits as usize);
                assert_eq!(
                    decimal.parse::<SampleRate>().unwrap(),
                    expected,
                    "{decimal}"
                );
            }
        }
    }
}

#[test]
fn syntax_range_and_container_limits_are_explicit() {
    for bad in [
        "", ".", "-1", "+1", " 1", "1 ", "1_000", "NaN", "inf", "0x10", "1/2/3", "1.0/2", "1//2",
        "1e", "1e+-2", "1..2", "k", "1m", "1.2.3", "１",
    ] {
        assert!(bad.parse::<SampleRate>().is_err(), "accepted {bad}");
    }
    for zero in ["0", "0.000", "0e123", "0/1", "1/0"] {
        assert_eq!(zero.parse::<SampleRate>(), Err(ParseRateError::Zero));
    }
    for large in ["18446744073709551616", "1e-20", "1e9999", "1e-2147483648"] {
        assert_eq!(large.parse::<SampleRate>(), Err(ParseRateError::OutOfRange));
    }
    assert_eq!(
        "1".repeat(257).parse::<SampleRate>(),
        Err(ParseRateError::TooLong)
    );
    assert_eq!(
        SampleRate::from_hz(u64::MAX).unwrap().container_hz(),
        Err(ParseRateError::ContainerRateOverflow)
    );
    let tiny = SampleRate::from_fraction(1, u64::MAX).unwrap();
    assert_eq!(
        SampleRate::from_hz(u64::MAX).unwrap().ratio_from(tiny),
        Err(ParseRateError::RatioOverflow)
    );
}
