// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};

/// Closed vocabulary for push-route refusals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasonCode {
    LinkedDeviceRequired,
    PushRequestInvalid,
    PushRegistryUnavailable,
    PushLedgerUnavailable,
    FeatureUnavailable,
}

impl ReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LinkedDeviceRequired => "linked_device_required",
            Self::PushRequestInvalid => "push_request_invalid",
            Self::PushRegistryUnavailable => "push_registry_unavailable",
            Self::PushLedgerUnavailable => "push_ledger_unavailable",
            Self::FeatureUnavailable => "feature_unavailable",
        }
    }
}

/// The APNS environment declared by a device registration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushEnvironment {
    Development,
    Production,
}

impl PushEnvironment {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "development" => Some(Self::Development),
            "production" => Some(Self::Production),
            _ => None,
        }
    }
}

/// The device platform accepted by the current push contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushPlatform {
    Ios,
}

impl PushPlatform {
    pub fn parse(value: &str) -> Option<Self> {
        (value == "ios").then_some(Self::Ios)
    }
}

/// Validate that a device token has even length in 16..=200 and lowercase hex chars.
pub(crate) fn device_token_is_valid(token: &str) -> bool {
    let len = token.len();
    if !(16..=200).contains(&len) || !len.is_multiple_of(2) {
        return false;
    }
    token
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Build the masked target string (...last4).
pub(crate) fn mask_target(token: &str) -> String {
    let suffix = &token[token.len().saturating_sub(4)..];
    format!("...{suffix}")
}

/// Sanitize an upstream error reason code to ensure no tokens or sensitive data leak.
pub(crate) fn sanitize_reason(reason: &str, batch_tokens: &[&str]) -> String {
    if !reason.is_empty()
        && reason.len() <= 64
        && reason
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !(reason.len() >= 16 && reason.chars().all(|c| c.is_ascii_hexdigit()))
        && !batch_tokens.contains(&reason)
    {
        reason.to_string()
    } else {
        "unspecified".to_string()
    }
}

/// The public status representation of one registered device.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PushDeviceItem {
    pub platform: PushPlatform,
    pub target: String,
    pub environment: PushEnvironment,
    pub registered_at: String,
}

#[derive(Serialize)]
pub(crate) struct StatusResponse {
    pub items: Vec<PushDeviceItem>,
    pub total: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TestItem {
    pub target: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TestResponse {
    pub items: Vec<TestItem>,
}
