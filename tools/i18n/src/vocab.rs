// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScalarType {
    String,
    Boolean,
    Integer,
    Cardinal,
}

impl ScalarType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScalarType::String => "string",
            ScalarType::Boolean => "boolean",
            ScalarType::Integer => "integer",
            ScalarType::Cardinal => "cardinal",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NumericStyle {
    Ungrouped,
    LocaleFormatted,
}

impl NumericStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            NumericStyle::Ungrouped => "ungrouped",
            NumericStyle::LocaleFormatted => "locale-formatted",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FormatIntent {
    Text,
    Cardinal,
    Enum,
}

impl FormatIntent {
    pub fn as_str(&self) -> &'static str {
        match self {
            FormatIntent::Text => "text",
            FormatIntent::Cardinal => "cardinal",
            FormatIntent::Enum => "enum",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocaleTier {
    Source,
    Draft,
    Test,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalePurpose {
    Ordinary,
    Test,
    Pseudo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    Development,
    Production,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseState {
    Released,
    Withheld,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtectedSlotKind {
    Command,
    Flag,
    AcceptedInput,
    Url,
    MarkWord,
    Whitespace,
}

impl ProtectedSlotKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Flag => "flag",
            Self::AcceptedInput => "accepted-input",
            Self::Url => "url",
            Self::MarkWord => "mark-word",
            Self::Whitespace => "whitespace",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Risk {
    Routine,
    Account,
    Payment,
    Legal,
    Statutory,
    Protocol,
    Consent,
}

impl Risk {
    pub fn is_high_risk(&self) -> bool {
        matches!(
            self,
            Risk::Payment | Risk::Legal | Risk::Statutory | Risk::Consent
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Routine => "routine",
            Self::Account => "account",
            Self::Payment => "payment",
            Self::Legal => "legal",
            Self::Statutory => "statutory",
            Self::Protocol => "protocol",
            Self::Consent => "consent",
        }
    }
}
