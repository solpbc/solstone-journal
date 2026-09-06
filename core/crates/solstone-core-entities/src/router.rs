// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Extension, Path as RoutePath, Query, RawQuery, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{Local, NaiveDate};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::{ErrorEnvelope, not_found_fallback};
use solstone_core_convey_http::gate::require_access;
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_http::refusal::{MergeRepairRequired, UndoRepairRequired};
use solstone_core_entity_matching::{
    EntityNameCandidate, EntityNameMatchOutcome, find_matching_entity_detailed,
};
use solstone_core_indexer::entity_search::json_truthy;
use solstone_core_indexer_query::{
    EdgeEvidenceRequest, EdgeFilters, EdgeQueryError, IndexAccessError, IndexDegraded,
    NetworkOverviewRequest, NetworkRequest, Order, SearchHit, SearchRequest, coverage,
    indexed_entity_ids, is_safe_entity_id_component, load_edge_evidence, load_entity_network,
    load_network_overview, search,
};

use crate::deferred_delete::DeferredDeleteRegistry;
use crate::model::{
    ATTENDANCE_KINDS, ENTITIES_COPY, ENTITY_TYPES, ReasonCode, compose_connections_horizon_note,
    refusal, refusal_with_status,
};

#[derive(Clone)]
struct RouterState {
    journal_root: PathBuf,
    deferred_deletes: Arc<DeferredDeleteRegistry>,
    delete_window: Duration,
}

fn unresolved_voiceprint_encoder() -> solstone_core_entity::EncoderIdentity {
    solstone_core_entity::EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

impl Deref for RouterState {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.journal_root
    }
}

/// Build the mergeable entity and facet-curation API route surface.
pub fn api_router(journal_root: impl AsRef<Path>) -> Router {
    api_router_with_delete_window(journal_root, Duration::from_secs_f64(10.0))
}

/// Build the mergeable entity routes with an injectable deferred-delete cancellation window.
pub fn api_router_with_delete_window(
    journal_root: impl AsRef<Path>,
    delete_window: Duration,
) -> Router {
    api_router_from_state(Arc::new(RouterState {
        journal_root: journal_root.as_ref().to_path_buf(),
        deferred_deletes: Arc::new(DeferredDeleteRegistry::new()),
        delete_window,
    }))
}

fn api_router_from_state(state: Arc<RouterState>) -> Router {
    Router::new()
        .route("/app/entities/api/state", get(state_route))
        .route("/app/entities/api/network", get(index_plate_network))
        .route("/app/entities/api/history", get(index_plate_history))
        .route("/app/entities/api/overview", get(index_plate_overview))
        .route("/app/entities/api/search", get(index_plate_search))
        .route("/app/entities/api/types", get(types_route))
        .route("/app/entities/api/journal", get(journal_route))
        .route(
            "/app/entities/api/journal/summary",
            get(journal_summary_route),
        )
        .route(
            "/app/entities/api/journal/entity/{entity_id}",
            get(journal_entity_route)
                .put(update_journal_entity_route)
                .delete(deferred_delete_journal_entity_route),
        )
        .route(
            "/app/entities/api/journal/entity/{entity_id}/history",
            get(history_route),
        )
        .route(
            "/app/entities/api/journal/entity/{entity_id}/restore",
            post(restore_journal_entity_version_route),
        )
        .route(
            "/app/entities/api/journal/entity/{entity_id}/block",
            post(block_journal_entity_route),
        )
        .route(
            "/app/entities/api/journal/entity/{entity_id}/unblock",
            post(unblock_journal_entity_route),
        )
        .route(
            "/app/entities/api/cancel-delete/{pending_id}",
            post(cancel_deferred_delete_route),
        )
        .route("/app/entities/api/merge", post(merge_route))
        .route(
            "/app/entities/api/merge/{merge_id}/undo",
            post(undo_merge_route),
        )
        .route(
            "/app/entities/api/merge-candidates",
            get(merge_candidates_route),
        )
        .route(
            "/app/entities/api/record-merge-candidate",
            post(record_merge_candidate_route),
        )
        .route(
            "/app/entities/api/accept-merge-candidate",
            post(accept_merge_candidate_route),
        )
        .route(
            "/app/entities/api/dismiss-merge-candidate",
            post(dismiss_merge_candidate_route),
        )
        .route("/app/entities/api/ambiguities", get(ambiguities_route))
        .route(
            "/app/entities/api/ambiguities/{ambiguity_id}/resolve",
            post(resolve_ambiguity_route),
        )
        .route("/app/entities/api/move", post(move_route))
        .route(
            "/app/entities/api/{facet_name}",
            get(facet_route).post(create_entity_route),
        )
        .route(
            "/app/entities/api/{facet_name}/resolve",
            get(resolve_facet_entity_route),
        )
        .route(
            "/app/entities/api/{facet_name}/detected",
            get(detected_route)
                .post(detect_entity_route)
                .delete(delete_detected_route),
        )
        .route(
            "/app/entities/api/{facet_name}/update-detected",
            post(update_detected_route),
        )
        .route(
            "/app/entities/api/{facet_name}/detected/preview",
            get(detected_preview_route),
        )
        .route("/app/entities/api/{facet_name}/attach", post(attach_route))
        .route("/app/entities/api/{facet_name}/aka", post(aka_route))
        .route(
            "/app/entities/api/{facet_name}/update",
            put(update_entity_route),
        )
        .route(
            "/app/entities/api/{facet_name}/update-description",
            post(update_description_route),
        )
        .route(
            "/app/entities/api/{facet_name}/generate-description",
            post(generate_description_route),
        )
        .route("/app/entities/api/{facet_name}/assist", post(assist_route))
        .route(
            "/app/entities/api/{facet_name}/observations",
            get(observations_route),
        )
        .route(
            "/app/entities/api/{facet_name}/observe",
            post(observe_route),
        )
        .route(
            "/app/entities/api/{facet_name}/entity/{entity_id}",
            get(entity_detail_route).delete(detach_route),
        )
        .route(
            "/app/entities/api/{facet_name}/entity/{entity_id}/grid",
            get(grid_route),
        )
        .route(
            "/app/entities/api/{facet_name}/entity/{entity_id}/description",
            put(update_path_description_route),
        )
        .route(
            "/app/curation/api/facet/candidates",
            get(curation_candidates_route),
        )
        .route(
            "/app/curation/api/facet/accept",
            post(accept_facet_candidate_route),
        )
        .route(
            "/app/curation/api/facet/dismiss",
            post(dismiss_facet_candidate_route),
        )
        .with_state(state)
}

// Kept for in-crate test use only (see router_tests.rs); deliberately excluded from the crate's public API — api_router()/api_router_with_delete_window() are the real external surface.
#[allow(dead_code)]
pub(crate) fn router(journal_root: impl AsRef<Path>) -> Router {
    router_with_delete_window(journal_root, Duration::from_secs_f64(10.0))
}

#[allow(dead_code)]
pub(crate) fn router_with_delete_window(
    journal_root: impl AsRef<Path>,
    delete_window: Duration,
) -> Router {
    api_router_with_delete_window(journal_root, delete_window).fallback(not_found_fallback)
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn router_with_delete_window_and_registry(
    journal_root: impl AsRef<Path>,
    delete_window: Duration,
    deferred_deletes: Arc<DeferredDeleteRegistry>,
) -> Router {
    api_router_from_state(Arc::new(RouterState {
        journal_root: journal_root.as_ref().to_path_buf(),
        deferred_deletes,
        delete_window,
    }))
    .fallback(not_found_fallback)
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum ForcedWriteOutcome {
    Contended,
    Acquired,
}

#[cfg(test)]
thread_local! {
    static FORCED_WRITE_OUTCOME: Cell<Option<ForcedWriteOutcome>> = const { Cell::new(None) };
}

#[cfg(test)]
pub(crate) struct ForcedWriteGuard;

#[cfg(test)]
pub(crate) fn force_write_outcome(outcome: ForcedWriteOutcome) -> ForcedWriteGuard {
    FORCED_WRITE_OUTCOME.with(|cell| cell.set(Some(outcome)));
    ForcedWriteGuard
}

#[cfg(test)]
impl Drop for ForcedWriteGuard {
    fn drop(&mut self) {
        FORCED_WRITE_OUTCOME.with(|cell| cell.set(None));
    }
}

#[cfg(test)]
fn forced_lock_timeout() -> solstone_core_entity::LockError {
    solstone_core_entity::LockError::Timeout(solstone_core_entity::LockTimeout {
        path: PathBuf::from("health/locks/test"),
        timeout: Duration::from_millis(1),
    })
}

async fn run_facet_entity_write<T, F>(
    operation: F,
) -> Result<Result<T, solstone_core_facets::FacetEntityWriteError>, tokio::task::JoinError>
where
    F: FnOnce() -> Result<T, solstone_core_facets::FacetEntityWriteError> + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    {
        if let Some(ForcedWriteOutcome::Contended) = FORCED_WRITE_OUTCOME.with(|cell| cell.get()) {
            return Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
                solstone_core_facets::FacetTrustLockError::Lock(forced_lock_timeout()),
            )));
        }
    }
    solstone_core_serving::seam::run_blocking(operation).await
}

async fn run_entity_write<T, F>(
    operation: F,
) -> Result<Result<T, solstone_core_entity::EntityWriteError>, tokio::task::JoinError>
where
    F: FnOnce() -> Result<T, solstone_core_entity::EntityWriteError> + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    {
        if let Some(ForcedWriteOutcome::Contended) = FORCED_WRITE_OUTCOME.with(|cell| cell.get()) {
            return Ok(Err(solstone_core_entity::EntityWriteError::TrustLock(
                solstone_core_entity::EntityTrustLockError::Lock(forced_lock_timeout()),
            )));
        }
    }
    solstone_core_serving::seam::run_blocking(operation).await
}

async fn run_entity_lifecycle<T, F>(
    operation: F,
) -> Result<Result<T, solstone_core_entity::EntityLifecycleError>, tokio::task::JoinError>
where
    F: FnOnce() -> Result<T, solstone_core_entity::EntityLifecycleError> + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    {
        if let Some(ForcedWriteOutcome::Contended) = FORCED_WRITE_OUTCOME.with(|cell| cell.get()) {
            return Ok(Err(solstone_core_entity::EntityLifecycleError::TrustLock(
                solstone_core_entity::EntityTrustLockError::Lock(forced_lock_timeout()),
            )));
        }
    }
    solstone_core_serving::seam::run_blocking(operation).await
}

async fn run_observation_write<T, F>(
    operation: F,
) -> Result<Result<T, solstone_core_facets::ObservationWriteError>, tokio::task::JoinError>
where
    F: FnOnce() -> Result<T, solstone_core_facets::ObservationWriteError> + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    {
        if let Some(ForcedWriteOutcome::Contended) = FORCED_WRITE_OUTCOME.with(|cell| cell.get()) {
            return Ok(Err(solstone_core_facets::ObservationWriteError::TrustLock(
                solstone_core_facets::FacetTrustLockError::Lock(forced_lock_timeout()),
            )));
        }
    }
    solstone_core_serving::seam::run_blocking(operation).await
}

#[derive(Deserialize)]
struct Candidates {
    facet: Option<String>,
    status: Option<String>,
}
#[derive(Deserialize)]
struct FacetFlags {
    include_detached: Option<String>,
    include_blocked: Option<String>,
}

#[derive(Default, Deserialize)]
struct IndexPlateQuery {
    entity: Option<String>,
    peer: Option<String>,
    #[serde(default)]
    kinds: Vec<String>,
    facet: Option<String>,
    day_from: Option<String>,
    day_to: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
    evidence_limit: Option<String>,
    include_principal: Option<String>,
}

#[derive(Default, Deserialize)]
struct EntitySearchQuery {
    query: Option<String>,
    #[serde(rename = "type")]
    entity_type: Option<String>,
    facet: Option<String>,
    since: Option<String>,
    limit: Option<String>,
}

#[derive(Clone, Copy)]
enum IndexPlateRoute {
    Network,
    History,
    Overview,
}

const RESOLUTION_FUZZY_THRESHOLD: f64 = 90.0;

#[derive(Clone)]
struct EntitySearchParams {
    query: Option<String>,
    entity_type: Option<String>,
    facet: Option<String>,
    since: Option<String>,
    limit: usize,
}

#[derive(Clone, Copy)]
enum EntitySearchFailure {
    ActivityUnavailable,
    IndexBusy,
    IndexStale,
    IndexUnavailable,
}

#[derive(Clone)]
struct FacetResolutionEntity {
    entity_dir: String,
    identity: Value,
    resolution: solstone_core_entity::EntityResolutionEntity,
}

#[derive(Clone)]
struct IndexPlateEntity {
    entity_dir: String,
    written: bool,
    identity: Value,
    resolution: solstone_core_entity::EntityResolutionEntity,
}

enum FacetResolutionError {
    Facet(solstone_core_facets::FacetEntityWriteError),
    Resolution(solstone_core_entity::EntityResolutionError),
}

/// Load the entity slice shared by the read and recording resolution-door routes.
fn load_facet_resolution_entities(
    journal_root: &Path,
    facet_name: &str,
    include_blocked: bool,
) -> Result<Vec<FacetResolutionEntity>, FacetResolutionError> {
    solstone_core_facets::list_scoped_facet_entities(
        journal_root,
        facet_name,
        false,
        include_blocked,
    )
    .map_err(FacetResolutionError::Facet)
    .map(|entities| {
        entities
            .into_iter()
            .map(|entity| FacetResolutionEntity {
                entity_dir: entity.entity_dir,
                resolution: solstone_core_entity::EntityResolutionEntity {
                    id: Some(entity.entity_id),
                    name: identity_name(&entity.identity),
                    aka: identity_string_list(&entity.identity, "aka"),
                    emails: identity_string_list(&entity.identity, "emails"),
                    blocked: entity.blocked,
                },
                identity: entity.identity,
            })
            .collect()
    })
}

