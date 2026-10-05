// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Authenticated linked-device rekey and takeover wire contract.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};
use serde_json::{Value, json};

#[cfg(feature = "host")]
#[path = "device_migration_host.rs"]
mod host;

#[cfg(feature = "host")]
pub use host::{
    DecisionApply, DecisionBoundary, MigrationError, MigrationIssuanceContext, RekeyOutcome,
    decide, ingest_blocked, migration_state, pending_push_cid, record_push_result, rekey,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RekeyRequest {
    pub protocol_version: u32,
    pub operation_id: String,
    pub csr: String,
    pub device_label: String,
    pub client_label: String,
    pub platform: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    NewDevice,
    SameDevice,
    ReplaceDevice,
}

impl Choice {
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::NewDevice => "new_device",
            Self::SameDevice => "same_device",
            Self::ReplaceDevice => "replace_device",
        }
    }

    pub const fn all() -> &'static [Self] {
        &[Self::NewDevice, Self::SameDevice, Self::ReplaceDevice]
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ReplacesCid {
    #[default]
    Missing,
    Null,
    Cid(String),
}

impl ReplacesCid {
    pub const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    pub fn as_cid(&self) -> Option<&str> {
        match self {
            Self::Cid(cid) => Some(cid),
            Self::Missing | Self::Null => None,
        }
    }
}

impl Serialize for ReplacesCid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Missing | Self::Null => serializer.serialize_none(),
            Self::Cid(cid) => serializer.serialize_str(cid),
        }
    }
}

impl<'de> Deserialize<'de> for ReplacesCid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CidVisitor;
        impl<'de> Visitor<'de> for CidVisitor {
            type Value = ReplacesCid;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a CID string or null")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ReplacesCid::Null)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ReplacesCid::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                String::deserialize(deserializer).map(ReplacesCid::Cid)
            }
        }
        deserializer.deserialize_option(CidVisitor)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    pub protocol_version: u32,
    pub operation_id: String,
    pub choice: Choice,
    #[serde(default, skip_serializing_if = "ReplacesCid::is_missing")]
    pub replaces_cid: ReplacesCid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationState {
    None,
    Pending,
    NewDevice,
    SameDevice,
    ReplacedDevice,
}

impl MigrationState {
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::NewDevice => "new_device",
            Self::SameDevice => "same_device",
            Self::ReplacedDevice => "replaced_device",
        }
    }

    pub const fn all() -> &'static [Self] {
        &[
            Self::None,
            Self::Pending,
            Self::NewDevice,
            Self::SameDevice,
            Self::ReplacedDevice,
        ]
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RekeyResponse {
    pub protocol_version: u32,
    pub operation_id: String,
    pub state: MigrationState,
    pub previous_cid: String,
    pub cid: String,
    pub pairing: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationStateResponse {
    pub protocol_version: u32,
    pub rekey_operation_id: Option<String>,
    pub previous_cid: Option<String>,
    pub state: MigrationState,
    pub replaced_cid: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionResponse {
    pub protocol_version: u32,
    pub operation_id: String,
    pub state: MigrationState,
    pub previous_cid: Option<String>,
    pub cid: String,
    pub replaced_cid: Option<String>,
    pub display_label: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationReasonCode {
    MigrationRequestInvalid,
    MigrationProtocolUnsupported,
    MigrationCsrInvalid,
    MigrationKeyNotFresh,
    MigrationForbidden,
    MigrationReplayForbidden,
    PairedDeviceNotFound,
    MigrationOperationConflict,
    MigrationProofMissing,
    MigrationAlreadyDecided,
    MigrationSelfReplacement,
    MigrationTargetConflict,
    MigrationStateUnavailable,
}

impl MigrationReasonCode {
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::MigrationRequestInvalid => "migration_request_invalid",
            Self::MigrationProtocolUnsupported => "migration_protocol_unsupported",
            Self::MigrationCsrInvalid => "migration_csr_invalid",
            Self::MigrationKeyNotFresh => "migration_key_not_fresh",
            Self::MigrationForbidden => "migration_forbidden",
            Self::MigrationReplayForbidden => "migration_replay_forbidden",
            Self::PairedDeviceNotFound => "paired_device_not_found",
            Self::MigrationOperationConflict => "migration_operation_conflict",
            Self::MigrationProofMissing => "migration_proof_missing",
            Self::MigrationAlreadyDecided => "migration_already_decided",
            Self::MigrationSelfReplacement => "migration_self_replacement",
            Self::MigrationTargetConflict => "migration_target_conflict",
            Self::MigrationStateUnavailable => "migration_state_unavailable",
        }
    }

    pub const fn all() -> &'static [Self] {
        &[
            Self::MigrationRequestInvalid,
            Self::MigrationProtocolUnsupported,
            Self::MigrationCsrInvalid,
            Self::MigrationKeyNotFresh,
            Self::MigrationForbidden,
            Self::MigrationReplayForbidden,
            Self::PairedDeviceNotFound,
            Self::MigrationOperationConflict,
            Self::MigrationProofMissing,
            Self::MigrationAlreadyDecided,
            Self::MigrationSelfReplacement,
            Self::MigrationTargetConflict,
            Self::MigrationStateUnavailable,
        ]
    }
}

