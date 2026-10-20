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
    let router = Router::new()
        .route("/app/agents/api/state", get(state))
        .route("/app/agents/api/capability", put(set_capability))
        .route("/app/agents/api/local-door", put(set_local_door))
        .route("/app/agents/api/lan-door", put(set_lan_door))
        .route("/app/agents/api/lan-door/ca.pem", get(lan_door_ca_pem))
        .route(
            "/app/agents/api/pairing",
            get(pairing_watch)
                .post(generate_pairing)
                .delete(revoke_pairing),
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
            "/app/agents/api/replay-notices/{id}/dismiss",
            post(dismiss_replay_notice),
        )
        .route(
            "/app/agents/api/solstone-me/account/replace",
            post(replace_sme_account),
        )
        .route(
            "/app/agents/api/solstone-me/account/notice/dismiss",
            post(dismiss_sme_account_notice),
        )
        .route("/app/agents/api/activity", get(activity));
    with_byo_routes(router)
        .route_layer(middleware::from_fn(admit_owner))
        .layer(Extension(journal))
}

fn with_byo_routes(router: Router) -> Router {
    router
        .route(
            "/app/agents/api/byo",
            put(set_byo_hostname).delete(remove_byo_hostname),
        )
        .route("/app/agents/api/byo/account", post(register_byo_account))
        .route(
            "/app/agents/api/byo/account/replace",
            post(replace_byo_account),
        )
        .route(
            "/app/agents/api/byo/relay",
            put(set_byo_relay).delete(remove_byo_relay),
        )
        .route(
            "/app/agents/api/byo/relay/commands",
            get(byo_relay_commands),
        )
        .route("/app/agents/api/byo/relay/check", post(byo_relay_check))
        .route(
            "/app/agents/api/byo/relay/trust-again",
            post(byo_relay_trust_again),
        )
        .route("/app/agents/api/byo/outside-check", post(byo_outside_check))
        .route("/app/agents/api/byo/dns-check", post(byo_dns_check))
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
        Json(json!({
            "code": code,
            "error": code,
            "reason_code": code,
            "detail": detail.to_string()
        })),
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
    let replay_notices = oauth
        .list_replay_notices()
        .map_err(|error| error.to_string())?;
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
        let door = grant_door(grant.resource.as_deref());
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
            "offline"
                | "failed"
                | "needs_subscription"
                | "not_accepted"
                | "update_required"
                | "account_changed"
                | "address_not_ready"
                | "address_refused"
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
    let socket_path = crate::byo_door::ingress_address(root);

    let byo_limits = json!([
        "owner_dns_control_required",
        "nameserver_ownership_unproven",
        "certificate_exclusivity_unproven",
        "forwarder_reads_plaintext",
        "dns_spoofing_outside_check"
    ]);

    let relay_config = solstone_core_journal_config::byo_relay_config(&config);

    let mut byo_json = match byo_config {
        solstone_core_journal_config::ByoHostnameConfigStatus::None => {
            let relay_obj = match &relay_config {
                solstone_core_journal_config::ByoRelayConfig::Valid { address, port } => {
                    json!({
                        "address": address.to_string(),
                        "port": port,
                        "state": "none",
                        "observed_at": Value::Null,
                    })
                }
                _ => Value::Null,
            };
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
                "caa_record": Value::Null,
                "dns_verdict": "unchecked",
                "dns_observed_at": Value::Null,
                "dns_addresses": [],
                "diagnostics": [],
                "relay": relay_obj,
                "socket_listening": false,
                "certificate_active": false,
                "socket_path": socket_path,
                "socket_blocker": Value::Null,
                "next_action": "set_hostname",
                "limits": byo_limits,
            })
        }
        solstone_core_journal_config::ByoHostnameConfigStatus::Invalid => {
            let relay_obj = match &relay_config {
                solstone_core_journal_config::ByoRelayConfig::Valid { address, port } => {
                    json!({
                        "address": address.to_string(),
                        "port": port,
                        "state": "none",
                        "observed_at": Value::Null,
                    })
                }
                _ => Value::Null,
            };
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
                "caa_record": Value::Null,
                "dns_verdict": "unchecked",
                "dns_observed_at": Value::Null,
                "dns_addresses": [],
                "diagnostics": [],
                "relay": relay_obj,
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
                                cfg.generation,
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

            let (
                matches_current,
                dns_verdict_str,
                dns_observed_at,
                raw_socket_listening,
                socket_blocker,
            ) = if let Some(state) = &byo_state {
                let age = Utc::now().signed_duration_since(state.observed_at);
                let current = state.hostname == cfg.hostname
                    && state.generation == cfg.generation
                    && state.enabled == cfg.enabled
                    && age >= Duration::seconds(-5)
                    && age <= Duration::seconds(5);
                (
                    current,
                    if current {
                        state.dns_verdict.as_deref().unwrap_or("unchecked")
                    } else {
                        "unchecked"
                    },
                    if current { state.dns_observed_at } else { None },
                    current && state.socket_listening,
                    if current {
                        state
                            .socket_blocker
                            .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
                    } else {
                        None
                    },
                )
            } else {
                (false, "unchecked", None, false, None)
            };

            let relay_obj = match &relay_config {
                solstone_core_journal_config::ByoRelayConfig::Valid { address, port } => {
                    let (r_state, r_obs) = if matches_current {
                        if let Some(state) = &byo_state {
                            (
                                state.relay_state.as_str(),
                                state
                                    .relay_observed_at
                                    .map(|t| json!(t))
                                    .unwrap_or(Value::Null),
                            )
                        } else {
                            ("none", Value::Null)
                        }
                    } else {
                        ("none", Value::Null)
                    };
                    json!({
                        "address": address.to_string(),
                        "port": port,
                        "state": r_state,
                        "observed_at": r_obs,
                    })
                }
                _ => Value::Null,
            };

            let dns_addresses_val = if matches_current {
                if let Some(state) = &byo_state {
                    json!(state.dns_addresses)
                } else {
                    json!([])
                }
            } else {
                json!([])
            };

            let diagnostics_val = if matches_current {
                if let Some(state) = &byo_state {
                    let verdict = state.dns_verdict.as_deref().unwrap_or("unchecked");
                    if verdict == "lookup_error" || verdict == "lookup_timeout" {
                        json!([])
                    } else {
                        let relay_ip = match &relay_config {
                            solstone_core_journal_config::ByoRelayConfig::Valid {
                                address, ..
                            } => Some(address),
                            _ => None,
                        };
                        let diags = crate::byo_dns::compute_byo_diagnostics(
                            relay_ip.copied(),
                            &state.dns_addresses,
                        );
                        json!(diags)
                    }
                } else {
                    json!([])
                }
            } else {
                json!([])
            };

            let caa_record = account_uri.as_ref().map(|uri| {
                format!(
                    "0 issue \"letsencrypt.org; accounturi={uri}; validationmethods=tls-alpn-01\""
                )
            });

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
                "caa_record": caa_record,
                "dns_verdict": dns_verdict_str,
                "dns_observed_at": dns_observed_at,
                "dns_addresses": dns_addresses_val,
                "diagnostics": diagnostics_val,
                "relay": relay_obj,
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
        "relay_available": true,
        "owner_state": owner_state,
        "certificate": certificate,
        "connections": connections,
        "replay_notices": replay_notices,
        "sme_account": {
            "replace_pending": status == "account_changed"
                && crate::sme_account_replace::pending_request(root, Utc::now()).is_some(),
            "replaced": crate::sme_account_replace::read_notice(root).map(|notice| json!({
                "address": notice.address,
                "replaced_at": notice.replaced_at,
            })),
        },
        "facets": facets,
        "pairing": pairing.map(pairing_json),
    });
    // What the forwarder reaches: a socket in the journal's folder on Unix,
    // a loopback address on Windows.
    byo_json["ingress"] = json!(if cfg!(windows) { "loopback" } else { "socket" });
    response["byo"] = byo_json;
    if status == "needs_subscription" {
        response["subscribe_url"] = json!(format!("{}/services/solstone-me", portal_origin()));
    }
    Ok(response)
}