fn identity_name(identity: &Value) -> String {
    identity
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn identity_string_list(identity: &Value, key: &str) -> Vec<String> {
    identity
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn identity_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn load_journal_resolution_entities(
    journal_root: &Path,
) -> Result<Vec<IndexPlateEntity>, solstone_core_entity::EntityStoreError> {
    let groups = solstone_core_entity::read_identity_group_map(journal_root)?;
    let mut entity_dirs = groups.groups.into_values().flatten().collect::<Vec<_>>();
    entity_dirs.sort();
    let mut entities = Vec::new();
    for entity_dir in entity_dirs {
        let Some(identity) = solstone_core_entity::read_entity_identity(journal_root, &entity_dir)?
        else {
            continue;
        };
        let value = identity.value().clone();
        let blocked = value.get("blocked") == Some(&Value::Bool(true));
        entities.push(IndexPlateEntity {
            entity_dir: entity_dir.clone(),
            written: identity.was_written(),
            identity: value,
            resolution: solstone_core_entity::EntityResolutionEntity {
                id: Some(entity_dir),
                name: identity_name(identity.value()),
                aka: identity_string_list(identity.value(), "aka"),
                emails: identity_string_list(identity.value(), "emails"),
                blocked,
            },
        });
    }
    Ok(entities)
}

fn exact_journal_entity_dir(entities: &[IndexPlateEntity], query: &str) -> Option<String> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    if let Some(entity) = entities.iter().find(|entity| entity.entity_dir == query) {
        return Some(entity.entity_dir.clone());
    }
    entities
        .iter()
        .filter(|entity| entity.identity.get("id").and_then(Value::as_str) == Some(query))
        .min_by_key(|entity| (!entity.written, entity.entity_dir.as_str()))
        .map(|entity| entity.entity_dir.clone())
}

fn find_journal_principal_dir(
    journal_root: &Path,
) -> Result<Option<(String, Value)>, solstone_core_entity::EntityStoreError> {
    let mut entity_dirs = solstone_core_entity::read_identity_group_map(journal_root)?
        .groups
        .into_values()
        .flatten()
        .collect::<Vec<_>>();
    entity_dirs.sort();
    for entity_dir in entity_dirs {
        let Some(identity) = solstone_core_entity::read_entity_identity(journal_root, &entity_dir)?
        else {
            continue;
        };
        if identity
            .value()
            .get("is_principal")
            .is_some_and(identity_is_truthy)
        {
            return Ok(Some((entity_dir, identity.value().clone())));
        }
    }
    Ok(None)
}

fn journal_resolution_slice(
    entities: &[IndexPlateEntity],
) -> Vec<solstone_core_entity::EntityResolutionEntity> {
    entities
        .iter()
        .filter(|entity| !entity.resolution.blocked)
        .map(|entity| entity.resolution.clone())
        .collect()
}

fn index_plate_entity_as_facet(entity: &IndexPlateEntity) -> FacetResolutionEntity {
    FacetResolutionEntity {
        entity_dir: entity.entity_dir.clone(),
        identity: entity.identity.clone(),
        resolution: entity.resolution.clone(),
    }
}

fn resolution_candidate_payloads(
    candidates: &[solstone_core_entity::ResolutionCandidate],
    entities: &[FacetResolutionEntity],
) -> Vec<Value> {
    candidates
        .iter()
        .map(|candidate| {
            let entity = entities
                .iter()
                .find(|entity| entity.resolution.id.as_deref() == Some(candidate.id.as_str()));
            json!({
                "name": candidate.name,
                "id": candidate.id,
                "type": entity
                    .and_then(|entity| entity.identity.get("type"))
                    .cloned()
                    .unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn closest_resolution_candidate_payloads(
    query: &str,
    entities: &[FacetResolutionEntity],
) -> Vec<Value> {
    let sorted_query = solstone_core_entity_matching::token_sort(query);
    let mut scored: Vec<_> = entities
        .iter()
        .filter_map(|entity| {
            let id = entity.resolution.id.as_ref()?.clone();
            let score = std::iter::once(entity.resolution.name.as_str())
                .chain(entity.resolution.aka.iter().map(String::as_str))
                .filter(|choice| !choice.is_empty())
                .map(|choice| {
                    rapidfuzz::fuzz::ratio(
                        sorted_query.chars(),
                        solstone_core_entity_matching::token_sort(choice).chars(),
                    ) * 100.0
                })
                .max_by(f64::total_cmp)
                .unwrap_or(0.0);
            Some((
                score,
                entity.resolution.name.clone(),
                id,
                entity.identity.clone(),
            ))
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });

    let mut names = BTreeSet::new();
    scored
        .into_iter()
        .filter(|(_, name, _, _)| names.insert(name.clone()))
        .take(3)
        .map(|(_, name, id, identity)| {
            json!({
                "name": name,
                "id": id,
                "type": identity.get("type").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn resolution_error_response(error: FacetResolutionError) -> Response {
    match error {
        FacetResolutionError::Resolution(
            solstone_core_entity::EntityResolutionError::ResolvedChoiceEntityAbsent { .. },
        ) => refusal(
            ReasonCode::ResolvedChoiceEntityAbsent,
            "recorded entity choice is absent from this facet",
        ),
        FacetResolutionError::Resolution(
            solstone_core_entity::EntityResolutionError::ResolvedChoiceEntityBlocked { .. },
        ) => refusal(
            ReasonCode::ResolvedChoiceEntityBlocked,
            "recorded entity choice is blocked",
        ),
        FacetResolutionError::Resolution(solstone_core_entity::EntityResolutionError::Read(
            solstone_core_entity::EntityStoreError::AmbiguityInvalidRow { path, .. },
        )) => refusal(
            ReasonCode::EntityAmbiguityCorrupt,
            format!("ambiguity file {} contains a corrupt row", path.display()),
        ),
        FacetResolutionError::Facet(error) => {
            refusal(ReasonCode::EntityOperationFailed, error.to_string())
        }
        FacetResolutionError::Resolution(error) => {
            refusal(ReasonCode::EntityOperationFailed, error.to_string())
        }
    }
}

async fn resolve_facet_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet_name): RoutePath<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let Some(name) = query
        .get("name")
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
    else {
        return refusal(ReasonCode::MissingRequiredField, "name is required");
    };
    let name = name.to_owned();
    let facet_exists = root.join("facets").join(&facet_name).is_dir();
    let primary_root = Arc::clone(&root);
    let primary_facet = facet_name.clone();
    let primary_name = name.clone();
    let primary = match solstone_core_serving::seam::run_blocking(move || {
        let entities = load_facet_resolution_entities(&primary_root, &primary_facet, false)?;
        let resolution_entities: Vec<_> = entities
            .iter()
            .map(|entity| entity.resolution.clone())
            .collect();
        let resolution = solstone_core_entity::record_entity_resolution(
            &primary_root,
            &primary_name,
            &resolution_entities,
            json!({"kind":"facet","facet":primary_facet}),
            Value::Null,
            RESOLUTION_FUZZY_THRESHOLD,
            true,
        )
        .map_err(FacetResolutionError::Resolution)?;
        Ok::<_, FacetResolutionError>((entities, resolution))
    })
    .await
    {
        Ok(Ok(primary)) => primary,
        Ok(Err(error)) => return resolution_error_response(error),
        Err(_) => return refusal(ReasonCode::EntityOperationFailed, "resolution task failed"),
    };

    let (entities, resolution) = primary;
    let (blocked, blocked_name) = if resolution.outcome
        == solstone_core_entity::EntityResolutionOutcome::Resolved
    {
        (false, None)
    } else {
        let blocked_root = Arc::clone(&root);
        let blocked_facet = facet_name.clone();
        let blocked_name_query = name.clone();
        match solstone_core_serving::seam::run_blocking(move || {
            let entities = load_facet_resolution_entities(&blocked_root, &blocked_facet, true)?;
            let entities: Vec<_> = entities
                .into_iter()
                .filter(|entity| entity.resolution.blocked)
                .collect();
            let resolution_entities: Vec<_> = entities
                .iter()
                .map(|entity| entity.resolution.clone())
                .collect();
            let resolution = solstone_core_entity::record_entity_resolution(
                &blocked_root,
                &blocked_name_query,
                &resolution_entities,
                json!({"kind":"facet","facet":blocked_facet}),
                Value::Null,
                RESOLUTION_FUZZY_THRESHOLD,
                true,
            )
            .map_err(FacetResolutionError::Resolution)?;
            Ok::<_, FacetResolutionError>((entities, resolution))
        })
        .await
        {
            Ok(Ok((entities, resolution)))
                if resolution.outcome
                    == solstone_core_entity::EntityResolutionOutcome::Resolved =>
            {
                let name = resolution
                    .entity_index
                    .and_then(|index| entities.get(index))
                    .map(|entity| entity.resolution.name.clone());
                (name.is_some(), name)
            }
            Ok(Ok(_)) => (false, None),
            Ok(Err(error)) => return resolution_error_response(error),
            Err(_) => return refusal(ReasonCode::EntityOperationFailed, "resolution task failed"),
        }
    };

    let (resolved, candidates) = match resolution.outcome {
        solstone_core_entity::EntityResolutionOutcome::Resolved => (
            resolution
                .entity_index
                .and_then(|index| entities.get(index))
                .map(|entity| entity.identity.clone()),
            Vec::new(),
        ),
        solstone_core_entity::EntityResolutionOutcome::Ambiguous => (
            None,
            resolution_candidate_payloads(&resolution.candidates, &entities),
        ),
        solstone_core_entity::EntityResolutionOutcome::NoMatch => (
            None,
            closest_resolution_candidate_payloads(&name, &entities),
        ),
    };
    Json(json!({
        "facet_exists": facet_exists,
        "resolved": resolved,
        "candidates": candidates,
        "blocked": blocked,
        "blocked_name": blocked_name,
    }))
    .into_response()
}

async fn detect_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet_name): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: Value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(body) => body,
        Err(_) => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let Some(day) = body.get("day").and_then(Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "day is required");
    };
    let Some(entity_type) = body.get("type").and_then(Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "type is required");
    };
    let Some(entity_query) = body.get("entity").and_then(Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "entity is required");
    };
    let Some(description) = body.get("description").and_then(Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "description is required");
    };
    if !solstone_core_entity::is_valid_entity_type(entity_type) {
        return refusal(
            ReasonCode::InvalidEntityType,
            format!("Invalid entity type {entity_type:?}"),
        );
    }

    let day = day.to_owned();
    let entity_type = entity_type.to_owned();
    let entity_query = entity_query.to_owned();
    let description = description.to_owned();
    let primary_root = Arc::clone(&root);
    let primary_facet = facet_name.clone();
    let primary_day = day.clone();
    let primary_query = entity_query.clone();
    let primary = match solstone_core_serving::seam::run_blocking(move || {
        let entities = load_facet_resolution_entities(&primary_root, &primary_facet, false)?;
        let resolution_entities: Vec<_> = entities
            .iter()
            .map(|entity| entity.resolution.clone())
            .collect();
        let resolution = solstone_core_entity::record_entity_resolution(
            &primary_root,
            &primary_query,
            &resolution_entities,
            json!({"kind":"facet","facet":primary_facet}),
            json!({
                "lane":"apps.entities.detect",
                "facet":primary_facet,
                "day":primary_day,
                "field":"entity",
            }),
            RESOLUTION_FUZZY_THRESHOLD,
            false,
        )
        .map_err(FacetResolutionError::Resolution)?;
        Ok::<_, FacetResolutionError>((entities, resolution))
    })
    .await
    {
        Ok(Ok(primary)) => primary,
        Ok(Err(error)) => return resolution_error_response(error),
        Err(_) => return refusal(ReasonCode::EntityOperationFailed, "resolution task failed"),
    };

    let (entities, resolution) = primary;
    if resolution.outcome != solstone_core_entity::EntityResolutionOutcome::Resolved {
        let blocked_root = Arc::clone(&root);
        let blocked_facet = facet_name.clone();
        let blocked_query = entity_query.clone();
        let blocked = match solstone_core_serving::seam::run_blocking(move || {
            let entities = load_facet_resolution_entities(&blocked_root, &blocked_facet, true)?;
            let entities: Vec<_> = entities
                .into_iter()
                .filter(|entity| entity.resolution.blocked)
                .collect();
            let resolution_entities: Vec<_> = entities
                .iter()
                .map(|entity| entity.resolution.clone())
                .collect();
            let resolution = solstone_core_entity::record_entity_resolution(
                &blocked_root,
                &blocked_query,
                &resolution_entities,
                json!({"kind":"facet","facet":blocked_facet}),
                Value::Null,
                RESOLUTION_FUZZY_THRESHOLD,
                true,
            )
            .map_err(FacetResolutionError::Resolution)?;
            Ok::<_, FacetResolutionError>((entities, resolution))
        })
        .await
        {
            Ok(Ok(blocked)) => blocked,
            Ok(Err(error)) => return resolution_error_response(error),
            Err(_) => return refusal(ReasonCode::EntityOperationFailed, "resolution task failed"),
        };
        if blocked.1.outcome == solstone_core_entity::EntityResolutionOutcome::Resolved {
            let name = blocked
                .1
                .entity_index
                .and_then(|index| blocked.0.get(index))
                .map(|entity| entity.resolution.name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(&entity_query);
            return refusal(ReasonCode::EntityBlocked, name.to_owned());
        }
    }

    let name = if resolution.outcome == solstone_core_entity::EntityResolutionOutcome::Resolved {
        resolution
            .entity_index
            .and_then(|index| entities.get(index))
            .map(|entity| entity.resolution.name.clone())
            .unwrap_or_else(|| entity_query.clone())
    } else {
        entity_query.clone()
    };
    let response_name = name.clone();
    match run_facet_entity_write(move || {
        solstone_core_facets::save_detected_entity(
            &root,
            &facet_name,
            &day,
            &entity_type,
            &name,
            &description,
        )
    })
    .await
    {
        Ok(Ok(_)) => Json(json!({"success":true,"name":response_name})).into_response(),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityExists { .. }))
        | Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. })) => {
            refusal(ReasonCode::InvalidRequestValue, "invalid detected entity")
        }
        Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )))
        | Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityTrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "detected entity save failed",
        ),
    }
}

fn admitted(b: &AccessBasis) -> Option<Response> {
    (!require_access(b)).then(|| refusal(ReasonCode::AgentUnavailable, "access denied"))
}

async fn state_route(Extension(b): Extension<AccessBasis>) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    Json(json!({"entities_copy":ENTITIES_COPY.clone(),"attendance_kinds":ATTENDANCE_KINDS}))
        .into_response()
}
async fn types_route(Extension(b): Extension<AccessBasis>) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    Json(json!({"types":ENTITY_TYPES.map(|name|json!({"name":name}))})).into_response()
}
async fn facet_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(f): RoutePath<String>,
    Query(q): Query<FacetFlags>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let d = q.include_detached.as_deref() == Some("true");
    let k = q.include_blocked.as_deref() == Some("true");
    match solstone_core_serving::seam::run_blocking(move || {
        let entities = solstone_core_facets::list_scoped_facet_entities(&root, &f, d, k)?;
        let mut attached = Vec::new();
        for entity in entities {
            let mut value = entity.identity;
            let observations =
                solstone_core_facets::load_observations(&root, &f, &entity.relationship_dir)
                    .unwrap_or_default();
            let voiceprint =
                solstone_core_entity::entity_memory_path(&root, &entity.entity_id, false)
                    .map(|path| path.join("voiceprints.npz").exists())
                    .unwrap_or(false);
            let object = value
                .as_object_mut()
                .expect("identity reader returns objects");
            object.insert("observation_count".to_owned(), json!(observations.len()));
            object.insert("has_voiceprint".to_owned(), json!(voiceprint));
            let snapshot = serde_json::Value::Object(object.clone());
            object.insert(
                "last_active_ts".to_owned(),
                json!(solstone_core_entity::entity_last_active_ts(&snapshot)),
            );
            object.insert(
                "last_active_day".to_owned(),
                json!(solstone_core_entity::entity_last_active_day(&snapshot)),
            );
            attached.push(value);
        }
        let detected = solstone_core_facets::load_detected_entities_recent(&root, &f, 30)?;
        Ok::<_, solstone_core_facets::FacetEntityWriteError>((attached, detected))
    })
    .await
    {
        Ok(Ok((attached, detected))) => {
            Json(json!({"attached":attached,"detected":detected})).into_response()
        }
        _ => refusal(ReasonCode::EntityOperationFailed, "facet read failed"),
    }
}
async fn detected_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(f): RoutePath<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let day = q.get("day").cloned().unwrap_or_default();
    if day.is_empty() {
        return refusal(ReasonCode::MissingRequiredField, "day is required");
    }
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::read_detected_entities(&root, &f, &day)
    })
    .await
    {
        Ok(Ok(v)) => Json(json!({"total":v.len(),"items":v})).into_response(),
        _ => refusal(ReasonCode::EntityOperationFailed, "detected read failed"),
    }
}

async fn detected_preview_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let Some(name) = q
        .get("name")
        .map(String::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return refusal(ReasonCode::MissingRequiredField, "Entity name is required");
    };
    let entities_dir = root.join("facets").join(&facet).join("entities");
    if !entities_dir.exists() {
        return Json(json!({"success":true,"days":[]})).into_response();
    }
    let name = name.to_owned();
    match solstone_core_serving::seam::run_blocking(move || {
        let days = detected_day_stems(&entities_dir)
            .map_err(solstone_core_facets::FacetEntityWriteError::Io)?;
        let mut entries = Vec::new();
        for day in days {
            for entity in solstone_core_facets::read_detected_entities(&root, &facet, &day)? {
                if entity.get("name").and_then(serde_json::Value::as_str) == Some(name.as_str()) {
                    entries.push(json!({
                        "day": day,
                        "type": entity.get("type").and_then(serde_json::Value::as_str).unwrap_or(""),
                        "description": entity.get("description").and_then(serde_json::Value::as_str).unwrap_or(""),
                    }));
                }
            }
        }
        Ok::<_, solstone_core_facets::FacetEntityWriteError>(entries)
    })
    .await
    {
        Ok(Ok(days)) => Json(json!({"success":true,"days":days})).into_response(),
        _ => refusal(ReasonCode::EntityOperationFailed, "detected read failed"),
    }
}

async fn merge_candidates_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    Query(q): Query<Candidates>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::load_merge_candidates(&root, q.facet.as_deref(), q.status.as_deref())
    })
    .await
    {
        Ok(Ok(v)) => Json(json!({"total":v.len(),"items":v})).into_response(),
        _ => refusal(ReasonCode::EntityOperationFailed, "candidate read failed"),
    }
}
async fn record_merge_candidate_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "facet is required"),
    };
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    let Some(facet) = body.get("facet").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "facet is required");
    };
    let Some(day) = body.get("day").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "day is required");
    };
    let Some(source) = body.get("source").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "source is required");
    };
    let Some(target) = body.get("target").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "target is required");
    };
    let Some(evidence) = body.get("evidence").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "evidence is required");
    };
    let source_slug = solstone_core_entity_matching::entity_slug(source);
    let target_slug = solstone_core_entity_matching::entity_slug(target);
    if source_slug == target_slug {
        return refusal(
            ReasonCode::InvalidRequestValue,
            "source and target resolve to the same entity.",
        );
    }
    let basis = body
        .get("basis")
        .and_then(serde_json::Value::as_str)
        .filter(|basis| !basis.is_empty())
        .unwrap_or("name-variant")
        .to_owned();
    let parse_integer = |value: &serde_json::Value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
    };
    let detections = body.get("detections").and_then(parse_integer);
    let needs = body.get("needs").and_then(parse_integer);
    let facet = facet.to_owned();
    let day = day.to_owned();
    let source = source.to_owned();
    let target = target.to_owned();
    let evidence = evidence.to_owned();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::record_merge_candidate(
            &root,
            &facet,
            &day,
            &source,
            &source_slug,
            &target,
            &target_slug,
            &evidence,
            Some(&basis),
            detections,
            needs,
        )
    })
    .await
    {
        Ok(Ok((row, created))) => {
            Json(json!({"success":true,"row":row,"created":created})).into_response()
        }
        Ok(Err(error)) => {
            entity_review_candidate_error_response(error, "merge candidate record failed")
        }
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "merge candidate record failed",
        ),
    }
}

