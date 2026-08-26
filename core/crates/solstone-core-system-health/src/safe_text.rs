// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Safe rendering of untrusted text for a terminal.

/// Unicode 16.0.0's complete `Cc | Cf | Zl | Zp` union, coalesced into
/// maximal scalar ranges.  The test-only retained fixture verifies this table.
const UNSAFE_RANGES: &[(u32, u32); 23] = &[
    (0x0000, 0x001f),
    (0x007f, 0x009f),
    (0x00ad, 0x00ad),
    (0x0600, 0x0605),
    (0x061c, 0x061c),
    (0x06dd, 0x06dd),
    (0x070f, 0x070f),
    (0x0890, 0x0891),
    (0x08e2, 0x08e2),
    (0x180e, 0x180e),
    (0x200b, 0x200f),
    (0x2028, 0x202e),
    (0x2060, 0x2064),
    (0x2066, 0x206f),
    (0xfeff, 0xfeff),
    (0xfff9, 0xfffb),
    (0x110bd, 0x110bd),
    (0x110cd, 0x110cd),
    (0x13430, 0x1343f),
    (0x1bca0, 0x1bca3),
    (0x1d173, 0x1d17a),
    (0xe0001, 0xe0001),
    (0xe0020, 0xe007f),
];

#[doc(hidden)]
#[must_use]
pub fn unsafe_ranges() -> &'static [(u32, u32)] {
    UNSAFE_RANGES
}

fn unsafe_scalar(scalar: u32) -> bool {
    UNSAFE_RANGES
        .binary_search_by(|(start, end)| {
            if scalar < *start {
                std::cmp::Ordering::Greater
            } else if scalar > *end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn push_sanitized_scalar(output: &mut String, scalar: char) {
    match scalar {
        '\\' => output.push_str("\\\\"),
        '\n' => output.push_str("\\n"),
        '\r' => output.push_str("\\r"),
        '\t' => output.push_str("\\t"),
        '\x1b' => output.push_str("\\x1b"),
        _ if unsafe_scalar(scalar as u32) => {
            use std::fmt::Write as _;
            let _ = write!(output, "\\u{{{:x}}}", scalar as u32);
        }
        _ => output.push(scalar),
    }
}

/// Render dynamic text without allowing it to control terminal layout.
pub fn sanitize_for_terminal(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for scalar in input.chars() {
        push_sanitized_scalar(&mut output, scalar);
    }
    output
}

/// Render raw Unix filesystem bytes without allowing terminal control or
/// losing undecodable bytes. Invalid UTF-8 bytes use reversible `\\xNN`
/// escapes; valid scalars share [`sanitize_for_terminal`]'s escaping rules.
#[must_use]
pub fn sanitize_os_bytes_for_terminal(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                for scalar in valid.chars() {
                    push_sanitized_scalar(&mut output, scalar);
                }
                break;
            }
            Err(error) => {
                let valid = std::str::from_utf8(&remaining[..error.valid_up_to()])
                    .expect("valid UTF-8 prefix");
                for scalar in valid.chars() {
                    push_sanitized_scalar(&mut output, scalar);
                }
                let invalid = remaining[error.valid_up_to()];
                use std::fmt::Write as _;
                let _ = write!(output, "\\x{invalid:02x}");
                remaining = &remaining[error.valid_up_to() + 1..];
            }
        }
    }
    output
}

const TERMINAL_RENDER_LIMIT: usize = 2048;
const TRUNCATION_MARKER: &str = "…[truncated]";

fn bound_sanitized(sanitized: String) -> String {
    let count = sanitized.chars().count();
    if count <= TERMINAL_RENDER_LIMIT {
        return sanitized;
    }
    let keep = TERMINAL_RENDER_LIMIT - TRUNCATION_MARKER.chars().count();
    let mut output: String = sanitized.chars().take(keep).collect();
    output.push_str(TRUNCATION_MARKER);
    output
}

