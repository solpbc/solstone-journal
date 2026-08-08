// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    BodyObject, BodyValue, CandidateError, CandidateErrorCode, CandidateErrorField, Coordinate,
};

/// A lossless presentation row over a decoded object value.
///
/// The tuple field is private, so external callers cannot construct a row directly:
///
/// ```compile_fail,E0423
/// use solstone_core_body_source::{BodyValue, PresentationRow};
///
/// let _ = PresentationRow(BodyValue::Null);
/// ```
///
/// There is no `From<BodyValue>` conversion into a row:
///
/// ```compile_fail,E0277
/// use solstone_core_body_source::{BodyValue, PresentationRow};
///
/// let _: PresentationRow = BodyValue::Null.into();
/// ```
///
/// There is no `Default` impl for a row:
///
/// ```compile_fail,E0277
/// use solstone_core_body_source::PresentationRow;
///
/// fn assert_default<T: Default>() {}
/// assert_default::<PresentationRow>();
/// ```
///
/// There is no `Deserialize` impl for a row:
///
/// ```compile_fail,E0277
/// use solstone_core_body_source::PresentationRow;
///
/// serde_json::from_str::<PresentationRow>("null");
/// ```
///
/// The only construction route is the checked constructor, and it losslessly recovers the original value:
///
/// ```
/// use solstone_core_body_source::{Coordinate, PresentationRow, parse};
///
/// let value = parse(br#"{"key":"value"}"#).unwrap();
/// let coordinate = Coordinate::new("bundle", "shard", 1);
/// let row = PresentationRow::new(&value, &coordinate).unwrap();
/// assert_eq!(row.into_value(), value);
/// ```
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
