// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::vocab::{NumericStyle, ProtectedSlotKind, ScalarType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ElementKind {
    Text,
    Arg,
    Slot,
}

impl ElementKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ElementKind::Text => "text",
            ElementKind::Arg => "arg",
            ElementKind::Slot => "slot",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternElement {
    pub kind: ElementKind,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub key: Option<String>,
    pub default: bool,
    pub pattern: Vec<PatternElement>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgumentSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub arg_type: ScalarType,
    pub numeric_style: Option<NumericStyle>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedSlotSpec {
    pub name: String,
    pub kind: ProtectedSlotKind,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectorSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub selector_type: String,
    pub enum_values: Vec<String>,
    pub enum_default: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedMessage {
    pub id: String,
    pub branches: Vec<Branch>,
    pub selector: Option<SelectorSpec>,
}
