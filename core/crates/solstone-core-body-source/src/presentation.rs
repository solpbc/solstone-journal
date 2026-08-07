// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::BodyValue;

/// A lossless presentation-row attempt over any decoded body value.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentationRow(BodyValue);

impl From<BodyValue> for PresentationRow {
    fn from(value: BodyValue) -> Self {
        Self(value)
    }
}

impl PresentationRow {
    /// Returns the original decoded value without transforming it.
    pub fn value(&self) -> &BodyValue {
        &self.0
    }

    /// Recovers the original decoded value without transforming it.
    pub fn into_value(self) -> BodyValue {
        self.0
    }
}
