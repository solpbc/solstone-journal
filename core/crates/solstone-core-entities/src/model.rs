// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use solstone_core_convey_http::envelope::error_envelope;
use std::sync::LazyLock;

pub const ENTITY_TYPES: [&str; 4] = ["Person", "Company", "Project", "Tool"];
pub const ATTENDANCE_KINDS: [&str; 3] = ["attended-with", "co-present", "scheduled-with"];
pub static ENTITIES_COPY: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("data/entities_copy.json"))
        .expect("entities_copy.json is valid JSON")
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasonCode {
    AgentUnavailable,
    /// Declared for oracle-vocabulary completeness; the 4 routes that would
    /// emit it (`/api/network`, `/api/history`, `/api/overview`, `/api/search`)
    /// serve `IndexPlateNotPorted` instead, so this port never constructs it.
    #[allow(dead_code)]
    EdgeIndexUnavailable,
    EntityAliasConflict,
    EntityAlreadyExists,
    EntityBlocked,
    EntityBusy,
    EntityNotFound,
    EntityOperationFailed,
    InvalidEntityType,
    InvalidRequestValue,
    MissingRequestBody,
    MissingRequiredField,
    OperationNoLongerAvailable,
    PrincipalEntityProtected,
    ResolvedChoiceEntityAbsent,
    ResolvedChoiceEntityBlocked,
    EntityAmbiguityCorrupt,
    IndexPlateNotPorted,
}

impl ReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentUnavailable => "agent_unavailable",
            Self::EdgeIndexUnavailable => "edge_index_unavailable",
            Self::EntityAliasConflict => "entity_alias_conflict",
            Self::EntityAlreadyExists => "entity_already_exists",
            Self::EntityBlocked => "entity_blocked",
            Self::EntityBusy => "entity_busy",
            Self::EntityNotFound => "entity_not_found",
            Self::EntityOperationFailed => "entity_operation_failed",
            Self::InvalidEntityType => "invalid_entity_type",
            Self::InvalidRequestValue => "invalid_request_value",
            Self::MissingRequestBody => "missing_request_body",
            Self::MissingRequiredField => "missing_required_field",
            Self::OperationNoLongerAvailable => "operation_no_longer_available",
            Self::PrincipalEntityProtected => "principal_entity_protected",
            Self::ResolvedChoiceEntityAbsent => "resolved_choice_entity_absent",
            Self::ResolvedChoiceEntityBlocked => "resolved_choice_entity_blocked",
            Self::EntityAmbiguityCorrupt => "entity_ambiguity_corrupt",
            Self::IndexPlateNotPorted => "index_plate_not_ported",
        }
    }
    pub const fn status(self) -> StatusCode {
        match self {
            Self::AgentUnavailable | Self::EdgeIndexUnavailable | Self::EntityBusy => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::EntityAliasConflict | Self::EntityAlreadyExists => StatusCode::CONFLICT,
            Self::EntityNotFound
            | Self::ResolvedChoiceEntityAbsent
            | Self::ResolvedChoiceEntityBlocked => StatusCode::NOT_FOUND,
            Self::OperationNoLongerAvailable => StatusCode::GONE,
            Self::IndexPlateNotPorted => StatusCode::NOT_IMPLEMENTED,
            Self::EntityOperationFailed | Self::EntityAmbiguityCorrupt => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Self::EntityBlocked
            | Self::InvalidEntityType
            | Self::InvalidRequestValue
            | Self::MissingRequestBody
            | Self::MissingRequiredField
            | Self::PrincipalEntityProtected => StatusCode::BAD_REQUEST,
        }
    }
}

pub fn refusal(code: ReasonCode, detail: impl Into<String>) -> Response {
    refusal_with_status(code, detail, code.status())
}

pub fn refusal_with_status(
    code: ReasonCode,
    detail: impl Into<String>,
    status: StatusCode,
) -> Response {
    error_envelope(code.as_str(), "Entity request refused", detail, status).into_response()
}
