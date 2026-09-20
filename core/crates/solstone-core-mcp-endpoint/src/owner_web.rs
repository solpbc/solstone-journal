// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local-owner control plane for journal agent connections.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    ActivityQuery, AuditToolName, OAuthStore, PermissionStore, ReadPermission, ReadScope,
    RecordedOutcome, TokenStore, read_activity, resolve_permission_facet_names, tally,
};
use axum::body::Body;
use axum::extract::{Extension, Path, Query};
use axum::http::{Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::{Datelike, Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_journal_config::{
    McpEndpointCapability, mcp_endpoint_capability, read_journal_config,
};
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
};

pub fn owner_routes(journal_root: PathBuf) -> Router {
    let journal = Arc::new(journal_root);
    Router::new()
        .route("/app/agents/api/state", get(state))
        .route("/app/agents/api/capability", put(set_capability))
        .route(
            "/app/agents/api/pairing",
            post(generate_pairing).delete(revoke_pairing),
        )
        .route(
            "/app/agents/api/connections/{kind}/{id}",
            delete(revoke_connection).patch(rename_connection),
        )
        .route(
            "/app/agents/api/connections/{kind}/{id}/permission",
            put(set_permission),
        )
        .route("/app/agents/api/activity", get(activity))
        .route_layer(middleware::from_fn(require_local_owner))
        .layer(Extension(journal))
}

async fn require_local_owner(request: Request<Body>, next: Next) -> Response {
    let is_local = matches!(
        request.extensions().get::<AccessBasis>(),
        Some(AccessBasis::Localhost)
    );
    if !is_local {
        return StatusCode::NOT_FOUND.into_response();
    }
    if request.method() != Method::GET
        && request
            .headers()
            .get("x-solstone-owner")
            .and_then(|value| value.to_str().ok())
            != Some("1")
    {
        return refusal(
            "owner_request_required",
            "this change must come from your journal",
            StatusCode::FORBIDDEN,
        );
    }
    next.run(request).await
}

fn refusal(code: &'static str, detail: impl ToString, status: StatusCode) -> Response {
    (
        status,
        Json(json!({"error": code, "reason_code": code, "detail": detail.to_string()})),
    )
        .into_response()
}

async fn state(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    match state_value(&journal) {
        Ok(value) => Json(value).into_response(),
        Err(detail) => refusal(
            "agents_state_unavailable",
            detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

fn state_value(root: &std::path::Path) -> Result<Value, String> {
    let config = read_journal_config(root).map_err(|error| error.to_string())?;
    let enabled = matches!(
        mcp_endpoint_capability(&config),
        Ok(McpEndpointCapability::Enabled)
    );
    let tokens = TokenStore::open(root)
        .list()
        .map_err(|error| error.to_string())?;
    let oauth = OAuthStore::open(root);
    let grants = oauth.list_grants().map_err(|error| error.to_string())?;
    let pairing = oauth
        .current_pairing_code()
        .map_err(|error| error.to_string())?;
    let permissions = PermissionStore::open(root)
        .read()
        .map_err(|error| error.to_string())?;
    let by_key: BTreeMap<_, _> = permissions
        .permissions
        .into_iter()
        .map(|permission| (permission.connection.clone(), permission))
        .collect();
    let (activity_by_connection, activity_complete) = activity_this_week(root)?;
    let mut connections = Vec::new();
    for token in tokens {
        let key = format!("bearer:{}", token.id);
        let activity = activity_by_connection.get(&key);
        connections.push(json!({
            "kind": "bearer", "id": token.id, "key": key, "name": token.label,
            "auth_method": "bearer token", "created_at": token.created_at,
            "permission": by_key.get(&key),
            "requests_this_week": activity.map_or(0, |(count, _)| *count),
            "last_request_at": activity.and_then(|(_, last)| *last),
            "activity_complete": activity_complete,
        }));
    }
    for grant in grants {
        let key = format!("oauth:{}", grant.id);
        let activity = activity_by_connection.get(&key);
        connections.push(json!({
            "kind": "oauth", "id": grant.id, "key": key,
            "name": grant.client_name.as_deref().unwrap_or(&grant.client_id),
            "client_id": grant.client_id, "auth_method": "OAuth",
            "created_at": grant.created_at, "access_expires_at": grant.access_expires_at,
            "permission": by_key.get(&key),
            "requests_this_week": activity.map_or(0, |(count, _)| *count),
            "last_request_at": activity.and_then(|(_, last)| *last),
            "activity_complete": activity_complete,
        }));
    }
    let facet_names =
        solstone_core_facets::list_declared_facet_names(root).map_err(|error| error.to_string())?;
    let facets = facet_names
        .into_iter()
        .map(|name| {
            let declaration = solstone_core_facets::read_facet_declaration(root, &name)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("facet {name:?} has no declaration"))?;
            Ok(json!({
                "name": name,
                "title": if declaration.title.is_empty() { &name } else { &declaration.title },
                "color": declaration.color,
                "id": declaration.value().get("id").and_then(Value::as_str),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let certificate = certificate_posture(root);
    let owner_state = crate::read_mcp_owner_state(root);
    let certificate_current = certificate.get("current").and_then(Value::as_bool) == Some(true);
    let renewal_due = certificate.get("renewal_due").and_then(Value::as_bool) == Some(true);
    let status = if !enabled {
        "off"
    } else if owner_state.as_ref().is_some_and(|state| {
        matches!(
            state.status.as_str(),
            "offline" | "failed" | "needs_subscription"
        )
    }) {
        owner_state
            .as_ref()
            .map_or("turning_on", |state| state.status.as_str())
    } else if certificate_current && renewal_due {
        "renewal_overdue"
    } else if certificate_current {
        owner_state
            .as_ref()
            .map_or("on", |state| state.status.as_str())
    } else if owner_state
        .as_ref()
        .is_some_and(|state| state.status == "on")
    {
        "renewal_overdue"
    } else {
        owner_state
            .as_ref()
            .map_or("turning_on", |state| state.status.as_str())
    };
    let mut response = json!({
        "enabled": enabled,
        "status": status,
        "owner_state": owner_state,
        "certificate": certificate,
        "connections": connections,
        "facets": facets,
        "pairing": pairing.map(|value| json!({"expires_at": value.expires_at, "generation": value.generation, "locked": value.locked})),
    });
    if status == "needs_subscription" {
        response["subscribe_url"] = json!(format!("{}/services/solstone-me", portal_origin()));
    }
    Ok(response)
}

fn portal_origin() -> String {
    std::env::var("SERVICES_PORTAL_URL")
        .unwrap_or_else(|_| "https://services.solstone.app".to_string())
        .trim_end_matches('/')
        .to_string()
}

type ConnectionActivity = BTreeMap<String, (usize, Option<chrono::DateTime<Utc>>)>;

fn activity_this_week(root: &std::path::Path) -> Result<(ConnectionActivity, bool), String> {
    let today = Utc::now().date_naive();
    let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let mut query = ActivityQuery {
        day_from: Some(monday.format("%Y%m%d").to_string()),
        limit: usize::MAX,
        ..ActivityQuery::default()
    };
    let mut aggregate = BTreeMap::new();
    let mut complete = true;
    loop {
        let page = read_activity(root, &query).map_err(|error| error.to_string())?;
        complete &= page.unreadable == 0;
        for entry in page.entries {
            let Some(connection) = entry.connection else {
                continue;
            };
            let summary = aggregate.entry(connection).or_insert((0, None));
            summary.0 += 1;
            if summary.1.is_none_or(|last| entry.timestamp > last) {
                summary.1 = Some(entry.timestamp);
            }
        }
        let Some(next) = page.next else {
            break;
        };
        if query.start_after.as_ref() == Some(&next) {
            return Err("activity pagination did not advance".to_owned());
        }
        query.start_after = Some(next);
        if page.examination_complete {
            break;
        }
    }
    Ok((aggregate, complete))
}

fn certificate_posture(root: &std::path::Path) -> Value {
    let path = root.join("mcp-endpoint/tls/state.json");
    let Ok(bytes) = std::fs::read(path) else {
        return json!({"current": false});
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return json!({"current": false});
    };
    let hostname = value.get("hostname").and_then(Value::as_str);
    let not_before = value.get("not_before").and_then(Value::as_i64);
    let not_after = value.get("not_after").and_then(Value::as_i64);
    let now = chrono::Utc::now().timestamp();
    let renewal_due = not_before
        .zip(not_after)
        .is_some_and(|(start, expiry)| now >= expiry - (expiry - start) / 3);
    json!({"current": not_after.is_some_and(|expiry| expiry > now), "renewal_due": renewal_due, "hostname": hostname, "not_before": not_before, "not_after": not_after})
}

#[derive(Deserialize)]
struct CapabilityBody {
    enabled: bool,
}

async fn set_capability(
    Extension(journal): Extension<Arc<PathBuf>>,
    Json(body): Json<CapabilityBody>,
) -> Response {
    let result = mutate_journal_config(&journal, LockOptions::default(), |config| {
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let Some(endpoint) = endpoint.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err("mcp_endpoint setting is not an object"),
            };
        };
        let changed = endpoint.get("enabled") != Some(&Value::Bool(body.enabled));
        endpoint.insert("enabled".to_owned(), Value::Bool(body.enabled));
        JournalConfigMutation {
            changed,
            value: Ok(()),
        }
    });
    match result {
        Ok(transaction) => match transaction.value {
            Ok(()) => Json(json!({"enabled": body.enabled, "changed": transaction.changed}))
                .into_response(),
            Err(detail) => refusal("agents_config_invalid", detail, StatusCode::CONFLICT),
        },
        Err(error) => refusal(
            "agents_config_write_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

async fn generate_pairing(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    match OAuthStore::open(&journal).generate_pairing_code() {
        Ok(created) => Json(json!({"code": created.code, "expires_at": created.expires_at, "generation": created.generation})).into_response(),
        Err(error) => refusal("pairing_create_failed", error, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn revoke_pairing(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    match OAuthStore::open(&journal).revoke_pairing_code() {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refusal("pairing_revoke_failed", error, StatusCode::CONFLICT),
    }
}

async fn revoke_connection(
    Extension(journal): Extension<Arc<PathBuf>>,
    Path((kind, id)): Path<(String, String)>,
) -> Response {
    let result = match kind.as_str() {
        "bearer" => TokenStore::open(&journal)
            .revoke_by_id(&id)
            .map_err(|error| error.to_string()),
        "oauth" => OAuthStore::open(&journal)
            .revoke_grant_by_id(&id)
            .map_err(|error| error.to_string()),
        _ => {
            return refusal(
                "connection_kind_invalid",
                "unknown connection kind",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    match result {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => refusal(
            "connection_not_found",
            "connection was not found",
            StatusCode::NOT_FOUND,
        ),
        Err(detail) => refusal(
            "connection_revoke_failed",
            detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

async fn rename_connection(
    Extension(journal): Extension<Arc<PathBuf>>,
    Path((kind, id)): Path<(String, String)>,
    Json(body): Json<RenameBody>,
) -> Response {
    if kind != "bearer" {
        return refusal(
            "connection_rename_unsupported",
            "OAuth names come from the connecting client",
            StatusCode::CONFLICT,
        );
    }
    match TokenStore::open(&journal).rename_by_id(&id, &body.name) {
        Ok(true) => Json(json!({"name": body.name})).into_response(),
        Ok(false) => refusal(
            "connection_not_found",
            "connection was not found",
            StatusCode::NOT_FOUND,
        ),
        Err(error) => refusal("connection_rename_failed", error, StatusCode::BAD_REQUEST),
    }
}

#[derive(Deserialize)]
struct PermissionBody {
    categories: Vec<String>,
    scope: String,
    #[serde(default)]
    facets: Vec<String>,
}

async fn set_permission(
    Extension(journal): Extension<Arc<PathBuf>>,
    Path((kind, id)): Path<(String, String)>,
    Json(body): Json<PermissionBody>,
) -> Response {
    if !matches!(kind.as_str(), "bearer" | "oauth") {
        return refusal(
            "connection_kind_invalid",
            "unknown connection kind",
            StatusCode::BAD_REQUEST,
        );
    }
    let allowed = ["transcripts", "entities", "facets"];
    if body.categories.is_empty()
        || body
            .categories
            .iter()
            .any(|category| !allowed.contains(&category.as_str()))
    {
        return refusal(
            "permission_categories_invalid",
            "choose at least one supported category",
            StatusCode::BAD_REQUEST,
        );
    }
    let scope = match body.scope.as_str() {
        "whole_journal" => ReadScope::WholeJournal,
        "facets" if !body.facets.is_empty() => {
            match resolve_permission_facet_names(&journal, &body.facets) {
                Ok(ids) => ReadScope::Facets { ids },
                Err(detail) => {
                    return refusal("permission_facets_invalid", detail, StatusCode::BAD_REQUEST);
                }
            }
        }
        _ => {
            return refusal(
                "permission_scope_invalid",
                "choose the whole journal or at least one facet",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let key = format!("{kind}:{id}");
    match PermissionStore::open(&journal).set_permission(
        &key,
        ReadPermission {
            categories: body.categories,
            scope,
        },
    ) {
        Ok(permission) => Json(json!({"permission": permission})).into_response(),
        Err(error) => refusal(
            "permission_write_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

#[derive(Default, Deserialize)]
struct ActivityParams {
    connection: Option<String>,
    tool: Option<String>,
    outcome: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

async fn activity(
    Extension(journal): Extension<Arc<PathBuf>>,
    Query(params): Query<ActivityParams>,
) -> Response {
    if params
        .from
        .iter()
        .chain(params.to.iter())
        .any(|day| day.len() != 8 || !day.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return refusal(
            "activity_day_invalid",
            "days must use YYYYMMDD",
            StatusCode::BAD_REQUEST,
        );
    }
    let tool = match params.tool.as_deref() {
        Some(value) => match AuditToolName::from_token(value) {
            Some(tool) => Some(tool),
            None => {
                return refusal(
                    "activity_tool_invalid",
                    "unknown tool",
                    StatusCode::BAD_REQUEST,
                );
            }
        },
        None => None,
    };
    let outcome = match params.outcome.as_deref() {
        Some(value) => match RecordedOutcome::from_token(value) {
            Some(outcome) => Some(outcome),
            None => {
                return refusal(
                    "activity_outcome_invalid",
                    "unknown outcome",
                    StatusCode::BAD_REQUEST,
                );
            }
        },
        None => None,
    };
    let query = ActivityQuery {
        connection: params.connection,
        tool,
        outcome,
        day_from: params.from,
        day_to: params.to,
        limit: 100,
        start_after: None,
    };
    match read_activity(&journal, &query) {
        Ok(page) => {
            let counts = tally(&page.entries);
            let entries: Vec<_> = page.entries.into_iter().map(|entry| json!({
                "day": entry.day, "segment": entry.segment, "timestamp": entry.timestamp,
                "connection": entry.connection, "agent_identity": entry.agent_identity,
                "tool": entry.tool_name.token(), "request": entry.request,
                "outcome": entry.outcome.token(), "reason": entry.reason, "result": entry.result,
            })).collect();
            Json(json!({"entries": entries, "counts": counts, "examined": page.examined, "examination_complete": page.examination_complete, "unreadable": page.unreadable, "next": page.next.map(|next| json!({"day": next.day, "segment": next.segment}))})).into_response()
        }
        Err(error) => refusal(
            "activity_read_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tower::ServiceExt;

    #[test]
    fn state_defaults_off_and_keeps_connection_state_separate() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let value = state_value(temp.path()).unwrap();
        assert_eq!(value["enabled"], false);
        assert_eq!(value["status"], "off");
        assert_eq!(value["connections"], json!([]));
    }

    #[test]
    fn state_projects_needs_subscription_status_and_subscribe_url() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"enabled":true}}"#,
        )
        .unwrap();
        let next_attempt = Utc::now() + Duration::seconds(300);
        crate::owner_state::write_mcp_needs_subscription_state(
            journal_root,
            None,
            ("waiting", "waiting", "waiting"),
            next_attempt,
        );

        let value = state_value(journal_root).unwrap();
        assert_eq!(value["enabled"], true);
        assert_eq!(value["status"], "needs_subscription");
        assert_eq!(
            value["subscribe_url"],
            "https://services.solstone.app/services/solstone-me"
        );
    }

    #[test]
    fn workspace_keeps_the_approved_trust_outcome_and_turnoff_copy() {
        let workspace =
            include_str!("../../solstone-core-convey-shell/assets/agents/workspace.html");
        for copy in [
            "it passes the traffic along and can't read it",
            "public certificate logs",
            "served",
            "nothing matched",
            "refused",
            "couldn't complete",
            "unknown ending",
            "the address and your agents are kept: turning back on uses the same address, with no new certificate",
        ] {
            assert!(workspace.contains(copy), "missing approved copy: {copy}");
        }
    }

    #[tokio::test]
    async fn owner_routes_are_absent_without_localhost_provenance() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let route = "/app/agents/api/state";
        let hidden = owner_routes(temp.path().to_path_buf())
            .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

        let mut request = Request::builder().uri(route).body(Body::empty()).unwrap();
        request.extensions_mut().insert(AccessBasis::Localhost);
        let visible = owner_routes(temp.path().to_path_buf())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(visible.status(), StatusCode::OK);

        let mut cross_site = Request::builder()
            .method(Method::PUT)
            .uri("/app/agents/api/capability")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"enabled":true}"#))
            .unwrap();
        cross_site.extensions_mut().insert(AccessBasis::Localhost);
        let refused = owner_routes(temp.path().to_path_buf())
            .oneshot(cross_site)
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    }
}