fn entity_merge_candidate_error(key: &str, error: impl Into<String>) -> Response {
    Json(json!({
        "status": "error",
        "kind": "entity_merge",
        "key": key,
        "error": error.into(),
    }))
    .into_response()
}

pub(crate) fn entity_review_candidate_error_response(
    error: solstone_core_entity::EntityReviewCandidateError,
    detail: &'static str,
) -> Response {
    match error {
        solstone_core_entity::EntityReviewCandidateError::TrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )
        | solstone_core_entity::EntityReviewCandidateError::Lock(
            solstone_core_entity::LockError::Timeout(_),
        ) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(ReasonCode::EntityOperationFailed, detail),
    }
}

fn entity_merge_undo(merge_id: Option<&str>) -> serde_json::Value {
    let merge_id = merge_id.filter(|merge_id| !merge_id.is_empty());
    json!({
        "available": merge_id.is_some(),
        "merge_id": merge_id,
        "reason": merge_id.is_none().then_some("No recorded merge id is available."),
    })
}

fn merge_report_value(report: &solstone_core_entity::EntityMergeReport) -> serde_json::Value {
    json!({
        "merge_id": report.merge_id,
        "source_id": report.source_id,
        "target_id": report.target_id,
        "completed_phases": report.completed_phases,
        "aliases_added": report.aliases_added,
        "emails_added": report.emails_added,
    })
}

fn merge_error_is_busy(error: &solstone_core_entity::EntityMergeError) -> bool {
    matches!(
        error,
        solstone_core_entity::EntityMergeError::Write(
            solstone_core_entity::EntityWriteError::TrustLock(
                solstone_core_entity::EntityTrustLockError::Lock(
                    solstone_core_entity::LockError::Timeout(_)
                )
            )
        ) | solstone_core_entity::EntityMergeError::Write(
            solstone_core_entity::EntityWriteError::AmbiguityLock(
                solstone_core_entity::LockError::Timeout(_)
            )
        )
    )
}

fn classified_operation_error(detail: String) -> Response {
    let lowered = detail.to_lowercase();
    if lowered.contains("already undone") {
        refusal(ReasonCode::OperationNoLongerAvailable, detail)
    } else if lowered.contains("not found") {
        refusal(ReasonCode::EntityNotFound, detail)
    } else if lowered.contains("blocked") {
        refusal(ReasonCode::EntityBlocked, detail)
    } else if lowered.contains("must be different") || lowered.contains("two principal") {
        refusal(ReasonCode::InvalidRequestValue, detail)
    } else if lowered.contains("lock") || lowered.contains("timed out") || lowered.contains("busy")
    {
        refusal(ReasonCode::EntityBusy, detail)
    } else {
        refusal(ReasonCode::EntityOperationFailed, detail)
    }
}

fn repair_envelope(detail: String) -> ErrorEnvelope {
    ErrorEnvelope {
        error: "Entity request refused".to_owned(),
        reason_code: ReasonCode::EntityOperationFailed.as_str().to_owned(),
        detail,
    }
}

pub(crate) fn classify_merge_error(error: &solstone_core_entity::EntityMergeError) -> Response {
    match error {
        solstone_core_entity::EntityMergeError::Failed {
            failed_phase,
            report,
            ..
        } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(MergeRepairRequired {
                envelope: repair_envelope(error.to_string()),
                failed_phase: failed_phase.clone(),
                source_id: report.source_id.clone(),
                target_id: report.target_id.clone(),
                operation_state: "partially_applied".to_owned(),
                mutation_applied: true,
                source_state: json!({"id": report.source_id}),
                target_state: json!({"id": report.target_id}),
                safe_remediation: "Contact an operator to repair this merge.".to_owned(),
            }),
        )
            .into_response(),
        _ => classified_operation_error(error.to_string()),
    }
}

fn undo_error_is_busy(error: &solstone_core_entity::EntityUndoError) -> bool {
    match error {
        solstone_core_entity::EntityUndoError::Write(
            solstone_core_entity::EntityWriteError::TrustLock(
                solstone_core_entity::EntityTrustLockError::Lock(
                    solstone_core_entity::LockError::Timeout(_),
                ),
            )
            | solstone_core_entity::EntityWriteError::AmbiguityLock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ) => true,
        solstone_core_entity::EntityUndoError::Failed {
            failed_phase,
            rollback_error,
            ..
        } if failed_phase == "trust_lock" => rollback_error.as_deref().is_some_and(|detail| {
            let lowered = detail.to_lowercase();
            lowered.contains("lock") || lowered.contains("timed out") || lowered.contains("busy")
        }),
        _ => false,
    }
}

pub(crate) fn classify_undo_error(
    error: &solstone_core_entity::EntityUndoError,
    merge_id: &str,
) -> Response {
    match error {
        solstone_core_entity::EntityUndoError::Failed { report, .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(UndoRepairRequired {
                envelope: repair_envelope(error.to_string()),
                merge_id: merge_id.to_owned(),
                source_id: report.source_id.clone(),
                target_id: report.target_id.clone(),
                operation_state: "partially_undone".to_owned(),
                mutation_applied: true,
                source_state: json!({"id": report.source_id}),
                target_state: json!({"id": report.target_id}),
                safe_remediation: "Contact an operator to repair this undo.".to_owned(),
            }),
        )
            .into_response(),
        _ => classified_operation_error(error.to_string()),
    }
}

fn body_bool(body: &serde_json::Value, name: &str, default: bool) -> bool {
    let Some(value) = body.get(name) else {
        return default;
    };
    value.as_bool().unwrap_or_else(|| {
        matches!(
            value.as_str().unwrap_or_default().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn merge_preview_value(preview: solstone_core_entity::EntityMergePreview) -> serde_json::Value {
    json!({
        "source_id": preview.source_id,
        "target_id": preview.target_id,
        "target_identity": preview.target_identity,
        "aliases_added": preview.aliases_added,
        "emails_added": preview.emails_added,
    })
}

async fn merge_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "source_slug is required"),
    };
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    let Some(source_slug) = body.get("source_slug").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "source_slug is required");
    };
    let Some(target_slug) = body.get("target_slug").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "target_slug is required");
    };
    let commit = body_bool(&body, "commit", false);
    let options = solstone_core_entity::EntityMergeOptions {
        keep_source_as_aka: body_bool(&body, "keep_source_as_aka", true),
    };
    let source_slug = source_slug.to_owned();
    let target_slug = target_slug.to_owned();
    if !commit {
        return match solstone_core_serving::seam::run_blocking(move || {
            solstone_core_entity::preview_entity_merge(&root, &source_slug, &target_slug, options)
        })
        .await
        {
            Ok(Ok(preview)) => Json(merge_preview_value(preview)).into_response(),
            Ok(Err(error)) if merge_error_is_busy(&error) => {
                refusal(ReasonCode::EntityBusy, "entity busy")
            }
            Ok(Err(error)) => classify_merge_error(&error),
            Err(_) => refusal(ReasonCode::EntityOperationFailed, "merge preview failed"),
        };
    }
    match solstone_core_serving::seam::run_blocking(move || {
        let fallback_encoder = unresolved_voiceprint_encoder();
        solstone_core_entity::commit_entity_merge(
            &root,
            &source_slug,
            &target_slug,
            options,
            &fallback_encoder,
        )
    })
    .await
    {
        Ok(Ok(report)) => {
            let merge_id = report.merge_id.clone();
            let mut result = merge_report_value(&report);
            result
                .as_object_mut()
                .expect("merge report is an object")
                .insert("undo".to_owned(), entity_merge_undo(Some(&merge_id)));
            Json(result).into_response()
        }
        Ok(Err(error)) if merge_error_is_busy(&error) => {
            refusal(ReasonCode::EntityBusy, "entity busy")
        }
        Ok(Err(error)) => classify_merge_error(&error),
        Err(_) => refusal(ReasonCode::EntityOperationFailed, "merge commit failed"),
    }
}

async fn undo_merge_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(merge_id): RoutePath<String>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let classifier_merge_id = merge_id.clone();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::undo_entity_merge(&root, &merge_id, json!("entities.merge.undo"))
    })
    .await
    {
        Ok(Ok(report)) => Json(json!({
            "merge_id": report.merge_id,
            "source_id": report.source_id,
            "target_id": report.target_id,
        }))
        .into_response(),
        Ok(Err(error)) if undo_error_is_busy(&error) => {
            refusal(ReasonCode::EntityBusy, "entity busy")
        }
        Ok(Err(error)) => classify_undo_error(&error, &classifier_merge_id),
        Err(_) => refusal(ReasonCode::EntityOperationFailed, "merge undo failed"),
    }
}

async fn accept_merge_candidate_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "facet is required"),
    };
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    let Some(facet) = body.get("facet").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "facet is required");
    };
    let Some(source_slug) = body.get("source_slug").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "source_slug is required");
    };
    let Some(target_slug) = body.get("target_slug").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "target_slug is required");
    };
    let commit = body
        .get("commit")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let facet = facet.to_owned();
    let source_slug = source_slug.to_owned();
    let target_slug = target_slug.to_owned();
    let key = format!("{facet}|{source_slug}|{target_slug}");

    let candidate = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let facet = facet.clone();
        let source_slug = source_slug.clone();
        let target_slug = target_slug.clone();
        move || {
            solstone_core_entity::load_merge_candidates(&root, Some(&facet), None).map(|rows| {
                rows.into_iter().find(|row| {
                    row.get("source_slug").and_then(serde_json::Value::as_str)
                        == Some(source_slug.as_str())
                        && row.get("target_slug").and_then(serde_json::Value::as_str)
                            == Some(target_slug.as_str())
                })
            })
        }
    })
    .await
    {
        Ok(Ok(Some(candidate))) => candidate,
        Ok(Ok(None)) => return entity_merge_candidate_error(&key, "candidate not found"),
        _ => return refusal(ReasonCode::EntityOperationFailed, "candidate read failed"),
    };
    let status = candidate
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("open");

    if !commit {
        if status != "open" {
            return entity_merge_candidate_error(
                &key,
                format!("cannot preview candidate with status {status}"),
            );
        }
        let preview = solstone_core_serving::seam::run_blocking({
            let root = Arc::clone(&root);
            let source_slug = source_slug.clone();
            let target_slug = target_slug.clone();
            move || {
                solstone_core_entity::preview_entity_merge(
                    &root,
                    &source_slug,
                    &target_slug,
                    solstone_core_entity::EntityMergeOptions::default(),
                )
            }
        })
        .await;
        return match preview {
            // `EntityMergePreview` does not expose Python's facet, segment, or
            // voiceprint statistics, so those response fields are zero-filled
            // rather than fabricated.
            Ok(Ok(preview)) => Json(json!({
                "status": "preview",
                "kind": "entity_merge",
                "key": key,
                "fields": {
                    "akas_added": preview.aliases_added,
                    "emails_added_count": preview.emails_added,
                    "facet_moved_count": 0,
                    "facet_merged_count": 0,
                    "observations_appended": 0,
                    "labels_rewritten": 0,
                    "corrections_rewritten": 0,
                    "segment_errors": [],
                    "voiceprints_added": 0,
                    "voiceprints_target_total": 0,
                },
            }))
            .into_response(),
            Ok(Err(error)) if merge_error_is_busy(&error) => {
                refusal(ReasonCode::EntityBusy, "entity busy")
            }
            Ok(Err(error)) => entity_merge_candidate_error(&key, error.to_string()),
            Err(_) => refusal(ReasonCode::EntityOperationFailed, "merge preview failed"),
        };
    }

    if status == "accepted" {
        let merge_id = candidate
            .get("merge_id")
            .and_then(serde_json::Value::as_str)
            .filter(|merge_id| !merge_id.is_empty());
        return Json(json!({
            "status": "already_accepted",
            "kind": "entity_merge",
            "key": key,
            "candidate": candidate,
            "merge_id": merge_id,
            "undo": entity_merge_undo(merge_id),
        }))
        .into_response();
    }
    if status != "open" {
        return entity_merge_candidate_error(
            &key,
            format!("cannot accept candidate with status {status}"),
        );
    }

    let report = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let source_slug = source_slug.clone();
        let target_slug = target_slug.clone();
        move || {
            let fallback_encoder = unresolved_voiceprint_encoder();
            solstone_core_entity::commit_entity_merge(
                &root,
                &source_slug,
                &target_slug,
                solstone_core_entity::EntityMergeOptions::default(),
                &fallback_encoder,
            )
        }
    })
    .await
    {
        Ok(Ok(report)) => report,
        Ok(Err(error)) if merge_error_is_busy(&error) => {
            return refusal(ReasonCode::EntityBusy, "entity busy");
        }
        Ok(Err(error)) => return entity_merge_candidate_error(&key, error.to_string()),
        Err(_) => return refusal(ReasonCode::EntityOperationFailed, "merge commit failed"),
    };
    let merge_id = report.merge_id.clone();
    let report_value = merge_report_value(&report);
    let merge_id_for_candidate = merge_id.clone();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::accept_merge_candidate(
            &root,
            &facet,
            &source_slug,
            &target_slug,
            Some(&merge_id_for_candidate),
        )
    })
    .await
    {
        Ok(Ok(candidate)) => Json(json!({
            "status": "accepted",
            "kind": "entity_merge",
            "key": key,
            "merge": report_value,
            "candidate": candidate,
            "merge_id": merge_id,
            "undo": entity_merge_undo(Some(&merge_id)),
        }))
        .into_response(),
        Ok(Err(error)) => {
            entity_review_candidate_error_response(error, "merge candidate accept failed")
        }
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "merge candidate accept failed",
        ),
    }
}

async fn dismiss_merge_candidate_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "facet is required"),
    };
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    let Some(facet) = body.get("facet").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "facet is required");
    };
    let Some(source_slug) = body.get("source_slug").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "source_slug is required");
    };
    let Some(target_slug) = body.get("target_slug").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "target_slug is required");
    };
    let facet = facet.to_owned();
    let source_slug = source_slug.to_owned();
    let target_slug = target_slug.to_owned();
    let key = format!("{facet}|{source_slug}|{target_slug}");
    let candidate = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let facet = facet.clone();
        let source_slug = source_slug.clone();
        let target_slug = target_slug.clone();
        move || {
            solstone_core_entity::load_merge_candidates(&root, Some(&facet), None).map(|rows| {
                rows.into_iter().find(|row| {
                    row.get("source_slug").and_then(serde_json::Value::as_str)
                        == Some(source_slug.as_str())
                        && row.get("target_slug").and_then(serde_json::Value::as_str)
                            == Some(target_slug.as_str())
                })
            })
        }
    })
    .await
    {
        Ok(Ok(Some(candidate))) => candidate,
        Ok(Ok(None)) => return entity_merge_candidate_error(&key, "candidate not found"),
        _ => return refusal(ReasonCode::EntityOperationFailed, "candidate read failed"),
    };
    let status = candidate
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("open");
    if status == "dismissed" {
        return Json(json!({
            "status": "already_dismissed",
            "kind": "entity_merge",
            "key": key,
            "candidate": candidate,
        }))
        .into_response();
    }
    if status != "open" {
        return entity_merge_candidate_error(
            &key,
            format!("cannot dismiss candidate with status {status}"),
        );
    }
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::dismiss_merge_candidate(&root, &facet, &source_slug, &target_slug)
    })
    .await
    {
        Ok(Ok(candidate)) => Json(json!({
            "status": "dismissed",
            "kind": "entity_merge",
            "key": key,
            "candidate": candidate,
        }))
        .into_response(),
        Ok(Err(error)) => {
            entity_review_candidate_error_response(error, "merge candidate dismiss failed")
        }
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "merge candidate dismiss failed",
        ),
    }
}

async fn curation_candidates_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::load_candidates(&root)
    })
    .await
    {
        Ok(Ok(v)) => Json(json!({"total":v.len(),"items":v})).into_response(),
        _ => refusal(ReasonCode::EntityOperationFailed, "candidate read failed"),
    }
}

fn facet_candidate_error(key: &str, error: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status":"error",
            "kind":"facet_candidate",
            "key":key,
            "error":error.into(),
        })),
    )
        .into_response()
}

fn facet_candidate_error_is_busy(error: &solstone_core_facets::FacetReviewCandidateError) -> bool {
    matches!(
        error,
        solstone_core_facets::FacetReviewCandidateError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_)
            )
        ) | solstone_core_facets::FacetReviewCandidateError::Lock(
            solstone_core_entity::LockError::Timeout(_)
        )
    )
}

fn facet_write_error_is_busy(error: &solstone_core_facets::FacetWriteError) -> bool {
    matches!(
        error,
        solstone_core_facets::FacetWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_)
            )
        )
    )
}

