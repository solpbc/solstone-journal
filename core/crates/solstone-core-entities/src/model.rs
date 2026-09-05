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

pub fn compose_connections_horizon_note(earlier_days: usize) -> String {
    debug_assert!(earlier_days >= 1);
    if earlier_days == 1 {
        ENTITIES_COPY
            .get("ENT_CONN_HORIZON_ONE")
            .and_then(serde_json::Value::as_str)
            .expect("ENT_CONN_HORIZON_ONE")
            .to_owned()
    } else {
        ENTITIES_COPY
            .get("ENT_CONN_HORIZON")
            .and_then(serde_json::Value::as_str)
            .expect("ENT_CONN_HORIZON")
            .replace("{n}", &earlier_days.to_string())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasonCode {
    AgentUnavailable,
    EdgeIndexUnavailable,
    EntityAliasConflict,
    EntityAlreadyExists,
    EntityBlocked,
    EntityBusy,
    EntityNotFound,
    EntityOperationFailed,
    EntitySearchActivityUnavailable,
    EntitySearchIndexBusy,
    EntitySearchIndexStale,
    EntitySearchIndexUnavailable,
    InvalidEntityType,
    InvalidRequestValue,
    MissingRequestBody,
    MissingRequiredField,
    OperationNoLongerAvailable,
    PrincipalEntityProtected,
    ResolvedChoiceEntityAbsent,
    ResolvedChoiceEntityBlocked,
    EntityAmbiguityCorrupt,
    /// Native talent spawn is not ported; not a transient agent outage.
    TalentNotPorted,
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
            Self::EntitySearchActivityUnavailable => "entity_search_activity_unavailable",
            Self::EntitySearchIndexBusy => "entity_search_index_busy",
            Self::EntitySearchIndexStale => "entity_search_index_stale",
            Self::EntitySearchIndexUnavailable => "entity_search_index_unavailable",
            Self::InvalidEntityType => "invalid_entity_type",
            Self::InvalidRequestValue => "invalid_request_value",
            Self::MissingRequestBody => "missing_request_body",
            Self::MissingRequiredField => "missing_required_field",
            Self::OperationNoLongerAvailable => "operation_no_longer_available",
            Self::PrincipalEntityProtected => "principal_entity_protected",
            Self::ResolvedChoiceEntityAbsent => "resolved_choice_entity_absent",
            Self::ResolvedChoiceEntityBlocked => "resolved_choice_entity_blocked",
            Self::EntityAmbiguityCorrupt => "entity_ambiguity_corrupt",
            Self::TalentNotPorted => "talent_not_ported",
        }
    }
    pub const fn status(self) -> StatusCode {
        match self {
            Self::AgentUnavailable
            | Self::EdgeIndexUnavailable
            | Self::EntityBusy
            | Self::EntitySearchActivityUnavailable
            | Self::EntitySearchIndexBusy
            | Self::EntitySearchIndexStale
            | Self::EntitySearchIndexUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::EntityAliasConflict | Self::EntityAlreadyExists => StatusCode::CONFLICT,
            Self::EntityNotFound
            | Self::ResolvedChoiceEntityAbsent
            | Self::ResolvedChoiceEntityBlocked => StatusCode::NOT_FOUND,
            Self::OperationNoLongerAvailable => StatusCode::GONE,
            Self::TalentNotPorted => StatusCode::NOT_IMPLEMENTED,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_selects_one_and_horizon_templates() {
        assert_eq!(
            compose_connections_horizon_note(1),
            ENTITIES_COPY["ENT_CONN_HORIZON_ONE"].as_str().unwrap()
        );
        assert!(compose_connections_horizon_note(1).contains("{day}"));
        assert!(!compose_connections_horizon_note(1).contains("{n}"));
        assert!(compose_connections_horizon_note(3).contains("{day}"));
        assert!(!compose_connections_horizon_note(3).contains("{n}"));
    }
}
