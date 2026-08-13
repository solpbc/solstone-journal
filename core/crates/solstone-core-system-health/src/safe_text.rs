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

/// Render dynamic text without allowing it to control terminal layout.
pub fn sanitize_for_terminal(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for scalar in input.chars() {
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
    output
}

#[cfg(test)]
mod tests {
    use super::sanitize_for_terminal;

    #[test]
    fn special_escapes_take_precedence() {
        assert_eq!(
            sanitize_for_terminal("\\\n\r\t\x1b\0\u{2028}"),
            "\\\\\\n\\r\\t\\x1b\\u{0}\\u{2028}"
        );
    }
}