fn is_valid_facet_slug(slug: &str) -> bool {
    let mut characters = slug.chars();
    matches!(characters.next(), Some(character) if character.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
}

async fn accept_facet_candidate_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "Missing name_key"),
    };
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let Some(name_key) = body
        .get("name_key")
        .and_then(Value::as_str)
        .filter(|name_key| !name_key.is_empty())
    else {
        return refusal(ReasonCode::MissingRequiredField, "Missing name_key");
    };
    let name_key = name_key.to_owned();
    let candidate = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let name_key = name_key.clone();
        move || {
            solstone_core_facets::load_candidates(&root).map(|candidates| {
                candidates.into_iter().find(|candidate| {
                    candidate.get("name_key").and_then(Value::as_str) == Some(name_key.as_str())
                })
            })
        }
    })
    .await
    {
        Ok(Ok(Some(candidate))) => candidate,
        Ok(Ok(None)) => return facet_candidate_error(&name_key, "candidate not found"),
        Ok(Err(error)) if facet_candidate_error_is_busy(&error) => {
            return refusal(ReasonCode::EntityBusy, "suggestions are busy; try again");
        }
        _ => return facet_candidate_error(&name_key, "candidate read failed"),
    };
    let status = candidate
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("open");
    if status == "accepted" {
        return Json(json!({
            "status":"already_accepted",
            "kind":"facet_candidate",
            "key":name_key,
            "candidate":candidate,
        }))
        .into_response();
    }
    if status != "open" {
        return facet_candidate_error(
            &name_key,
            format!("cannot accept candidate with status {status}"),
        );
    }
    let raw_name = candidate
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let facet_slug = solstone_core_facets::facet_slug(&raw_name);
    let title = solstone_core_facets::humanize_facet_title(&raw_name);
    if !is_valid_facet_slug(&facet_slug) {
        return facet_candidate_error(
            &name_key,
            format!(
                "Invalid facet name '{facet_slug}': must be lowercase, start with a letter, and contain only letters, digits, hyphens, or underscores"
            ),
        );
    }
    match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let facet_slug = facet_slug.clone();
        move || solstone_core_facets::read_facet_declaration(&root, &facet_slug)
    })
    .await
    {
        Ok(Ok(Some(_))) => {
            return facet_candidate_error(
                &name_key,
                format!("Facet '{facet_slug}' already exists"),
            );
        }
        Ok(Ok(None)) => {}
        _ => return facet_candidate_error(&name_key, "facet lookup failed"),
    }
    match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let facet_slug = facet_slug.clone();
        let title = title.clone();
        move || solstone_core_facets::create_facet(&root, &facet_slug, &title, "", "", "", None)
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) if facet_write_error_is_busy(&error) => {
            return refusal(ReasonCode::EntityBusy, "suggestions are busy; try again");
        }
        Ok(Err(error)) => return facet_candidate_error(&name_key, error.to_string()),
        Err(_) => return facet_candidate_error(&name_key, "facet creation failed"),
    }
    let response_key = name_key.clone();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::accept_candidate(&root, &name_key)
    })
    .await
    {
        Ok(Ok(Some(candidate))) => Json(json!({
            "status":"accepted",
            "kind":"facet_candidate",
            "key":response_key,
            "facet_slug":facet_slug,
            "candidate":candidate,
        }))
        .into_response(),
        Ok(Ok(None)) => facet_candidate_error(&response_key, "candidate not found"),
        Ok(Err(error)) if facet_candidate_error_is_busy(&error) => {
            refusal(ReasonCode::EntityBusy, "suggestions are busy; try again")
        }
        _ => facet_candidate_error(&response_key, "candidate accept failed"),
    }
}

async fn dismiss_facet_candidate_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "Missing name_key"),
    };
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let Some(name_key) = body
        .get("name_key")
        .and_then(Value::as_str)
        .filter(|name_key| !name_key.is_empty())
    else {
        return refusal(ReasonCode::MissingRequiredField, "Missing name_key");
    };
    let name_key = name_key.to_owned();
    let candidate = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let name_key = name_key.clone();
        move || {
            solstone_core_facets::load_candidates(&root).map(|candidates| {
                candidates.into_iter().find(|candidate| {
                    candidate.get("name_key").and_then(Value::as_str) == Some(name_key.as_str())
                })
            })
        }
    })
    .await
    {
        Ok(Ok(Some(candidate))) => candidate,
        Ok(Ok(None)) => return facet_candidate_error(&name_key, "candidate not found"),
        Ok(Err(error)) if facet_candidate_error_is_busy(&error) => {
            return refusal(ReasonCode::EntityBusy, "suggestions are busy; try again");
        }
        _ => return facet_candidate_error(&name_key, "candidate read failed"),
    };
    let status = candidate
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("open");
    if status == "dismissed" {
        return Json(json!({
            "status":"already_dismissed",
            "kind":"facet_candidate",
            "key":name_key,
            "candidate":candidate,
        }))
        .into_response();
    }
    if status != "open" {
        return facet_candidate_error(
            &name_key,
            format!("cannot dismiss candidate with status {status}"),
        );
    }
    let response_key = name_key.clone();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::dismiss_candidate(&root, &name_key)
    })
    .await
    {
        Ok(Ok(Some(candidate))) => Json(json!({
            "status":"dismissed",
            "kind":"facet_candidate",
            "key":response_key,
            "candidate":candidate,
        }))
        .into_response(),
        Ok(Ok(None)) => facet_candidate_error(&response_key, "candidate not found"),
        Ok(Err(error)) if facet_candidate_error_is_busy(&error) => {
            refusal(ReasonCode::EntityBusy, "suggestions are busy; try again")
        }
        _ => facet_candidate_error(&response_key, "candidate dismiss failed"),
    }
}

fn assemble_journal_entity_records(
    root: &Path,
    only: Option<&str>,
) -> Result<Vec<Value>, solstone_core_facets::FacetEntityWriteError> {
    let groups = solstone_core_entity::read_identity_group_map(root)?;
    let mut records = Vec::new();
    for entity_dir in groups.groups.into_values().flatten() {
        if only.is_some_and(|requested| requested != entity_dir.as_str()) {
            continue;
        }
        let identity = match solstone_core_entity::read_entity_identity(root, &entity_dir) {
            Ok(Some(identity)) => identity,
            Ok(None) | Err(_) => continue,
        };
        let value = identity.value();
        records.push(json!({
            "id": entity_dir,
            "name": value.get("name").cloned().unwrap_or_else(|| json!("")),
            "type": value.get("type").cloned().unwrap_or_else(|| json!("")),
            "aka": value.get("aka").cloned().unwrap_or_else(|| json!([])),
            "is_principal": value.get("is_principal").cloned().unwrap_or_else(|| json!(false)),
            "blocked": value.get("blocked").cloned().unwrap_or_else(|| json!(false)),
            "facets": [],
            "total_observation_count": 0,
            "last_active_ts": 0,
            "last_active_day": Value::Null,
        }));
    }

    let record_indexes: HashMap<_, _> = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            record
                .get("id")
                .and_then(Value::as_str)
                .map(|entity_dir| (entity_dir.to_owned(), index))
        })
        .collect();
    let mut aggregates = HashMap::<String, (usize, i64)>::new();
    for facet_dir in solstone_core_facets::list_facet_directories(root)? {
        let declaration = match solstone_core_facets::read_facet_declaration(root, &facet_dir) {
            Ok(Some(declaration)) => declaration,
            Ok(None) | Err(_) => continue,
        };
        let declaration = declaration.value();
        let title = declaration
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(&facet_dir);
        let color = declaration
            .get("color")
            .and_then(Value::as_str)
            .unwrap_or("");
        let emoji = declaration
            .get("emoji")
            .and_then(Value::as_str)
            .unwrap_or("");
        for scoped in
            solstone_core_facets::list_scoped_facet_entities_tolerant(root, &facet_dir, true, true)?
        {
            let Some(&record_index) = record_indexes.get(&scoped.entity_dir) else {
                continue;
            };
            let observation_count = solstone_core_facets::count_observations(
                root,
                &facet_dir,
                &scoped.relationship_dir,
            )?;
            let relationship = &scoped.relationship;
            let activity_ts = solstone_core_entity::entity_last_active_ts(relationship);
            let mut facet = json!({
                "name": facet_dir,
                "title": title,
                "color": color,
                "emoji": emoji,
                "description": relationship.get("description").cloned().unwrap_or_else(|| json!("")),
                "last_seen": relationship.get("last_seen").cloned().unwrap_or(Value::Null),
                "attached_at": relationship.get("attached_at").cloned().unwrap_or(Value::Null),
                "updated_at": relationship.get("updated_at").cloned().unwrap_or(Value::Null),
                "observation_count": observation_count,
                "has_voiceprint": root.join("entities").join(&scoped.entity_dir).join("voiceprints.npz").exists(),
                "last_active_ts": activity_ts,
                "last_active_day": solstone_core_entity::entity_last_active_day(relationship),
            });
            if scoped.detached {
                facet
                    .as_object_mut()
                    .expect("facet record is an object")
                    .insert("detached".to_owned(), Value::Bool(true));
            } else {
                let aggregate = aggregates.entry(scoped.entity_dir.clone()).or_default();
                aggregate.0 += observation_count;
                aggregate.1 = aggregate.1.max(activity_ts);
            }
            records[record_index]
                .get_mut("facets")
                .and_then(Value::as_array_mut)
                .expect("entity record has facets")
                .push(facet);
        }
    }
    for record in &mut records {
        let entity_dir = record["id"].as_str().expect("entity record has id");
        let (observation_count, activity_ts) =
            aggregates.get(entity_dir).copied().unwrap_or_default();
        let facets = record["facets"]
            .as_array_mut()
            .expect("entity record has facets");
        facets
            .sort_by_key(|facet| std::cmp::Reverse(facet["last_active_ts"].as_i64().unwrap_or(0)));
        let object = record.as_object_mut().expect("entity record is an object");
        object.insert(
            "total_observation_count".to_owned(),
            json!(observation_count),
        );
        object.insert("last_active_ts".to_owned(), json!(activity_ts));
        object.insert(
            "last_active_day".to_owned(),
            if activity_ts == 0 {
                Value::Null
            } else {
                json!(solstone_core_entity::last_active_day_for_ts(activity_ts))
            },
        );
    }
    Ok(records)
}

/// Card-sized projection of the journal entity graph, paged for the list view.
///
/// The full `/app/entities/api/journal` read stays available for callers that
/// need every facet description; the list surface only needs what a card
/// draws, so this route carries names, bucketed types, badge counts and dot
/// colors and nothing else.
const JOURNAL_SUMMARY_DEFAULT_LIMIT: usize = 60;
const JOURNAL_SUMMARY_MAX_LIMIT: usize = 100;
/// Canonical filter buckets, in display order. Every stored type outside this
/// set collapses into [`JOURNAL_OTHER_BUCKET`] rather than becoming its own
/// filter option.
const JOURNAL_TYPE_BUCKETS: [&str; 4] = ["person", "company", "project", "tool"];
const JOURNAL_OTHER_BUCKET: &str = "other";

fn journal_type_bucket(stored: &str) -> &'static str {
    let normalized = stored.trim().to_lowercase();
    JOURNAL_TYPE_BUCKETS
        .into_iter()
        .find(|bucket| *bucket == normalized)
        .unwrap_or(JOURNAL_OTHER_BUCKET)
}

fn journal_bucket_rank(bucket: &str) -> usize {
    JOURNAL_TYPE_BUCKETS
        .iter()
        .position(|candidate| *candidate == bucket)
        .unwrap_or(JOURNAL_TYPE_BUCKETS.len())
}

#[derive(Deserialize)]
struct JournalSummaryQuery {
    offset: Option<String>,
    limit: Option<String>,
    #[serde(rename = "type")]
    entity_type: Option<String>,
    q: Option<String>,
    sort: Option<String>,
}

struct JournalSummaryParams {
    offset: usize,
    limit: usize,
    bucket: Option<String>,
    needle: Option<String>,
    by_name: bool,
}

fn parse_journal_summary_query(
    query: &JournalSummaryQuery,
) -> Result<JournalSummaryParams, Box<Response>> {
    let offset = index_plate_integer(query.offset.as_deref(), "offset", 0)?;
    let limit = index_plate_integer(
        query.limit.as_deref(),
        "limit",
        JOURNAL_SUMMARY_DEFAULT_LIMIT as i64,
    )?;
    if offset < 0 {
        return Err(Box::new(refusal(
            ReasonCode::InvalidRequestValue,
            "offset must not be negative",
        )));
    }
    if limit < 0 || limit > JOURNAL_SUMMARY_MAX_LIMIT as i64 {
        return Err(Box::new(refusal(
            ReasonCode::InvalidRequestValue,
            format!("limit must be between 0 and {JOURNAL_SUMMARY_MAX_LIMIT}"),
        )));
    }
    let bucket =
        normalize_entity_search_value(query.entity_type.clone()).map(|value| value.to_lowercase());
    if let Some(bucket) = bucket.as_deref()
        && bucket != JOURNAL_OTHER_BUCKET
        && !JOURNAL_TYPE_BUCKETS.contains(&bucket)
    {
        return Err(Box::new(refusal(
            ReasonCode::InvalidEntityType,
            "type must be a canonical entity type or other",
        )));
    }
    let by_name = match query.sort.as_deref().map(str::trim) {
        None | Some("") | Some("activity") => false,
        Some("name") => true,
        Some(_) => {
            return Err(Box::new(refusal(
                ReasonCode::InvalidRequestValue,
                "sort must be activity or name",
            )));
        }
    };
    Ok(JournalSummaryParams {
        offset: usize::try_from(offset).unwrap_or_default(),
        limit: usize::try_from(limit).unwrap_or_default(),
        bucket,
        needle: normalize_entity_search_value(query.q.clone()).map(|value| value.to_lowercase()),
        by_name,
    })
}

struct JournalSummaryRow {
    rank: usize,
    last_active_ts: i64,
    name_key: String,
    bucket: &'static str,
    item: Value,
}

fn summarize_journal_entities(
    root: &Path,
    params: &JournalSummaryParams,
) -> Result<Value, solstone_core_facets::FacetEntityWriteError> {
    let records = assemble_journal_entity_records(root, None)?;
    let entity_total = records.len();
    let mut type_counts: HashMap<&'static str, usize> = HashMap::new();
    let mut facet_counts: HashMap<String, usize> = HashMap::new();
    let mut rows: Vec<JournalSummaryRow> = Vec::with_capacity(entity_total);
    for record in &records {
        let stored_type = record.get("type").and_then(Value::as_str).unwrap_or("");
        let bucket = journal_type_bucket(stored_type);
        *type_counts.entry(bucket).or_default() += 1;
        let blocked = record
            .get("blocked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let empty = Vec::new();
        let facets = record
            .get("facets")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        let mut dots = Vec::with_capacity(facets.len());
        let mut memberships = BTreeSet::new();
        for facet in facets {
            let detached = facet
                .get("detached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let name = facet.get("name").and_then(Value::as_str).unwrap_or("");
            if !detached && !blocked && !name.is_empty() {
                memberships.insert(name.to_owned());
            }
            let mut dot = json!({
                "name": name,
                "title": facet.get("title").cloned().unwrap_or_else(|| json!("")),
                "color": facet.get("color").cloned().unwrap_or_else(|| json!("")),
            });
            if detached {
                dot.as_object_mut()
                    .expect("facet dot is an object")
                    .insert("detached".to_owned(), Value::Bool(true));
            }
            dots.push(dot);
        }
        for name in memberships {
            *facet_counts.entry(name).or_default() += 1;
        }
        let name = record.get("name").and_then(Value::as_str).unwrap_or("");
        let last_active_ts = record
            .get("last_active_ts")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        rows.push(JournalSummaryRow {
            rank: journal_bucket_rank(bucket),
            last_active_ts,
            name_key: name.to_lowercase(),
            bucket,
            item: json!({
                "id": record.get("id").cloned().unwrap_or_else(|| json!("")),
                "name": name,
                "type": stored_type,
                "type_bucket": bucket,
                "is_principal": record.get("is_principal").cloned().unwrap_or_else(|| json!(false)),
                "blocked": blocked,
                "total_observation_count": record.get("total_observation_count").cloned().unwrap_or_else(|| json!(0)),
                "last_active_ts": last_active_ts,
                "last_active_day": record.get("last_active_day").cloned().unwrap_or(Value::Null),
                "facets": dots,
            }),
        });
    }
    rows.retain(|row| {
        params
            .bucket
            .as_deref()
            .is_none_or(|bucket| bucket == row.bucket)
            && params
                .needle
                .as_deref()
                .is_none_or(|needle| row.name_key.contains(needle))
    });
    if params.by_name {
        rows.sort_by(|left, right| {
            left.name_key
                .cmp(&right.name_key)
                .then_with(|| right.last_active_ts.cmp(&left.last_active_ts))
        });
    } else {
        rows.sort_by(|left, right| {
            left.rank
                .cmp(&right.rank)
                .then_with(|| right.last_active_ts.cmp(&left.last_active_ts))
                .then_with(|| left.name_key.cmp(&right.name_key))
        });
    }
    let total = rows.len();
    let items: Vec<Value> = rows
        .into_iter()
        .skip(params.offset)
        .take(params.limit)
        .map(|row| row.item)
        .collect();
    let type_totals: Vec<Value> = JOURNAL_TYPE_BUCKETS
        .into_iter()
        .chain(std::iter::once(JOURNAL_OTHER_BUCKET))
        .filter_map(|bucket| {
            type_counts
                .get(bucket)
                .map(|count| json!({"bucket": bucket, "count": count}))
        })
        .collect();
    Ok(json!({
        "items": items,
        "total": total,
        "offset": params.offset,
        "limit": params.limit,
        "entity_total": entity_total,
        "type_counts": type_totals,
        "facet_counts": facet_counts,
    }))
}

async fn journal_summary_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    Query(query): Query<JournalSummaryQuery>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let params = match parse_journal_summary_query(&query) {
        Ok(params) => params,
        Err(response) => return *response,
    };
    match solstone_core_serving::seam::run_blocking(move || {
        summarize_journal_entities(&root, &params)
    })
    .await
    {
        Ok(Ok(summary)) => Json(summary).into_response(),
        _ => refusal(ReasonCode::EntityOperationFailed, "journal read failed"),
    }
}

async fn journal_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        match solstone_core_entity::read_entity_identity(&root, &id) {
            Err(error) => Err(error.into()),
            Ok(None) => Ok(None),
            Ok(Some(_)) => {
                assemble_journal_entity_records(&root, Some(&id)).map(|mut records| records.pop())
            }
        }
    })
    .await
    {
        Ok(Ok(entity)) => match entity {
            Some(entity) => Json(json!({"entity":entity})).into_response(),
            None => refusal(ReasonCode::EntityNotFound, "entity not found"),
        },
        _ => refusal(ReasonCode::EntityOperationFailed, "entity read failed"),
    }
}