/// Sanitize helper bytes, then cap the rendered output at 2048 Unicode scalars.
///
/// Truncated results end with `…[truncated]` (12 scalars). The cap includes
/// the marker: 2036 content scalars + 12 marker scalars.
#[must_use]
pub fn sanitize_os_bytes_for_terminal_bounded(input: &[u8]) -> String {
    bound_sanitized(sanitize_os_bytes_for_terminal(input))
}

/// Sanitize text, then cap the rendered output at 2048 Unicode scalars.
#[must_use]
pub fn sanitize_str_for_terminal_bounded(input: &str) -> String {
    bound_sanitized(sanitize_for_terminal(input))
}

#[cfg(test)]
mod tests {
    use super::{
        TERMINAL_RENDER_LIMIT, TRUNCATION_MARKER, sanitize_for_terminal,
        sanitize_os_bytes_for_terminal, sanitize_os_bytes_for_terminal_bounded,
        sanitize_str_for_terminal_bounded,
    };

    fn decode_terminal_bytes(input: &str) -> Vec<u8> {
        let mut output = Vec::new();
        let mut cursor = 0;
        while cursor < input.len() {
            let rest = &input[cursor..];
            if !rest.starts_with('\\') {
                let scalar = rest.chars().next().unwrap();
                let mut encoded = [0; 4];
                output.extend_from_slice(scalar.encode_utf8(&mut encoded).as_bytes());
                cursor += scalar.len_utf8();
                continue;
            }
            let escaped = rest.as_bytes().get(1).copied().unwrap();
            match escaped {
                b'\\' => output.push(b'\\'),
                b'n' => output.push(b'\n'),
                b'r' => output.push(b'\r'),
                b't' => output.push(b'\t'),
                b'x' => {
                    let digits = &rest[2..4];
                    output.push(u8::from_str_radix(digits, 16).unwrap());
                    cursor += 2;
                }
                b'u' => {
                    let end = rest.find('}').unwrap();
                    let scalar = u32::from_str_radix(&rest[3..end], 16).unwrap();
                    let scalar = char::from_u32(scalar).unwrap();
                    let mut encoded = [0; 4];
                    output.extend_from_slice(scalar.encode_utf8(&mut encoded).as_bytes());
                    cursor += end - 1;
                }
                _ => unreachable!("unknown terminal escape"),
            }
            cursor += 2;
        }
        output
    }

    #[test]
    fn special_escapes_take_precedence() {
        assert_eq!(
            sanitize_for_terminal("\\\n\r\t\x1b\0\u{2028}"),
            "\\\\\\n\\r\\t\\x1b\\u{0}\\u{2028}"
        );
    }

    #[test]
    fn raw_bytes_preserve_invalid_utf8_and_terminal_escaping() {
        assert_eq!(
            sanitize_os_bytes_for_terminal(b"\\\n\x1b\xc2\x80\x80"),
            "\\\\\\n\\x1b\\u{80}\\x80"
        );
    }

    #[test]
    fn raw_byte_escaping_round_trips_adversarial_inputs() {
        for input in [
            b"plain".as_slice(),
            b"\\\n\r\t\x1b".as_slice(),
            b"\xc2\x80\x80".as_slice(),
            b"\xf0\x9f\x98".as_slice(),
            "safe\u{202e}text".as_bytes(),
        ] {
            assert_eq!(
                decode_terminal_bytes(&sanitize_os_bytes_for_terminal(input)),
                input
            );
        }
    }

    #[test]
    fn bounded_render_leaves_short_input_untruncated() {
        let rendered = sanitize_os_bytes_for_terminal_bounded(b"plain helper stderr");
        assert_eq!(rendered, "plain helper stderr");
        assert!(!rendered.contains(TRUNCATION_MARKER));
        assert!(rendered.chars().count() <= TERMINAL_RENDER_LIMIT);
    }

