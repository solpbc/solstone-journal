// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::BodyString;

pub(crate) const fn is_python_whitespace(code_point: u32) -> bool {
    matches!(
        code_point,
        0x0009..=0x000d
            | 0x001c..=0x001f
            | 0x0020
            | 0x0085
            | 0x00a0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
    )
}

pub(crate) fn strip_python_whitespace(value: &BodyString) -> BodyString {
    let code_points = value.code_points();
    let start = code_points
        .iter()
        .position(|code_point| !is_python_whitespace(*code_point))
        .unwrap_or(code_points.len());
    let end = code_points
        .iter()
        .rposition(|code_point| !is_python_whitespace(*code_point))
        .map_or(start, |index| index + 1);
    BodyString::from_code_points(code_points[start..end].to_vec())
        .expect("a slice of valid body-string code points remains valid")
}
