// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};

/// Closed vocabulary for push-route refusals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasonCode {
    LinkedDeviceRequired,
    PushRequestInvalid,
    PushRegistryUnavailable,
    FeatureUnavailable,
}

impl ReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LinkedDeviceRequired => "linked_device_required",
            Self::PushRequestInvalid => "push_request_invalid",
            Self::PushRegistryUnavailable => "push_registry_unavailable",
            Self::FeatureUnavailable => "feature_unavailable",
        }
    }
}

/// The APNS environment declared by a device registration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushPlatform {
    Ios,
}

impl PushPlatform {
    pub fn parse(value: &str) -> Option<Self> {
        (value == "ios").then_some(Self::Ios)
    }
}

/// The display-safe representation of one registered device.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PushDeviceStatus {
    pub bundle_id: String,
    pub environment: PushEnvironment,
    pub platform: PushPlatform,
    pub registered_at: String,
    pub device_token: String,
}

#[derive(Serialize)]
pub(crate) struct RegisterResponse {
    pub registered: bool,
}

#[derive(Serialize)]
pub(crate) struct DeregisterResponse {
    pub removed: bool,
}

#[derive(Serialize)]
pub(crate) struct StatusResponse {
    pub count: usize,
    pub devices: Vec<PushDeviceStatus>,
}

#[derive(Serialize)]
pub(crate) struct TestResponse {
    pub device_count: usize,
}
