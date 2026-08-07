// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    BodyObject, BodyValue, CandidateError, CandidateErrorCode, CandidateErrorField, Coordinate,
};

/// A lossless presentation row over a decoded object value.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentationRow(BodyValue);

impl PresentationRow {
    /// Builds a presentation row from a decoded object value.
    pub fn new(value: &BodyValue, coordinate: &Coordinate) -> Result<Self, CandidateError> {
        match value {
            BodyValue::Object(_) => Ok(Self(value.clone())),
            _ => Err(CandidateError::new(
                coordinate,
                CandidateErrorCode::WrongType,
                CandidateErrorField::Row,
            )),
        }
    }

    pub(crate) fn object(&self) -> &BodyObject {
        match &self.0 {
            BodyValue::Object(object) => object,
            _ => {
                unreachable!("PresentationRow only ever holds an Object, enforced at construction")
            }
        }
    }

    /// Returns the original decoded value without transforming it.
    pub fn value(&self) -> &BodyValue {
        &self.0
    }

    /// Recovers the original decoded value without transforming it.
    pub fn into_value(self) -> BodyValue {
        self.0
    }
}