fn random_pending_id() -> Option<String> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

async fn deferred_delete_journal_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(state): State<Arc<RouterState>>,
    RoutePath(entity_id): RoutePath<String>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let root = Arc::clone(&state);
    let identity_id = entity_id.clone();
    let identity = match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::read_entity_identity(&root, &identity_id)
    })
    .await
    {
        Ok(Ok(Some(identity))) => identity.value().clone(),
        Ok(Ok(None)) => {
            return refusal_with_status(
                ReasonCode::EntityNotFound,
                "entity not found",
                StatusCode::BAD_REQUEST,
            );
        }
        _ => return refusal(ReasonCode::EntityOperationFailed, "entity read failed"),
    };
    if identity.get("is_principal") == Some(&Value::Bool(true)) {
        return refusal(
            ReasonCode::PrincipalEntityProtected,
            "Cannot delete the principal (self) entity",
        );
    }
    let Some(pending_id) = random_pending_id() else {
        return refusal(
            ReasonCode::EntityOperationFailed,
            "unable to create a pending delete id",
        );
    };
    let commit_at_ms = unix_time_ms().saturating_add(
        state
            .delete_window
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    state.deferred_deletes.schedule(
        state.journal_root.clone(),
        entity_id,
        pending_id.clone(),
        state.delete_window,
    );
    Json(json!({
        "success": true,
        "pending": pending_id,
        "commit_at_ms": commit_at_ms,
        "ttl_seconds": state.delete_window.as_secs_f64(),
    }))
    .into_response()
}

async fn cancel_deferred_delete_route(
    Extension(b): Extension<AccessBasis>,
    State(state): State<Arc<RouterState>>,
    RoutePath(pending_id): RoutePath<String>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    if pending_id.len() != 32
        || !pending_id.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
        })
    {
        return refusal(
            ReasonCode::OperationNoLongerAvailable,
            "already committed or unknown",
        );
    }
    if !state.deferred_deletes.cancel(&pending_id) {
        return refusal(
            ReasonCode::OperationNoLongerAvailable,
            "already committed or unknown",
        );
    }
    let _ = crate::action_log::cancelled(&state.journal_root, &pending_id);
    Json(json!({"cancelled":pending_id})).into_response()
}

async fn generate_description_route(
    Extension(b): Extension<AccessBasis>,
    RoutePath(_facet_name): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: Value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(body) if body.as_object().is_some_and(|body| !body.is_empty()) => body,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let entity_type = body
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let entity_name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if entity_type.is_empty() || entity_name.is_empty() {
        return refusal(
            ReasonCode::MissingRequiredField,
            "Type and name are required",
        );
    }
    refusal(
        ReasonCode::TalentNotPorted,
        "This entity talent route is not ported yet.",
    )
}

async fn assist_route(
    Extension(b): Extension<AccessBasis>,
    RoutePath(_facet_name): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: Value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(body) if body.as_object().is_some_and(|body| !body.is_empty()) => body,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    if body
        .get("name")
        .and_then(Value::as_str)
        .is_none_or(|name| name.trim().is_empty())
    {
        return refusal(ReasonCode::MissingRequiredField, "Entity name is required");
    }
    refusal(
        ReasonCode::TalentNotPorted,
        "This entity talent route is not ported yet.",
    )
}

async fn update_journal_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(entity_id): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(body) if body.as_object().is_some_and(|body| !body.is_empty()) => body,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let existing = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let entity_id = entity_id.clone();
        move || solstone_core_entity::read_entity_identity(&root, &entity_id)
    })
    .await
    {
        Ok(Ok(Some(existing))) => existing.value().clone(),
        Ok(Ok(None)) => {
            return refusal(
                ReasonCode::EntityNotFound,
                format!("Entity '{entity_id}' not found"),
            );
        }
        _ => return refusal(ReasonCode::EntityOperationFailed, "entity read failed"),
    };
    let mut updated = existing.clone();
    {
        let object = updated
            .as_object_mut()
            .expect("identity reader returns objects");

        if let Some(name) = body
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .filter(|name| existing.get("name").and_then(serde_json::Value::as_str) != Some(*name))
        {
            object.insert("name".to_owned(), json!(name));
        }
        if let Some(kind) = body
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
        {
            if !solstone_core_entity::is_valid_entity_type(kind) {
                return refusal(
                    ReasonCode::InvalidEntityType,
                    format!("Invalid entity type: {kind}"),
                );
            }
            if existing.get("type").and_then(serde_json::Value::as_str) != Some(kind) {
                object.insert("type".to_owned(), json!(kind));
            }
        }
        if let Some(aka) = body.get("aka") {
            let new_akas: Vec<String> = match aka {
                serde_json::Value::String(akas) => akas
                    .split(',')
                    .map(str::trim)
                    .filter(|aka| !aka.is_empty())
                    .map(str::to_owned)
                    .collect(),
                serde_json::Value::Array(akas) => akas
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                _ => Vec::new(),
            };
            let old_akas: BTreeSet<_> = existing
                .get("aka")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .collect();
            let new_aka_set: BTreeSet<_> = new_akas.iter().map(String::as_str).collect();
            if new_aka_set != old_akas {
                object.insert(
                    "aka".to_owned(),
                    serde_json::Value::Array(
                        new_akas
                            .into_iter()
                            .map(serde_json::Value::String)
                            .collect(),
                    ),
                );
            }
        }
    }
    if updated == existing {
        return Json(json!({"success":true,"message":"No changes made"})).into_response();
    }
    updated
        .as_object_mut()
        .expect("identity reader returns objects")
        .insert(
            "updated_at".to_owned(),
            json!(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64
            ),
        );
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::save_entity_identity(&root, &entity_id, &updated, None)
            .map(|_| updated)
    })
    .await
    {
        Ok(Ok(entity)) => Json(json!({"success":true,"entity":entity})).into_response(),
        Ok(Err(solstone_core_entity::EntityWriteError::TrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(ReasonCode::EntityOperationFailed, "entity update failed"),
    }
}

async fn restore_journal_entity_version_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(entity_id): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "version_id is required"),
    };
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    let Some(version_id) = body.get("version_id").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "version_id is required");
    };
    let version_id = version_id.to_owned();
    let present = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let entity_id = entity_id.clone();
        move || solstone_core_entity::read_entity_identity(&root, &entity_id)
    })
    .await
    {
        Ok(Ok(Some(_))) => true,
        Ok(Ok(None)) => false,
        _ => return refusal(ReasonCode::EntityOperationFailed, "entity read failed"),
    };
    if !present {
        return refusal(ReasonCode::EntityNotFound, entity_id);
    }
    match run_entity_lifecycle(move || {
        let event = solstone_core_entity::restore_journal_entity_version(
            &root,
            &entity_id,
            &version_id,
            Some(json!("entities.restore-version")),
        )?;
        let entity =
            solstone_core_entity::read_entity_identity(&root, &entity_id)?.ok_or_else(|| {
                solstone_core_entity::EntityLifecycleError::EntityNotFound {
                    entity_id: entity_id.clone(),
                }
            })?;
        Ok::<_, solstone_core_entity::EntityLifecycleError>((event, entity.value().clone()))
    })
    .await
    {
        Ok(Ok((event, entity))) => {
            Json(json!({"restored":true,"entity":entity,"event":event})).into_response()
        }
        Ok(Err(solstone_core_entity::EntityLifecycleError::TrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )))
        | Ok(Err(solstone_core_entity::EntityLifecycleError::Write(
            solstone_core_entity::EntityWriteError::TrustLock(
                solstone_core_entity::EntityTrustLockError::Lock(
                    solstone_core_entity::LockError::Timeout(_),
                ),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        Ok(Err(
            solstone_core_entity::EntityLifecycleError::EntityNotFound { .. }
            | solstone_core_entity::EntityLifecycleError::HistoryVersionNotFound { .. },
        )) => refusal(ReasonCode::EntityNotFound, "history version was not found"),
        Ok(Err(
            solstone_core_entity::EntityLifecycleError::RestoreTargetsRecordedMerge
            | solstone_core_entity::EntityLifecycleError::RestoreCrossesRecordedMerge
            | solstone_core_entity::EntityLifecycleError::RestoreWouldCreateSecondPrincipal {
                ..
            },
        )) => refusal(ReasonCode::InvalidRequestValue, "restore is not available"),
        _ => refusal(ReasonCode::EntityOperationFailed, "entity restore failed"),
    }
}

async fn block_journal_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(entity_id): RoutePath<String>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::block_journal_entity(&root, &entity_id)
    })
    .await
    {
        Ok(Ok(report)) => Json(json!({"facets_detached":report.facets_detached})).into_response(),
        Ok(Err(
            error @ solstone_core_facets::FacetEntityLifecycleError::PrincipalEntityProtected {
                ..
            },
        ))
        | Ok(Err(
            error @ solstone_core_facets::FacetEntityLifecycleError::Entity(
                solstone_core_entity::EntityLifecycleError::EntityNotFound { .. },
            ),
        )) => refusal_with_status(
            ReasonCode::EntityOperationFailed,
            error.to_string(),
            axum::http::StatusCode::BAD_REQUEST,
        ),
        Ok(Err(error)) => refusal(ReasonCode::EntityOperationFailed, error.to_string()),
        Err(_) => refusal(ReasonCode::EntityOperationFailed, "entity block failed"),
    }
}

async fn unblock_journal_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(entity_id): RoutePath<String>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::unblock_journal_entity(&root, &entity_id)
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error @ solstone_core_entity::EntityLifecycleError::EntityNotFound { .. }))
        | Ok(Err(error @ solstone_core_entity::EntityLifecycleError::EntityNotBlocked { .. })) => {
            refusal_with_status(
                ReasonCode::EntityOperationFailed,
                error.to_string(),
                axum::http::StatusCode::BAD_REQUEST,
            )
        }
        Ok(Err(error)) => refusal(ReasonCode::EntityOperationFailed, error.to_string()),
        Err(_) => refusal(ReasonCode::EntityOperationFailed, "entity unblock failed"),
    }
}

async fn journal_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        assemble_journal_entity_records(&root, None)
    })
    .await
    {
        Ok(Ok(records)) => Json(json!({"entities":records})).into_response(),
        _ => refusal(ReasonCode::EntityOperationFailed, "journal read failed"),
    }
}

#[derive(Deserialize)]
struct AmbiguityQuery {
    status: Option<String>,
}

async fn ambiguities_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    Query(query): Query<AmbiguityQuery>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    if !matches!(
        query.status.as_deref(),
        None | Some("") | Some("open") | Some("resolved")
    ) {
        return refusal(
            ReasonCode::InvalidRequestValue,
            "status must be open or resolved",
        );
    }
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_entity::read_ambiguities(&root, solstone_core_entity::MalformedPolicy::Raise)
    })
    .await
    {
        Ok(Ok(v)) => {
            let rows: Vec<_> = match query.status.as_deref().filter(|value| !value.is_empty()) {
                Some(status) => v
                    .into_iter()
                    .filter(|row| {
                        row.get("status").and_then(serde_json::Value::as_str) == Some(status)
                    })
                    .collect(),
                None => v,
            };
            Json(json!({"total":rows.len(),"items":rows})).into_response()
        }
        _ => refusal(ReasonCode::EntityOperationFailed, "ambiguity read failed"),
    }
}

async fn resolve_ambiguity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(ambiguity_id): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "entity_id is required"),
    };
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    let Some(entity_id) = body.get("entity_id").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "entity_id is required");
    };
    let entity_id = entity_id.to_owned();
    let plan = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let ambiguity_id = ambiguity_id.clone();
        let entity_id = entity_id.clone();
        move || {
            let rows = solstone_core_entity::read_ambiguities(
                &root,
                solstone_core_entity::MalformedPolicy::Raise,
            )
            .map_err(|error| error.to_string())?;
            let Some(row) = rows.into_iter().find(|row| {
                row.get("ambiguity_id").and_then(serde_json::Value::as_str)
                    == Some(ambiguity_id.as_str())
            }) else {
                return Ok::<_, String>(None);
            };
            let scope = row
                .get("scope")
                .cloned()
                .ok_or_else(|| "ambiguity row has no scope".to_owned())?;
            let query = row
                .get("latest_query")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    row.get("original_query")
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or_default()
                .to_owned();
            let mut eligible = Vec::new();
            let mut chosen = None;
            if scope.get("kind").and_then(serde_json::Value::as_str) == Some("journal") {
                let identities = solstone_core_entity::read_identity_map(&root)
                    .map_err(|error| error.to_string())?;
                for (id, entity_dir) in identities.resolved {
                    let Some(identity) =
                        solstone_core_entity::read_entity_identity(&root, &entity_dir)
                            .map_err(|error| error.to_string())?
                    else {
                        continue;
                    };
                    let value = identity.value().clone();
                    if id == entity_id {
                        chosen = Some(value.clone());
                    }
                    eligible.push(solstone_core_entity::AmbiguityChoiceEntity {
                        id,
                        blocked: value.get("blocked") == Some(&serde_json::Value::Bool(true)),
                    });
                }
            } else {
                let facet = scope
                    .get("facet")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "facet ambiguity scope has no facet".to_owned())?;
                let entities =
                    solstone_core_facets::list_scoped_facet_entities(&root, facet, false, false)
                        .map_err(|error| error.to_string())?;
                for entity in entities {
                    if entity.entity_id == entity_id {
                        chosen = Some(entity.identity.clone());
                    }
                    eligible.push(solstone_core_entity::AmbiguityChoiceEntity {
                        id: entity.entity_id,
                        blocked: entity.blocked,
                    });
                }
            }
            Ok(Some((scope, query, eligible, chosen)))
        }
    })
    .await
    {
        Ok(Ok(Some(plan))) => plan,
        Ok(Ok(None)) => return refusal(ReasonCode::EntityNotFound, ambiguity_id),
        _ => return refusal(ReasonCode::InvalidRequestValue, "ambiguity resolve failed"),
    };
    let (scope, query, eligible, chosen) = plan;
    let request = solstone_core_entity::AmbiguityChoiceRequest {
        scope,
        query,
        entity_id,
        origin: Some(json!({
            "lane": "apps.entities.resolve_ambiguity",
            "field": "entity_id",
        })),
    };
    match run_entity_write(move || {
        solstone_core_entity::record_ambiguity_choice(&root, &request, &eligible)
    })
    .await
    {
        Ok(Ok(ambiguity)) => Json(json!({"ambiguity":ambiguity,"entity":chosen})).into_response(),
        Ok(Err(solstone_core_entity::EntityWriteError::TrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )))
        | Ok(Err(solstone_core_entity::EntityWriteError::AmbiguityLock(
            solstone_core_entity::LockError::Timeout(_),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(ReasonCode::InvalidRequestValue, "ambiguity resolve failed"),
    }
}

async fn observations_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let Some(name) = q.get("name").filter(|name| !name.is_empty()) else {
        return refusal(ReasonCode::MissingRequiredField, "name is required");
    };
    let name = name.to_owned();
    match solstone_core_serving::seam::run_blocking(move || {
        let entity_dir = facet_observation_entity_dir(&root, &facet, &name)
            .map_err(|error| error.to_string())?;
        solstone_core_facets::load_observations(&root, &facet, &entity_dir)
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(Ok(v)) => Json(json!({"total":v.len(),"items":v})).into_response(),
        _ => refusal(ReasonCode::EntityOperationFailed, "observation read failed"),
    }
}

/// Resolve a stored facet relationship before using the legacy slug fallback.
fn facet_observation_entity_dir(
    journal_root: &Path,
    facet: &str,
    name: &str,
) -> Result<String, solstone_core_facets::FacetEntityWriteError> {
    let attached =
        solstone_core_facets::list_scoped_facet_entities(journal_root, facet, false, false)?;
    if let Some(entity) = attached
        .iter()
        .find(|entity| entity.identity.get("name").and_then(Value::as_str) == Some(name))
    {
        return Ok(entity.relationship_dir.clone());
    }
    let inclusive =
        solstone_core_facets::list_scoped_facet_entities(journal_root, facet, true, true)?;
    if let Some(entity) = inclusive
        .iter()
        .find(|entity| entity.identity.get("name").and_then(Value::as_str) == Some(name))
    {
        return Ok(entity.relationship_dir.clone());
    }
    Ok(solstone_core_entity_matching::entity_slug(name))
}

fn facet_entity_write_error_is_busy(error: &solstone_core_facets::FacetEntityWriteError) -> bool {
    matches!(
        error,
        solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ) | solstone_core_facets::FacetEntityWriteError::EntityTrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )
    )
}

