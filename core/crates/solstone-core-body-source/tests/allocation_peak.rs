// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{ParseError, parse};

const MAX_PEAK_BYTES: u64 = 128 * 1024;

fn measured(input: &[u8]) -> allocation_counter::AllocationInfo {
    allocation_counter::measure(|| {
        drop(parse(input));
    })
}

#[test]
fn large_number_parsing_has_bounded_peak_allocation() {
    let fraction = format!("0.{}", "1234567890".repeat(104_858));
    let positive_exponent = format!("1e{}", "9".repeat(1_048_576));
    let negative_exponent = format!("1e-{}", "9".repeat(1_048_576));
    let malformed = format!("1e+{}x", "9".repeat(1_048_576));
    let integer = "9".repeat(1_048_576);
    let negative_integer = format!("-{integer}");

    for input in [&fraction, &positive_exponent, &negative_exponent] {
        let info = measured(input.as_bytes());
        assert!(
            info.bytes_max <= MAX_PEAK_BYTES,
            "peak was {} bytes",
            info.bytes_max
        );
    }
    assert!(matches!(
        parse(malformed.as_bytes()),
        Err(ParseError::MalformedJson { .. })
    ));
    assert!(measured(malformed.as_bytes()).bytes_max <= MAX_PEAK_BYTES);
    for input in [&integer, &negative_integer] {
        assert!(matches!(
            parse(input.as_bytes()),
            Err(ParseError::NumberTooLong { .. })
        ));
        assert!(measured(input.as_bytes()).bytes_max <= MAX_PEAK_BYTES);
    }
    for (input, offset) in [
        ("1".to_owned() + &"0".repeat(4300), 4300),
        ("-1".to_owned() + &"0".repeat(4300), 4301),
    ] {
        assert_eq!(
            parse(input.as_bytes()),
            Err(ParseError::NumberTooLong {
                byte_offset: offset
            })
        );
    }

    let sentinel = "sentinel-input-must-not-appear";
    let error = ParseError::MalformedJson { byte_offset: 123 };
    let display = error.to_string();
    let debug = format!("{error:?}");
    assert!(display.len() <= 256 && debug.len() <= 256);
    assert!(!display.contains(sentinel) && !debug.contains(sentinel));
    assert!(Error::source(&error).is_none());
}