/// The door a grant was made at, by the resource it is bound to.
fn grant_door(resource: Option<&str>) -> Value {
    match resource {
        Some(solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE) => {
            Value::String("local".to_owned())
        }
        Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE) => {
            Value::String("lan".to_owned())
        }
        Some(r) if r.starts_with("https://") => Value::String("byo".to_owned()),
        None => Value::String("relay".to_owned()),
        _ => Value::Null,
    }
}

/// The open code's non-secret state. The code itself is shown once, when it is made.
fn pairing_json(value: crate::PairingCodeSummary) -> Value {
    let mut pairing_obj = json!({
        "expires_at": value.expires_at,
        "generation": value.generation,
        "locked": value.locked,
    });
    if let Some(door) = value.door {
        pairing_obj["door"] = Value::String(door);
    }
    pairing_obj
}

async fn pairing_watch(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    match pairing_watch_value(&journal) {
        Ok(value) => Json(value).into_response(),
        Err(detail) => refusal(
            "agents_state_unavailable",
            detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// What the connect dialog watches while an owner connects an agent: the open
/// code and the agents paired in the browser. It reads only the OAuth ledger and
/// the permissions, never the week's activity, so the dialog can ask every few
/// seconds. `now` is the journal's clock, so the page can tell an expired code
/// from a used one without trusting the browser's.
pub(crate) fn pairing_watch_value(root: &std::path::Path) -> Result<Value, String> {
    let oauth = OAuthStore::open(root);
    let pairing = oauth
        .current_pairing_code()
        .map_err(|error| error.to_string())?;
    let grants = oauth.list_grants().map_err(|error| error.to_string())?;
    let permissions = PermissionStore::open(root)
        .read()
        .map_err(|error| error.to_string())?;
    let by_key: BTreeMap<_, _> = permissions
        .permissions
        .into_iter()
        .map(|permission| (permission.connection.clone(), permission))
        .collect();
    let connections: Vec<Value> = grants
        .into_iter()
        .map(|grant| {
            let key = format!("oauth:{}", grant.id);
            json!({
                "kind": "oauth", "id": grant.id, "key": key,
                "name": grant.client_name.as_deref().unwrap_or(&grant.client_id),
                "created_at": grant.created_at,
                "permission": by_key.get(&key),
                "door": grant_door(grant.resource.as_deref()),
            })
        })
        .collect();
    let zone = solstone_core_journal_config::owner_zone(root);
    let today = Utc::now().with_timezone(&zone).format("%Y%m%d").to_string();
    Ok(json!({
        "now": Utc::now(),
        "today": today,
        "pairing": pairing.map(pairing_json),
        "connections": connections,
    }))
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
        endpoint.remove("byo_relay");
        JournalConfigMutation {
            changed: true,
            value: Ok(()),
        }
    });

    match mutation {
        Ok(trans) => match trans.value {
            Ok(()) => {
                let _ = crate::byo_relay::delete_known_host(&journal);
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

pub(crate) async fn set_byo_relay(
    Extension(journal): Extension<Arc<PathBuf>>,
    Json(body): Json<Value>,
) -> Response {
    let Some(obj) = body.as_object() else {
        return refusal(
            "relay_invalid",
            "payload must be an object",
            StatusCode::BAD_REQUEST,
        );
    };
    let Some(addr_val) = obj.get("address") else {
        return refusal(
            "relay_invalid",
            "address is required",
            StatusCode::BAD_REQUEST,
        );
    };
    let Some(addr_str) = addr_val.as_str() else {
        return refusal(
            "relay_invalid",
            "address must be a string",
            StatusCode::BAD_REQUEST,
        );
    };
    let Ok(addr) = addr_str.parse::<std::net::Ipv4Addr>() else {
        return refusal(
            "relay_invalid",
            "address must be a valid ipv4 address",
            StatusCode::BAD_REQUEST,
        );
    };
    if !solstone_core_journal_config::is_public_relay_address(addr) {
        return refusal(
            "relay_address_not_public",
            "relay address is not public",
            StatusCode::BAD_REQUEST,
        );
    }
    let port: u16 = match obj.get("port") {
        None | Some(Value::Null) => 22,
        Some(Value::Number(n)) => match n.as_u64() {
            Some(p) if (1..=65535).contains(&p) => p as u16,
            _ => {
                return refusal(
                    "relay_port_invalid",
                    "port must be between 1 and 65535",
                    StatusCode::BAD_REQUEST,
                );
            }
        },
        _ => {
            return refusal(
                "relay_port_invalid",
                "port must be an integer",
                StatusCode::BAD_REQUEST,
            );
        }
    };

    let mutation = mutate_journal_config(&journal, LockOptions::default(), move |config| {
        let existing = solstone_core_journal_config::byo_relay_config_from_map(config);
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let Some(endpoint) = endpoint.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err("mcp_endpoint setting is not an object".to_string()),
            };
        };
        let mut relay_obj = serde_json::Map::new();
        relay_obj.insert("address".to_string(), json!(addr.to_string()));
        relay_obj.insert("port".to_string(), json!(port));
        endpoint.insert("byo_relay".to_string(), Value::Object(relay_obj));

        let changed_target = match existing {
            solstone_core_journal_config::ByoRelayConfig::Valid {
                address: a,
                port: p,
            } => a != addr || p != port,
            _ => true,
        };
        JournalConfigMutation {
            changed: true,
            value: Ok(changed_target),
        }
    });

    match mutation {
        Ok(trans) => match trans.value {
            Ok(changed_target) => {
                if changed_target {
                    let _ = crate::byo_relay::delete_known_host(&journal);
                }
                perform_byo_cutover(journal).await
            }
            Err(e) => refusal("byo_relay_mutation_failed", e, StatusCode::CONFLICT),
        },
        Err(err) => refusal(
            "byo_relay_mutation_failed",
            err.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub(crate) async fn remove_byo_relay(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let mutation = mutate_journal_config(&journal, LockOptions::default(), |config| {
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let Some(endpoint) = endpoint.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err("mcp_endpoint setting is not an object".to_string()),
            };
        };
        let changed = endpoint.remove("byo_relay").is_some();
        JournalConfigMutation {
            changed,
            value: Ok(()),
        }
    });

    match mutation {
        Ok(trans) => match trans.value {
            Ok(()) => {
                let _ = crate::byo_relay::delete_known_host(&journal);
                if trans.changed {
                    perform_byo_cutover(journal).await
                } else {
                    state(Extension(journal)).await
                }
            }
            Err(e) => refusal("byo_relay_mutation_failed", e, StatusCode::CONFLICT),
        },
        Err(err) => refusal(
            "byo_relay_mutation_failed",
            err.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

async fn byo_relay_commands(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    match crate::byo_relay::ensure_or_read_client_key(&journal) {
        Ok((_, pub_key)) => {
            let root = std::path::Path::new("/");
            // Paste-ready: run as a script under sudo, so a refusal ends the script
            // rather than the admin's interactive shell.
            let setup = format!(
                "sudo sh -s <<'SETUP'\n{}SETUP\n",
                crate::byo_relay::generate_setup_script(root, &pub_key)
            );
            let remove = format!(
                "sudo sh -s <<'REMOVE'\n{}REMOVE\n",
                crate::byo_relay::generate_remove_script(root)
            );
            Json(json!({
                "setup": setup,
                "remove": remove,
            }))
            .into_response()
        }
        Err(_) => refusal(
            "client_key_failed",
            "client key could not be stored",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub(crate) async fn byo_relay_check(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let config = read_journal_config(&journal).ok();
    let relay_cfg = config
        .as_ref()
        .map(solstone_core_journal_config::byo_relay_config)
        .unwrap_or(solstone_core_journal_config::ByoRelayConfig::Absent);
    let (relay_addr, relay_port) = match relay_cfg {
        solstone_core_journal_config::ByoRelayConfig::Valid { address, port } => (address, port),
        _ => return Json(json!({"code": "relay_not_configured"})).into_response(),
    };

    let byo_cfg = config
        .as_ref()
        .map(solstone_core_journal_config::byo_hostname_config);
    let byo_state = crate::byo_door::read_byo_door_state(&journal);

    let (_matches_current, bound) = if let (
        Some(state),
        Some(solstone_core_journal_config::ByoHostnameConfigStatus::Configured(cfg)),
    ) = (&byo_state, &byo_cfg)
    {
        let age = Utc::now().signed_duration_since(state.observed_at);
        let cur = state.hostname == cfg.hostname
            && state.generation == cfg.generation
            && state.enabled == cfg.enabled
            && age >= Duration::seconds(-5)
            && age <= Duration::seconds(5);
        (cur, cur && state.socket_listening)
    } else {
        (false, false)
    };

    if bound && let Some(state) = &byo_state {
        match state.relay_state.as_str() {
            "connecting" => return Json(json!({"code": "relay_connecting"})).into_response(),
            "connected" => return Json(json!({"code": "relay_ok"})).into_response(),
            "unreachable" => return Json(json!({"code": "relay_unreachable"})).into_response(),
            "auth_refused" => return Json(json!({"code": "relay_auth_refused"})).into_response(),
            "forward_refused" => {
                return Json(json!({"code": "relay_forward_refused"})).into_response();
            }
            "host_key_changed" => {
                return Json(json!({"code": "relay_host_key_changed"})).into_response();
            }
            _ => {}
        }
    }

    let code =
        match crate::byo_relay::execute_byo_relay_check(&journal, relay_addr, relay_port).await {
            Ok(()) => "relay_ok",
            Err("host_key_changed") => "relay_host_key_changed",
            Err("unreachable") => "relay_unreachable",
            Err("auth_refused") => "relay_auth_refused",
            Err("forward_refused") => "relay_forward_refused",
            Err(e) => e,
        };
    Json(json!({"code": code})).into_response()
}

pub(crate) async fn byo_relay_trust_again(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let _ = crate::byo_relay::delete_known_host(&journal);
    Json(json!({"ok": true})).into_response()
}

async fn byo_dns_check(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let config = read_journal_config(&journal).ok();
    let byo_cfg = config
        .as_ref()
        .map(solstone_core_journal_config::byo_hostname_config);
    let hostname = match byo_cfg {
        Some(solstone_core_journal_config::ByoHostnameConfigStatus::Configured(ref c)) => {
            c.hostname.clone()
        }
        _ => None,
    };
    let Some(hostname) = hostname else {
        return refusal(
            "hostname_not_configured",
            "hostname not configured",
            StatusCode::BAD_REQUEST,
        );
    };

    let account_uri = {
        if let Ok(root_jr) = solstone_core_journal_io::journal_root::JournalRoot::open(&journal)
            && let Ok(byo_dir) = crate::unix::open_byo_directory(&root_jr)
            && let Ok(acc_dir) = crate::unix::open_byo_account_directory(&byo_dir, &hostname)
        {
            crate::unix::read_byo_account_uri(&acc_dir).ok().flatten()
        } else {
            None
        }
    };
    let uri_str = account_uri.as_deref().unwrap_or("");

    let (verdict, addresses) =
        crate::byo_dns::resolve_byo_dns(&hostname, uri_str, Utc::now()).await;
    let verdict_code = verdict.code.as_str();

    let diagnostics = if verdict.code == crate::byo_dns::DnsVerdictCode::LookupError
        || verdict.code == crate::byo_dns::DnsVerdictCode::LookupTimeout
    {
        Vec::new()
    } else {
        let relay_cfg = config
            .as_ref()
            .map(solstone_core_journal_config::byo_relay_config);
        let relay_ip = match relay_cfg {
            Some(solstone_core_journal_config::ByoRelayConfig::Valid { address, .. }) => {
                Some(address)
            }
            _ => None,
        };
        crate::byo_dns::compute_byo_diagnostics(relay_ip, &addresses)
    };

    Json(json!({
        "verdict": verdict_code,
        "addresses": addresses,
        "diagnostics": diagnostics,
    }))
    .into_response()
}

#[derive(Debug)]
struct OutsideCheckVerifier {
    expected_leaf_der: Vec<u8>,
    seen_different_leaf: Arc<std::sync::atomic::AtomicBool>,
}

impl rustls::client::danger::ServerCertVerifier for OutsideCheckVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.expected_leaf_der.as_slice() {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            self.seen_different_leaf
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("tls 1.3 required".to_string()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub(crate) async fn byo_outside_check(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let config = read_journal_config(&journal).ok();
    let byo_cfg = config
        .as_ref()
        .map(solstone_core_journal_config::byo_hostname_config);
    let (hostname, generation) = match byo_cfg {
        Some(solstone_core_journal_config::ByoHostnameConfigStatus::Configured(ref c)) => {
            (c.hostname.clone(), c.generation)
        }
        _ => (None, 0),
    };
    let Some(hostname) = hostname else {
        return Json(json!({"code": "outside_not_ready"})).into_response();
    };

    let (cert_active, held_leaf_der) = if let Ok(root_jr) =
        solstone_core_journal_io::journal_root::JournalRoot::open(&journal)
        && let Ok(byo_dir) = crate::unix::open_byo_directory(&root_jr)
        && let Ok(cert_dir) = crate::unix::open_byo_cert_directory(&byo_dir, &hostname, generation)
        && let Ok(service) = crate::tls::McpEndpointTlsService::for_byo_cert_directory(
            cert_dir,
            hostname.clone(),
            generation,
        ) {
        (
            service.ordinary_certificate_is_active(),
            service.active_ordinary_leaf_der(),
        )
    } else {
        (false, None)
    };

    if !cert_active || held_leaf_der.is_none() {
        return Json(json!({"code": "outside_not_ready"})).into_response();
    }
    let expected_leaf_der = held_leaf_der.unwrap();

    let dial_future = async {
        let dial_addrs: Vec<std::net::SocketAddr> = {
            #[cfg(all(test, feature = "test-hooks"))]
            {
                if let Some(override_addr) = *crate::byo_dns::BYO_TEST_SEAM
                    .outside_check_addr_override
                    .read()
                    .unwrap()
                {
                    vec![override_addr]
                } else {
                    tokio::net::lookup_host(format!("{hostname}:443"))
                        .await
                        .map(|iter| iter.collect())
                        .unwrap_or_default()
                }
            }
            #[cfg(not(all(test, feature = "test-hooks")))]
            {
                tokio::net::lookup_host(format!("{hostname}:443"))
                    .await
                    .map(|iter| iter.collect())
                    .unwrap_or_default()
            }
        };

        if dial_addrs.is_empty() {
            return "outside_unreachable";
        }

        let seen_different_leaf = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let verifier = Arc::new(OutsideCheckVerifier {
            expected_leaf_der,
            seen_different_leaf: seen_different_leaf.clone(),
        });

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let client_config = match rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
        {
            Ok(b) => b
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth(),
            Err(_) => return "outside_unreachable",
        };

        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
        let server_name = match rustls::pki_types::ServerName::try_from(hostname.as_str()) {
            Ok(sn) => sn.to_owned(),
            Err(_) => return "outside_unreachable",
        };

        let tcp_stream = match tokio::net::TcpStream::connect(dial_addrs[0]).await {
            Ok(s) => s,
            Err(_) => return "outside_unreachable",
        };

        match connector.connect(server_name, tcp_stream).await {
            Ok(_tls_stream) => "outside_ok",
            Err(_) => {
                if seen_different_leaf.load(std::sync::atomic::Ordering::SeqCst) {
                    "outside_wrong_certificate"
                } else {
                    "outside_unreachable"
                }
            }
        }
    };

    let code = tokio::time::timeout(std::time::Duration::from_secs(10), dial_future)
        .await
        .unwrap_or("outside_unreachable");

    Json(json!({"code": code})).into_response()
}

/// Apply an owner-hostname change on Windows and wait for the door to confirm
/// it. There is no cutover endpoint on Windows: the door re-reads the setting
/// every half second and writes its state only after it has applied it, so a
/// withdrawn ingress is closed before that state can say so. If the door does
/// not confirm in time, the change still stands; the page may report the
/// ingress off only when nothing answers on it.
#[cfg(windows)]
pub(crate) async fn perform_byo_cutover(journal: Arc<PathBuf>) -> Response {
    let requested_at = Utc::now();
    let expected = match read_journal_config(&journal)
        .ok()
        .map(|config| solstone_core_journal_config::byo_hostname_config(&config))
    {
        Some(solstone_core_journal_config::ByoHostnameConfigStatus::Configured(c)) => {
            (c.hostname, c.enabled, c.generation)
        }
        _ => (None, false, 0),
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(current) = crate::byo_door::read_byo_door_state(&journal)
            && current.observed_at >= requested_at
            && (
                current.hostname.clone(),
                current.enabled,
                current.generation,
            ) == expected
        {
            return state(Extension(journal)).await;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // The door did not confirm. That is safe to report only if nothing
    // listens on the ingress, which is the case whenever the door is not
    // running.
    let address = crate::byo_door::ingress_address(&journal);
    let answering = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tokio::net::TcpStream::connect(address.as_str()),
    )
    .await
    .is_ok_and(|connected| connected.is_ok());
    if answering {
        return refusal(
            "byo_cutover_unconfirmed",
            "BYO door did not confirm cutover within 2 seconds",
            StatusCode::SERVICE_UNAVAILABLE,
        );
    }
    let (hostname, enabled, generation) = expected;
    let mut current_state =
        crate::byo_door::read_byo_door_state(&journal).unwrap_or(crate::byo_door::ByoDoorState {
            hostname,
            enabled,
            generation,
            account_uri: None,
            caa: None,
            dns_verdict: None,
            dns_observed_at: None,
            dns_addresses: Vec::new(),
            relay_state: "none".to_string(),
            relay_detail: None,
            relay_observed_at: None,
            socket_listening: false,
            certificate_active: false,
            socket_path: None,
            socket_blocker: None,
            next_action: None,
            observed_at: Utc::now(),
        });
    current_state.socket_listening = false;
    current_state.socket_blocker = None;
    current_state.relay_state = "none".to_string();
    current_state.relay_detail = None;
    current_state.relay_observed_at = None;
    current_state.dns_addresses = Vec::new();
    current_state.observed_at = Utc::now();
    crate::byo_door::write_byo_door_state(&journal, &current_state);
    state(Extension(journal)).await
}

#[cfg(unix)]
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
                        dns_addresses: Vec::new(),
                        relay_state: "none".to_string(),
                        relay_detail: None,
                        relay_observed_at: None,
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
            current_state.relay_state = "none".to_string();
            current_state.relay_detail = None;
            current_state.relay_observed_at = None;
            current_state.dns_addresses = Vec::new();
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
    let client = crate::acme_http::McpEndpointHttpClient::new();
    let pkcs8_der = rustls::pki_types::PrivatePkcs8KeyDer::from(key_der.to_vec());
    let instant_key =
        instant_acme::Key::from_pkcs8_der(pkcs8_der.clone_key()).map_err(|e| e.to_string())?;
    let pki_key = rustls::pki_types::PrivateKeyDer::Pkcs8(pkcs8_der);
    let (account, _creds) = instant_acme::Account::builder_with_http(Box::new(client))
        .create_from_key(
            (instant_key, pki_key),
            instant_acme::LetsEncrypt::Production.url().to_string(),
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(account.id().to_owned())
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
    activity_this_week_at(root, Utc::now())
}

fn activity_this_week_at(
    root: &std::path::Path,
    now: chrono::DateTime<Utc>,
) -> Result<(ConnectionActivity, bool), String> {
    // Audit records are filed under owner-zone days, so the week starts on the owner-zone Monday.
    let zone = solstone_core_journal_config::owner_zone(root);
    let today = now.with_timezone(&zone).date_naive();
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
    if !body.enabled {
        crate::sme_account_replace::withdraw_request(&journal);
    }
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
    if trimmed.is_empty() {
        return refusal(
            "pairing_create_failed",
            "missing door",
            StatusCode::BAD_REQUEST,
        );
    }
    let val: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return refusal(
                "pairing_create_failed",
                "invalid request body",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let Value::Object(map) = val else {
        return refusal(
            "pairing_create_failed",
            "invalid request body",
            StatusCode::BAD_REQUEST,
        );
    };
    let Some(Value::String(door_str)) = map.get("door") else {
        return refusal(
            "pairing_create_failed",
            "invalid door field",
            StatusCode::BAD_REQUEST,
        );
    };
    if !matches!(door_str.as_str(), "local" | "relay" | "lan" | "byo") {
        return refusal(
            "pairing_create_failed",
            "unsupported door",
            StatusCode::BAD_REQUEST,
        );
    }
    match OAuthStore::open(&journal).generate_pairing_code_with_door(door_str.as_str()) {
        Ok(created) => Json(json!({
            "code": created.code,
            "expires_at": created.expires_at,
            "generation": created.generation,
            "door": created.door,
        }))
        .into_response(),
        Err(error) => refusal(
            "pairing_create_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
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

async fn dismiss_replay_notice(
    Extension(journal): Extension<Arc<PathBuf>>,
    Path(id): Path<String>,
) -> Response {
    match OAuthStore::open(&journal).dismiss_replay_notice(&id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => refusal(
            "replay_notice_not_found",
            "replay notice was not found",
            StatusCode::NOT_FOUND,
        ),
        Err(error) => refusal(
            "replay_notice_dismiss_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// The owner's click to bring the solstone.me address back to this journal.
/// Accepted only while the address is refused for a changed certificate
/// account; the service process carries it on its next address request.
async fn replace_sme_account(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    let enabled = read_journal_config(&journal).is_ok_and(|config| {
        matches!(
            mcp_endpoint_capability(&config),
            Ok(McpEndpointCapability::Enabled)
        )
    });
    let account_changed = crate::read_mcp_owner_state(&journal)
        .is_some_and(|owner_state| owner_state.status == "account_changed");
    if !enabled || !account_changed {
        return refusal(
            "not_account_changed",
            "solstone.me is not refusing this journal for a changed certificate account",
            StatusCode::CONFLICT,
        );
    }
    if let Err(error) = crate::sme_account_replace::record_request(&journal, Utc::now()) {
        return refusal(
            "account_replace_request_failed",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    state(Extension(journal)).await
}

async fn dismiss_sme_account_notice(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    match crate::sme_account_replace::dismiss_notice(&journal) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refusal(
            "account_notice_dismiss_failed",
            error,
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
    let allowed = ["transcripts", "entities", "facets", "screen_text"];
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
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use rustls::client::danger::ServerCertVerifier;
    use sha2::{Digest, Sha256};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use tempfile::TempDir;
    use tower::ServiceExt;

    #[test]
    fn state_defaults_off_and_keeps_connection_state_separate() {
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
        let value = state_value(temp.path()).unwrap();
        assert_eq!(value["enabled"], false);
        assert_eq!(value["status"], "off");
        assert_eq!(value["connections"], json!([]));
    }

    #[test]
    fn pairing_watch_follows_a_code_to_its_connection_and_never_carries_the_code() {
        use crate::oauth::OAuthRuntime;

        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        let empty = pairing_watch_value(root).unwrap();
        assert!(empty["pairing"].is_null());
        assert_eq!(empty["connections"], json!([]));
        assert!(empty["now"].is_string());
        assert_eq!(empty["today"].as_str().map(str::len), Some(8));

        let relay = OAuthRuntime::new(root, "https://mcp.test".to_owned());
        let binding = relay.binding();
        let pairing = relay
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let open = pairing_watch_value(root).unwrap();
        assert_eq!(open["pairing"]["generation"], pairing.generation);
        assert_eq!(open["pairing"]["door"], "relay");
        assert_eq!(open["pairing"]["locked"], false);
        assert!(!open.to_string().contains(&pairing.code));

        let client = relay
            .store
            .register_client(
                "https://client.example/watch.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("Watching Agent".to_owned()),
                "192.0.2.1",
            )
            .unwrap();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(b"watch-verifier"));
        let transaction = relay
            .store
            .create_transaction_with_generation(
                &client.id,
                "http://127.0.0.1/callback",
                binding.canonical(),
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.1",
                binding.stored_grant_generation(),
            )
            .unwrap();
        let authorization = relay
            .store
            .complete_pairing_with_permission(
                &transaction,
                &pairing.code,
                Some(ReadPermission {
                    categories: vec!["entities".to_owned()],
                    scope: ReadScope::WholeJournal,
                }),
                &binding,
            )
            .unwrap();
        // The code is spent as soon as it verifies, before the agent has its tokens.
        let spent = pairing_watch_value(root).unwrap();
        assert!(spent["pairing"].is_null());
        assert_eq!(spent["connections"], json!([]));
        let tokens = relay
            .store
            .redeem_authorization_code(
                &authorization.code,
                &client.client_id,
                "http://127.0.0.1/callback",
                binding.canonical(),
                "watch-verifier",
                &binding,
            )
            .unwrap();
        let connected = pairing_watch_value(root).unwrap();
        let connections = connected["connections"].as_array().unwrap();
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0]["key"], format!("oauth:{}", tokens.token_id));
        assert_eq!(connections[0]["name"], "Watching Agent");
        assert_eq!(connections[0]["door"], "relay");
        assert_eq!(
            connections[0]["permission"]["read"]["categories"],
            json!(["entities"])
        );
        assert_eq!(
            connections[0]["permission"]["read"]["scope"]["kind"],
            "whole_journal"
        );
        // The full state reports the same connection the same way.
        let state = state_value(root).unwrap();
        let listed = state["connections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|connection| connection["key"] == connections[0]["key"])
            .unwrap();
        assert_eq!(listed["door"], connections[0]["door"]);
        assert_eq!(listed["permission"], connections[0]["permission"]);
    }

    #[test]
    fn pairing_watch_today_matches_configured_owner_zone() {
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            json!({ "identity": { "timezone": "Asia/Tokyo" } }).to_string(),
        )
        .unwrap();

        let value = pairing_watch_value(root).unwrap();
        let expected_today = chrono::Utc::now()
            .with_timezone(&solstone_core_journal_config::owner_zone(root))
            .format("%Y%m%d")
            .to_string();
        assert_eq!(value["today"], expected_today);
    }

    #[test]
    fn activity_this_week_uses_owner_zone_monday_window() {
        use chrono::TimeZone;
        use solstone_core_journal_config::owner_zone;
        use solstone_core_mcp_audit::{Admission, ToolName, write_interaction_record};

        for tz_name in ["Pacific/Kiritimati", "Etc/GMT+12"] {
            let temp = TempDir::new_in(crate::test_scratch()).unwrap();
            let root = temp.path();
            std::fs::create_dir_all(root.join("config")).unwrap();
            std::fs::write(
                root.join("config/journal.json"),
                json!({ "identity": { "timezone": tz_name } }).to_string(),
            )
            .unwrap();

            let zone = owner_zone(root);
            let monday_civil = chrono::NaiveDate::from_ymd_opt(2026, 8, 31)
                .unwrap()
                .and_hms_opt(0, 0, 5)
                .unwrap();
            let monday_dt = zone.from_local_datetime(&monday_civil).single().unwrap();
            let monday_utc = monday_dt.with_timezone(&Utc);

            let admission = Admission {
                connection: "operator",
                agent_identity: "operator",
                tool_name: ToolName::Search,
                arguments: serde_json::Map::new(),
                permission: None,
            };

            write_interaction_record(root, monday_dt, &admission)
                .expect("write interaction record");

            let query_utc = monday_utc + Duration::hours(2);
            let (activity, complete) =
                activity_this_week_at(root, query_utc).expect("activity this week");
            assert!(complete);
            assert!(
                activity
                    .get("operator")
                    .map(|(count, _)| *count)
                    .unwrap_or(0)
                    >= 1,
                "connection count for operator must be at least 1 in {tz_name}"
            );
        }

        let temp_host = TempDir::new_in(crate::test_scratch()).unwrap();
        let root_host = temp_host.path();
        let zone = owner_zone(root_host);
        let monday_civil = chrono::NaiveDate::from_ymd_opt(2026, 8, 31)
            .unwrap()
            .and_hms_opt(0, 0, 5)
            .unwrap();
        let monday_dt = zone.from_local_datetime(&monday_civil).single().unwrap();
        let monday_utc = monday_dt.with_timezone(&Utc);

        let admission = Admission {
            connection: "operator",
            agent_identity: "operator",
            tool_name: ToolName::Search,
            arguments: serde_json::Map::new(),
            permission: None,
        };
        write_interaction_record(root_host, monday_dt, &admission)
            .expect("write interaction record");

        let query_utc = monday_utc + Duration::hours(2);
        let (activity, complete) =
            activity_this_week_at(root_host, query_utc).expect("activity this week");
        assert!(complete);
        assert!(
            activity
                .get("operator")
                .map(|(count, _)| *count)
                .unwrap_or(0)
                >= 1,
            "connection count for operator must be at least 1 in host zone"
        );
    }

    #[test]
    fn replay_notice_is_snapshotted_dismissible_and_bounded() {
        use crate::oauth::OAuthRuntime;

        fn replay_one(
            runtime: &OAuthRuntime,
            client_record_id: &str,
            client_id: &str,
            door: &str,
            source: &str,
        ) -> String {
            let binding = runtime.binding();
            let pairing = runtime.store.generate_pairing_code_with_door(door).unwrap();
            let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(b"notice-verifier"));
            let transaction = runtime
                .store
                .create_transaction_with_generation(
                    client_record_id,
                    "http://127.0.0.1/callback",
                    binding.canonical(),
                    "https://mcp.test",
                    &challenge,
                    "S256",
                    None,
                    source,
                    binding.stored_grant_generation(),
                )
                .unwrap();
            let authorization = runtime
                .store
                .complete_pairing(&transaction, &pairing.code, &binding)
                .unwrap();
            let tokens = runtime
                .store
                .redeem_authorization_code(
                    &authorization.code,
                    client_id,
                    "http://127.0.0.1/callback",
                    binding.canonical(),
                    "notice-verifier",
                    &binding,
                )
                .unwrap();
            let rotated = runtime
                .store
                .refresh_grant(&tokens.refresh_token, client_id, &binding)
                .unwrap();
            runtime
                .store
                .refresh_grant(&rotated.refresh_token, client_id, &binding)
                .unwrap();
            assert!(matches!(
                runtime
                    .store
                    .refresh_grant(&tokens.refresh_token, client_id, &binding),
                Err(crate::oauth::store::OAuthStoreError::RefreshReused { .. })
            ));
            tokens.token_id
        }

        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        let relay = OAuthRuntime::new(root, "https://mcp.test".to_owned());
        let client = relay
            .store
            .register_client(
                "https://client.example/notices.json",
                vec!["http://127.0.0.1/callback".to_owned()],
                Some("Known Claude".to_owned()),
                "192.0.2.1",
            )
            .unwrap();
        let oldest = replay_one(&relay, &client.id, &client.client_id, "relay", "192.0.2.1");
        let state_after_replay = state_value(root).unwrap();
        let notice = state_after_replay["replay_notices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|notice| notice["id"] == oldest)
            .unwrap();
        assert_eq!(notice["name"], "Known Claude");
        assert_eq!(notice["door"], "solstone.me");
        assert!(
            state_after_replay["connections"]
                .as_array()
                .unwrap()
                .iter()
                .all(|connection| connection["id"] != oldest)
        );
        assert!(relay.store.dismiss_replay_notice(&oldest).unwrap());
        let dismissed_state = state_value(root).unwrap();
        let dismissed = dismissed_state["replay_notices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|notice| notice["id"] == oldest)
            .unwrap();
        assert_eq!(dismissed["dismissed"], true);

        std::fs::write(
            root.join("config/journal.json"),
            serde_json::to_vec(&json!({
                "mcp_endpoint": {
                    "byo_hostname": {
                        "hostname": "journal.example",
                        "enabled": true,
                        "generation": 1,
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let hostname = OAuthRuntime::new_byo(root, "journal.example", 1);
        let hostname_id = replay_one(&hostname, &client.id, &client.client_id, "byo", "192.0.2.1");
        let hostname_state = state_value(root).unwrap();
        let hostname_notice = hostname_state["replay_notices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|notice| notice["id"] == hostname_id)
            .unwrap();
        assert_eq!(hostname_notice["door"], "your own domain");

        for _ in 0..15 {
            replay_one(&relay, &client.id, &client.client_id, "relay", "192.0.2.1");
        }
        let bounded_state = state_value(root).unwrap();
        let notices = bounded_state["replay_notices"].as_array().unwrap();
        assert_eq!(notices.len(), 16);
        assert!(!notices.iter().any(|notice| notice["id"] == oldest));
        assert!(
            notices.iter().any(|notice| {
                notice["id"] == hostname_id && notice["door"] == "your own domain"
            })
        );
    }

    #[test]
    fn state_projects_needs_subscription_status_and_subscribe_url() {
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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
    fn state_projects_each_certificate_account_hold_with_its_own_status_and_words() {
        use crate::bridge_carrier::RegistrationHold;
        for (hold, status) in [
            (RegistrationHold::UpdateRequired, "update_required"),
            (RegistrationHold::AccountChanged, "account_changed"),
            (RegistrationHold::AddressNotReady, "address_not_ready"),
            (RegistrationHold::AddressRefused, "address_refused"),
        ] {
            let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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
                hold,
                Some("aaaqeaye.solstone.me"),
                ("done", "done", "waiting"),
                Utc::now() + Duration::seconds(60),
            );
            let value = state_value(journal_root).unwrap();
            assert_eq!(value["status"], status, "{status}");
            let detail = value["owner_state"]["detail"].as_str().unwrap_or_default();
            assert!(!detail.is_empty(), "{status} carries owner words");
            assert!(value.get("subscribe_url").is_none(), "{status}");
        }
    }

    fn post_owner(uri: &str) -> Request<Body> {
        let mut request = Request::builder()
            .method(axum::http::Method::POST)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(AccessBasis::Localhost);
        request
    }

    #[tokio::test]
    async fn the_owner_can_ask_to_bring_the_address_back_only_while_the_account_changed() {
        use crate::bridge_carrier::RegistrationHold;
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
        std::fs::write(
            journal_root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"enabled":true}}"#,
        )
        .unwrap();
        let replace = || {
            owner_routes(journal_root.to_path_buf())
                .oneshot(post_owner("/app/agents/api/solstone-me/account/replace"))
        };

        // No refusal on record, then a different refusal: nothing to bring back.
        assert_eq!(replace().await.unwrap().status(), StatusCode::CONFLICT);
        crate::owner_state::write_mcp_hold_state(
            journal_root,
            RegistrationHold::AddressNotReady,
            None,
            ("waiting", "waiting", "waiting"),
            Utc::now() + Duration::seconds(60),
        );
        assert_eq!(replace().await.unwrap().status(), StatusCode::CONFLICT);
        assert!(crate::sme_account_replace::pending_request(journal_root, Utc::now()).is_none());

        crate::owner_state::write_mcp_hold_state(
            journal_root,
            RegistrationHold::AccountChanged,
            None,
            ("waiting", "waiting", "waiting"),
            Utc::now() + Duration::seconds(300),
        );
        let before = state_value(journal_root).unwrap();
        assert_eq!(before["status"], "account_changed");
        assert_eq!(before["sme_account"]["replace_pending"], false);
        let response = replace().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let after: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(after["sme_account"]["replace_pending"], true);
        assert!(crate::sme_account_replace::pending_request(journal_root, Utc::now()).is_some());

        // Turning solstone.me off withdraws the waiting click, and while it is
        // off a click is refused even with the refusal still on record.
        let mut off = Request::builder()
            .method(axum::http::Method::PUT)
            .uri("/app/agents/api/capability")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"enabled":false}"#))
            .unwrap();
        off.extensions_mut().insert(AccessBasis::Localhost);
        let off = owner_routes(journal_root.to_path_buf())
            .oneshot(off)
            .await
            .unwrap();
        assert!(off.status().is_success(), "{}", off.status());
        assert!(crate::sme_account_replace::pending_request(journal_root, Utc::now()).is_none());
        assert_eq!(replace().await.unwrap().status(), StatusCode::CONFLICT);
        assert!(crate::sme_account_replace::pending_request(journal_root, Utc::now()).is_none());
    }

    #[tokio::test]
    async fn the_notice_that_the_address_came_back_shows_until_dismissed() {
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
        let journal_root = temp.path();
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
        assert_eq!(
            state_value(journal_root).unwrap()["sme_account"]["replaced"],
            Value::Null
        );
        let at = Utc::now();
        crate::sme_account_replace::observe_replacement(
            journal_root,
            "aaaqeaye.solstone.me",
            "reported",
            Some(at),
            at,
        );
        let value = state_value(journal_root).unwrap();
        assert_eq!(
            value["sme_account"]["replaced"]["address"],
            "aaaqeaye.solstone.me"
        );
        assert_eq!(value["sme_account"]["replaced"]["replaced_at"], json!(at));
        let response = owner_routes(journal_root.to_path_buf())
            .oneshot(post_owner(
                "/app/agents/api/solstone-me/account/notice/dismiss",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            state_value(journal_root).unwrap()["sme_account"]["replaced"],
            Value::Null
        );
    }

    #[test]
    fn state_projects_not_accepted_status_and_no_subscribe_url() {
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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

    #[tokio::test(start_paused = true)]
    async fn local_door_switch_writes_its_own_key_and_repairs_an_invalid_value() {
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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

        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
        for route in ["/app/agents/api/state", "/app/agents/api/pairing"] {
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
                    leaf_spki: vec![0x30, 0x00],
                });
            let linked_res = owner_routes(temp.path().to_path_buf())
                .oneshot(linked_req)
                .await
                .unwrap();
            assert_eq!(linked_res.status(), StatusCode::OK);
        }
        let cid = LinkedDeviceCid::try_from(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();

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
                leaf_spki: vec![0x30, 0x00],
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
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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
        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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

        let temp = TempDir::new_in(crate::test_scratch()).unwrap();
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
            .tempdir_in(crate::test_scratch())
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
            dns_addresses: Vec::new(),
            relay_state: "none".to_string(),
            relay_detail: None,
            relay_observed_at: None,
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
        assert_eq!(
            val["byo"]["ingress"],
            if cfg!(windows) { "loopback" } else { "socket" }
        );
        assert_eq!(
            val["byo"]["socket_path"],
            crate::byo_door::ingress_address(root).as_str()
        );
    }

    #[test]
    fn byo_owner_state_does_not_report_a_stale_or_other_generation_socket() {
        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-byo-heartbeat-test-")
            .tempdir_in(crate::test_scratch())
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
            dns_addresses: Vec::new(),
            relay_state: "none".to_string(),
            relay_detail: None,
            relay_observed_at: None,
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

    #[tokio::test]
    async fn owner_api_pairing_generate_validates_door_and_preserves_oauth_file_on_refusal() {
        use axum::body::to_bytes;

        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-owner-pairing-test-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let root = dir.path();
        let journal = Arc::new(root.to_path_buf());
        let oauth_path = root.join("mcp-endpoint/oauth.json");

        // 1. Empty body -> 400
        let resp = generate_pairing(Extension(journal.clone()), axum::body::Bytes::new()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!oauth_path.exists());

        // 2. Missing door field -> 400
        let resp =
            generate_pairing(Extension(journal.clone()), axum::body::Bytes::from(r#"{}"#)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!oauth_path.exists());

        // 3. Non-string door -> 400
        let resp = generate_pairing(
            Extension(journal.clone()),
            axum::body::Bytes::from(r#"{"door": 123}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!oauth_path.exists());

        // 4. Invalid door symbol -> 400
        let resp = generate_pairing(
            Extension(journal.clone()),
            axum::body::Bytes::from(r#"{"door": "unsupported"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!oauth_path.exists());

        // 5. Raw LAN resource string -> 400
        let resp = generate_pairing(
            Extension(journal.clone()),
            axum::body::Bytes::from(r#"{"door": "urn:solstone:mcp-door:lan"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!oauth_path.exists());

        // 6. Valid door "relay" -> 200
        let resp = generate_pairing(
            Extension(journal.clone()),
            axum::body::Bytes::from(r#"{"door": "relay"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(oauth_path.exists());
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["door"], "relay");
        assert!(val.get("code").is_some());
        assert!(val.get("expires_at").is_some());
        assert!(val.get("generation").is_some());

        let bytes_after_relay = std::fs::read(&oauth_path).unwrap();

        // 7. Invalid request when oauth.json already exists -> leaves file byte-identical
        let resp = generate_pairing(
            Extension(journal.clone()),
            axum::body::Bytes::from(r#"{"door": "invalid"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes_after_refusal = std::fs::read(&oauth_path).unwrap();
        assert_eq!(bytes_after_relay, bytes_after_refusal);

        // 8. Valid door "local" -> 200
        let resp = generate_pairing(
            Extension(journal.clone()),
            axum::body::Bytes::from(r#"{"door": "local"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["door"], "local");

        // 9. Valid door "byo" when hostname is configured -> 200
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"byo_hostname":{"hostname":"mcp.example.com","enabled":true,"generation":1}}}"#,
        )
        .unwrap();
        let resp = generate_pairing(
            Extension(journal.clone()),
            axum::body::Bytes::from(r#"{"door": "byo"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["door"], "byo");
    }

    #[test]
    fn caa_record_format_and_policy_admission() {
        let account_uri = "https://acme-v02.api.letsencrypt.org/acme/acct/12345";
        let caa_record_str = format!(
            "0 issue \"letsencrypt.org; accounturi={account_uri}; validationmethods=tls-alpn-01\""
        );

        let parts: Vec<&str> = caa_record_str.splitn(3, ' ').collect();
        assert_eq!(parts.len(), 3);
        let flags: u8 = parts[0].parse().unwrap();
        assert_eq!(flags, 0);
        let tag = parts[1];
        assert_eq!(tag, "issue");
        let val_unquoted = parts[2].trim_matches('"');
        assert_eq!(
            val_unquoted,
            format!("letsencrypt.org; accounturi={account_uri}; validationmethods=tls-alpn-01")
        );

        let mut records_by_domain = std::collections::HashMap::new();
        records_by_domain.insert(
            "mcp.example.com".to_string(),
            crate::byo_dns::HostDnsRecords {
                a: vec![std::net::Ipv4Addr::new(203, 0, 113, 10)],
                aaaa: Vec::new(),
                cname: Vec::new(),
                caa: vec![crate::byo_dns::CaaRecord {
                    flags,
                    tag: tag.to_string(),
                    value: val_unquoted.to_string(),
                }],
            },
        );

        let verdict = crate::byo_dns::evaluate_byo_dns_policy(
            "mcp.example.com",
            account_uri,
            &records_by_domain,
            Utc::now(),
        );
        assert_eq!(verdict.code, crate::byo_dns::DnsVerdictCode::Admitted);
    }

    #[test]
    fn byo_owner_state_projection_diagnostics_and_relay_expiry() {
        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-byo-proj-test-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"byo_hostname":{"hostname":"mcp.example.com","enabled":true,"generation":1},"byo_relay":{"address":"203.0.113.10","port":22}}}"#,
        )
        .unwrap();

        // 1. Current lookup_error file with empty addresses -> empty diagnostics
        let mut door_state = crate::byo_door::ByoDoorState {
            hostname: Some("mcp.example.com".to_string()),
            enabled: true,
            generation: 1,
            account_uri: None,
            caa: None,
            dns_verdict: Some("lookup_error".to_string()),
            dns_observed_at: Some(Utc::now()),
            dns_addresses: Vec::new(),
            relay_state: "connected".to_string(),
            relay_detail: None,
            relay_observed_at: Some(Utc::now()),
            socket_listening: true,
            certificate_active: false,
            socket_path: None,
            socket_blocker: None,
            next_action: None,
            observed_at: Utc::now(),
        };
        crate::byo_door::write_byo_door_state(root, &door_state);
        let val = state_value(root).unwrap();
        assert_eq!(val["byo"]["diagnostics"], json!([]));
        assert_eq!(val["byo"]["relay"]["state"], "connected");

        // 2. Current admitted file whose dns_addresses do not match relay -> ["address_mismatch"]
        door_state.dns_verdict = Some("admitted".to_string());
        door_state.dns_addresses = vec!["198.51.100.5".to_string()];
        crate::byo_door::write_byo_door_state(root, &door_state);
        let val = state_value(root).unwrap();
        assert_eq!(val["byo"]["diagnostics"], json!(["address_mismatch"]));

        // 3. A connected file with observed_at older than 5 seconds projects relay.state "none"
        door_state.observed_at = Utc::now() - Duration::seconds(6);
        crate::byo_door::write_byo_door_state(root, &door_state);
        let val = state_value(root).unwrap();
        assert_eq!(val["byo"]["relay"]["state"], "none");
    }

    #[tokio::test]
    async fn byo_config_and_relay_mutations() {
        use axum::body::to_bytes;

        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-byo-mut-test-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let root = dir.path();
        let journal = Arc::new(root.to_path_buf());
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"byo_hostname":{"hostname":"initial.example.com","enabled":false,"generation":1},"byo_relay":{"address":"203.0.113.10","port":22}}}"#,
        )
        .unwrap();

        // 1. Hostname save keeps byo_relay
        let resp = set_byo_hostname(
            Extension(journal.clone()),
            Json(SetByoRequest {
                hostname: Some("updated.example.com".to_string()),
                enabled: false,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let cfg = read_journal_config(root).unwrap();
        let relay = solstone_core_journal_config::byo_relay_config(&cfg);
        assert!(
            matches!(relay, solstone_core_journal_config::ByoRelayConfig::Valid { address, port } if address == "203.0.113.10".parse::<std::net::Ipv4Addr>().unwrap() && port == 22)
        );

        // 2. PUT private address returns relay_address_not_public and does not write
        let resp = set_byo_relay(
            Extension(journal.clone()),
            Json(json!({"address": "192.168.1.1", "port": 22})),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_address_not_public");
        let cfg = read_journal_config(root).unwrap();
        let relay = solstone_core_journal_config::byo_relay_config(&cfg);
        assert!(
            matches!(relay, solstone_core_journal_config::ByoRelayConfig::Valid { address, .. } if address == "203.0.113.10".parse::<std::net::Ipv4Addr>().unwrap())
        );

        // 3. Bad port returns relay_port_invalid
        let resp = set_byo_relay(
            Extension(journal.clone()),
            Json(json!({"address": "203.0.113.20", "port": 70000})),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_port_invalid");

        // 4. Hostname removal deletes byo_relay
        let resp = remove_byo_hostname(Extension(journal.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let cfg = read_journal_config(root).unwrap();
        let relay = solstone_core_journal_config::byo_relay_config(&cfg);
        assert!(matches!(
            relay,
            solstone_core_journal_config::ByoRelayConfig::Absent
        ));
    }

    #[tokio::test]
    async fn byo_dns_check_writes_no_door_file_and_uses_override() {
        use axum::body::to_bytes;

        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-byo-dns-test-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let root = dir.path();
        let journal = Arc::new(root.to_path_buf());
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"byo_hostname":{"hostname":"mcp.example.com","enabled":true,"generation":1},"byo_relay":{"address":"203.0.113.10","port":22}}}"#,
        )
        .unwrap();

        *crate::byo_dns::TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
            crate::byo_dns::DnsVerdict::new(crate::byo_dns::DnsVerdictCode::Admitted, Utc::now()),
            vec!["203.0.113.10".to_string()],
        ));

        let door_path = root.join(crate::byo_door::BYO_DOOR_STATE_PATH);
        assert!(!door_path.exists());

        let resp = byo_dns_check(Extension(journal.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["verdict"], "admitted");
        assert_eq!(val["addresses"], json!(["203.0.113.10"]));
        assert_eq!(val["diagnostics"], json!([]));

        assert!(!door_path.exists());

        *crate::byo_dns::TEST_VERDICT_OVERRIDE.write().unwrap() = None;
    }

    #[tokio::test]
    async fn byo_relay_check_codes() {
        use axum::body::to_bytes;

        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-byo-check-test-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let root = dir.path();
        let journal = Arc::new(root.to_path_buf());

        // 1. Relay not configured
        let resp = byo_relay_check(Extension(journal.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_not_configured");

        // 2. Configured and bound in state:
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"mcp_endpoint":{"byo_hostname":{"hostname":"mcp.example.com","enabled":true,"generation":1},"byo_relay":{"address":"203.0.113.10","port":22}}}"#,
        )
        .unwrap();
        let mut state = crate::byo_door::ByoDoorState {
            hostname: Some("mcp.example.com".to_string()),
            enabled: true,
            generation: 1,
            account_uri: None,
            caa: None,
            dns_verdict: None,
            dns_observed_at: None,
            dns_addresses: Vec::new(),
            relay_state: "connecting".to_string(),
            relay_detail: None,
            relay_observed_at: Some(Utc::now()),
            socket_listening: true,
            certificate_active: false,
            socket_path: None,
            socket_blocker: None,
            next_action: None,
            observed_at: Utc::now(),
        };
        crate::byo_door::write_byo_door_state(root, &state);

        let resp = byo_relay_check(Extension(journal.clone())).await;
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_connecting");

        state.relay_state = "connected".to_string();
        crate::byo_door::write_byo_door_state(root, &state);
        let resp = byo_relay_check(Extension(journal.clone())).await;
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_ok");

        state.relay_state = "unreachable".to_string();
        crate::byo_door::write_byo_door_state(root, &state);
        let resp = byo_relay_check(Extension(journal.clone())).await;
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_unreachable");

        state.relay_state = "auth_refused".to_string();
        crate::byo_door::write_byo_door_state(root, &state);
        let resp = byo_relay_check(Extension(journal.clone())).await;
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_auth_refused");

        state.relay_state = "forward_refused".to_string();
        crate::byo_door::write_byo_door_state(root, &state);
        let resp = byo_relay_check(Extension(journal.clone())).await;
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_forward_refused");

        state.relay_state = "host_key_changed".to_string();
        crate::byo_door::write_byo_door_state(root, &state);
        let resp = byo_relay_check(Extension(journal.clone())).await;
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let val: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(val["code"], "relay_host_key_changed");
    }
}