    #[test]
    fn bounded_render_keeps_exactly_the_limit_without_a_marker() {
        let input = "a".repeat(TERMINAL_RENDER_LIMIT);
        let rendered = sanitize_os_bytes_for_terminal_bounded(input.as_bytes());
        assert_eq!(rendered.chars().count(), TERMINAL_RENDER_LIMIT);
        assert!(!rendered.contains(TRUNCATION_MARKER));
    }

    #[test]
    fn bounded_render_caps_at_2048_scalars_including_the_marker() {
        let input = "a".repeat(TERMINAL_RENDER_LIMIT + 1);
        let rendered = sanitize_os_bytes_for_terminal_bounded(input.as_bytes());
        assert!(rendered.ends_with(TRUNCATION_MARKER));
        assert_eq!(rendered.chars().count(), TERMINAL_RENDER_LIMIT);
        assert_eq!(
            rendered.chars().count() - TRUNCATION_MARKER.chars().count(),
            2036
        );
    }

    #[test]
    fn bounded_render_caps_after_sanitize_expansion() {
        let input = b"\n".repeat(1025);
        let unbounded = sanitize_os_bytes_for_terminal(&input);
        assert!(unbounded.chars().count() > TERMINAL_RENDER_LIMIT);
        let rendered = sanitize_os_bytes_for_terminal_bounded(&input);
        assert!(rendered.ends_with(TRUNCATION_MARKER));
        assert_eq!(rendered.chars().count(), TERMINAL_RENDER_LIMIT);
    }

    #[test]
    fn truncation_marker_appears_only_when_truncated() {
        assert!(!sanitize_os_bytes_for_terminal_bounded(b"ok").contains(TRUNCATION_MARKER));
        assert!(
            sanitize_os_bytes_for_terminal_bounded("b".repeat(3000).as_bytes())
                .contains(TRUNCATION_MARKER)
        );
    }

    #[test]
    fn bounded_string_render_leaves_short_input_untruncated() {
        let rendered = sanitize_str_for_terminal_bounded("plain helper stderr");
        assert_eq!(rendered, "plain helper stderr");
        assert!(!rendered.contains(TRUNCATION_MARKER));
        assert!(rendered.chars().count() <= TERMINAL_RENDER_LIMIT);
    }

    #[test]
    fn bounded_string_render_keeps_exactly_the_limit_without_a_marker() {
        let input = "a".repeat(TERMINAL_RENDER_LIMIT);
        let rendered = sanitize_str_for_terminal_bounded(&input);
        assert_eq!(rendered.chars().count(), TERMINAL_RENDER_LIMIT);
        assert!(!rendered.contains(TRUNCATION_MARKER));
    }

    #[test]
    fn bounded_string_render_caps_at_2048_scalars_including_the_marker() {
        let input = "a".repeat(TERMINAL_RENDER_LIMIT + 1);
        let rendered = sanitize_str_for_terminal_bounded(&input);
        assert!(rendered.ends_with(TRUNCATION_MARKER));
        assert_eq!(rendered.chars().count(), TERMINAL_RENDER_LIMIT);
        assert_eq!(
            rendered.chars().count() - TRUNCATION_MARKER.chars().count(),
            2036
        );
    }

    #[test]
    fn bounded_string_render_caps_after_sanitize_expansion() {
        let input = "\n".repeat(1025);
        let unbounded = sanitize_for_terminal(&input);
        assert!(unbounded.chars().count() > TERMINAL_RENDER_LIMIT);
        let rendered = sanitize_str_for_terminal_bounded(&input);
        assert!(rendered.ends_with(TRUNCATION_MARKER));
        assert_eq!(rendered.chars().count(), TERMINAL_RENDER_LIMIT);
    }

    #[test]
    fn bounded_string_truncation_marker_appears_only_when_truncated() {
        assert!(!sanitize_str_for_terminal_bounded("ok").contains(TRUNCATION_MARKER));
        assert!(sanitize_str_for_terminal_bounded(&"b".repeat(3000)).contains(TRUNCATION_MARKER));
    }
}