pub(crate) fn attach_entity_write_error_response(
    error: solstone_core_facets::FacetEntityWriteError,
) -> Response {
    match error {
        solstone_core_facets::FacetEntityWriteError::EntityExists { .. } => {
            refusal(ReasonCode::EntityAlreadyExists, "entity already exists")
        }
        solstone_core_facets::FacetEntityWriteError::EntityBlocked { .. } => {
            refusal(ReasonCode::EntityBlocked, "entity blocked")
        }
        solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. } => {
            refusal(ReasonCode::EntityNotFound, "entity not found")
        }
        _ => refusal(ReasonCode::EntityBusy, "entity busy"),
    }
}

pub(crate) fn create_entity_write_error_response(
    error: solstone_core_facets::FacetEntityWriteError,
) -> Response {
    match error {
        solstone_core_facets::FacetEntityWriteError::EntityBlocked { .. } => {
            refusal(ReasonCode::EntityBlocked, "entity blocked")
        }
        solstone_core_facets::FacetEntityWriteError::EntityExists { .. } => refusal(
            ReasonCode::EntityAlreadyExists,
            "Entity with this name already exists in facet",
        ),
        error if facet_entity_write_error_is_busy(&error) => {
            refusal(ReasonCode::EntityBusy, "entity busy")
        }
        _ => refusal(ReasonCode::EntityOperationFailed, "entity create failed"),
    }
}

async fn attach_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    if bytes.is_empty() {
        return refusal(ReasonCode::MissingRequestBody, "No data provided");
    }
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let Some(kind) = body
        .get("type")
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return refusal(ReasonCode::MissingRequiredField, "type is required");
    };
    let Some(name) = body
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return refusal(ReasonCode::MissingRequiredField, "name is required");
    };
    if !solstone_core_entity::is_valid_entity_type(kind) {
        return refusal(
            ReasonCode::InvalidEntityType,
            format!("Invalid entity type '{kind}'"),
        );
    }
    let description = body
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let kind = kind.to_owned();
    let name = name.to_owned();
    let kind_for_response = kind.clone();
    let name_for_response = name.clone();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::attach_or_reactivate_entity(&root, &facet, &kind, &name, &description)
    })
    .await
    {
        Ok(Ok(result)) => {
            if result.reactivated {
                return Json(json!({"success":true})).into_response();
            }
            let relationship = result.relationship;
            Json(json!({"id":relationship["entity_id"],"name":name_for_response,"type":kind_for_response,"description":relationship["description"],"attached_at":relationship["attached_at"],"updated_at":relationship["updated_at"]})).into_response()
        }
        Ok(Err(error)) => attach_entity_write_error_response(error),
        _ => refusal(ReasonCode::EntityBusy, "entity busy"),
    }
}

async fn create_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(body) if body.as_object().is_some_and(|body| !body.is_empty()) => body,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let kind = body
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let name = body
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if kind.is_empty() || name.is_empty() {
        return refusal(
            ReasonCode::MissingRequiredField,
            "Type and name are required",
        );
    }
    if !solstone_core_entity::is_valid_entity_type(kind) {
        return refusal(
            ReasonCode::InvalidEntityType,
            format!("Invalid entity type '{kind}'"),
        );
    }
    let description = body
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let kind = kind.to_owned();
    let name = name.to_owned();
    let kind_for_response = kind.clone();
    let name_for_response = name.clone();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::attach_or_reactivate_entity(&root, &facet, &kind, &name, &description)
    })
    .await
    {
        Ok(Ok(result)) => {
            if result.reactivated {
                return Json(json!({"success":true,"reattached":true})).into_response();
            }
            let relationship = result.relationship;
            (
                axum::http::StatusCode::CREATED,
                Json(json!({
                    "id":relationship["entity_id"],
                    "name":name_for_response,
                    "type":kind_for_response,
                    "description":relationship["description"],
                    "attached_at":relationship["attached_at"],
                    "updated_at":relationship["updated_at"],
                })),
            )
                .into_response()
        }
        Ok(Err(error)) => create_entity_write_error_response(error),
        _ => refusal(ReasonCode::EntityOperationFailed, "entity create failed"),
    }
}

async fn update_description_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let Some(entity_id) = body.get("entity_id").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "entity_id is required");
    };
    let Some(description) = body.get("description").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "description is required");
    };
    let entity_id = entity_id.to_owned();
    let description = description.to_owned();
    match run_facet_entity_write(move || {
        solstone_core_facets::update_facet_entity_description(
            &root,
            &facet,
            &entity_id,
            &description,
        )
    })
    .await
    {
        Ok(Ok(entity)) => Json(json!({"success":true,"entity":entity})).into_response(),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. })) => {
            refusal(ReasonCode::EntityNotFound, "entity not found")
        }
        Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )))
        | Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityTrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "entity description update failed",
        ),
    }
}

async fn update_detected_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let Some(day) = body.get("day").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "day is required");
    };
    let Some(entity) = body.get("entity").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "entity is required");
    };
    let Some(description) = body.get("description").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "description is required");
    };
    let day = day.to_owned();
    let entity = entity.to_owned();
    let description = description.to_owned();
    match run_facet_entity_write(move || {
        solstone_core_facets::update_detected_entity(&root, &facet, &day, &entity, &description)
    })
    .await
    {
        Ok(Ok(entity)) => Json(json!({"success":true,"entity":entity})).into_response(),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. })) => {
            refusal(ReasonCode::InvalidRequestValue, "detected entity not found")
        }
        Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "detected entity update failed",
        ),
    }
}

async fn move_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let Some(entity) = body.get("entity").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "entity is required");
    };
    let Some(from_facet) = body.get("from_facet").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "from_facet is required");
    };
    let Some(to_facet) = body.get("to_facet").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "to_facet is required");
    };
    let merge = body
        .get("merge")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let _consent = body
        .get("consent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let entity = entity.to_owned();
    let from_facet = from_facet.to_owned();
    let to_facet = to_facet.to_owned();
    let entity_for_response = entity.clone();
    let from_facet_for_response = from_facet.clone();
    let to_facet_for_response = to_facet.clone();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::move_facet_entity(&root, &entity, &from_facet, &to_facet, merge)
    })
    .await
    {
        Ok(Ok(result)) => Json(json!({
            "success":true,
            "entity":entity_for_response,
            "moved_from":from_facet_for_response,
            "moved_to":to_facet_for_response,
            "merged":result.merged,
        }))
        .into_response(),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. })) => refusal(
            ReasonCode::EntityOperationFailed,
            "Entity data directory not found in source facet.",
        ),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityExists { .. })) => refusal(
            ReasonCode::EntityAlreadyExists,
            "Entity already exists in destination facet. Use --merge to merge.",
        ),
        _ => refusal(ReasonCode::EntityOperationFailed, "entity move failed"),
    }
}

async fn detach_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath((facet, entity_id)): RoutePath<(String, String)>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match run_facet_entity_write(move || {
        solstone_core_facets::detach_facet_entity(&root, &facet, &entity_id)
    })
    .await
    {
        Ok(Ok(_)) => Json(json!({"success":true})).into_response(),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. })) => {
            refusal(ReasonCode::EntityNotFound, "Entity not found in facet")
        }
        Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(ReasonCode::EntityOperationFailed, "entity detach failed"),
    }
}

async fn update_path_description_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath((facet, entity_id)): RoutePath<(String, String)>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(body) if body.as_object().is_some_and(|body| !body.is_empty()) => body,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let description = body
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    match run_facet_entity_write(move || {
        solstone_core_facets::update_facet_entity_description(
            &root,
            &facet,
            &entity_id,
            &description,
        )
    })
    .await
    {
        Ok(Ok(_)) => Json(json!({"success":true})).into_response(),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. })) => {
            refusal(ReasonCode::EntityNotFound, "Entity not found in facet")
        }
        Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "entity description update failed",
        ),
    }
}

async fn observe_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let Some(name) = body.get("name").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "name is required");
    };
    let Some(content) = body.get("content").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "content is required");
    };
    let name = name.to_owned();
    let entity_dir = match solstone_core_serving::seam::run_blocking({
        let root = Arc::clone(&root);
        let facet = facet.clone();
        let name = name.clone();
        move || facet_observation_entity_dir(&root, &facet, &name)
    })
    .await
    {
        Ok(Ok(entity_dir)) => entity_dir,
        _ => {
            return refusal(
                ReasonCode::EntityOperationFailed,
                "observation entity lookup failed",
            );
        }
    };
    if entity_dir.is_empty() {
        return refusal(ReasonCode::InvalidRequestValue, "entity name is invalid");
    }
    let content = content.to_owned();
    let source_day = body
        .get("source_day")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    match run_observation_write(move || {
        solstone_core_facets::add_observation(
            &root,
            &facet,
            &entity_dir,
            &content,
            source_day.as_deref(),
            None,
        )
    })
    .await
    {
        Ok(Ok((observations, count))) => Json(json!({
            "success":true,
            "result":{"observations":observations,"count":count}
        }))
        .into_response(),
        Ok(Err(solstone_core_facets::ObservationWriteError::EmptyContent)) => refusal(
            ReasonCode::InvalidRequestValue,
            "observation content cannot be empty",
        ),
        Ok(Err(solstone_core_facets::ObservationWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )))
        | Ok(Err(solstone_core_facets::ObservationWriteError::Write(
            solstone_core_facets::FacetWriteError::TrustLock(
                solstone_core_facets::FacetTrustLockError::Lock(
                    solstone_core_entity::LockError::Timeout(_),
                ),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(ReasonCode::EntityOperationFailed, "observation add failed"),
    }
}

fn detected_day_stems(entities_dir: &Path) -> std::io::Result<Vec<String>> {
    let mut days = std::fs::read_dir(entities_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path
                .extension()
                .is_some_and(|extension| extension == "jsonl"))
            .then(|| path.file_stem()?.to_str().map(str::to_owned))
            .flatten()
        })
        .collect::<Vec<_>>();
    days.sort();
    Ok(days)
}

async fn delete_detected_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(body) if body.as_object().is_some_and(|body| !body.is_empty()) => body,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let Some(name) = body
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return refusal(ReasonCode::MissingRequiredField, "Entity name is required");
    };
    let entities_dir = root.join("facets").join(&facet).join("entities");
    if !entities_dir.exists() {
        return Json(json!({"success":true,"days_modified":[]})).into_response();
    }
    let days = match detected_day_stems(&entities_dir) {
        Ok(days) => days,
        Err(_) => {
            return refusal(
                ReasonCode::EntityOperationFailed,
                "detected entity scan failed",
            );
        }
    };
    let name = name.to_owned();
    match run_facet_entity_write(move || {
        let mut days_modified = Vec::new();
        for day in days {
            let entities = solstone_core_facets::read_detected_entities(&root, &facet, &day)?;
            if entities
                .iter()
                .any(|entity| entity.get("name").and_then(serde_json::Value::as_str) == Some(&name))
            {
                solstone_core_facets::delete_detected_entity(&root, &facet, &day, &name)?;
                days_modified.push(day);
            }
        }
        Ok::<_, solstone_core_facets::FacetEntityWriteError>(days_modified)
    })
    .await
    {
        Ok(Ok(days_modified)) => {
            Json(json!({"success":true,"days_modified":days_modified})).into_response()
        }
        Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "detected entity delete failed",
        ),
    }
}

// Alias uniqueness is enforced by the pre-existing facet store guard; these
// handlers only translate its error variants into route-level refusal codes.
async fn aka_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return refusal(ReasonCode::MissingRequiredField, "entity_id is required"),
    };
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    let Some(entity_id) = body.get("entity_id").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "entity_id is required");
    };
    let Some(aka) = body.get("aka").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "aka is required");
    };
    let Some(exclude_name) = body.get("exclude_name").and_then(serde_json::Value::as_str) else {
        return refusal(ReasonCode::MissingRequiredField, "exclude_name is required");
    };
    let entity_id = entity_id.to_owned();
    let aka = aka.to_owned();
    let exclude_name = exclude_name.to_owned();
    match solstone_core_serving::seam::run_blocking(move || {
        solstone_core_facets::add_entity_aka(&root, &facet, &entity_id, &aka)
    })
    .await
    {
        Ok(Ok(aka)) => Json(json!({"success":true,"aka":aka})).into_response(),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::AkaConflict {
            alias,
            conflict_name,
        })) => refusal(
            ReasonCode::EntityAliasConflict,
            format!("Alias '{alias}' conflicts with entity '{conflict_name}'."),
        ),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. })) => {
            refusal(ReasonCode::EntityNotFound, exclude_name)
        }
        Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )))
        | Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityTrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(
            ReasonCode::EntityOperationFailed,
            "entity alias update failed",
        ),
    }
}

async fn update_entity_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(facet): RoutePath<String>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let body: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(body) if body.as_object().is_some_and(|body| !body.is_empty()) => body,
        _ => return refusal(ReasonCode::MissingRequestBody, "No data provided"),
    };
    let old_name = body
        .get("old_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let new_name = body
        .get("new_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if old_name.is_empty() || new_name.is_empty() {
        return refusal(
            ReasonCode::MissingRequiredField,
            "old_name and new_name are required",
        );
    }
    let entity_type = body
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let aka_list = body
        .get("aka_list")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|aka| !aka.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let old_name = old_name.to_owned();
    let new_name = new_name.to_owned();
    let new_name_for_detail = new_name.clone();
    match run_facet_entity_write(move || {
        solstone_core_facets::update_facet_entity_identity(
            &root,
            &facet,
            &old_name,
            &new_name,
            &entity_type,
            &aka_list,
        )
    })
    .await
    {
        Ok(Ok(entity)) => Json(json!({"success":true,"entity":entity})).into_response(),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityNotFound { .. })) => {
            refusal(ReasonCode::EntityNotFound, "Entity not found")
        }
        Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityExists { .. })) => refusal(
            ReasonCode::EntityAlreadyExists,
            format!("Entity '{new_name_for_detail}' already exists"),
        ),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::AkaConflict {
            alias,
            conflict_name,
        })) => refusal(
            ReasonCode::EntityAliasConflict,
            format!("Alias '{alias}' conflicts with entity '{conflict_name}'"),
        ),
        Ok(Err(solstone_core_facets::FacetEntityWriteError::TrustLock(
            solstone_core_facets::FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        )))
        | Ok(Err(solstone_core_facets::FacetEntityWriteError::EntityTrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_),
            ),
        ))) => refusal(ReasonCode::EntityBusy, "entity busy"),
        _ => refusal(ReasonCode::EntityOperationFailed, "entity update failed"),
    }
}

