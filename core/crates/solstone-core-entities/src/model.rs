// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use solstone_core_convey_http::coalesce::ResponseLocale;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::window_catalog::{
    ENGLISH, Value as CatalogValue, render, resolve_table, strings, text,
};
use std::sync::LazyLock;

pub const ENTITY_TYPES: [&str; 4] = ["Person", "Company", "Project", "Tool"];
pub const ATTENDANCE_KINDS: [&str; 3] = ["attended-with", "co-present", "scheduled-with"];
static ENTITIES_COPY_IDS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("data/entities_copy.json"))
        .expect("entities_copy.json is valid JSON")
});
pub static ENTITIES_COPY: LazyLock<Value> = LazyLock::new(|| entities_copy(ENGLISH));

pub fn entities_copy(locale: &str) -> Value {
    let mut copy = ENTITIES_COPY_IDS.clone();
    resolve_table(locale, &mut copy);
    copy
}

pub fn copy_text(locale: &str, key: &str) -> String {
    text(
        locale,
        ENTITIES_COPY_IDS[key]
            .as_str()
            .expect("entities message ID"),
    )
}

/// How connection reads see merged and deleted entities: each merged id's
/// rows count under the live entity it joined, and an id no entity holds
/// any more (a deleted one) is listed as no one. The live ids are the ones
/// a network request resolves exactly: every entity directory and every
/// identity's id. When they can't be read, every stored id is listed.
pub fn edge_aliases(journal_root: &std::path::Path) -> solstone_core_indexer_query::EdgeAliases {
    let mut aliases = solstone_core_indexer_query::EdgeAliases::default();
    for alias in solstone_core_entity::entity_edge_aliases(journal_root) {
        aliases.insert(&alias.raw, &alias.canonical, alias.name);
    }
    if let Ok(named) = solstone_core_entity::read_named_identity_groups(journal_root) {
        aliases.set_live_entities(
            named
                .groups
                .groups
                .into_iter()
                .flat_map(|(id, dirs)| std::iter::once(id).chain(dirs)),
        );
        aliases.set_entity_names(named.names);
    }
    aliases
}

pub fn compose_connections_horizon_note(earlier_days: usize) -> String {
    connections_horizon_note(ENGLISH, earlier_days)
}

pub fn connections_horizon_note(locale: &str, earlier_days: usize) -> String {
    debug_assert!(earlier_days >= 1);
    // Older API clients still receive a day placeholder in the compatibility
    // note. The journal window renders from horizon_day and horizon_earlier_days.
    let day = strings(
        locale,
        "journal.window.home.connections.horizon.day",
        [("day", "{day}")],
    );
    let quantity = i32::try_from(earlier_days)
        .map(i64::from)
        .unwrap_or_else(|_| 1_000_000 + (earlier_days % 1_000_000) as i64);
    let earlier = render(
        locale,
        "journal.window.home.connections.horizon.earlier.days",
        &[
            ("quantity", CatalogValue::Cardinal(quantity)),
            (
                "earlier_days",
                CatalogValue::String(&earlier_days.to_string()),
            ),
        ],
    );
    format!("{day} {earlier}")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasonCode {
    AgentUnavailable,
    EdgeIndexUnavailable,
    EntityAliasConflict,
    EntityAlreadyExists,
    /// The name's entity was merged into another; merges are permanent.
    EntityMerged,
    EntityRecordDamaged,
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
    OperationUndone,
    PrincipalEntityProtected,
    ResolvedChoiceEntityAbsent,
    ResolvedChoiceEntityBlocked,
    EntityAmbiguityCorrupt,
}

impl ReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentUnavailable => "agent_unavailable",
            Self::EdgeIndexUnavailable => "edge_index_unavailable",
            Self::EntityAliasConflict => "entity_alias_conflict",
            Self::EntityAlreadyExists => "entity_already_exists",
            Self::EntityMerged => "entity_merged",
            Self::EntityRecordDamaged => "entity_record_damaged",
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
            Self::OperationUndone => "operation_undone",
            Self::PrincipalEntityProtected => "principal_entity_protected",
            Self::ResolvedChoiceEntityAbsent => "resolved_choice_entity_absent",
            Self::ResolvedChoiceEntityBlocked => "resolved_choice_entity_blocked",
            Self::EntityAmbiguityCorrupt => "entity_ambiguity_corrupt",
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
            Self::EntityAliasConflict
            | Self::EntityAlreadyExists
            | Self::EntityMerged
            | Self::EntityRecordDamaged
            | Self::OperationUndone => StatusCode::CONFLICT,
            Self::EntityNotFound
            | Self::ResolvedChoiceEntityAbsent
            | Self::ResolvedChoiceEntityBlocked => StatusCode::NOT_FOUND,
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

pub(crate) fn refusal_label(locale: &str) -> String {
    text(locale, "journal.window.entities.response.refused")
}

#[derive(Clone)]
pub(crate) enum RefusalDetail {
    Raw(String),
    Merge(solstone_core_entity::EntityMergeRefusal),
    Catalog(&'static str),
    Formatted(&'static str, Vec<(&'static str, String)>),
}

impl From<String> for RefusalDetail {
    fn from(value: String) -> Self {
        Self::Raw(value)
    }
}

impl From<&str> for RefusalDetail {
    fn from(value: &str) -> Self {
        Self::Raw(value.to_owned())
    }
}

impl RefusalDetail {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Raw(reason) => reason.clone(),
            Self::Merge(reason) => reason.render(locale),
            Self::Catalog(id) => text(locale, id),
            Self::Formatted(id, arguments) => render(
                locale,
                id,
                &arguments
                    .iter()
                    .map(|(name, value)| (*name, CatalogValue::String(value)))
                    .collect::<Vec<_>>(),
            ),
        }
    }
}

#[derive(Clone)]
struct RefusalCopy {
    reason: &'static str,
    detail: RefusalDetail,
}

impl RefusalCopy {
    fn response(&self, locale: &str, status: StatusCode) -> Response {
        error_envelope(
            self.reason,
            refusal_label(locale),
            self.detail.render(locale),
            status,
        )
        .into_response()
    }
}

pub(crate) fn refusal(code: ReasonCode, detail: impl Into<RefusalDetail>) -> Response {
    refusal_with_status(code, detail, code.status())
}

pub(crate) fn refusal_with_status(
    code: ReasonCode,
    detail: impl Into<RefusalDetail>,
    status: StatusCode,
) -> Response {
    let copy = RefusalCopy {
        reason: code.as_str(),
        detail: detail.into(),
    };
    let mut response = copy.response(ENGLISH, status);
    response.extensions_mut().insert(copy);
    response
}

pub(crate) async fn localize_refusal(request: Request, next: Next) -> Response {
    let locale = request
        .extensions()
        .get::<ResponseLocale>()
        .map_or(ENGLISH, |locale| locale.0);
    let mut response = next.run(request).await;
    if let Some(copy) = response
        .extensions_mut()
        .remove::<RefusalCopy>()
        .filter(|_| locale != ENGLISH)
    {
        let localized = copy.response(locale, response.status());
        *response.body_mut() = localized.into_body();
        response.headers_mut().remove(header::CONTENT_LENGTH);
    }
    response
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
        for days in [3, 2_147_483_648] {
            assert_eq!(
                compose_connections_horizon_note(days),
                format!(
                    "connections from time in the same place go back only to {{day}}. the {days} earlier days are still in your journal; those connections haven't been built yet."
                )
            );
        }
    }
}
