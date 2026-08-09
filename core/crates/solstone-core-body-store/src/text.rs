// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::BodyString;

pub(crate) fn body_string_to_text(value: &BodyString) -> Option<String> {
    value
        .code_points()
        .iter()
        .copied()
        .map(char::from_u32)
        .collect()
}
