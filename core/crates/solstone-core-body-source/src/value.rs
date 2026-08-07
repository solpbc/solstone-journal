// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use crate::{BodyInteger, BodyString};

/// A JSON object with Python-compatible decoded-string keys.
pub type BodyObject = BTreeMap<BodyString, BodyValue>;

/// The value model produced by [`crate::parse`].
#[derive(Clone, Debug, PartialEq)]
pub enum BodyValue {
    Null,
    Bool(bool),
    Integer(BodyInteger),
    Number(f64),
    String(BodyString),
    Array(Vec<BodyValue>),
    Object(BodyObject),
}