async fn entity_detail_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath((facet, id)): RoutePath<(String, String)>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        let rows = solstone_core_facets::list_scoped_facet_entities(&root, &facet, true, true)
            .map_err(|error| error.to_string())?;
        if let Some(row) = rows.into_iter().find(|row| row.entity_id == id) {
            let observations =
                solstone_core_facets::load_observations(&root, &facet, &row.relationship_dir)
                    .unwrap_or_default();
            let mut entity = row.identity;
            let voiceprint = solstone_core_entity::entity_memory_path(&root, &row.entity_id, false)
                .map(|path| path.join("voiceprints.npz").exists())
                .unwrap_or(false);
            let object = entity.as_object_mut().expect("identity reader returns objects");
            object.insert("observation_count".to_owned(), json!(observations.len()));
            object.insert("has_voiceprint".to_owned(), json!(voiceprint));
            let snapshot = serde_json::Value::Object(object.clone());
            object.insert("last_active_ts".to_owned(), json!(solstone_core_entity::entity_last_active_ts(&snapshot)));
            object.insert("last_active_day".to_owned(), json!(solstone_core_entity::entity_last_active_day(&snapshot)));
            return Ok::<_, String>(Some((entity, observations)));
        }
        let identity = solstone_core_entity::read_entity_identity(&root, &id).map_err(|error| error.to_string())?;
        Ok(identity.map(|identity| {
            let value = identity.value();
            (json!({"id":id,"name":value.get("name").cloned().unwrap_or_default(),"type":value.get("type").cloned().unwrap_or_default(),"aka":value.get("aka").cloned().unwrap_or_else(||json!([])),"is_principal":value.get("is_principal").cloned().unwrap_or_else(||json!(false)),"needs_attachment":true,"observation_count":0,"has_voiceprint":false}), Vec::new())
        }))
    })
    .await
    {
        Ok(Ok(Some((entity, observations)))) => {
            Json(json!({"entity":entity,"observations":observations})).into_response()
        }
        Ok(Ok(None)) => refusal(ReasonCode::EntityNotFound, "entity not found"),
        _ => refusal(ReasonCode::EntityOperationFailed, "entity read failed"),
    }
}
async fn grid_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath((facet, id)): RoutePath<(String, String)>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        let rows = solstone_core_facets::list_scoped_facet_entities(&root, &facet, true, true)
            .map_err(|error| error.to_string())?;
        let Some(row) = rows.into_iter().find(|row| row.entity_id == id) else {
            return solstone_core_entity::read_entity_identity(&root, &id)
                .map(|identity| identity.map(|_| std::collections::BTreeMap::new()))
                .map_err(|error| error.to_string());
        };
        solstone_core_facets::observation_day_counts(&root, &facet, &row.relationship_dir)
            .map(Some)
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(Ok(Some(counts))) => {
            let watermark = counts.keys().next_back().cloned();
            let mut days = serde_json::Map::new();
            let mut pending = serde_json::Map::new();
            for (day, count) in &counts {
                let target = if watermark
                    .as_deref()
                    .is_some_and(|watermark| day.as_str() <= watermark)
                {
                    &mut days
                } else {
                    &mut pending
                };
                target.insert(day.clone(), json!(count));
            }
            let keys: Vec<_> = days.keys().chain(pending.keys()).collect();
            let coverage = keys
                .iter()
                .min()
                .zip(keys.iter().max())
                .map(|(start, end)| json!({"start":start,"end":end}))
                .unwrap_or(serde_json::Value::Null);
            Json(json!({"coverage":coverage,"days":days,"pending":pending})).into_response()
        }
        Ok(Ok(None)) => refusal(ReasonCode::EntityNotFound, "entity not found"),
        _ => refusal(ReasonCode::EntityOperationFailed, "grid read failed"),
    }
}
async fn history_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    match solstone_core_serving::seam::run_blocking(move || {
        if solstone_core_entity::read_entity_identity(&root, &id)?.is_none() {
            return Ok::<_, solstone_core_entity::EntityStoreError>(None);
        };
        let mut items = solstone_core_entity::read_visible_history(&root, &id)
            .unwrap_or_default()
            .into_iter()
            .map(|event| event.value().clone())
            .collect::<Vec<_>>();
        let latest_merge_seq = items
            .iter()
            .filter(|event| {
                matches!(
                    event.get("kind").and_then(serde_json::Value::as_str),
                    Some("merge" | "merge_undo")
                )
            })
            .filter_map(|event| event.get("seq").and_then(serde_json::Value::as_i64))
            .max()
            .unwrap_or_default();
        let undone_ids: std::collections::HashSet<String> = items
            .iter()
            .filter(|event| {
                event.get("kind").and_then(serde_json::Value::as_str) == Some("merge_undo")
            })
            .filter_map(|event| {
                event
                    .get("operation")
                    .and_then(serde_json::Value::as_object)?
                    .get("undo_of")
                    .map(|value| value.to_string().trim_matches('"').to_owned())
            })
            .collect();
        for event in &mut items {
            let kind = event
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let seq = event
                .get("seq")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or_default();
            let object = event
                .as_object_mut()
                .expect("history reader returns objects");
            object.insert(
                "restore_available".to_owned(),
                json!(kind != "merge" && kind != "merge_undo" && seq > latest_merge_seq),
            );
            if kind == "merge"
                && let Some(operation) = object
                    .get("operation")
                    .and_then(serde_json::Value::as_object)
            {
                let merge_id = operation
                    .get("merge_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                object.insert(
                    "merge_id".to_owned(),
                    merge_id
                        .clone()
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                );
                object.insert(
                    "merge_state".to_owned(),
                    json!(if merge_id.is_some_and(|id| undone_ids.contains(&id)) {
                        "undone"
                    } else {
                        "open"
                    }),
                );
            }
        }
        Ok(Some((id, items)))
    })
    .await
    {
        Ok(Ok(Some((id, items)))) => Json(json!({"entity_id":id,"items":items})).into_response(),
        Ok(Ok(None)) => refusal(ReasonCode::EntityNotFound, "entity not found"),
        _ => refusal(ReasonCode::EntityOperationFailed, "history read failed"),
    }
}

fn percent_decode_query_component(value: &str) -> String {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(hex) = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            output.push(hex);
            index += 3;
            continue;
        }
        output.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn parse_index_plate_query(raw: Option<&str>) -> IndexPlateQuery {
    let mut query = IndexPlateQuery::default();
    for pair in raw
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
    {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode_query_component(key);
        let value = percent_decode_query_component(value);
        match key.as_str() {
            "entity" => query.entity = Some(value),
            "peer" => query.peer = Some(value),
            "kinds" => {
                query.kinds.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|kind| !kind.is_empty())
                        .map(str::to_owned),
                );
            }
            "facet" => query.facet = Some(value),
            "day_from" => query.day_from = Some(value),
            "day_to" => query.day_to = Some(value),
            "limit" => query.limit = Some(value),
            "offset" => query.offset = Some(value),
            "evidence_limit" => query.evidence_limit = Some(value),
            "include_principal" => query.include_principal = Some(value),
            _ => {}
        }
    }
    query
}

fn optional_query_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn include_principal_flag(value: Option<&str>) -> bool {
    matches!(
        value
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "true" | "1" | "yes"
    )
}

fn index_plate_edge_filters(query: &IndexPlateQuery) -> EdgeFilters {
    EdgeFilters {
        kinds: if query.kinds.is_empty() {
            None
        } else {
            Some(query.kinds.clone())
        },
        facet: optional_query_text(query.facet.as_deref()),
        day_from: optional_query_text(query.day_from.as_deref()),
        day_to: optional_query_text(query.day_to.as_deref()),
    }
}

fn required_index_plate_entity(query: &IndexPlateQuery) -> Result<String, Box<Response>> {
    optional_query_text(query.entity.as_deref()).ok_or_else(|| {
        Box::new(refusal(
            ReasonCode::MissingRequiredField,
            "entity is required",
        ))
    })
}

fn edge_query_error_response(error: EdgeQueryError) -> Response {
    match error {
        EdgeQueryError::EdgeIndexUnavailable { detail, .. } => {
            refusal(ReasonCode::EdgeIndexUnavailable, detail)
        }
        EdgeQueryError::InvalidRequestValue { detail } => {
            refusal(ReasonCode::InvalidRequestValue, detail)
        }
        EdgeQueryError::Internal { detail } => refusal(ReasonCode::EntityOperationFailed, detail),
    }
}

fn index_plate_store_error(error: solstone_core_entity::EntityStoreError) -> Response {
    match error {
        solstone_core_entity::EntityStoreError::AmbiguityInvalidRow { path, .. } => refusal(
            ReasonCode::EntityAmbiguityCorrupt,
            format!("ambiguity file {} contains a corrupt row", path.display()),
        ),
        error => refusal(ReasonCode::EntityOperationFailed, error.to_string()),
    }
}

pub(crate) enum IndexPlateError {
    Store(solstone_core_entity::EntityStoreError),
    Facet(solstone_core_facets::FacetEntityWriteError),
    Resolution(solstone_core_entity::EntityResolutionError),
    Query(EdgeQueryError),
    InvalidRequestValue(&'static str),
    OperationFailed(String),
}

fn index_plate_error_response(error: IndexPlateError) -> Response {
    match error {
        IndexPlateError::Store(error) => index_plate_store_error(error),
        IndexPlateError::Facet(error) => {
            resolution_error_response(FacetResolutionError::Facet(error))
        }
        IndexPlateError::Resolution(error) => {
            resolution_error_response(FacetResolutionError::Resolution(error))
        }
        IndexPlateError::Query(error) => edge_query_error_response(error),
        IndexPlateError::InvalidRequestValue(detail) => {
            refusal(ReasonCode::InvalidRequestValue, detail)
        }
        IndexPlateError::OperationFailed(detail) => {
            refusal(ReasonCode::EntityOperationFailed, detail)
        }
    }
}

enum IndexPlateOutcome {
    Unresolved {
        query: String,
        candidates: Vec<Value>,
    },
    Payload(Value),
}

enum IndexPlateTarget {
    Directory(String),
    Unresolved {
        query: String,
        candidates: Vec<Value>,
    },
}

fn index_plate_entity_type(journal_root: &Path, entity_id: &str) -> Option<String> {
    if !is_safe_entity_id_component(entity_id) {
        return None;
    }
    solstone_core_entity::read_entity_identity(journal_root, entity_id)
        .ok()
        .flatten()?
        .value()
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(crate) fn require_existing_entity_dir(
    journal_root: &Path,
    entity_dir: String,
) -> Result<String, IndexPlateError> {
    match solstone_core_entity::read_entity_identity(journal_root, &entity_dir) {
        Ok(Some(_)) => Ok(entity_dir),
        Ok(None) => Err(IndexPlateError::OperationFailed(
            "resolved entity is not a journal entity".to_owned(),
        )),
        Err(error) => Err(IndexPlateError::Store(error)),
    }
}

fn resolved_directory(
    resolution: &solstone_core_entity::EntityResolution,
    entity_dir: impl Fn(usize) -> Option<String>,
) -> Result<String, IndexPlateError> {
    resolution.entity_index.and_then(entity_dir).ok_or_else(|| {
        IndexPlateError::OperationFailed("resolved entity is not a journal entity".to_owned())
    })
}

fn resolve_index_plate_target(
    journal_root: &Path,
    query: &str,
    facet: Option<&str>,
    journal_entities: &[IndexPlateEntity],
) -> Result<IndexPlateTarget, IndexPlateError> {
    if let Some(entity_dir) = exact_journal_entity_dir(journal_entities, query) {
        return Ok(IndexPlateTarget::Directory(entity_dir));
    }
    if let Some(facet_name) = facet {
        let entities =
            load_facet_resolution_entities(journal_root, facet_name, false).map_err(|error| {
                match error {
                    FacetResolutionError::Facet(error) => IndexPlateError::Facet(error),
                    FacetResolutionError::Resolution(error) => IndexPlateError::Resolution(error),
                }
            })?;
        let resolution_entities: Vec<_> = entities
            .iter()
            .map(|entity| entity.resolution.clone())
            .collect();
        let resolution = solstone_core_entity::record_entity_resolution(
            journal_root,
            query,
            &resolution_entities,
            json!({"kind":"facet","facet":facet_name}),
            Value::Null,
            RESOLUTION_FUZZY_THRESHOLD,
            true,
        )
        .map_err(IndexPlateError::Resolution)?;
        return index_plate_target_from_resolution(query, &entities, &resolution, |entity| {
            entity.entity_dir.clone()
        });
    }
    let resolution_entities = journal_resolution_slice(journal_entities);
    let resolution = solstone_core_entity::record_entity_resolution(
        journal_root,
        query,
        &resolution_entities,
        json!({"kind":"journal"}),
        Value::Null,
        RESOLUTION_FUZZY_THRESHOLD,
        true,
    )
    .map_err(IndexPlateError::Resolution)?;
    let views: Vec<_> = journal_entities
        .iter()
        .filter(|entity| !entity.resolution.blocked)
        .map(index_plate_entity_as_facet)
        .collect();
    index_plate_target_from_resolution(query, &views, &resolution, |entity| {
        entity.entity_dir.clone()
    })
}

fn index_plate_target_from_resolution(
    query: &str,
    entities: &[FacetResolutionEntity],
    resolution: &solstone_core_entity::EntityResolution,
    entity_dir: impl Fn(&FacetResolutionEntity) -> String,
) -> Result<IndexPlateTarget, IndexPlateError> {
    match resolution.outcome {
        solstone_core_entity::EntityResolutionOutcome::Resolved => {
            let entity_dir =
                resolved_directory(resolution, |index| entities.get(index).map(&entity_dir))?;
            Ok(IndexPlateTarget::Directory(entity_dir))
        }
        solstone_core_entity::EntityResolutionOutcome::Ambiguous => {
            Ok(IndexPlateTarget::Unresolved {
                query: query.to_owned(),
                candidates: resolution_candidate_payloads(&resolution.candidates, entities),
            })
        }
        solstone_core_entity::EntityResolutionOutcome::NoMatch => {
            Ok(IndexPlateTarget::Unresolved {
                query: query.to_owned(),
                candidates: closest_resolution_candidate_payloads(query, entities),
            })
        }
    }
}

fn payload_value<T: Serialize>(value: T) -> Result<IndexPlateOutcome, IndexPlateError> {
    serde_json::to_value(value)
        .map(IndexPlateOutcome::Payload)
        .map_err(|error| IndexPlateError::OperationFailed(error.to_string()))
}

fn network_plate_work(
    journal_root: &Path,
    query: &IndexPlateQuery,
    entity: &str,
) -> Result<IndexPlateOutcome, IndexPlateError> {
    let journal_entities =
        load_journal_resolution_entities(journal_root).map_err(IndexPlateError::Store)?;
    match resolve_index_plate_target(
        journal_root,
        entity,
        optional_query_text(query.facet.as_deref()).as_deref(),
        &journal_entities,
    )? {
        IndexPlateTarget::Unresolved { query, candidates } => {
            Ok(IndexPlateOutcome::Unresolved { query, candidates })
        }
        IndexPlateTarget::Directory(entity_dir) => {
            let entity_dir = require_existing_entity_dir(journal_root, entity_dir)?;
            let principal_id = find_journal_principal_dir(journal_root)
                .map_err(IndexPlateError::Store)?
                .map(|(entity_dir, _)| entity_dir);
            let limit = index_plate_integer(query.limit.as_deref(), "limit", 25).unwrap_or(25);
            let evidence_limit =
                index_plate_integer(query.evidence_limit.as_deref(), "evidence_limit", 5)
                    .unwrap_or(5);
            let request = NetworkRequest {
                filters: index_plate_edge_filters(query),
                include_principal: include_principal_flag(query.include_principal.as_deref()),
                limit,
                evidence_limit,
                reference_day: None,
            };
            let response = load_entity_network(
                journal_root,
                &entity_dir,
                &request,
                principal_id.as_deref(),
                &ATTENDANCE_KINDS,
            )
            .map_err(IndexPlateError::Query)?;
            let mut value = serde_json::to_value(&response)
                .map_err(|error| IndexPlateError::OperationFailed(error.to_string()))?;
            if principal_id.as_deref() == Some(entity_dir.as_str())
                && let Some(horizon) =
                    solstone_core_facets::refresh_connections_horizon(journal_root)
            {
                let object = value
                    .as_object_mut()
                    .expect("NetworkResponse serializes to an object");
                object.insert("horizon_day".to_owned(), Value::String(horizon.day));
                object.insert(
                    "horizon_note".to_owned(),
                    Value::String(compose_connections_horizon_note(horizon.earlier_days)),
                );
            }
            Ok(IndexPlateOutcome::Payload(value))
        }
    }
}

fn history_plate_work(
    journal_root: &Path,
    query: &IndexPlateQuery,
    entity: &str,
) -> Result<IndexPlateOutcome, IndexPlateError> {
    let journal_entities =
        load_journal_resolution_entities(journal_root).map_err(IndexPlateError::Store)?;
    let facet = optional_query_text(query.facet.as_deref());
    let entity_dir = match resolve_index_plate_target(
        journal_root,
        entity,
        facet.as_deref(),
        &journal_entities,
    )? {
        IndexPlateTarget::Unresolved { query, candidates } => {
            return Ok(IndexPlateOutcome::Unresolved { query, candidates });
        }
        IndexPlateTarget::Directory(entity_dir) => {
            require_existing_entity_dir(journal_root, entity_dir)?
        }
    };
    let peer_dir = match optional_query_text(query.peer.as_deref()) {
        Some(peer) => match resolve_index_plate_target(
            journal_root,
            &peer,
            facet.as_deref(),
            &journal_entities,
        )? {
            IndexPlateTarget::Unresolved { query, candidates } => {
                return Ok(IndexPlateOutcome::Unresolved { query, candidates });
            }
            IndexPlateTarget::Directory(peer_dir) => {
                require_existing_entity_dir(journal_root, peer_dir)?
            }
        },
        None => match find_journal_principal_dir(journal_root).map_err(IndexPlateError::Store)? {
            Some((entity_dir, _)) => entity_dir,
            None => {
                return Err(IndexPlateError::InvalidRequestValue(
                    "history requires PEER because no principal entity is configured",
                ));
            }
        },
    };
    let limit = index_plate_integer(query.limit.as_deref(), "limit", 50).unwrap_or(50);
    let offset = index_plate_integer(query.offset.as_deref(), "offset", 0).unwrap_or(0);
    let request = EdgeEvidenceRequest {
        filters: index_plate_edge_filters(query),
        limit,
        offset,
    };
    let response = load_edge_evidence(journal_root, &entity_dir, &peer_dir, &request)
        .map_err(IndexPlateError::Query)?;
    payload_value(response)
}

fn overview_plate_work(
    journal_root: &Path,
    query: &IndexPlateQuery,
) -> Result<IndexPlateOutcome, IndexPlateError> {
    let limit = index_plate_integer(query.limit.as_deref(), "limit", 25).unwrap_or(25);
    let request = NetworkOverviewRequest {
        filters: index_plate_edge_filters(query),
        limit,
        reference_day: None,
    };
    let response = load_network_overview(journal_root, &request, &ATTENDANCE_KINDS, &|entity_id| {
        index_plate_entity_type(journal_root, entity_id)
    })
    .map_err(IndexPlateError::Query)?;
    payload_value(response)
}

fn index_plate_outcome_response(outcome: IndexPlateOutcome) -> Response {
    match outcome {
        IndexPlateOutcome::Unresolved { query, candidates } => Json(json!({
            "resolved": Value::Null,
            "query": query,
            "candidates": candidates,
        }))
        .into_response(),
        IndexPlateOutcome::Payload(value) => Json(value).into_response(),
    }
}

async fn finish_index_plate(
    work: impl std::future::Future<
        Output = Result<Result<IndexPlateOutcome, IndexPlateError>, tokio::task::JoinError>,
    >,
) -> Response {
    match work.await {
        Ok(Ok(outcome)) => index_plate_outcome_response(outcome),
        Ok(Err(error)) => index_plate_error_response(error),
        Err(_) => refusal(ReasonCode::EntityOperationFailed, "index plate task failed"),
    }
}

fn index_plate_integer(
    value: Option<&str>,
    name: &str,
    default: i64,
) -> Result<i64, Box<Response>> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value.parse::<i64>().map_err(|_| {
            Box::new(refusal(
                ReasonCode::InvalidRequestValue,
                format!("{name} must be an integer"),
            ))
        }),
        None => Ok(default),
    }
}

