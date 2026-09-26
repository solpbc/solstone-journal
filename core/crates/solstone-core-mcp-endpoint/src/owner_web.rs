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
use x509_parser::prelude::parse_x509_certificate;

pub fn owner_routes(journal_root: PathBuf) -> Router {
    let journal = Arc::new(journal_root);
    Router::new()
        .route("/app/agents/api/state", get(state))
        .route("/app/agents/api/capability", put(set_capability))
        .route("/app/agents/api/local-door", put(set_local_door))
        .route("/app/agents/api/lan-door", put(set_lan_door))
        .route("/app/agents/api/lan-door/ca.pem", get(lan_door_ca_pem))
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
        .route(
            "/app/agents/api/byo",
            put(set_byo_hostname).delete(remove_byo_hostname),
        )
        .route("/app/agents/api/byo/account", post(register_byo_account))
        .route(
            "/app/agents/api/byo/account/replace",
            post(replace_byo_account),
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
            Some(r) if r.starts_with("https://") => Value::String("byo".to_owned()),
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
    let lan_door_enabled_flag = solstone_core_journal_config::lan_door_enabled(&config);
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

    // Read CA fingerprint if lan-ca.pem is a valid single public cert
    let ca_pem_path = root.join(crate::lan_door::LAN_CA_PEM_PATH);
    let (ca_fingerprint, ca_cert_der) =
        if let Ok(ca_content) = std::fs::read_to_string(&ca_pem_path) {
            if !ca_content.is_empty()
                && !ca_content.contains("PRIVATE KEY")
                && let Ok(entries) = pem::parse_many(&ca_content)
                && entries.len() == 1
                && entries[0].tag() == "CERTIFICATE"
            {
                let der = entries[0].contents().to_vec();
                if parse_x509_certificate(&der).is_ok() {
                    let fp = crate::lan_door::compute_cert_fingerprint(&der);
                    (Some(fp), Some(der))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

    let ca_x509_opt = ca_cert_der
        .as_ref()
        .and_then(|der| parse_x509_certificate(der).ok().map(|(_, x)| x));

    // Read Leaf fingerprint and SANs if door is enabled and leaf chains to CA
    let (leaf_fingerprint, leaf_sans) = if lan_door_enabled_flag && ca_x509_opt.is_some() {
        let leaf_pem_path = root.join(crate::lan_door::LAN_LEAF_PEM_PATH);
        if let Ok(leaf_content) = std::fs::read_to_string(&leaf_pem_path) {
            if let Ok(entries) = pem::parse_many(&leaf_content) {
                let cert_entry = entries.iter().find(|e| e.tag() == "CERTIFICATE");
                let key_entry = entries
                    .iter()
                    .find(|e| e.tag() == "PRIVATE KEY" || e.tag() == "EC PRIVATE KEY");
                if entries.len() == 2
                    && let (Some(cert), Some(_key)) = (cert_entry, key_entry)
                    && let Ok((_, leaf_x509)) = parse_x509_certificate(cert.contents())
                    && leaf_x509
                        .verify_signature(
                            ca_x509_opt
                                .as_ref()
                                .map(|ca| &ca.tbs_certificate.subject_pki),
                        )
                        .is_ok()
                {
                    let now = Utc::now().timestamp();
                    let not_before = leaf_x509.validity().not_before.timestamp();
                    let not_after = leaf_x509.validity().not_after.timestamp();
                    if not_before <= now && now <= not_after {
                        let fp = crate::lan_door::compute_cert_fingerprint(cert.contents());
                        let sans = crate::lan_door::parse_cert_san_ips(&leaf_x509);
                        (Some(fp), Some(sans))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let lan_door_state_res = crate::lan_door::read_lan_door_state(root);
    let (_lan_listening, lan_reason, fresh_state) = match lan_door_state_res {
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
                    (true, None, Some(state))
                } else {
                    (
                        false,
                        state
                            .reason
                            .clone()
                            .or_else(|| Some("not_running".to_string())),
                        Some(state),
                    )
                }
            } else if !lan_door_enabled_flag {
                (false, Some("disabled".to_string()), None)
            } else {
                (false, Some("not_running".to_string()), None)
            }
        }
        None => {
            if !lan_door_enabled_flag {
                (false, Some("disabled".to_string()), None)
            } else {
                (false, Some("not_running".to_string()), None)
            }
        }
    };

    let (admitted_ips, enum_error) = match iface_source.enumerate() {
        Ok(raw) => {
            let endpoints =
                solstone_core_sol_link::pairing::addresses::classify_interface_addresses(&raw);
            let mut ips = Vec::new();
            for ep in endpoints {
                if crate::lan_door::is_admitted_lan_bind_endpoint(&ep) && !ips.contains(&ep.ip) {
                    ips.push(ep.ip);
                }
            }
            (ips, false)
        }
        Err(_) => (Vec::new(), true),
    };

    let lan_door = if enum_error {
        let mut obj = json!({
            "enabled": lan_door_enabled_flag,
            "listening": false,
            "reason": "enumeration_failed",
            "port": solstone_core_journal_config::MCP_LAN_DOOR_PORT,
            "addresses": [],
            "urls": [],
        });
        if let Some(ca_fp) = ca_fingerprint {
            obj["ca_fingerprint"] = Value::String(ca_fp);
        }
        obj
    } else {
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
            let (addr_state_listening, addr_state_reason) = if let Some(fresh) = &fresh_state {
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

            let folded_ip = crate::lan_door::fold_ipv4_mapped(ip);
            let addr_listening = addr_state_listening
                && leaf_sans
                    .as_ref()
                    .is_some_and(|sans| sans.contains(&folded_ip));

            let mut addr_obj = json!({
                "address": addr_str,
                "url": url_str.clone(),
                "listening": addr_listening,
            });
            if !addr_listening {
                let reason = if addr_state_listening {
                    "tls_unavailable".to_string()
                } else {
                    addr_state_reason
                        .or_else(|| lan_reason.clone())
                        .unwrap_or_else(|| "not_running".to_string())
                };
                addr_obj["reason"] = Value::String(reason);
            }
            addresses_json.push(addr_obj);
            urls.push(url_str);
        }

        let any_listening = addresses_json.iter().any(|a| a["listening"] == true);
        let mut obj = json!({
            "enabled": lan_door_enabled_flag,
            "listening": any_listening,
            "port": solstone_core_journal_config::MCP_LAN_DOOR_PORT,
            "addresses": addresses_json,
            "urls": urls,
        });
        if any_listening {
            if let Some(fp) = leaf_fingerprint {
                obj["fingerprint"] = Value::String(fp);
            }
        } else {
            let top_reason = if !lan_door_enabled_flag {
                "disabled".to_string()
            } else {
                lan_reason.unwrap_or_else(|| "not_running".to_string())
            };
            obj["reason"] = Value::String(top_reason);
        }
        if let Some(ca_fp) = ca_fingerprint {
            obj["ca_fingerprint"] = Value::String(ca_fp);
        }
        obj
    };

    let byo_config = solstone_core_journal_config::byo_hostname_config(&config);
    let byo_state = crate::byo_door::read_byo_door_state(root);
    let socket_path = root
        .join("mcp-endpoint/byo")
        .join(crate::unix::BYO_INGRESS_SOCKET)
        .to_string_lossy()
        .to_string();

    let byo_limits = json!([
        "owner_dns_control_required",
        "nameserver_ownership_unproven",
        "certificate_exclusivity_unproven",
        "forwarder_reads_plaintext",
        "dns_spoofing_outside_check"
    ]);

    let byo_json = match byo_config {
        solstone_core_journal_config::ByoHostnameConfigStatus::None => {
            json!({
                "hostname": Value::Null,
                "enabled": false,
                "generation": Value::Null,
                "account_uri": Value::Null,
                "caa": {
                    "ca": "letsencrypt.org",
                    "account_uri": Value::Null,
                    "validation_method": "tls-alpn-01",
                },
                "dns_verdict": "unchecked",
                "dns_observed_at": Value::Null,
                "socket_listening": false,
                "certificate_active": false,
                "socket_path": socket_path,
                "socket_blocker": Value::Null,
                "next_action": "set_hostname",
                "limits": byo_limits,
            })
        }
        solstone_core_journal_config::ByoHostnameConfigStatus::Invalid => {
            json!({
                "hostname": Value::Null,
                "enabled": false,
                "generation": Value::Null,
                "account_uri": Value::Null,
                "caa": {
                    "ca": "letsencrypt.org",
                    "account_uri": Value::Null,
                    "validation_method": "tls-alpn-01",
                },
                "dns_verdict": "unchecked",
                "dns_observed_at": Value::Null,
                "socket_listening": false,
                "certificate_active": false,
                "socket_path": socket_path,
                "socket_blocker": Value::Null,
                "next_action": "set_hostname",
                "limits": byo_limits,
            })
        }
        solstone_core_journal_config::ByoHostnameConfigStatus::Configured(cfg) => {
            let (account_uri, account_key_valid, certificate_active) = {
                if let Ok(root_jr) = solstone_core_journal_io::journal_root::JournalRoot::open(root)
                    && let Ok(byo_dir) = crate::unix::open_byo_directory(&root_jr)
                {
                    let (uri, key_valid) = if let Some(h) = &cfg.hostname
                        && let Ok(acc_dir) = crate::unix::open_byo_account_directory(&byo_dir, h)
                    {
                        let u = crate::unix::read_byo_account_uri(&acc_dir).ok().flatten();
                        let k = crate::unix::read_byo_account_key(&acc_dir).ok().flatten();
                        let kv = k.as_deref().is_some_and(|bytes| {
                            crate::tls::validate_acme_account_key(bytes).is_ok()
                        });
                        (u, kv)
                    } else {
                        (None, false)
                    };
                    let cert_active = if let Some(h) = &cfg.hostname
                        && let Ok(cert_dir) =
                            crate::unix::open_byo_cert_directory(&byo_dir, h, cfg.generation)
                        && let Ok(service) =
                            crate::tls::McpEndpointTlsService::for_byo_cert_directory(
                                cert_dir,
                                h.clone(),
                            ) {
                        service.ordinary_certificate_is_active()
                    } else {
                        false
                    };
                    (uri, key_valid, cert_active)
                } else {
                    (None, false, false)
                }
            };

            let (dns_verdict_str, dns_observed_at, raw_socket_listening, socket_blocker) =
                if let Some(state) = &byo_state {
                    let age = Utc::now().signed_duration_since(state.observed_at);
                    let matches_current = state.hostname == cfg.hostname
                        && state.generation == cfg.generation
                        && state.enabled == cfg.enabled
                        && age >= Duration::seconds(-5)
                        && age <= Duration::seconds(5);
                    (
                        if matches_current {
                            state.dns_verdict.as_deref().unwrap_or("unchecked")
                        } else {
                            "unchecked"
                        },
                        if matches_current {
                            state.dns_observed_at
                        } else {
                            None
                        },
                        matches_current && state.socket_listening,
                        if matches_current {
                            state
                                .socket_blocker
                                .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
                        } else {
                            None
                        },
                    )
                } else {
                    ("unchecked", None, false, None)
                };

            let dns_fresh = dns_observed_at
                .is_some_and(|obs| (Utc::now() - obs) <= chrono::Duration::seconds(60));
            let is_admitted = dns_fresh && dns_verdict_str == "admitted";
            let socket_listening = cfg.enabled && is_admitted && raw_socket_listening;

            let next_action = if cfg.hostname.is_none() {
                "set_hostname"
            } else if account_uri.is_none() && !account_key_valid {
                "register_account"
            } else if account_uri.is_some() && !account_key_valid {
                "account_key_lost"
            } else if !is_admitted {
                "publish_caa"
            } else if !cfg.enabled {
                "enable"
            } else if socket_blocker.is_some() {
                "socket_blocked"
            } else if !certificate_active {
                "issue_certificate"
            } else {
                "none"
            };

            let caa_obj = json!({
                "ca": "letsencrypt.org",
                "account_uri": account_uri,
                "validation_method": "tls-alpn-01",
            });

            json!({
                "hostname": cfg.hostname,
                "enabled": cfg.enabled,
                "generation": cfg.generation,
                "account_uri": account_uri,
                "caa": caa_obj,
                "dns_verdict": dns_verdict_str,
                "dns_observed_at": dns_observed_at,
                "socket_listening": socket_listening,
                "certificate_active": certificate_active,
                "socket_path": socket_path,
                "socket_blocker": socket_blocker,
                "next_action": next_action,
                "limits": byo_limits,
            })
        }
    };

    let mut response = json!({
        "enabled": enabled,
        "status": status,
        "local_door": local_door,
        "lan_door": lan_door,
        "byo": byo_json,
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

#[derive(Deserialize)]
struct SetByoRequest {
    #[serde(default)]
    hostname: Option<String>,
    enabled: bool,
}

async fn set_byo_hostname(
    Extension(journal): Extension<Arc<PathBuf>>,
    Json(payload): Json<SetByoRequest>,
) -> Response {
    let canonical = if let Some(ref raw_host) = payload.hostname {
        match solstone_core_journal_config::canonicalize_byo_hostname(raw_host) {
            Ok(c) => Some(c),
            Err(err) => {
                return refusal("invalid_hostname", err.to_string(), StatusCode::BAD_REQUEST);
            }
        }
    } else {
        None
    };

    let canonical_str = canonical;
    let enabled = payload.enabled;

    let mutation = mutate_journal_config(&journal, LockOptions::default(), move |config| {
        let status = solstone_core_journal_config::byo_hostname_config_from_map(config);
        let current = match &status {
            solstone_core_journal_config::ByoHostnameConfigStatus::Configured(c) => Some(c),
            _ => None,
        };
        let op = if let Some(ref h) = canonical_str {
            solstone_core_journal_config::ByoHostnameOp::SetHostname {
                hostname: h.as_str(),
                enabled,
            }
        } else {
            solstone_core_journal_config::ByoHostnameOp::SetEnabled { enabled }
        };
        let next = match solstone_core_journal_config::transition_byo_hostname(current, op) {
            Ok(n) => n,
            Err(e) => {
                return JournalConfigMutation {
                    changed: false,
                    value: Err(e.to_string()),
                };
            }
        };
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let Some(endpoint) = endpoint.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err("mcp_endpoint setting is not an object".to_string()),
            };
        };
        let mut byo_obj = serde_json::Map::new();
        if let Some(h) = next.hostname {
            byo_obj.insert("hostname".to_string(), json!(h));
        }
        byo_obj.insert("enabled".to_string(), json!(next.enabled));
        byo_obj.insert("generation".to_string(), json!(next.generation));
        endpoint.insert("byo_hostname".to_string(), Value::Object(byo_obj));
        JournalConfigMutation {
            changed: true,
            value: Ok(()),
        }
    });

    match mutation {
        Ok(trans) => match trans.value {
            Ok(()) => {
                if trans.changed {
                    perform_byo_cutover(journal).await
                } else {
                    state(Extension(journal)).await
                }
            }
            Err(e) => refusal("byo_transition_failed", e, StatusCode::CONFLICT),
        },
        Err(err) => refusal(
            "byo_mutation_failed",
            err.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

async fn remove_byo_hostname(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let mutation = mutate_journal_config(&journal, LockOptions::default(), |config| {
        let status = solstone_core_journal_config::byo_hostname_config_from_map(config);
        let current = match &status {
            solstone_core_journal_config::ByoHostnameConfigStatus::Configured(c) => Some(c),
            _ => None,
        };
        let next = match solstone_core_journal_config::transition_byo_hostname(
            current,
            solstone_core_journal_config::ByoHostnameOp::RemoveHostname,
        ) {
            Ok(n) => n,
            Err(e) => {
                return JournalConfigMutation {
                    changed: false,
                    value: Err(e.to_string()),
                };
            }
        };
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let Some(endpoint) = endpoint.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err("mcp_endpoint setting is not an object".to_string()),
            };
        };
        let mut byo_obj = serde_json::Map::new();
        if let Some(h) = next.hostname {
            byo_obj.insert("hostname".to_string(), json!(h));
        }
        byo_obj.insert("enabled".to_string(), json!(next.enabled));
        byo_obj.insert("generation".to_string(), json!(next.generation));
        endpoint.insert("byo_hostname".to_string(), Value::Object(byo_obj));
        JournalConfigMutation {
            changed: true,
            value: Ok(()),
        }
    });

    match mutation {
        Ok(trans) => match trans.value {
            Ok(()) => {
                if trans.changed {
                    perform_byo_cutover(journal).await
                } else {
                    state(Extension(journal)).await
                }
            }
            Err(e) => refusal("byo_transition_failed", e, StatusCode::CONFLICT),
        },
        Err(err) => refusal(
            "byo_mutation_failed",
            err.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

async fn perform_byo_cutover(journal: Arc<PathBuf>) -> Response {
    let journal_clone = Arc::clone(&journal);
    let cutover_result = tokio::task::spawn_blocking(move || {
        let cutover_path = journal_clone
            .join("mcp-endpoint")
            .join("byo")
            .join(crate::unix::BYO_CUTOVER_SOCKET);
        match std::os::unix::net::UnixStream::connect(&cutover_path) {
            Ok(stream) => {
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
                let sent = nix::unistd::write(&stream, b"apply\n");
                let mut buf = [0u8; 16];
                let recvd = nix::unistd::read(&stream, &mut buf);
                if matches!(sent, Ok(6))
                    && matches!(recvd, Ok(n) if n > 0 && std::str::from_utf8(&buf[..n]).unwrap_or("").trim() == "ok")
                {
                    Ok(())
                } else {
                    Err(false)
                }
            }
            Err(_) => Err(true),
        }
    })
    .await
    .unwrap_or(Err(true));

    match cutover_result {
        Ok(()) => state(Extension(journal)).await,
        Err(false) => refusal(
            "byo_cutover_unconfirmed",
            "BYO door did not confirm cutover within 2 seconds",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Err(true) => {
            let reclaimed = solstone_core_journal_io::journal_root::JournalRoot::open(&journal)
                .ok()
                .and_then(|root| crate::unix::open_byo_directory(&root).ok())
                .is_some_and(|byo_dir| {
                    let ingress_path = journal
                        .join("mcp-endpoint")
                        .join("byo")
                        .join(crate::unix::BYO_INGRESS_SOCKET);
                    crate::unix::reclaim_stale_byo_socket_if_inactive(
                        &byo_dir,
                        crate::unix::BYO_INGRESS_SOCKET,
                        &ingress_path,
                    )
                    .is_ok()
                });
            if !reclaimed {
                return refusal(
                    "byo_cutover_unconfirmed",
                    "BYO ingress could not be proved inactive",
                    StatusCode::SERVICE_UNAVAILABLE,
                );
            }
            let mut current_state =
                crate::byo_door::read_byo_door_state(&journal).unwrap_or_else(|| {
                    let config = read_journal_config(&journal).ok();
                    let byo_cfg = config
                        .as_ref()
                        .map(solstone_core_journal_config::byo_hostname_config);
                    let (hostname, enabled, generation) = match byo_cfg {
                        Some(
                            solstone_core_journal_config::ByoHostnameConfigStatus::Configured(c),
                        ) => (c.hostname, c.enabled, c.generation),
                        _ => (None, false, 0),
                    };
                    crate::byo_door::ByoDoorState {
                        hostname,
                        enabled,
                        generation,
                        account_uri: None,
                        caa: None,
                        dns_verdict: None,
                        dns_observed_at: None,
                        socket_listening: false,
                        certificate_active: false,
                        socket_path: None,
                        socket_blocker: None,
                        next_action: None,
                        observed_at: Utc::now(),
                    }
                });
            current_state.socket_listening = false;
            current_state.socket_blocker = None;
            current_state.observed_at = Utc::now();
            crate::byo_door::write_byo_door_state(&journal, &current_state);
            state(Extension(journal)).await
        }
    }
}

#[cfg(test)]
pub type TestRegistrarFn = Arc<dyn Fn(&[u8]) -> Result<String, String> + Send + Sync>;
#[cfg(test)]
pub static TEST_REGISTRAR: std::sync::RwLock<Option<TestRegistrarFn>> =
    std::sync::RwLock::new(None);

pub async fn register_acme_account(key_der: &[u8]) -> Result<String, String> {
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_REGISTRAR.read()
            && let Some(ref registrar) = *guard
        {
            return registrar(key_der);
        }
    }
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_config = Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(root_store)
        .with_no_client_auth(),
    );
    let directory = rustls_acme::acme::Directory::discover(
        &client_config,
        rustls_acme::acme::LETS_ENCRYPT_PRODUCTION_DIRECTORY,
    )
    .await
    .map_err(|e| e.to_string())?;
    let empty_contact: [String; 0] = [];
    let account = rustls_acme::acme::Account::create_with_keypair(
        &client_config,
        directory,
        empty_contact.iter(),
        key_der,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(account.kid)
}

async fn register_byo_account(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let config = match read_journal_config(&journal) {
        Ok(c) => c,
        Err(e) => {
            return refusal(
                "config_read_failed",
                e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let byo_config = solstone_core_journal_config::byo_hostname_config(&config);
    let hostname = match byo_config {
        solstone_core_journal_config::ByoHostnameConfigStatus::Configured(cfg) => {
            match cfg.hostname {
                Some(h) => h,
                None => {
                    return refusal(
                        "no_byo_hostname",
                        "BYO hostname is not configured",
                        StatusCode::BAD_REQUEST,
                    );
                }
            }
        }
        _ => {
            return refusal(
                "no_byo_hostname",
                "BYO hostname is not configured",
                StatusCode::BAD_REQUEST,
            );
        }
    };

    let root = match solstone_core_journal_io::journal_root::JournalRoot::open(&journal) {
        Ok(r) => r,
        Err(e) => {
            return refusal(
                "journal_root_failed",
                e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let byo_dir = match crate::unix::open_byo_directory(&root) {
        Ok(d) => d,
        Err(e) => {
            return refusal(
                "byo_dir_failed",
                e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let account_dir = match crate::unix::open_byo_account_directory(&byo_dir, &hostname) {
        Ok(d) => d,
        Err(e) => {
            return refusal(
                "account_dir_failed",
                e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };

    let existing_uri = crate::unix::read_byo_account_uri(&account_dir)
        .ok()
        .flatten();
    let existing_key = crate::unix::read_byo_account_key(&account_dir)
        .ok()
        .flatten();

    let is_key_valid = existing_key
        .as_deref()
        .is_some_and(|bytes| crate::tls::validate_acme_account_key(bytes).is_ok());

    match (existing_uri, is_key_valid) {
        (Some(_), true) => state(Extension(journal)).await,
        (Some(_), false) | (None, true) => refusal(
            "account_state_corrupt",
            "account key or URI is missing, call /replace",
            StatusCode::CONFLICT,
        ),
        (None, false) => {
            let keypair = match rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256) {
                Ok(kp) => kp,
                Err(_) => {
                    return refusal(
                        "key_generation_failed",
                        "P-256 generation failed",
                        StatusCode::INTERNAL_SERVER_ERROR,
                    );
                }
            };
            let key_der = keypair.serialize_der();
            let uri = match register_acme_account(&key_der).await {
                Ok(u) => u,
                Err(e) => {
                    return refusal(
                        "acme_registration_failed",
                        e,
                        StatusCode::INTERNAL_SERVER_ERROR,
                    );
                }
            };
            if let Err(e) = crate::unix::persist_byo_account_key(&account_dir, &key_der) {
                return refusal(
                    "persist_key_failed",
                    e.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            }
            if let Err(e) = crate::unix::persist_byo_account_uri(&account_dir, &uri) {
                return refusal(
                    "persist_uri_failed",
                    e.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            }
            state(Extension(journal)).await
        }
    }
}

async fn replace_byo_account(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let config = match read_journal_config(&journal) {
        Ok(c) => c,
        Err(e) => {
            return refusal(
                "config_read_failed",
                e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let byo_config = solstone_core_journal_config::byo_hostname_config(&config);
    let hostname = match byo_config {
        solstone_core_journal_config::ByoHostnameConfigStatus::Configured(cfg) => {
            match cfg.hostname {
                Some(h) => h,
                None => {
                    return refusal(
                        "no_byo_hostname",
                        "BYO hostname is not configured",
                        StatusCode::BAD_REQUEST,
                    );
                }
            }
        }
        _ => {
            return refusal(
                "no_byo_hostname",
                "BYO hostname is not configured",
                StatusCode::BAD_REQUEST,
            );
        }
    };

    let root = match solstone_core_journal_io::journal_root::JournalRoot::open(&journal) {
        Ok(r) => r,
        Err(e) => {
            return refusal(
                "journal_root_failed",
                e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let byo_dir = match crate::unix::open_byo_directory(&root) {
        Ok(d) => d,
        Err(e) => {
            return refusal(
                "byo_dir_failed",
                e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let account_dir = match crate::unix::open_byo_account_directory(&byo_dir, &hostname) {
        Ok(d) => d,
        Err(e) => {
            return refusal(
                "account_dir_failed",
                e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };

    // Replacement changes the CAA account authorization. Withdraw the public
    // ingress, including accepted streams, before touching either account
    // file. The owner must publish the new CAA pin and turn BYO on again.
    let cutover = set_byo_hostname(
        Extension(Arc::clone(&journal)),
        Json(SetByoRequest {
            hostname: None,
            enabled: false,
        }),
    )
    .await;
    if !cutover.status().is_success() {
        return cutover;
    }

    let _ = crate::unix::delete_byo_account_pair(&account_dir);
    let keypair = match rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256) {
        Ok(kp) => kp,
        Err(_) => {
            return refusal(
                "key_generation_failed",
                "P-256 generation failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let key_der = keypair.serialize_der();
    let uri = match register_acme_account(&key_der).await {
        Ok(u) => u,
        Err(e) => {
            return refusal(
                "acme_registration_failed",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    if let Err(e) = crate::unix::persist_byo_account_key(&account_dir, &key_der) {
        return refusal(
            "persist_key_failed",
            e.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    if let Err(e) = crate::unix::persist_byo_account_uri(&account_dir, &uri) {
        return refusal(
            "persist_uri_failed",
            e.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    state(Extension(journal)).await
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

async fn lan_door_ca_pem(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let path = journal.join(crate::lan_door::LAN_CA_PEM_PATH);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if content.is_empty() || content.contains("PRIVATE KEY") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(entries) = pem::parse_many(&content) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if entries.len() != 1 || entries[0].tag() != "CERTIFICATE" {
        return StatusCode::NOT_FOUND.into_response();
    }
    if parse_x509_certificate(entries[0].contents()).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/x-pem-file")
        .header(
            axum::http::header::CONTENT_DISPOSITION,
            "attachment; filename=\"solstone-lan-ca.pem\"",
        )
        .body(Body::from(content))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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
                    } else if door_str == "byo" {
                        Some("byo".to_string())
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
    use rustls::client::danger::ServerCertVerifier;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
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
    async fn owner_route_lan_ca_pem_download_and_isolation() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();

        // No basis -> 404
        let req_anon = Request::builder()
            .uri("/app/agents/api/lan-door/ca.pem")
            .body(Body::empty())
            .unwrap();
        let res_anon = owner_routes(journal_root.to_path_buf())
            .oneshot(req_anon)
            .await
            .unwrap();
        assert_eq!(res_anon.status(), StatusCode::NOT_FOUND);
        let bytes_anon = axum::body::to_bytes(res_anon.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_anon = String::from_utf8_lossy(&bytes_anon);
        assert!(!body_anon.contains("CERTIFICATE") && !body_anon.contains("PRIVATE KEY"));

        // PairingPeer -> 404
        let mut req_peer = Request::builder()
            .uri("/app/agents/api/lan-door/ca.pem")
            .body(Body::empty())
            .unwrap();
        req_peer.extensions_mut().insert(AccessBasis::PairingPeer {
            carrier: solstone_core_convey_http::identity::Carrier::Direct,
        });
        let res_peer = owner_routes(journal_root.to_path_buf())
            .oneshot(req_peer)
            .await
            .unwrap();
        assert_eq!(res_peer.status(), StatusCode::NOT_FOUND);

        // Owner with no CA file -> 404
        let mut req_no_ca = Request::builder()
            .uri("/app/agents/api/lan-door/ca.pem")
            .body(Body::empty())
            .unwrap();
        req_no_ca.extensions_mut().insert(AccessBasis::Localhost);
        let res_no_ca = owner_routes(journal_root.to_path_buf())
            .oneshot(req_no_ca)
            .await
            .unwrap();
        assert_eq!(res_no_ca.status(), StatusCode::NOT_FOUND);
        assert!(res_no_ca.headers().get("content-disposition").is_none());

        // Ready reconcile
        let ip_admitted = [IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))];
        let identity = crate::lan_door::reconcile_lan_identity(journal_root, &ip_admitted);
        assert!(identity.selected.is_some());

        // Owner request after ready reconcile
        let mut req_owner = Request::builder()
            .uri("/app/agents/api/lan-door/ca.pem")
            .body(Body::empty())
            .unwrap();
        req_owner.extensions_mut().insert(AccessBasis::Localhost);
        let res_owner = owner_routes(journal_root.to_path_buf())
            .oneshot(req_owner)
            .await
            .unwrap();
        assert_eq!(res_owner.status(), StatusCode::OK);
        assert_eq!(
            res_owner.headers().get("content-type").unwrap(),
            "application/x-pem-file"
        );
        assert_eq!(
            res_owner.headers().get("content-disposition").unwrap(),
            "attachment; filename=\"solstone-lan-ca.pem\""
        );

        let bytes_owner = axum::body::to_bytes(res_owner.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_owner = String::from_utf8_lossy(&bytes_owner);
        assert!(!body_owner.contains("PRIVATE KEY"));
        let entries = pem::parse_many(body_owner.as_bytes()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tag(), "CERTIFICATE");

        let ca_fp = crate::lan_door::compute_cert_fingerprint(entries[0].contents());
        let val = state_value(journal_root).unwrap();
        assert_eq!(val["lan_door"]["ca_fingerprint"], ca_fp);

        // WebPkiServerVerifier with downloaded CA as root verifies active leaf
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(
                entries[0].contents().to_vec(),
            ))
            .unwrap();
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .unwrap();

        let leaf_pem =
            std::fs::read_to_string(journal_root.join(crate::lan_door::LAN_LEAF_PEM_PATH)).unwrap();
        let leaf_entries = pem::parse_many(&leaf_pem).unwrap();
        let leaf_der = rustls::pki_types::CertificateDer::from(leaf_entries[0].contents().to_vec());
        let server_name = rustls::pki_types::ServerName::IpAddress(ip_admitted[0].into());
        let now = rustls::pki_types::UnixTime::now();
        assert!(
            verifier
                .verify_server_cert(&leaf_der, &[], &server_name, &[], now)
                .is_ok()
        );

        // Status JSON, state JSON, and downloaded PEM contain no private key
        let status_json_str = serde_json::to_string(&val).unwrap();
        assert!(!status_json_str.contains("PRIVATE KEY"));
        if let Ok(state_file_str) =
            std::fs::read_to_string(journal_root.join(crate::lan_door::LAN_DOOR_STATE_PATH))
        {
            assert!(!state_file_str.contains("PRIVATE KEY"));
        }
    }

    #[tokio::test]
    async fn state_projects_lan_door_posture_addresses_and_urls() {
        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

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

        // 1. Off/default with no CA: enabled: false, listening: false, reason: "disabled", no fingerprint, no ca_fingerprint.
        let val = state_value(journal_root).unwrap();
        assert_eq!(val["lan_door"]["enabled"], false);
        assert_eq!(val["lan_door"]["listening"], false);
        assert_eq!(val["lan_door"]["reason"], "disabled");
        assert_eq!(val["lan_door"]["port"], 7660);
        assert!(val["lan_door"].get("fingerprint").is_none());
        assert!(val["lan_door"].get("ca_fingerprint").is_none());

        // Phase 1: Turn on lan_door in config, forged state fingerprint sha256:0123456789abcdef and no leaf.
        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"lan_door":true}}"#,
        )
        .unwrap();

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
            Some(addr_states.clone()),
        );

        let val1 = state_value_with_iface(journal_root, &mock_iface).unwrap();
        assert_eq!(val1["lan_door"]["enabled"], true);
        assert_eq!(val1["lan_door"]["listening"], false); // no leaf -> door not listening
        assert_eq!(val1["lan_door"]["addresses"][0]["address"], "192.168.1.50");
        assert_eq!(val1["lan_door"]["addresses"][0]["listening"], false); // 192.168.1.50 not listening without leaf
        assert!(val1["lan_door"].get("fingerprint").is_none()); // no forged fingerprint echoed

        // Phase 2: Reconcile leaf for the admitted set
        let admitted = vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
        ];
        let identity = crate::lan_door::reconcile_lan_identity(journal_root, &admitted);
        assert!(identity.selected.is_some());
        let expected_leaf_fp = identity.selected.unwrap().leaf_fingerprint.clone();

        // Fresh state file marking only 192.168.1.50 listening, fd00::1 port_in_use
        crate::lan_door::write_lan_door_state(
            journal_root,
            true,
            None,
            Some(&expected_leaf_fp),
            Some(addr_states),
        );

        let val2 = state_value_with_iface(journal_root, &mock_iface).unwrap();
        assert_eq!(val2["lan_door"]["enabled"], true);
        assert_eq!(val2["lan_door"]["listening"], true);
        assert_eq!(val2["lan_door"]["fingerprint"], expected_leaf_fp);
        assert!(val2["lan_door"].get("ca_fingerprint").is_some());

        let addrs = val2["lan_door"]["addresses"].as_array().unwrap();
        assert_eq!(addrs.len(), 3);
        assert_eq!(addrs[0]["address"], "192.168.1.50");
        assert_eq!(addrs[0]["listening"], true);
        assert_eq!(addrs[1]["address"], "fd00::1");
        assert_eq!(addrs[1]["listening"], false);
        assert_eq!(addrs[1]["reason"], "port_in_use");
        assert_eq!(addrs[2]["address"], "100.64.0.1");
        assert_eq!(addrs[2]["listening"], false);

        let urls: Vec<&str> = val2["lan_door"]["urls"]
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

    #[tokio::test]
    async fn state_projects_enumeration_error_without_state_mutation() {
        struct FailingInterfaceSource;
        impl solstone_core_sol_link::pairing::addresses::RawInterfaceSource for FailingInterfaceSource {
            fn enumerate(
                &self,
            ) -> Result<
                Vec<solstone_core_sol_link::pairing::addresses::RawInterfaceAddress>,
                solstone_core_sol_link::pairing::addresses::AddressError,
            > {
                Err(
                    solstone_core_sol_link::pairing::addresses::AddressError::Enumeration(
                        std::io::Error::other("simulated error"),
                    ),
                )
            }
        }

        let temp = TempDir::new_in("/var/tmp").unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();

        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"lan_door":true}}"#,
        )
        .unwrap();

        // Write a ready reconcile first and initial state
        let admitted = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50))];
        let identity = crate::lan_door::reconcile_lan_identity(journal_root, &admitted);
        assert!(identity.selected.is_some());
        crate::lan_door::write_lan_door_state(journal_root, false, Some("retrying"), None, None);

        let state_bytes_before =
            std::fs::read(journal_root.join(crate::lan_door::LAN_DOOR_STATE_PATH)).unwrap();

        let val = state_value_with_iface(journal_root, &FailingInterfaceSource).unwrap();
        assert_eq!(val["lan_door"]["listening"], false);
        assert_eq!(val["lan_door"]["reason"], "enumeration_failed");
        assert_eq!(val["lan_door"]["addresses"], json!([]));
        assert!(val["lan_door"].get("fingerprint").is_none());
        assert!(val["lan_door"].get("ca_fingerprint").is_some());

        let state_bytes_after =
            std::fs::read(journal_root.join(crate::lan_door::LAN_DOOR_STATE_PATH)).unwrap();
        assert_eq!(state_bytes_before, state_bytes_after);
    }

    #[test]
    fn byo_state_json_disabled_limits() {
        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-byo-state-test-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let root = dir.path();
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("journal.json"),
            serde_json::to_string(&serde_json::json!({
                "mcp_endpoint": {
                    "byo_hostname": {
                        "hostname": "mcp.example.com",
                        "enabled": false,
                        "generation": 1
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        // Write door state with socket_listening: true
        let state = crate::byo_door::ByoDoorState {
            hostname: Some("mcp.example.com".to_string()),
            enabled: true,
            generation: 1,
            account_uri: Some("https://acme-v02.api.letsencrypt.org/acme/acct/12345".to_string()),
            caa: Some("admitted".to_string()),
            dns_verdict: Some("admitted".to_string()),
            dns_observed_at: Some(Utc::now()),
            socket_listening: true,
            certificate_active: true,
            socket_path: Some(
                root.join("mcp-endpoint/byo/ingress.sock")
                    .to_string_lossy()
                    .to_string(),
            ),
            socket_blocker: None,
            next_action: None,
            observed_at: Utc::now(),
        };
        crate::byo_door::write_byo_door_state(root, &state);

        let val = state_value(root).unwrap();
        assert_eq!(val["byo"]["hostname"], "mcp.example.com");
        assert_eq!(val["byo"]["enabled"], false);
        assert_eq!(val["byo"]["generation"], 1);
        assert_eq!(val["byo"]["socket_listening"], false);

        let limits: Vec<&str> = val["byo"]["limits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(limits.contains(&"forwarder_reads_plaintext"));
        assert!(limits.contains(&"owner_dns_control_required"));
    }

    #[test]
    fn byo_owner_state_does_not_report_a_stale_or_other_generation_socket() {
        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-byo-heartbeat-test-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"byo_hostname":{"hostname":"mcp.example.com","enabled":true,"generation":2}}}"#,
        )
        .unwrap();
        let mut state = crate::byo_door::ByoDoorState {
            hostname: Some("mcp.example.com".to_string()),
            enabled: true,
            generation: 1,
            account_uri: None,
            caa: Some("admitted".to_string()),
            dns_verdict: Some("admitted".to_string()),
            dns_observed_at: Some(Utc::now()),
            socket_listening: true,
            certificate_active: false,
            socket_path: None,
            socket_blocker: None,
            next_action: None,
            observed_at: Utc::now(),
        };
        crate::byo_door::write_byo_door_state(root, &state);
        let other_generation = state_value(root).unwrap();
        assert_eq!(other_generation["byo"]["socket_listening"], false);
        assert_eq!(other_generation["byo"]["dns_verdict"], "unchecked");

        state.generation = 2;
        state.observed_at = Utc::now() - Duration::seconds(10);
        crate::byo_door::write_byo_door_state(root, &state);
        let stale = state_value(root).unwrap();
        assert_eq!(stale["byo"]["socket_listening"], false);
        assert_eq!(stale["byo"]["dns_verdict"], "unchecked");
    }
}