/// Generate the shared schema from the wire types' closed enum values.
pub fn schema_json() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Solstone device migration v1",
        "x-solstone-rust-source": "core/crates/solstone-core-sol-link/src/device_migration.rs",
        "x-solstone-regeneration-command": "cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib device_migration_contract::regenerate_device_migration_contract -- --ignored",
        "type": "object",
        "$defs": {
            "choice": {"type": "string", "enum": Choice::all().iter().map(|choice| choice.as_wire()).collect::<Vec<_>>()},
            "state": {"type": "string", "enum": MigrationState::all().iter().map(|state| state.as_wire()).collect::<Vec<_>>()},
            "reason_code": {"type": "string", "enum": MigrationReasonCode::all().iter().map(|code| code.as_wire()).collect::<Vec<_>>()},
            "rekey_request": {
                "type": "object", "additionalProperties": false,
                "required": ["protocol_version", "operation_id", "csr", "device_label", "client_label", "platform"],
                "properties": {"protocol_version": {"const": 1}, "operation_id": {"type": "string", "format": "uuid"}, "csr": {"type": "string"}, "device_label": {"type": "string", "minLength": 1, "maxLength": 80}, "client_label": {"type": "string", "minLength": 1, "maxLength": 253}, "platform": {"type": "string", "enum": ["linux", "macos", "windows", "ios", "android"]}}
            },
            "decision_request": {
                "type": "object", "additionalProperties": false,
                "required": ["protocol_version", "operation_id", "choice"],
                "properties": {"protocol_version": {"const": 1}, "operation_id": {"type": "string", "format": "uuid"}, "choice": {"$ref": "#/$defs/choice"}, "replaces_cid": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"}},
                "allOf": [
                    {"if": {"properties": {"choice": {"const": "replace_device"}}, "required": ["choice"]}, "then": {"required": ["replaces_cid"]}},
                    {"if": {"properties": {"choice": {"enum": ["new_device", "same_device"]}}, "required": ["choice"]}, "then": {"not": {"required": ["replaces_cid"]}}}
                ]
            },
            "rekey_response": {
                "type": "object", "additionalProperties": false,
                "required": ["protocol_version", "operation_id", "state", "previous_cid", "cid", "pairing"],
                "properties": {"protocol_version": {"const": 1}, "operation_id": {"type": "string", "format": "uuid"}, "state": {"const": "pending"}, "previous_cid": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"}, "cid": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"}, "pairing": {"type": "object"}}
            },
            "migration_state": {
                "type": "object", "additionalProperties": false,
                "required": ["protocol_version", "rekey_operation_id", "previous_cid", "state", "replaced_cid"],
                "properties": {"protocol_version": {"const": 1}, "rekey_operation_id": {"type": ["string", "null"]}, "previous_cid": {"type": ["string", "null"]}, "state": {"$ref": "#/$defs/state"}, "replaced_cid": {"type": ["string", "null"]}}
            },
            "decision_response": {
                "type": "object", "additionalProperties": false,
                "required": ["protocol_version", "operation_id", "state", "previous_cid", "cid", "replaced_cid", "display_label"],
                "properties": {"protocol_version": {"const": 1}, "operation_id": {"type": "string", "format": "uuid"}, "state": {"$ref": "#/$defs/state"}, "previous_cid": {"type": ["string", "null"], "pattern": "^sha256:[0-9a-f]{64}$"}, "cid": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"}, "replaced_cid": {"type": ["string", "null"], "pattern": "^sha256:[0-9a-f]{64}$"}, "display_label": {"type": "string"}}
            }
        }
    })
}

#[cfg(feature = "host")]
pub use host::contract_pairing_value;
