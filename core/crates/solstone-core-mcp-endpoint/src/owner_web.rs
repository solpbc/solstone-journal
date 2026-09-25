// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner control plane for journal agent connections.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    ActivityQuery, AuditToolName, OAuthStore, PermissionStore, ReadPermission, ReadScope,
    RecordedOutcome, TokenStore, read_activity, resolve_permission_facet_names, tally,
};
use axum::body::Body;
use axum::extract::{Extension, Path, Query};
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::{Datelike, Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_convey_http::gate::require_access;
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
        .route("/app/agents/api/local-door", put(set_local_door))
        .route("/app/agents/api/lan-door", put(set_lan_door))
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
        .route_layer(middleware::from_fn(admit_owner))
        .layer(Extension(journal))
}

async fn admit_owner(request: Request<Body>, next: Next) -> Response {
    let is_owner = request
        .extensions()
        .get::<AccessBasis>()
        .map(require_access)
        .unwrap_or(false);
    if !is_owner {
        return StatusCode::NOT_FOUND.into_response();
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

pub(crate) fn state_value(root: &std::path::Path) -> Result<Value, String> {
    state_value_with_iface(
        root,
        &solstone_core_sol_link::pairing::addresses::SystemInterfaceSource,
    )
}

pub(crate) fn state_value_with_iface(
    root: &std::path::Path,
    iface_source: &dyn solstone_core_sol_link::pairing::addresses::RawInterfaceSource,
) -> Result<Value, String> {
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
            "door": Value::Null,
            "requests_this_week": activity.map_or(0, |(count, _)| *count),
            "last_request_at": activity.and_then(|(_, last)| *last),
            "activity_complete": activity_complete,
        }));
    }
    for grant in grants {
        let key = format!("oauth:{}", grant.id);
        let activity = activity_by_connection.get(&key);
        let door = match grant.resource.as_deref() {
            Some(solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE) => {
                Value::String("local".to_owned())
            }
            Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE) => {
                Value::String("lan".to_owned())
            }
            None => Value::String("relay".to_owned()),
            _ => Value::Null,
        };
        connections.push(json!({
            "kind": "oauth", "id": grant.id, "key": key,
            "name": grant.client_name.as_deref().unwrap_or(&grant.client_id),
            "client_id": grant.client_id, "auth_method": "OAuth",
            "created_at": grant.created_at, "access_expires_at": grant.access_expires_at,
            "permission": by_key.get(&key),
            "door": door,
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
            "offline" | "failed" | "needs_subscription" | "not_accepted"
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
    let local_door_enabled_flag = solstone_core_journal_config::local_door_enabled(&config);
    let (listening, reason) = match crate::local_door::read_local_door_state(root) {
        Some(state) => {
            let now = Utc::now();
            let age = now.signed_duration_since(state.observed_at);
            #[cfg(test)]
            let window_valid = if let Some(custom) = *TEST_READER_WINDOW.lock().unwrap() {
                age >= custom.0 && age <= custom.1
            } else {
                age >= Duration::seconds(-5) && age <= Duration::seconds(35)
            };
            #[cfg(not(test))]
            let window_valid = age >= Duration::seconds(-5) && age <= Duration::seconds(35);

            if window_valid {
                if state.listening {
                    (true, None)
                } else {
                    (
                        false,
                        state.reason.or_else(|| Some("not_running".to_string())),
                    )
                }
            } else {
                (false, Some("not_running".to_string()))
            }
        }
        None => (false, Some("not_running".to_string())),
    };
    let mut local_door = json!({
        "address": solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE,
        "enabled": local_door_enabled_flag,
        "listening": listening,
    });
    if let Some(reason) = reason {
        local_door["reason"] = Value::String(reason);
    }

    let lan_door_enabled_flag = solstone_core_journal_config::lan_door_enabled(&config);
    let (lan_listening, lan_reason, lan_fingerprint, fresh_state) =
        match crate::lan_door::read_lan_door_state(root) {
            Some(state) => {
                let now = Utc::now();
                let age = now.signed_duration_since(state.observed_at);
                #[cfg(test)]
                let window_valid = if let Some(custom) = *TEST_READER_WINDOW.lock().unwrap() {
                    age >= custom.0 && age <= custom.1
                } else {
                    age >= Duration::seconds(-5) && age <= Duration::seconds(35)
                };
                #[cfg(not(test))]
                let window_valid = age >= Duration::seconds(-5) && age <= Duration::seconds(35);

                if window_valid {
                    if state.listening {
                        (true, None, state.fingerprint.clone(), Some(state))
                    } else {
                        (
                            false,
                            state
                                .reason
                                .clone()
                                .or_else(|| Some("not_running".to_string())),
                            None,
                            Some(state),
                        )
                    }
                } else if !lan_door_enabled_flag {
                    (false, Some("disabled".to_string()), None, None)
                } else {
                    (false, Some("not_running".to_string()), None, None)
                }
            }
            None => {
                if !lan_door_enabled_flag {
                    (false, Some("disabled".to_string()), None, None)
                } else {
                    (false, Some("not_running".to_string()), None, None)
                }
            }
        };

    let iface_endpoints = iface_source
        .enumerate()
        .map(|raw| solstone_core_sol_link::pairing::addresses::classify_interface_addresses(&raw))
        .unwrap_or_default();
    let mut admitted_ips: Vec<std::net::IpAddr> = Vec::new();
    for ep in iface_endpoints {
        if crate::lan_door::is_admitted_lan_bind_endpoint(&ep) && !admitted_ips.contains(&ep.ip) {
            admitted_ips.push(ep.ip);
        }
    }

    let mut addresses_json = Vec::new();
    let mut urls = Vec::new();
    for ip in admitted_ips {
        let addr_str = ip.to_string();
        let url_str = match ip {
            std::net::IpAddr::V4(v4) => {
                format!(
                    "https://{v4}:{}/mcp",
                    solstone_core_journal_config::MCP_LAN_DOOR_PORT
                )
            }
            std::net::IpAddr::V6(v6) => {
                format!(
                    "https://[{v6}]:{}/mcp",
                    solstone_core_journal_config::MCP_LAN_DOOR_PORT
                )
            }
        };
        let (addr_listening, addr_reason) = if let Some(fresh) = &fresh_state {
            if let Some(entry) = fresh
                .addresses
                .as_ref()
                .and_then(|addrs| addrs.iter().find(|a| a.address == addr_str))
            {
                (entry.listening, entry.reason.clone())
            } else {
                (
                    false,
                    fresh
                        .reason
                        .clone()
                        .or_else(|| Some("not_running".to_string())),
                )
            }
        } else {
            (false, lan_reason.clone())
        };

        let mut addr_obj = json!({
            "address": addr_str,
            "url": url_str.clone(),
            "listening": addr_listening,
        });
        if !addr_listening && let Some(reason) = addr_reason {
            addr_obj["reason"] = Value::String(reason);
        }
        addresses_json.push(addr_obj);
        urls.push(url_str);
    }

    let mut lan_door = json!({
        "enabled": lan_door_enabled_flag,
        "listening": lan_listening,
        "port": solstone_core_journal_config::MCP_LAN_DOOR_PORT,
        "addresses": addresses_json,
        "urls": urls,
    });
    if let Some(reason) = lan_reason {
        lan_door["reason"] = Value::String(reason);
    }
    if let Some(fingerprint) = lan_fingerprint {
        lan_door["fingerprint"] = Value::String(fingerprint);
    }

    let mut response = json!({
        "enabled": enabled,
        "status": status,
        "local_door": local_door,
        "lan_door": lan_door,
        "owner_state": owner_state,
        "certificate": certificate,
        "connections": connections,
        "facets": facets,
        "pairing": pairing.map(|value| {
            let mut pairing_obj = json!({
                "expires_at": value.expires_at,
                "generation": value.generation,
                "locked": value.locked,
            });
            if let Some(door) = value.door {
                pairing_obj["door"] = Value::String(door);
            }
            pairing_obj
        }),
    });
    if status == "needs_subscription" {
        response["subscribe_url"] = json!(format!("{}/services/solstone-me", portal_origin()));
    }
    Ok(response)
}

#[cfg(test)]
pub(crate) static TEST_READER_WINDOW: std::sync::Mutex<Option<(Duration, Duration)>> =
    std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static TEST_PUT_POLL_BOUNDS: std::sync::Mutex<
    Option<(std::time::Duration, std::time::Duration)>,
> = std::sync::Mutex::new(None);

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
    write_endpoint_switch(&journal, "enabled", body.enabled)
}

/// Turn the loopback agent door on or off. It is on unless the owner turns it off.
async fn set_local_door(
    Extension(journal): Extension<Arc<PathBuf>>,
    Json(body): Json<CapabilityBody>,
) -> Response {
    let start_utc = Utc::now();
    let result = mutate_journal_config(&journal, LockOptions::default(), |config| {
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let endpoint_obj = if let Some(obj) = endpoint.as_object_mut() {
            obj
        } else {
            *endpoint = json!({});
            endpoint.as_object_mut().unwrap()
        };
        let changed = endpoint_obj.get("local_door") != Some(&Value::Bool(body.enabled));
        endpoint_obj.insert("local_door".to_owned(), Value::Bool(body.enabled));
        JournalConfigMutation {
            changed,
            value: Ok::<(), &'static str>(()),
        }
    });

    match result {
        Ok(transaction) => match transaction.value {
            Ok(()) => {
                if !transaction.changed {
                    return Json(json!({"enabled": body.enabled, "changed": false}))
                        .into_response();
                }
                #[cfg(test)]
                let (bound, poll_interval) = TEST_PUT_POLL_BOUNDS.lock().unwrap().unwrap_or((
                    std::time::Duration::from_secs(3),
                    std::time::Duration::from_millis(50),
                ));
                #[cfg(not(test))]
                let (bound, poll_interval) = (
                    std::time::Duration::from_secs(3),
                    std::time::Duration::from_millis(50),
                );

                let deadline = tokio::time::Instant::now() + bound;
                while tokio::time::Instant::now() < deadline {
                    if let Some(state) = crate::local_door::read_local_door_state(&journal) {
                        if !body.enabled {
                            if !state.listening && state.reason.as_deref() == Some("disabled") {
                                break;
                            }
                        } else if state.observed_at >= start_utc {
                            let reason = state.reason.as_deref();
                            if reason != Some("disabled") && reason != Some("config_invalid") {
                                break;
                            }
                        }
                    }
                    tokio::time::sleep(poll_interval).await;
                }
                Json(json!({"enabled": body.enabled, "changed": transaction.changed}))
                    .into_response()
            }
            Err(detail) => refusal("agents_config_invalid", detail, StatusCode::CONFLICT),
        },
        Err(error) => refusal(
            "agents_config_write_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// Turn the LAN agent door on or off. It is off unless the owner turns it on.
async fn set_lan_door(
    Extension(journal): Extension<Arc<PathBuf>>,
    Json(body): Json<CapabilityBody>,
) -> Response {
    let start_utc = Utc::now();
    let result = mutate_journal_config(&journal, LockOptions::default(), |config| {
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let endpoint_obj = if let Some(obj) = endpoint.as_object_mut() {
            obj
        } else {
            *endpoint = json!({});
            endpoint.as_object_mut().unwrap()
        };
        let changed = endpoint_obj.get("lan_door") != Some(&Value::Bool(body.enabled));
        endpoint_obj.insert("lan_door".to_owned(), Value::Bool(body.enabled));
        JournalConfigMutation {
            changed,
            value: Ok::<(), &'static str>(()),
        }
    });

    match result {
        Ok(transaction) => match transaction.value {
            Ok(()) => {
                if !transaction.changed {
                    return Json(json!({"enabled": body.enabled, "changed": false}))
                        .into_response();
                }
                #[cfg(test)]
                let (bound, poll_interval) = TEST_PUT_POLL_BOUNDS.lock().unwrap().unwrap_or((
                    std::time::Duration::from_secs(3),
                    std::time::Duration::from_millis(50),
                ));
                #[cfg(not(test))]
                let (bound, poll_interval) = (
                    std::time::Duration::from_secs(3),
                    std::time::Duration::from_millis(50),
                );

                let deadline = tokio::time::Instant::now() + bound;
                while tokio::time::Instant::now() < deadline {
                    if let Some(state) = crate::lan_door::read_lan_door_state(&journal) {
                        if !body.enabled {
                            if !state.listening && state.reason.as_deref() == Some("disabled") {
                                break;
                            }
                        } else if state.observed_at >= start_utc {
                            let reason = state.reason.as_deref();
                            if reason != Some("disabled") && reason != Some("config_invalid") {
                                break;
                            }
                        }
                    }
                    tokio::time::sleep(poll_interval).await;
                }
                Json(json!({"enabled": body.enabled, "changed": transaction.changed}))
                    .into_response()
            }
            Err(detail) => refusal("agents_config_invalid", detail, StatusCode::CONFLICT),
        },
        Err(error) => refusal(
            "agents_config_write_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

fn write_endpoint_switch(journal: &std::path::Path, key: &str, enabled: bool) -> Response {
    let result = mutate_journal_config(journal, LockOptions::default(), |config| {
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let Some(endpoint) = endpoint.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err("mcp_endpoint setting is not an object"),
            };
        };
        let changed = endpoint.get(key) != Some(&Value::Bool(enabled));
        endpoint.insert(key.to_owned(), Value::Bool(enabled));
        JournalConfigMutation {
            changed,
            value: Ok(()),
        }
    });
    match result {
        Ok(transaction) => match transaction.value {
            Ok(()) => {
                Json(json!({"enabled": enabled, "changed": transaction.changed})).into_response()
            }
            Err(detail) => refusal("agents_config_invalid", detail, StatusCode::CONFLICT),
        },
        Err(error) => refusal(
            "agents_config_write_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

async fn generate_pairing(
    Extension(journal): Extension<Arc<PathBuf>>,
    body: axum::body::Bytes,
) -> Response {
    let trimmed = body.trim_ascii();
    let door = if trimmed.is_empty() {
        None
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(map)) => match map.get("door") {
                None | Some(Value::Null) => None,
                Some(Value::String(door_str)) => {
                    if door_str == "lan" {
                        Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE.to_string())
                    } else {
                        return refusal(
                            "pairing_create_failed",
                            "unsupported door",
                            StatusCode::BAD_REQUEST,
                        );
                    }
                }
                _ => {
                    return refusal(
                        "pairing_create_failed",
                        "invalid door field",
                        StatusCode::BAD_REQUEST,
                    );
                }
            },
            _ => {
                return refusal(
                    "pairing_create_failed",
                    "invalid request body",
                    StatusCode::BAD_REQUEST,
                );
            }
        }
    };
    match OAuthStore::open(&journal).generate_pairing_code_with_door(door.as_deref()) {
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
        crate::owner_state::write_mcp_hold_state(
            journal_root,
            crate::bridge_carrier::RegistrationHold::NeedsSubscription,
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
    fn state_projects_not_accepted_status_and_no_subscribe_url() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"enabled":true}}"#,
        )
        .unwrap();
        crate::owner_state::write_mcp_hold_state(
            journal_root,
            crate::bridge_carrier::RegistrationHold::NotAccepted,
            None,
            ("waiting", "waiting", "waiting"),
            Utc::now() + Duration::seconds(300),
        );

        let value = state_value(journal_root).unwrap();
        assert_eq!(value["status"], "not_accepted");
        assert!(
            value.get("subscribe_url").is_none(),
            "a refusal that is not about a subscription must not offer one"
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
    async fn local_door_switch_writes_its_own_key_and_repairs_an_invalid_value() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"enabled":true,"local_door":"yes"}}"#,
        )
        .unwrap();
        let put = |enabled: bool| {
            let mut request = Request::builder()
                .method(axum::http::Method::PUT)
                .uri("/app/agents/api/local-door")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"enabled":{enabled}}}"#)))
                .unwrap();
            request.extensions_mut().insert(AccessBasis::Localhost);
            owner_routes(journal_root.to_path_buf()).oneshot(request)
        };
        let config = || {
            serde_json::from_slice::<Value>(
                &std::fs::read(journal_root.join("config/journal.json")).unwrap(),
            )
            .unwrap()
        };

        let off = put(false).await.unwrap();
        assert_eq!(off.status(), StatusCode::OK);
        assert_eq!(config()["mcp_endpoint"]["local_door"], false);
        assert_eq!(
            config()["mcp_endpoint"]["enabled"],
            true,
            "the local door switch must not touch the solstone.me capability"
        );

        let on = put(true).await.unwrap();
        assert_eq!(on.status(), StatusCode::OK);
        assert_eq!(config()["mcp_endpoint"]["local_door"], true);
    }

    #[tokio::test]
    async fn owner_routes_are_absent_without_owner_provenance() {
        use axum::http::Method;
        use solstone_core_convey_http::identity::{Carrier, LinkedDeviceCid};

        let temp = TempDir::new_in("/var/tmp").unwrap();
        let route = "/app/agents/api/state";
        let hidden = owner_routes(temp.path().to_path_buf())
            .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

        let mut peer_req = Request::builder().uri(route).body(Body::empty()).unwrap();
        peer_req.extensions_mut().insert(AccessBasis::PairingPeer {
            carrier: Carrier::Direct,
        });
        let peer_res = owner_routes(temp.path().to_path_buf())
            .oneshot(peer_req)
            .await
            .unwrap();
        assert_eq!(peer_res.status(), StatusCode::NOT_FOUND);

        let mut request = Request::builder().uri(route).body(Body::empty()).unwrap();
        request.extensions_mut().insert(AccessBasis::Localhost);
        let visible = owner_routes(temp.path().to_path_buf())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(visible.status(), StatusCode::OK);

        let cid = LinkedDeviceCid::try_from(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();

        let mut linked_req = Request::builder().uri(route).body(Body::empty()).unwrap();
        linked_req
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid: cid.clone(),
                carrier: Carrier::Direct,
            });
        let linked_res = owner_routes(temp.path().to_path_buf())
            .oneshot(linked_req)
            .await
            .unwrap();
        assert_eq!(linked_res.status(), StatusCode::OK);

        let mut linked_put = Request::builder()
            .method(Method::PUT)
            .uri("/app/agents/api/capability")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"enabled":true}"#))
            .unwrap();
        linked_put
            .extensions_mut()
            .insert(AccessBasis::LinkedDevice {
                cid,
                carrier: Carrier::Direct,
            });
        let put_res = owner_routes(temp.path().to_path_buf())
            .oneshot(linked_put)
            .await
            .unwrap();
        assert_eq!(put_res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(put_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["enabled"], true);
    }

    #[tokio::test]
    async fn state_projects_local_door_posture_and_connection_door_origin() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        // 1. Unset config -> default is on, but no local-door-state.json -> not_running
        let value = state_value(journal_root).unwrap();
        assert_eq!(
            value["local_door"]["address"],
            solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE
        );
        assert_eq!(value["local_door"]["enabled"], true);
        assert_eq!(value["local_door"]["listening"], false);
        assert_eq!(value["local_door"]["reason"], "not_running");

        // 2. Explicitly disabled config -> disabled
        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"local_door":false}}"#,
        )
        .unwrap();
        crate::local_door::write_local_door_state(journal_root, false, Some("disabled"));
        let value = state_value(journal_root).unwrap();
        assert_eq!(value["local_door"]["enabled"], false);
        assert_eq!(value["local_door"]["listening"], false);
        assert_eq!(value["local_door"]["reason"], "disabled");

        // 3. Write fresh local door state
        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"local_door":true}}"#,
        )
        .unwrap();
        crate::local_door::write_local_door_state(journal_root, true, None);
        let value = state_value(journal_root).unwrap();
        assert_eq!(value["local_door"]["enabled"], true);
        assert_eq!(value["local_door"]["listening"], true);
        assert!(value["local_door"]["reason"].is_null());
    }

    #[tokio::test]
    async fn reader_window_and_reason_passthrough_and_bound_canonical() {
        use crate::oauth::OAuthRuntime;
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        let runtime = OAuthRuntime::new_bound(
            journal_root,
            solstone_core_journal_config::MCP_LOCAL_DOOR_ORIGIN.to_string(),
        );
        let binding = runtime.binding();
        assert_eq!(
            binding.canonical(),
            solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE
        );

        let value = state_value(journal_root).unwrap();
        assert_eq!(value["local_door"]["address"], binding.canonical());
        assert_eq!(value["local_door"]["listening"], false);
        assert_eq!(value["local_door"]["reason"], "not_running");

        // Helper to write arbitrary LocalDoorState
        let write_custom_state =
            |listening: bool, observed_at: chrono::DateTime<Utc>, reason: Option<&str>| {
                let state = crate::local_door::LocalDoorState {
                    listening,
                    observed_at,
                    reason: reason.map(str::to_owned),
                };
                let path = journal_root.join(crate::local_door::LOCAL_DOOR_STATE_PATH);
                std::fs::write(path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
            };

        // 36s old -> not_running
        write_custom_state(true, Utc::now() - Duration::seconds(36), None);
        let val = state_value(journal_root).unwrap();
        assert_eq!(val["local_door"]["listening"], false);
        assert_eq!(val["local_door"]["reason"], "not_running");

        // 6s future -> not_running
        write_custom_state(true, Utc::now() + Duration::seconds(6), None);
        let val = state_value(journal_root).unwrap();
        assert_eq!(val["local_door"]["listening"], false);
        assert_eq!(val["local_door"]["reason"], "not_running");

        // 34s old -> valid record
        write_custom_state(true, Utc::now() - Duration::seconds(34), None);
        let val = state_value(journal_root).unwrap();
        assert_eq!(val["local_door"]["listening"], true);
        assert!(val["local_door"]["reason"].is_null());

        // 4s future -> valid record
        write_custom_state(true, Utc::now() + Duration::seconds(4), None);
        let val = state_value(journal_root).unwrap();
        assert_eq!(val["local_door"]["listening"], true);
        assert!(val["local_door"]["reason"].is_null());

        // Test every reason pass-through
        for reason in [
            "disabled",
            "port_in_use",
            "config_invalid",
            "config_unreadable",
            "not_running",
        ] {
            write_custom_state(false, Utc::now(), Some(reason));
            let val = state_value(journal_root).unwrap();
            assert_eq!(val["local_door"]["listening"], false);
            assert_eq!(val["local_door"]["reason"], reason);
        }
    }

    struct MockRawInterfaceSource {
        addrs: Vec<solstone_core_sol_link::pairing::addresses::RawInterfaceAddress>,
    }

    impl solstone_core_sol_link::pairing::addresses::RawInterfaceSource for MockRawInterfaceSource {
        fn enumerate(
            &self,
        ) -> Result<
            Vec<solstone_core_sol_link::pairing::addresses::RawInterfaceAddress>,
            solstone_core_sol_link::pairing::addresses::AddressError,
        > {
            Ok(self.addrs.clone())
        }
    }

    #[tokio::test]
    async fn state_projects_lan_door_posture_addresses_and_urls() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        // 1. No config key + no state file -> enabled: false, listening: false, reason: "disabled", port: 7660, no fingerprint.
        let val = state_value(journal_root).unwrap();
        assert_eq!(val["lan_door"]["enabled"], false);
        assert_eq!(val["lan_door"]["listening"], false);
        assert_eq!(val["lan_door"]["reason"], "disabled");
        assert_eq!(val["lan_door"]["port"], 7660);
        assert!(val["lan_door"].get("fingerprint").is_none());

        // 2 & 3. Injected enumerator with:
        // - private Lan IPv4 (192.168.1.50 on eth0)
        // - ULA IPv6 (fd00::1 on eth0)
        // - CGNAT Vpn IPv4 (100.64.0.1 on tun0)
        // - public Lan IPv4 (8.8.8.8 on eth0)
        // - public Vpn IPv4 (8.8.4.4 on tun0)
        let mock_iface = MockRawInterfaceSource {
            addrs: vec![
                solstone_core_sol_link::pairing::addresses::RawInterfaceAddress {
                    interface: "eth0".to_string(),
                    address: "192.168.1.50".parse().unwrap(),
                },
                solstone_core_sol_link::pairing::addresses::RawInterfaceAddress {
                    interface: "eth0".to_string(),
                    address: "fd00::1".parse().unwrap(),
                },
                solstone_core_sol_link::pairing::addresses::RawInterfaceAddress {
                    interface: "tun0".to_string(),
                    address: "100.64.0.1".parse().unwrap(),
                },
                solstone_core_sol_link::pairing::addresses::RawInterfaceAddress {
                    interface: "eth0".to_string(),
                    address: "8.8.8.8".parse().unwrap(),
                },
                solstone_core_sol_link::pairing::addresses::RawInterfaceAddress {
                    interface: "tun0".to_string(),
                    address: "8.8.4.4".parse().unwrap(),
                },
            ],
        };

        // Turn on lan_door in config
        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"lan_door":true}}"#,
        )
        .unwrap();

        // Fresh record with 1 listening address and 1 port_in_use address
        let addr_states = vec![
            crate::lan_door::LanAddressState {
                address: "192.168.1.50".to_string(),
                listening: true,
                reason: None,
            },
            crate::lan_door::LanAddressState {
                address: "fd00::1".to_string(),
                listening: false,
                reason: Some("port_in_use".to_string()),
            },
        ];
        crate::lan_door::write_lan_door_state(
            journal_root,
            true,
            None,
            Some("sha256:0123456789abcdef"),
            Some(addr_states),
        );

        let val = state_value_with_iface(journal_root, &mock_iface).unwrap();
        assert_eq!(val["lan_door"]["enabled"], true);
        assert_eq!(val["lan_door"]["listening"], true);
        assert!(val["lan_door"]["reason"].is_null());
        assert_eq!(val["lan_door"]["fingerprint"], "sha256:0123456789abcdef");

        // Check addresses:
        let addresses = val["lan_door"]["addresses"].as_array().unwrap();
        assert_eq!(addresses.len(), 3);

        // Address 1: 192.168.1.50 (listening: true, reason omitted)
        assert_eq!(addresses[0]["address"], "192.168.1.50");
        assert_eq!(addresses[0]["url"], "https://192.168.1.50:7660/mcp");
        assert_eq!(addresses[0]["listening"], true);
        assert!(addresses[0].get("reason").is_none());

        // Address 2: fd00::1 (listening: false, reason: "port_in_use")
        // IPv6 spelled same in address and inside [brackets] of url
        assert_eq!(addresses[1]["address"], "fd00::1");
        assert_eq!(addresses[1]["url"], "https://[fd00::1]:7660/mcp");
        assert_eq!(addresses[1]["listening"], false);
        assert_eq!(addresses[1]["reason"], "port_in_use");

        // Address 3: 100.64.0.1
        assert_eq!(addresses[2]["address"], "100.64.0.1");
        assert_eq!(addresses[2]["url"], "https://100.64.0.1:7660/mcp");
        assert_eq!(addresses[2]["listening"], false);

        // Check URLs in order (yields first 3 URLs in that order and omits both public addresses)
        let urls: Vec<&str> = val["lan_door"]["urls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            urls,
            vec![
                "https://192.168.1.50:7660/mcp",
                "https://[fd00::1]:7660/mcp",
                "https://100.64.0.1:7660/mcp",
            ]
        );
    }
}
