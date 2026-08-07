// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A decoded JSON string represented as Unicode code points.
///
/// Its derived ordering is lexicographic over `u32` code-point values, which
/// matches Python string comparison and deliberately does not compare UTF-16
/// code units. Lone surrogate values are retained because Python accepts them
/// in `json.loads` escape sequences.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyString(Vec<u32>);

impl BodyString {
    /// Builds a string from valid Unicode scalar values and/or surrogate code points.
    pub fn from_code_points(code_points: Vec<u32>) -> Option<Self> {
        code_points
            .iter()
            .all(|code_point| *code_point <= 0x10ffff)
            .then_some(Self(code_points))
    }

    /// Returns this string's decoded code points.
    pub fn code_points(&self) -> &[u32] {
        &self.0
    }

    pub(crate) fn from_decoded(code_points: Vec<u32>) -> Self {
        debug_assert!(code_points.iter().all(|code_point| *code_point <= 0x10ffff));
        Self(code_points)
    }
}
