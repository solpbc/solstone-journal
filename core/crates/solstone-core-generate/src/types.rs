// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use crate::fixture::known_reason_code;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentPart {
    Text { text: String },
    Image { mime_type: String, data: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateRequest {
    pub id: Option<String>,
    pub context: String,
    pub contents: Vec<ContentPart>,
    pub system_instruction: Option<String>,
    pub temperature: f64,
    pub max_output_tokens: u64,
    pub thinking_budget: Option<u64>,
    pub timeout_s: Option<f64>,
    pub json_output: bool,
    pub json_schema: Option<Value>,
    pub enforce_responsiveness: bool,
    pub attempt_index: u64,
    pub exclusive_admission: bool,
    pub transport_retries: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Generated,
    Refused,
}

impl Outcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generated => "generated",
            Self::Refused => "refused",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    AttestationNotVerified,
    AttestationFailed,
    AttestationStale,
    NoEngineConfigured,
    IncompleteJson,
    IncompleteText,
    ProviderResponseInvalid,
    SchemaValidationFailed,
    NonResponsiveOutput,
    Unknown,
}

impl RefusalReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AttestationNotVerified => "attestation-not-verified",
            Self::AttestationFailed => "attestation-failed",
            Self::AttestationStale => "attestation-stale",
            Self::NoEngineConfigured => "no-engine-configured",
            Self::IncompleteJson => "incomplete-json",
            Self::IncompleteText => "incomplete-text",
            Self::ProviderResponseInvalid => "provider-response-invalid",
            Self::SchemaValidationFailed => "schema-validation-failed",
            Self::NonResponsiveOutput => "non-responsive-output",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn from_wire(value: &str) -> Self {
        match value {
            "attestation-not-verified" => Self::AttestationNotVerified,
            "attestation-failed" => Self::AttestationFailed,
            "attestation-stale" => Self::AttestationStale,
            "no-engine-configured" => Self::NoEngineConfigured,
            "incomplete-json" => Self::IncompleteJson,
            "incomplete-text" => Self::IncompleteText,
            "provider-response-invalid" => Self::ProviderResponseInvalid,
            "schema-validation-failed" => Self::SchemaValidationFailed,
            "non-responsive-output" => Self::NonResponsiveOutput,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasonCode(String);

impl ReasonCode {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if known_reason_code(&value) {
            Ok(Self(value))
        } else {
            Err(format!("unknown generate reason code {value:?}"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownReasonCode {
    pub received: String,
    pub canonical: ReasonCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasonCodeValue {
    Known(ReasonCode),
    Unknown(UnknownReasonCode),
}

impl ReasonCodeValue {
    pub(crate) fn from_wire(value: String) -> Self {
        match ReasonCode::new(value.clone()) {
            Ok(code) => Self::Known(code),
            Err(_) => Self::Unknown(UnknownReasonCode {
                received: value,
                canonical: ReasonCode::new("unknown").expect("unknown code is in fixture"),
            }),
        }
    }

    pub fn as_wire(&self) -> &str {
        match self {
            Self::Known(code) => code.as_str(),
            Self::Unknown(code) => &code.received,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedResponse {
    pub id: Option<String>,
    pub text: String,
    pub model: String,
    pub usage: Value,
    pub finish_reason: String,
    pub thinking: Option<Value>,
    pub schema_validation: Option<Value>,
    pub input_budget: Option<Value>,
    pub request_budget: Option<Value>,
    pub inference: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefusedResponse {
    pub id: Option<String>,
    pub reason: RefusalReason,
    pub reason_code: Option<ReasonCodeValue>,
    pub retryable: bool,
    pub blocking: bool,
    pub reset_at_ms: Option<u64>,
    pub provider: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenerateResponse {
    Generated(Box<GeneratedResponse>),
    Refused(RefusedResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    pub id: Option<String>,
    pub reason: String,
    pub detail: String,
}