fn validate_index_plate_pagination(
    route: IndexPlateRoute,
    query: &IndexPlateQuery,
) -> Result<(), Box<Response>> {
    match route {
        IndexPlateRoute::Network => {
            let _limit = index_plate_integer(query.limit.as_deref(), "limit", 25)?;
            let _evidence_limit =
                index_plate_integer(query.evidence_limit.as_deref(), "evidence_limit", 5)?;
        }
        IndexPlateRoute::History => {
            let _limit = index_plate_integer(query.limit.as_deref(), "limit", 50)?;
            let _offset = index_plate_integer(query.offset.as_deref(), "offset", 0)?;
        }
        IndexPlateRoute::Overview => {
            let _limit = index_plate_integer(query.limit.as_deref(), "limit", 25)?;
        }
    }
    Ok(())
}

fn normalize_entity_search_value(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn parse_entity_search_query(
    query: EntitySearchQuery,
) -> Result<EntitySearchParams, Box<Response>> {
    let since = normalize_entity_search_value(query.since);
    if since
        .as_deref()
        .is_some_and(|day| !is_entity_search_day(day))
    {
        return Err(Box::new(refusal(
            ReasonCode::InvalidRequestValue,
            "since must be a YYYYMMDD day",
        )));
    }
    let limit = query
        .limit
        .as_deref()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(20);
    Ok(EntitySearchParams {
        query: normalize_entity_search_value(query.query),
        entity_type: normalize_entity_search_value(query.entity_type),
        facet: normalize_entity_search_value(query.facet).map(|facet| facet.to_lowercase()),
        since,
        limit: usize::try_from(limit).unwrap_or_default(),
    })
}

fn entity_search_index_access_failure(error: IndexAccessError) -> EntitySearchFailure {
    match error {
        IndexAccessError::Locked { .. } => EntitySearchFailure::IndexBusy,
        IndexAccessError::Absent { .. }
        | IndexAccessError::Unreadable { .. }
        | IndexAccessError::Empty { .. } => EntitySearchFailure::IndexUnavailable,
    }
}

fn entity_search_degradation_failure(
    degraded: Option<&IndexDegraded>,
) -> Result<(), EntitySearchFailure> {
    match degraded {
        Some(IndexDegraded::Building { .. }) => Err(EntitySearchFailure::IndexBusy),
        Some(IndexDegraded::Unknown) => Err(EntitySearchFailure::IndexStale),
        None => Ok(()),
    }
}

fn entity_search_failure_response(failure: EntitySearchFailure) -> Response {
    match failure {
        EntitySearchFailure::ActivityUnavailable => refusal(
            ReasonCode::EntitySearchActivityUnavailable,
            "detected entity activity is unavailable",
        ),
        EntitySearchFailure::IndexBusy => refusal(
            ReasonCode::EntitySearchIndexBusy,
            "entity search index is busy",
        ),
        EntitySearchFailure::IndexStale => refusal(
            ReasonCode::EntitySearchIndexStale,
            "entity search index is stale",
        ),
        EntitySearchFailure::IndexUnavailable => refusal(
            ReasonCode::EntitySearchIndexUnavailable,
            "entity search index is unavailable",
        ),
    }
}

fn eligible_entity_records(
    root: &Path,
    records: Vec<Value>,
) -> Result<Vec<Value>, EntitySearchFailure> {
    let mut truthy_detached = HashSet::new();
    for facet in solstone_core_facets::list_facet_directories(root)
        .map_err(|_| EntitySearchFailure::IndexStale)?
    {
        for entity in
            solstone_core_facets::list_scoped_facet_entities_tolerant(root, &facet, true, true)
                .map_err(|_| EntitySearchFailure::IndexStale)?
        {
            if json_truthy(entity.relationship.get("detached")) {
                truthy_detached.insert((entity.entity_dir, facet.clone()));
            }
        }
    }

    Ok(records
        .into_iter()
        .filter(|record| !json_truthy(record.get("blocked")))
        .map(|mut record| {
            let entity_id = entity_record_id(&record).unwrap_or_default().to_owned();
            if let Some(facets) = record.get_mut("facets").and_then(Value::as_array_mut) {
                facets.retain(|facet| {
                    let name = facet
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    !json_truthy(facet.get("detached"))
                        && !truthy_detached.contains(&(entity_id.clone(), name.to_owned()))
                });
            }
            record
        })
        .collect())
}

fn entity_record_id(record: &Value) -> Option<&str> {
    record
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

fn entity_search_records_for_request(records: &[Value], params: &EntitySearchParams) -> Vec<Value> {
    records
        .iter()
        .filter(|record| {
            params.entity_type.as_deref().is_none_or(|entity_type| {
                record
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|current_type| current_type.eq_ignore_ascii_case(entity_type))
            })
        })
        .filter_map(|record| {
            let mut record = record.clone();
            if let Some(facet) = params.facet.as_deref() {
                let facets = record.get_mut("facets").and_then(Value::as_array_mut)?;
                facets.retain(|item| {
                    item.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(facet))
                });
                if facets.is_empty() {
                    return None;
                }
            }
            Some(record)
        })
        .collect()
}

fn entity_name_candidates(records: &[Value]) -> Vec<EntityNameCandidate> {
    records
        .iter()
        .filter_map(|record| {
            let id = entity_record_id(record)?.to_owned();
            let name = record.get("name").and_then(Value::as_str)?.to_owned();
            Some(EntityNameCandidate {
                id: Some(id),
                name,
                aka: record
                    .get("aka")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                emails: Vec::new(),
            })
        })
        .collect()
}

fn resolve_detected_entity_name(name: &str, candidates: &[EntityNameCandidate]) -> Option<String> {
    match find_matching_entity_detailed(name, candidates, RESOLUTION_FUZZY_THRESHOLD) {
        EntityNameMatchOutcome::Matched {
            candidate_index, ..
        } => candidates.get(candidate_index)?.id.clone(),
        EntityNameMatchOutcome::Ambiguous { .. } | EntityNameMatchOutcome::NoMatch => None,
    }
}

fn entity_ids_detected_since(
    root: &Path,
    since: &str,
    facet: Option<&str>,
    candidates: &[EntityNameCandidate],
) -> Result<HashSet<String>, EntitySearchFailure> {
    solstone_core_facets::iter_detected_entity_names_since_strict(root, since, facet)
        .map_err(|_| EntitySearchFailure::ActivityUnavailable)
        .map(|detections| {
            detections
                .into_iter()
                .filter_map(|(name, _, _)| resolve_detected_entity_name(&name, candidates))
                .collect()
        })
}

fn is_entity_search_day(value: &str) -> bool {
    NaiveDate::parse_from_str(value, "%Y%m%d")
        .ok()
        .is_some_and(|day| day.format("%Y%m%d").to_string() == value)
}

fn detected_hit_entity_id(
    root: &Path,
    hit: &SearchHit,
    names_by_day: &mut HashMap<(String, String), Vec<String>>,
    candidates: &[EntityNameCandidate],
) -> Result<Option<String>, EntitySearchFailure> {
    let facet = &hit.metadata.facet;
    let day = &hit.metadata.day;
    let Some(index) = usize::try_from(hit.metadata.idx).ok() else {
        return Err(EntitySearchFailure::IndexStale);
    };
    if facet.is_empty() || !is_entity_search_day(day) {
        return Err(EntitySearchFailure::IndexStale);
    }
    let expected_path = format!("facets/{facet}/entities/{day}.jsonl");
    if hit.metadata.path != expected_path {
        return Err(EntitySearchFailure::IndexStale);
    }
    let key = (facet.clone(), day.clone());
    if !names_by_day.contains_key(&key) {
        let names = solstone_core_facets::read_detected_entity_names_strict(root, facet, day)
            .map_err(|_| EntitySearchFailure::ActivityUnavailable)?;
        names_by_day.insert(key.clone(), names);
    }
    let name = names_by_day
        .get(&key)
        .and_then(|names| names.get(index))
        .ok_or(EntitySearchFailure::IndexStale)?;
    Ok(resolve_detected_entity_name(name, candidates))
}

fn entity_search_item(record: &Value) -> Value {
    let facets = record
        .get("facets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let description = facets
        .first()
        .and_then(|facet| facet.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let facet_names: Vec<_> = facets
        .iter()
        .filter_map(|facet| facet.get("name").and_then(Value::as_str))
        .collect();
    json!({
        "entity_id": entity_record_id(record).unwrap_or_default(),
        "name": record.get("name").and_then(Value::as_str).unwrap_or_default(),
        "type": record.get("type").and_then(Value::as_str).unwrap_or_default(),
        "description": description,
        "facets": facet_names,
    })
}

fn entity_search_work(
    root: &Path,
    params: &EntitySearchParams,
) -> Result<Value, EntitySearchFailure> {
    let coverage_response = coverage(root).map_err(entity_search_index_access_failure)?;
    entity_search_degradation_failure(coverage_response.degraded.as_ref())?;

    let eligible = eligible_entity_records(
        root,
        assemble_journal_entity_records(root, None).map_err(|_| EntitySearchFailure::IndexStale)?,
    )?;
    let eligible_ids: BTreeSet<_> = eligible
        .iter()
        .filter_map(entity_record_id)
        .map(str::to_owned)
        .collect();
    let indexed_ids = indexed_entity_ids(root).map_err(entity_search_index_access_failure)?;
    if indexed_ids != eligible_ids {
        return Err(EntitySearchFailure::IndexStale);
    }

    let candidates = entity_name_candidates(&eligible);
    let requested_records = entity_search_records_for_request(&eligible, params);
    let requested_by_id: HashMap<_, _> = requested_records
        .into_iter()
        .filter_map(|record| {
            let id = entity_record_id(&record)?.to_owned();
            Some((id, record))
        })
        .collect();

    let (entity_hits, detected_hits) = if let Some(query) = params.query.as_deref() {
        let mut entity_request = SearchRequest::new(query, Order::Relevance);
        entity_request.limit = usize::MAX;
        entity_request.agent = Some("entity".to_owned());
        entity_request.facet = params.facet.clone();
        let entity_response = search(root, &entity_request, Local::now().date_naive())
            .map_err(entity_search_index_access_failure)?;
        entity_search_degradation_failure(entity_response.degraded.as_ref())?;

        let mut detected_request = SearchRequest::new(query, Order::Relevance);
        detected_request.limit = usize::MAX;
        detected_request.agent = Some("entity:detected".to_owned());
        detected_request.facet = params.facet.clone();
        detected_request.day_from = params.since.clone();
        let detected_response = search(root, &detected_request, Local::now().date_naive())
            .map_err(entity_search_index_access_failure)?;
        entity_search_degradation_failure(detected_response.degraded.as_ref())?;
        (entity_response.results, detected_response.results)
    } else {
        (Vec::new(), Vec::new())
    };

    let since_ids = params
        .since
        .as_deref()
        .map(|since| entity_ids_detected_since(root, since, params.facet.as_deref(), &candidates))
        .transpose()?;

    let mut records = if params.query.is_some() {
        let mut hits: Vec<_> = entity_hits
            .into_iter()
            .map(|hit| (0_u8, hit))
            .chain(detected_hits.into_iter().map(|hit| (1_u8, hit)))
            .collect();
        hits.sort_by(|(left_priority, left), (right_priority, right)| {
            left.score
                .total_cmp(&right.score)
                .then_with(|| left_priority.cmp(right_priority))
                .then_with(|| left.metadata.path.cmp(&right.metadata.path))
                .then_with(|| left.metadata.idx.cmp(&right.metadata.idx))
        });

        let mut names_by_day = HashMap::new();
        let mut seen = HashSet::new();
        let mut records = Vec::new();
        for (priority, hit) in hits {
            let entity_id = if priority == 0 {
                hit.metadata
                    .path
                    .strip_prefix("entity_search:")
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .ok_or(EntitySearchFailure::IndexStale)?
            } else {
                let Some(entity_id) =
                    detected_hit_entity_id(root, &hit, &mut names_by_day, &candidates)?
                else {
                    continue;
                };
                entity_id
            };
            if since_ids
                .as_ref()
                .is_some_and(|ids| !ids.contains(&entity_id))
            {
                continue;
            }
            let Some(record) = requested_by_id.get(&entity_id) else {
                continue;
            };
            if seen.insert(entity_id) {
                records.push(record.clone());
            }
        }
        records
    } else {
        requested_by_id
            .into_values()
            .filter(|record| {
                since_ids
                    .as_ref()
                    .is_none_or(|ids| entity_record_id(record).is_some_and(|id| ids.contains(id)))
            })
            .collect()
    };

    if params.query.is_none() {
        records.sort_by(|left, right| {
            left.get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase()
                .cmp(
                    &right
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase(),
                )
                .then_with(|| entity_record_id(left).cmp(&entity_record_id(right)))
        });
    }
    records.truncate(params.limit);
    Ok(json!({
        "items": records.iter().map(entity_search_item).collect::<Vec<_>>(),
    }))
}

async fn index_plate_network(
    Extension(basis): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    if let Some(response) = admitted(&basis) {
        return response;
    }
    let query = parse_index_plate_query(raw.as_deref());
    if let Err(response) = validate_index_plate_pagination(IndexPlateRoute::Network, &query) {
        return *response;
    }
    let entity = match required_index_plate_entity(&query) {
        Ok(entity) => entity,
        Err(response) => return *response,
    };
    finish_index_plate(solstone_core_serving::seam::run_blocking(move || {
        network_plate_work(&root, &query, &entity)
    }))
    .await
}

async fn index_plate_history(
    Extension(basis): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    if let Some(response) = admitted(&basis) {
        return response;
    }
    let query = parse_index_plate_query(raw.as_deref());
    if let Err(response) = validate_index_plate_pagination(IndexPlateRoute::History, &query) {
        return *response;
    }
    let entity = match required_index_plate_entity(&query) {
        Ok(entity) => entity,
        Err(response) => return *response,
    };
    finish_index_plate(solstone_core_serving::seam::run_blocking(move || {
        history_plate_work(&root, &query, &entity)
    }))
    .await
}

async fn index_plate_overview(
    Extension(basis): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    if let Some(response) = admitted(&basis) {
        return response;
    }
    let query = parse_index_plate_query(raw.as_deref());
    if let Err(response) = validate_index_plate_pagination(IndexPlateRoute::Overview, &query) {
        return *response;
    }
    finish_index_plate(solstone_core_serving::seam::run_blocking(move || {
        overview_plate_work(&root, &query)
    }))
    .await
}

async fn index_plate_search(
    Extension(basis): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    Query(query): Query<EntitySearchQuery>,
) -> Response {
    if let Some(response) = admitted(&basis) {
        return response;
    }
    let params = match parse_entity_search_query(query) {
        Ok(params) => params,
        Err(response) => return *response,
    };
    match solstone_core_serving::seam::run_blocking(move || entity_search_work(&root, &params))
        .await
    {
        Ok(Ok(body)) => Json(body).into_response(),
        Ok(Err(failure)) => entity_search_failure_response(failure),
        Err(_) => entity_search_failure_response(EntitySearchFailure::IndexUnavailable),
    }
}

#[cfg(test)]
mod access_tests {
    use super::admitted;
    use solstone_core_convey_http::identity::{AccessBasis, Carrier};

    #[test]
    fn entity_admission_accepts_localhost_and_refuses_pairing_peers() {
        assert!(admitted(&AccessBasis::Localhost).is_none());
        assert!(
            admitted(&AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            })
            .is_some()
        );
    }
}
