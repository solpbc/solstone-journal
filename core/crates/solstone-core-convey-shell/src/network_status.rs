// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native network status, identity, and local-endpoint read routes.

use std::fs;
use std::sync::Arc;

use axum::Json;
use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_sol_link::committed::{
    CommittedIdentity, CommittedIdentityError, load_committed_identity,
};
use solstone_core_sol_link::mark::mark_from_jid;
use solstone_core_sol_link::pairing::addresses::{
    AddressError, EndpointScope, PairingSnapshot, SystemInterfaceSource, SystemRouteIpv4Source,
    resolve_pair_link_candidates, snapshot_from_sources,
};
use solstone_core_spl::{
    LinkServiceTokenRead, LinkStateRead, OFFLINE_TUNNEL_REASONS, load_link_service_token,
    load_link_state,
};
use solstone_core_thinking::confidential::OperationRegistry;

use solstone_core_journal_config::read_direct_door_port;

use crate::JournalRoot;
use crate::link_health_cache::{RelayHealthCache, RelayHealthCacheStore};
use crate::network::{hardened_loopback, read_posture};
use crate::network_writes::NetworkOperationsOverride;

const DEFAULT_RELAY_URL: &str = "https://link.solstone.app";
const HOME_CANDIDATES_ERROR: &str = "couldn't check home addresses";
const LINK_HEALTH_FRESHNESS_MS: i64 = 90_000;

/// The non-I/O health fields published by the status route.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LinkHealthProjection {
    state: Option<String>,
    last_link_event_at: Option<i64>,
    relay_listen_generation: Option<i64>,
    last_successful_relay_tunnel_at: Option<i64>,
    last_relay_tunnel_error: Option<String>,
    last_relay_tunnel_error_at: Option<i64>,
    last_relay_listener_ack_at: Option<i64>,
    last_relay_listener_ack_generation: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinkPosture {
    Direct,
    Spl,
}

impl LinkPosture {
    fn from_read(posture: &str) -> Self {
        if posture == "spl" {
            Self::Spl
        } else {
            Self::Direct
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Spl => "spl",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SplServiceState {
    NotEnabled,
    Enabled,
    Inconsistent,
}

impl SplServiceState {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotEnabled => "not_enabled",
            Self::Enabled => "enabled",
            Self::Inconsistent => "inconsistent",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RelayState {
    NotEnrolled,
    Connecting,
    Parked,
    Reconnecting,
    Offline,
}

impl RelayState {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotEnrolled => "not-enrolled",
            Self::Connecting => "connecting",
            Self::Parked => "parked",
            Self::Reconnecting => "reconnecting",
            Self::Offline => "offline",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reachability {
    LanUnreachable,
    Online,
    NotEnrolled,
    FinishingSetup,
    Reconnecting,
    Offline,
}

impl Reachability {
    fn as_str(self) -> &'static str {
        match self {
            Self::LanUnreachable => "lan-unreachable",
            Self::Online => "online",
            Self::NotEnrolled => "not-enrolled",
            Self::FinishingSetup => "finishing-setup",
            Self::Reconnecting => "reconnecting",
            Self::Offline => "offline",
        }
    }
}

#[derive(Serialize)]
struct HomeCandidate {
    address: String,
    selected: bool,
    source: &'static str,
}

#[derive(Serialize)]
struct VpnCandidate {
    label: &'static str,
    address: String,
}

#[derive(Serialize)]
struct VpnStatus {
    active: Option<Value>,
    candidates: Vec<VpnCandidate>,
}

#[derive(Serialize)]
struct StatusBody {
    ca_fingerprint: Option<String>,
    enrolled: bool,
    home_address: Option<String>,
    home_candidates: Vec<HomeCandidate>,
    home_candidates_error: Option<&'static str>,
    home_candidates_state: &'static str,
    home_label: Option<String>,
    instance_id: Option<String>,
    lan_accessible: bool,
    last_link_event_at: Option<i64>,
    last_relay_listener_ack_at: Option<i64>,
    last_relay_listener_ack_generation: Option<i64>,
    last_relay_tunnel_error: Option<String>,
    last_relay_tunnel_error_at: Option<i64>,
    last_successful_relay_tunnel_at: Option<i64>,
    posture: &'static str,
    reachability: &'static str,
    relay_listen_generation: Option<i64>,
    relay_state: &'static str,
    relay_url: String,
    vpn: VpnStatus,
}

struct StatusInputs<'a> {
    link_state: LinkStateRead,
    token_present: bool,
    posture: LinkPosture,
    relay_url: String,
    ca_fingerprint: Option<String>,
    health: Option<&'a LinkHealthProjection>,
    home_address: Option<String>,
    snapshot: Result<PairingSnapshot, AddressError>,
    now_ms: i64,
    direct_port: u16,
}

#[derive(Serialize)]
struct IdentityBody {
    committed: bool,
    instance_id: Option<String>,
    mark: Option<Value>,
}

#[derive(Serialize)]
struct PrivateLinkActions {
    enable: bool,
    disable: bool,
}

#[derive(Serialize)]
pub(crate) struct PrivateLinkBody {
    success: bool,
    service: &'static str,
    pub(crate) state: &'static str,
    posture: &'static str,
    enrolled: bool,
    relay_url: String,
    actions: PrivateLinkActions,
    operation: Option<Value>,
}

#[derive(Serialize)]
struct LocalEndpointBody {
    ip: String,
    port: u16,
    scope: &'static str,
}

#[derive(Serialize)]
struct LocalEndpointsBody {
    v: u8,
    endpoints: Vec<LocalEndpointBody>,
    ttl_s: u16,
    generated_at: String,
}

pub(crate) async fn status(
    Extension(root): Extension<Arc<JournalRoot>>,
    health_cache: Option<Extension<RelayHealthCacheStore>>,
    snapshot: Option<Extension<PairingSnapshot>>,
) -> Response {
    let posture = LinkPosture::from_read(read_posture(&root.0));
    let config = read_config(&root.0);
    let snapshot = snapshot
        .map(|Extension(snapshot)| Ok(snapshot))
        .unwrap_or_else(|| snapshot_from_sources(&SystemInterfaceSource, &SystemRouteIpv4Source));
    let token_present = matches!(
        load_link_service_token(&root.0),
        LinkServiceTokenRead::Present(_)
    );
    let health = health_cache.and_then(|Extension(health_cache)| {
        health_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .as_ref()
            .map(project_relay_health)
    });
    let direct_port = match read_direct_door_port(&root.0) {
        Ok(port) => port,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let body = build_status_body(StatusInputs {
        link_state: load_link_state(&root.0, "solstone"),
        token_present,
        posture,
        relay_url: relay_url(
            std::env::var("SOL_LINK_RELAY_URL").ok().as_deref(),
            config.as_ref(),
        ),
        ca_fingerprint: ca_fingerprint(load_committed_identity(&root.0).ok().as_ref()),
        health: health.as_ref(),
        home_address: configured_home_address(config.as_ref()),
        snapshot,
        now_ms: Utc::now().timestamp_millis(),
        direct_port,
    });
    Json(body).into_response()
}

pub(crate) async fn identity(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    Json(build_identity_body(load_committed_identity(&root.0))).into_response()
}

pub(crate) async fn private_link(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
    override_operations: Option<Extension<NetworkOperationsOverride>>,
) -> Response {
    let operations = override_operations
        .map(|Extension(value)| value.0)
        .unwrap_or(operations);
    Json(private_link_body(
        &root.0,
        Some(operations.operation_raw("spl")),
    ))
    .into_response()
}

pub(crate) fn private_link_body(
    journal_root: &std::path::Path,
    operation: Option<Value>,
) -> PrivateLinkBody {
    let posture = LinkPosture::from_read(read_posture(journal_root));
    let token_present = matches!(
        load_link_service_token(journal_root),
        LinkServiceTokenRead::Present(_)
    );
    build_private_link_body(
        posture,
        token_present,
        relay_url(
            std::env::var("SOL_LINK_RELAY_URL").ok().as_deref(),
            read_config(journal_root).as_ref(),
        ),
        operation,
    )
}

pub(crate) async fn local_endpoints(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    headers: HeaderMap,
    snapshot: Option<Extension<PairingSnapshot>>,
) -> Response {
    if !hardened_loopback(&basis, &headers) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let snapshot = snapshot
        .map(|Extension(snapshot)| Ok(snapshot))
        .unwrap_or_else(|| snapshot_from_sources(&SystemInterfaceSource, &SystemRouteIpv4Source));
    let Ok(snapshot) = snapshot else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(port) = read_direct_door_port(&root.0) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(build_local_endpoints_body(&snapshot, port)).into_response()
}

fn derive_spl_service_state(posture: LinkPosture, token_present: bool) -> SplServiceState {
    match (posture, token_present) {
        (LinkPosture::Spl, true) => SplServiceState::Enabled,
        (LinkPosture::Spl, false) => SplServiceState::Inconsistent,
        (LinkPosture::Direct, _) => SplServiceState::NotEnabled,
    }
}

fn derive_direct_relay_state(token_present: bool) -> RelayState {
    if token_present {
        RelayState::Offline
    } else {
        RelayState::NotEnrolled
    }
}

fn derive_spl_relay_state(
    token_present: bool,
    health: Option<&LinkHealthProjection>,
    now_ms: i64,
) -> RelayState {
    if !token_present {
        return RelayState::NotEnrolled;
    }
    let Some(health) = health else {
        return RelayState::Connecting;
    };
    if !link_health_is_fresh(health, now_ms) {
        return RelayState::Offline;
    }
    if current_tunnel_error(health).is_some_and(|error| OFFLINE_TUNNEL_REASONS.contains(&error)) {
        return RelayState::Offline;
    }
    match health.state.as_deref() {
        Some("reconnecting") => RelayState::Reconnecting,
        Some("connected") => RelayState::Parked,
        _ => RelayState::Connecting,
    }
}

fn derive_reachability(
    lan_accessible: bool,
    posture: LinkPosture,
    relay_state: RelayState,
) -> Reachability {
    if !lan_accessible {
        return Reachability::LanUnreachable;
    }
    match posture {
        LinkPosture::Direct => Reachability::Online,
        LinkPosture::Spl => match relay_state {
            RelayState::Connecting => Reachability::FinishingSetup,
            RelayState::Parked => Reachability::Online,
            RelayState::Reconnecting => Reachability::Reconnecting,
            RelayState::Offline => Reachability::Offline,
            RelayState::NotEnrolled => Reachability::NotEnrolled,
        },
    }
}

fn project_relay_health(cache: &RelayHealthCache) -> LinkHealthProjection {
    LinkHealthProjection {
        state: Some(cache.state.clone()),
        last_link_event_at: Some(cache.ts),
        relay_listen_generation: cache.listen_generation.and_then(to_i64),
        last_successful_relay_tunnel_at: cache.last_successful_relay_tunnel_at.and_then(to_i64),
        last_relay_tunnel_error: cache.last_relay_tunnel_error.clone(),
        last_relay_tunnel_error_at: cache.last_relay_tunnel_error_at.and_then(to_i64),
        last_relay_listener_ack_at: cache.last_relay_listener_ack_at.and_then(to_i64),
        last_relay_listener_ack_generation: cache
            .last_relay_listener_ack_generation
            .and_then(to_i64),
    }
}

fn to_i64(value: u64) -> Option<i64> {
    i64::try_from(value).ok()
}

fn current_tunnel_error(health: &LinkHealthProjection) -> Option<&str> {
    let error = health.last_relay_tunnel_error.as_deref()?;
    let error_at = health.last_relay_tunnel_error_at.unwrap_or(0);
    let success_at = health.last_successful_relay_tunnel_at.unwrap_or(0);
    (error_at >= success_at).then_some(error)
}

fn link_health_is_fresh(health: &LinkHealthProjection, now_ms: i64) -> bool {
    health
        .last_link_event_at
        .is_some_and(|timestamp| now_ms - timestamp <= LINK_HEALTH_FRESHNESS_MS)
}

fn build_status_body(inputs: StatusInputs<'_>) -> StatusBody {
    let StatusInputs {
        link_state,
        token_present,
        posture,
        relay_url,
        ca_fingerprint,
        health,
        home_address,
        snapshot,
        now_ms,
        direct_port,
    } = inputs;
    let (instance_id, home_label) = match link_state {
        LinkStateRead::Present(state) => (Some(state.instance_id), Some(state.home_label)),
        LinkStateRead::Missing | LinkStateRead::Unreadable | LinkStateRead::Malformed => {
            (None, None)
        }
    };
    let (lan_accessible, home_candidates, home_candidates_state, home_candidates_error, vpn) =
        match snapshot {
            Ok(snapshot) => {
                let detected =
                    resolve_pair_link_candidates(&snapshot.endpoints, snapshot.route_ipv4)
                        .into_iter()
                        .map(|address| format!("{address}:{direct_port}"))
                        .collect::<Vec<_>>();
                let selected = home_address
                    .as_deref()
                    .or_else(|| detected.first().map(String::as_str));
                let mut entries = detected
                    .iter()
                    .map(|address| HomeCandidate {
                        address: address.clone(),
                        selected: Some(address.as_str()) == selected,
                        source: "detected",
                    })
                    .collect::<Vec<_>>();
                if let Some(address) = &home_address
                    && !detected.contains(address)
                {
                    entries.push(HomeCandidate {
                        address: address.clone(),
                        selected: true,
                        source: "override",
                    });
                }
                let candidates = snapshot
                    .endpoints
                    .iter()
                    .filter(|endpoint| endpoint.scope == EndpointScope::Vpn)
                    .map(|endpoint| VpnCandidate {
                        label: endpoint_scope_name(endpoint.scope),
                        address: format!("{}:{direct_port}", endpoint.ip),
                    })
                    .collect::<Vec<_>>();
                let vpn = VpnStatus {
                    active: candidates
                        .first()
                        .map(|candidate| Value::String(candidate.address.clone())),
                    candidates,
                };
                (
                    home_address.is_some() || !detected.is_empty(),
                    entries,
                    "ready",
                    None,
                    vpn,
                )
            }
            Err(_) => match &home_address {
                Some(address) => (
                    true,
                    vec![HomeCandidate {
                        address: address.clone(),
                        selected: true,
                        source: "override",
                    }],
                    "ready",
                    None,
                    VpnStatus {
                        active: None,
                        candidates: Vec::new(),
                    },
                ),
                None => (
                    false,
                    Vec::new(),
                    "unavailable",
                    Some(HOME_CANDIDATES_ERROR),
                    VpnStatus {
                        active: None,
                        candidates: Vec::new(),
                    },
                ),
            },
        };
    let relay_state = match posture {
        LinkPosture::Direct => derive_direct_relay_state(token_present),
        LinkPosture::Spl => derive_spl_relay_state(token_present, health, now_ms),
    };
    StatusBody {
        ca_fingerprint,
        enrolled: token_present,
        home_address,
        home_candidates,
        home_candidates_error,
        home_candidates_state,
        home_label,
        instance_id,
        lan_accessible,
        last_link_event_at: health.and_then(|value| value.last_link_event_at),
        last_relay_listener_ack_at: health.and_then(|value| value.last_relay_listener_ack_at),
        last_relay_listener_ack_generation: health
            .and_then(|value| value.last_relay_listener_ack_generation),
        last_relay_tunnel_error: health.and_then(|value| value.last_relay_tunnel_error.clone()),
        last_relay_tunnel_error_at: health.and_then(|value| value.last_relay_tunnel_error_at),
        last_successful_relay_tunnel_at: health
            .and_then(|value| value.last_successful_relay_tunnel_at),
        posture: posture.as_str(),
        reachability: derive_reachability(lan_accessible, posture, relay_state).as_str(),
        relay_listen_generation: health.and_then(|value| value.relay_listen_generation),
        relay_state: relay_state.as_str(),
        relay_url,
        vpn,
    }
}

fn build_identity_body(
    identity: Result<CommittedIdentity, CommittedIdentityError>,
) -> IdentityBody {
    let Ok(identity) = identity else {
        return IdentityBody {
            committed: false,
            instance_id: None,
            mark: None,
        };
    };
    let Ok(mark) = mark_from_jid(identity.instance_id()) else {
        return IdentityBody {
            committed: false,
            instance_id: None,
            mark: None,
        };
    };
    let Ok(mark) = serde_json::to_value(mark.to_render_spec()) else {
        return IdentityBody {
            committed: false,
            instance_id: None,
            mark: None,
        };
    };
    IdentityBody {
        committed: true,
        instance_id: Some(identity.instance_id().to_owned()),
        mark: Some(mark),
    }
}

fn build_private_link_body(
    posture: LinkPosture,
    token_present: bool,
    relay_url: String,
    operation: Option<Value>,
) -> PrivateLinkBody {
    let state = derive_spl_service_state(posture, token_present);
    PrivateLinkBody {
        success: true,
        service: "spl",
        state: state.as_str(),
        posture: posture.as_str(),
        enrolled: token_present,
        relay_url,
        actions: PrivateLinkActions {
            enable: matches!(
                state,
                SplServiceState::NotEnabled | SplServiceState::Inconsistent
            ),
            disable: matches!(
                state,
                SplServiceState::Enabled | SplServiceState::Inconsistent
            ),
        },
        operation: operation.filter(|value| !value.is_null()),
    }
}

fn build_local_endpoints_body(snapshot: &PairingSnapshot, port: u16) -> LocalEndpointsBody {
    LocalEndpointsBody {
        v: 1,
        endpoints: snapshot
            .endpoints
            .iter()
            .map(|endpoint| LocalEndpointBody {
                ip: endpoint.ip.to_string(),
                port,
                scope: endpoint_scope_name(endpoint.scope),
            })
            .collect(),
        ttl_s: 3600,
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    }
}

/// Deliberately diverges from the Python reference, which calls
/// `load_or_generate_ca` and can mint a CA keypair from a GET request. This is
/// a read-only route (CLAUDE.md L3/L4): an absent CA reads as `None` here
/// rather than being generated on read.
fn ca_fingerprint(identity: Option<&CommittedIdentity>) -> Option<String> {
    identity.map(|identity| format!("{:x}", Sha256::digest(identity.certificate_der())))
}

fn relay_url(environment: Option<&str>, config: Option<&Value>) -> String {
    environment
        .and_then(clean_relay_url)
        .or_else(|| {
            config
                .and_then(|value| value.get("link"))
                .and_then(|value| value.get("relay_url"))
                .and_then(Value::as_str)
                .and_then(clean_relay_url)
        })
        .unwrap_or(DEFAULT_RELAY_URL)
        .to_owned()
}

fn clean_relay_url(value: &str) -> Option<&str> {
    let cleaned = value.trim().trim_end_matches('/');
    (!cleaned.is_empty()).then_some(cleaned)
}

fn read_config(journal_root: &std::path::Path) -> Option<Value> {
    fs::read(journal_root.join("config/journal.json"))
        .ok()
        .and_then(|contents| serde_json::from_slice(&contents).ok())
}

fn configured_home_address(config: Option<&Value>) -> Option<String> {
    config
        .and_then(|value| value.get("pairing"))
        .and_then(Value::as_object)
        .and_then(|pairing| pairing.get("home_address"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(str::to_owned)
}

fn endpoint_scope_name(scope: EndpointScope) -> &'static str {
    match scope {
        EndpointScope::Lan => "lan",
        EndpointScope::Ula => "ula",
        EndpointScope::Vpn => "vpn",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::Path;
    use std::sync::Arc;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use axum::routing::get;
    use serde_json::json;
    use solstone_core_convey_http::identity::Carrier;
    use solstone_core_sol_link::establish;
    use solstone_core_sol_link::pairing::addresses::LocalEndpoint;
    use solstone_core_spl::REASON_SERVICE_TOKEN_REJECTED;
    use tower::ServiceExt;

    use super::*;

    struct TempDir(tempfile::TempDir);

    impl TempDir {
        fn new() -> Self {
            Self(tempfile::TempDir::new_in("/var/tmp").expect("temporary root"))
        }

        fn path(&self) -> &Path {
            self.0.path()
        }
    }

    fn health(timestamp: Option<i64>) -> LinkHealthProjection {
        LinkHealthProjection {
            last_link_event_at: timestamp,
            ..Default::default()
        }
    }

    fn snapshot(endpoints: Vec<(Ipv4Addr, EndpointScope)>) -> PairingSnapshot {
        PairingSnapshot {
            endpoints: endpoints
                .into_iter()
                .map(|(ip, scope)| LocalEndpoint {
                    ip: IpAddr::V4(ip),
                    scope,
                })
                .collect(),
            route_ipv4: None,
        }
    }

    fn status_json(
        home_address: Option<&str>,
        snapshot: Result<PairingSnapshot, AddressError>,
        health: Option<&LinkHealthProjection>,
    ) -> Value {
        serde_json::to_value(build_status_body(StatusInputs {
            link_state: LinkStateRead::Missing,
            token_present: false,
            posture: LinkPosture::Direct,
            relay_url: DEFAULT_RELAY_URL.to_owned(),
            ca_fingerprint: None,
            health,
            home_address: home_address.map(str::to_owned),
            snapshot,
            now_ms: 1_000_000,
            direct_port: 7657,
        }))
        .expect("status serializes")
    }

    #[test]
    fn direct_relay_state_covers_enrollment_pair() {
        assert_eq!(derive_direct_relay_state(false), RelayState::NotEnrolled);
        assert_eq!(derive_direct_relay_state(true), RelayState::Offline);
    }

    #[test]
    fn health_freshness_preserves_boundary_and_signed_comparison() {
        assert!(link_health_is_fresh(&health(Some(910_000)), 1_000_000));
        assert!(!link_health_is_fresh(&health(Some(909_999)), 1_000_000));
        assert!(link_health_is_fresh(&health(Some(1_000_001)), 1_000_000));
        assert_eq!(
            derive_spl_relay_state(true, Some(&health(None)), 1_000_000),
            RelayState::Offline
        );
    }

    #[test]
    fn spl_relay_state_covers_every_ordered_branch() {
        let fresh = 1_000_000;
        let cases = [
            (false, None, RelayState::NotEnrolled),
            (true, None, RelayState::Connecting),
            (
                true,
                Some(LinkHealthProjection {
                    state: Some("connected".into()),
                    last_link_event_at: Some(fresh - LINK_HEALTH_FRESHNESS_MS - 1),
                    ..Default::default()
                }),
                RelayState::Offline,
            ),
            (
                true,
                Some(LinkHealthProjection {
                    last_link_event_at: Some(fresh),
                    last_relay_tunnel_error: Some(OFFLINE_TUNNEL_REASONS[0].to_owned()),
                    ..Default::default()
                }),
                RelayState::Offline,
            ),
            (
                true,
                Some(LinkHealthProjection {
                    state: Some("reconnecting".into()),
                    last_link_event_at: Some(fresh),
                    ..Default::default()
                }),
                RelayState::Reconnecting,
            ),
            (
                true,
                Some(LinkHealthProjection {
                    state: Some("connected".into()),
                    last_link_event_at: Some(fresh),
                    ..Default::default()
                }),
                RelayState::Parked,
            ),
            (
                true,
                Some(LinkHealthProjection {
                    state: Some("idle".into()),
                    last_link_event_at: Some(fresh),
                    ..Default::default()
                }),
                RelayState::Connecting,
            ),
        ];
        for (token_present, health, expected) in cases {
            assert_eq!(
                derive_spl_relay_state(token_present, health.as_ref(), fresh),
                expected
            );
        }
        assert!(OFFLINE_TUNNEL_REASONS.contains(&REASON_SERVICE_TOKEN_REJECTED));
    }

    #[test]
    fn reachability_is_total_and_lan_precedes_everything() {
        let cases = [
            (
                false,
                LinkPosture::Direct,
                RelayState::NotEnrolled,
                Reachability::LanUnreachable,
            ),
            (
                false,
                LinkPosture::Spl,
                RelayState::Parked,
                Reachability::LanUnreachable,
            ),
            (
                true,
                LinkPosture::Direct,
                RelayState::Offline,
                Reachability::Online,
            ),
            (
                true,
                LinkPosture::Spl,
                RelayState::Connecting,
                Reachability::FinishingSetup,
            ),
            (
                true,
                LinkPosture::Spl,
                RelayState::Parked,
                Reachability::Online,
            ),
            (
                true,
                LinkPosture::Spl,
                RelayState::Reconnecting,
                Reachability::Reconnecting,
            ),
            (
                true,
                LinkPosture::Spl,
                RelayState::Offline,
                Reachability::Offline,
            ),
            (
                true,
                LinkPosture::Spl,
                RelayState::NotEnrolled,
                Reachability::NotEnrolled,
            ),
        ];
        for (lan, posture, relay, expected) in cases {
            assert_eq!(derive_reachability(lan, posture, relay), expected);
        }
    }

    #[test]
    fn current_tunnel_error_obeys_timestamp_order_and_ties() {
        let cases = [
            (Some(2), Some(1), true),
            (Some(1), Some(2), false),
            (Some(2), Some(2), true),
            (None, None, true),
        ];
        for (error_at, success_at, expected) in cases {
            let health = LinkHealthProjection {
                last_relay_tunnel_error: Some("error".into()),
                last_relay_tunnel_error_at: error_at,
                last_successful_relay_tunnel_at: success_at,
                ..Default::default()
            };
            assert_eq!(current_tunnel_error(&health).is_some(), expected);
        }
    }

    #[test]
    fn spl_health_state_uses_freshness_error_order_and_reconnect_state() {
        let fresh = 1_000_000;
        let base = LinkHealthProjection {
            state: Some("connected".to_owned()),
            last_link_event_at: Some(fresh),
            last_successful_relay_tunnel_at: Some(700),
            ..Default::default()
        };
        let mut equal_error = base.clone();
        equal_error.last_relay_tunnel_error = Some(REASON_SERVICE_TOKEN_REJECTED.to_owned());
        equal_error.last_relay_tunnel_error_at = Some(700);
        assert_eq!(
            derive_spl_relay_state(true, Some(&equal_error), fresh),
            RelayState::Offline
        );

        let mut earlier_error = equal_error.clone();
        earlier_error.last_relay_tunnel_error_at = Some(699);
        assert_eq!(
            derive_spl_relay_state(true, Some(&earlier_error), fresh),
            RelayState::Parked
        );

        let mut missing_error_time = base.clone();
        missing_error_time.last_successful_relay_tunnel_at = None;
        missing_error_time.last_relay_tunnel_error = Some(REASON_SERVICE_TOKEN_REJECTED.to_owned());
        assert_eq!(
            derive_spl_relay_state(true, Some(&missing_error_time), fresh),
            RelayState::Offline
        );

        let mut unrelated_error = equal_error;
        unrelated_error.last_relay_tunnel_error = Some("unrelated".to_owned());
        assert_eq!(
            derive_spl_relay_state(true, Some(&unrelated_error), fresh),
            RelayState::Parked
        );

        let reconnecting = LinkHealthProjection {
            state: Some("reconnecting".to_owned()),
            last_link_event_at: Some(fresh),
            ..Default::default()
        };
        assert_eq!(
            derive_spl_relay_state(true, Some(&reconnecting), fresh),
            RelayState::Reconnecting
        );
    }

    #[test]
    fn direct_not_enrolled_remains_online_when_lan_is_reachable() {
        assert_eq!(
            derive_reachability(true, LinkPosture::Direct, RelayState::NotEnrolled),
            Reachability::Online
        );
    }

    #[test]
    fn health_fields_pass_through_to_status_body() {
        let health = LinkHealthProjection {
            state: Some("connected".into()),
            last_link_event_at: Some(11),
            relay_listen_generation: Some(12),
            last_successful_relay_tunnel_at: Some(13),
            last_relay_tunnel_error: Some("error-14".into()),
            last_relay_tunnel_error_at: Some(15),
            last_relay_listener_ack_at: Some(16),
            last_relay_listener_ack_generation: Some(17),
        };
        let body = status_json(None, Ok(snapshot(Vec::new())), Some(&health));
        assert_eq!(body["last_link_event_at"], 11);
        assert_eq!(body["relay_listen_generation"], 12);
        assert_eq!(body["last_successful_relay_tunnel_at"], 13);
        assert_eq!(body["last_relay_tunnel_error"], "error-14");
        assert_eq!(body["last_relay_tunnel_error_at"], 15);
        assert_eq!(body["last_relay_listener_ack_at"], 16);
        assert_eq!(body["last_relay_listener_ack_generation"], 17);
    }

    #[test]
    fn relay_health_projection_renames_generation_and_drops_raw_only_fields() {
        let cache = RelayHealthCache {
            state: "connected".to_owned(),
            listen_generation: Some(41),
            last_successful_relay_tunnel_at: Some(42),
            last_relay_tunnel_error: Some("error-43".to_owned()),
            last_relay_tunnel_error_at: Some(44),
            relay_tunnel_error_status: Some(503),
            relay_admission_saturated_count: 45,
            last_relay_listener_ack_at: Some(46),
            last_relay_listener_ack_generation: Some(47),
            ts: 48,
        };
        let health = project_relay_health(&cache);
        let body = status_json(None, Ok(snapshot(Vec::new())), Some(&health));
        assert_eq!(body["last_link_event_at"], 48);
        assert_eq!(body["relay_listen_generation"], 41);
        assert_eq!(body["last_successful_relay_tunnel_at"], 42);
        assert_eq!(body["last_relay_tunnel_error"], "error-43");
        assert_eq!(body["last_relay_tunnel_error_at"], 44);
        assert_eq!(body["last_relay_listener_ack_at"], 46);
        assert_eq!(body["last_relay_listener_ack_generation"], 47);
        assert!(body.get("relay_tunnel_error_status").is_none());
        assert!(body.get("relay_admission_saturated_count").is_none());
    }

    #[test]
    fn status_vpn_candidates_are_direct_scope_projections() {
        let positive = status_json(
            None,
            Ok(snapshot(vec![(
                Ipv4Addr::new(203, 0, 113, 9),
                EndpointScope::Vpn,
            )])),
            None,
        );
        assert_eq!(
            positive["vpn"]["candidates"],
            json!([{"label":"vpn","address":"203.0.113.9:7657"}])
        );
        assert_eq!(positive["vpn"]["active"], "203.0.113.9:7657");
        let negative = status_json(
            None,
            Ok(snapshot(vec![(
                Ipv4Addr::new(10, 0, 0, 2),
                EndpointScope::Lan,
            )])),
            None,
        );
        assert_eq!(negative["vpn"]["candidates"], json!([]));
        assert_eq!(negative["vpn"]["active"], Value::Null);
    }

    #[test]
    fn home_candidates_cover_override_and_detected_selection() {
        let override_body = status_json(
            Some("10.0.0.9:7657"),
            Ok(snapshot(vec![(
                Ipv4Addr::new(192, 168, 1, 2),
                EndpointScope::Lan,
            )])),
            None,
        );
        assert_eq!(
            override_body["home_candidates"],
            json!([
                {"address":"192.168.1.2:7657","selected":false,"source":"detected"},
                {"address":"10.0.0.9:7657","selected":true,"source":"override"}
            ])
        );
        let matching_body = status_json(
            Some("192.168.1.2:7657"),
            Ok(snapshot(vec![(
                Ipv4Addr::new(192, 168, 1, 2),
                EndpointScope::Lan,
            )])),
            None,
        );
        assert_eq!(
            matching_body["home_candidates"],
            json!([
                {"address":"192.168.1.2:7657","selected":true,"source":"detected"}
            ])
        );
        let configured_only = status_json(Some("10.0.0.9:7657"), Ok(snapshot(Vec::new())), None);
        assert_eq!(configured_only["lan_accessible"], true);
        let custom = serde_json::to_value(build_status_body(StatusInputs {
            link_state: LinkStateRead::Missing,
            token_present: false,
            posture: LinkPosture::Direct,
            relay_url: DEFAULT_RELAY_URL.to_owned(),
            ca_fingerprint: None,
            health: None,
            home_address: None,
            snapshot: Ok(snapshot(vec![(
                Ipv4Addr::new(192, 168, 1, 2),
                EndpointScope::Lan,
            )])),
            now_ms: 1_000_000,
            direct_port: 9000,
        }))
        .expect("status serializes");
        assert_eq!(
            custom["home_candidates"],
            json!([
                {"address":"192.168.1.2:9000","selected":true,"source":"detected"}
            ])
        );
    }

    #[test]
    fn absent_health_keeps_configured_override_and_null_event_timestamp() {
        let body = status_json(
            Some("203.0.113.77:7657"),
            Ok(snapshot(vec![(
                Ipv4Addr::new(192, 168, 1, 2),
                EndpointScope::Lan,
            )])),
            None,
        );
        assert_eq!(body["last_link_event_at"], Value::Null);
        assert_eq!(
            body["home_candidates"],
            json!([
                {"address":"192.168.1.2:7657","selected":false,"source":"detected"},
                {"address":"203.0.113.77:7657","selected":true,"source":"override"}
            ])
        );
    }

    #[test]
    fn home_candidate_enumeration_failures_keep_override_or_report_copy() {
        let error = || AddressError::Enumeration(std::io::Error::other("test enumeration failure"));
        let override_body = status_json(Some("10.0.0.9:7657"), Err(error()), None);
        assert_eq!(override_body["lan_accessible"], true);
        assert_eq!(
            override_body["home_candidates"],
            json!([
                {"address":"10.0.0.9:7657","selected":true,"source":"override"}
            ])
        );
        assert_eq!(override_body["home_candidates_state"], "ready");
        assert_eq!(override_body["home_candidates_error"], Value::Null);
        let unavailable_body = status_json(None, Err(error()), None);
        assert_eq!(unavailable_body["lan_accessible"], false);
        assert_eq!(unavailable_body["home_candidates"], json!([]));
        assert_eq!(unavailable_body["home_candidates_state"], "unavailable");
        assert_eq!(
            unavailable_body["home_candidates_error"],
            HOME_CANDIDATES_ERROR
        );
    }

    #[test]
    fn private_link_state_machine_offers_repair_actions() {
        let enabled =
            build_private_link_body(LinkPosture::Spl, true, DEFAULT_RELAY_URL.to_owned(), None);
        let inconsistent =
            build_private_link_body(LinkPosture::Spl, false, DEFAULT_RELAY_URL.to_owned(), None);
        let not_enabled = build_private_link_body(
            LinkPosture::Direct,
            true,
            DEFAULT_RELAY_URL.to_owned(),
            None,
        );
        assert_eq!(enabled.state, "enabled");
        assert!(!enabled.actions.enable && enabled.actions.disable);
        assert_eq!(inconsistent.state, "inconsistent");
        assert!(inconsistent.actions.enable && inconsistent.actions.disable);
        assert_eq!(not_enabled.state, "not_enabled");
        assert!(not_enabled.actions.enable && !not_enabled.actions.disable);
    }

    #[tokio::test]
    async fn private_link_loads_legacy_account_token_as_enrollment() {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.path().join("config")).expect("config");
        fs::create_dir_all(temporary.path().join("link")).expect("link");
        fs::write(
            temporary.path().join("config/journal.json"),
            br#"{"link":{"posture":"spl"}}"#,
        )
        .expect("config writes");
        fs::create_dir_all(temporary.path().join("link/tokens")).expect("tokens");
        fs::write(
            temporary.path().join("link/tokens/account.json"),
            br#"{"account_token":"legacy"}"#,
        )
        .expect("token writes");
        let app = Router::new()
            .route("/private-link", get(private_link))
            .layer(Extension(Arc::new(JournalRoot(
                temporary.path().to_owned(),
            ))));
        let app = app.layer(Extension(Arc::new(OperationRegistry::default())));
        let response = app
            .oneshot(
                Request::get("/private-link")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(body["state"], "enabled");
        assert_eq!(body["enrolled"], true);
    }

    #[tokio::test]
    async fn status_reads_native_committed_link_state() {
        let temporary = TempDir::new();
        establish::current_candidate(temporary.path()).expect("candidate");
        let expected = establish::lock_in(temporary.path(), Some("Native Study")).expect("lock in");
        assert!(!temporary.path().join("link/state.json").exists());
        let app = Router::new()
            .route("/status", get(status))
            .layer(Extension(Arc::new(JournalRoot(
                temporary.path().to_owned(),
            ))));

        let response = app
            .oneshot(
                Request::get("/status")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(
            body["instance_id"].as_str(),
            Some(expected.instance_id.as_str())
        );
        assert_eq!(
            body["home_label"].as_str(),
            Some(expected.home_label.as_str())
        );
    }

    #[tokio::test]
    async fn identity_failure_is_neutral_success() {
        let temporary = TempDir::new();
        let app = Router::new()
            .route("/identity", get(identity))
            .layer(Extension(Arc::new(JournalRoot(
                temporary.path().to_owned(),
            ))));
        let response = app
            .oneshot(
                Request::get("/identity")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(
            body,
            json!({"committed":false,"instance_id":null,"mark":null})
        );
    }

    #[tokio::test]
    async fn local_endpoints_require_hardened_loopback_and_publish_raw_snapshot() {
        let temporary = TempDir::new();
        let root = Arc::new(JournalRoot(temporary.path().to_owned()));
        let snapshot = snapshot(vec![
            (Ipv4Addr::new(10, 0, 0, 2), EndpointScope::Lan),
            (Ipv4Addr::new(203, 0, 113, 9), EndpointScope::Vpn),
        ]);
        let app = Router::new()
            .route("/endpoints", get(local_endpoints))
            .layer(Extension(AccessBasis::Localhost))
            .layer(Extension(snapshot.clone()))
            .layer(Extension(root.clone()));
        for header in ["x-forwarded-for", "x-real-ip", "x-forwarded-host"] {
            let response = app
                .clone()
                .oneshot(
                    Request::get("/endpoints")
                        .header(header, "1.2.3.4")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{header}");
        }
        let response = app
            .oneshot(
                Request::get("/endpoints")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(body["v"], 1);
        assert_eq!(body["ttl_s"], 3600);
        assert_eq!(
            body["endpoints"],
            json!([
                {"ip":"10.0.0.2","port":7657,"scope":"lan"},
                {"ip":"203.0.113.9","port":7657,"scope":"vpn"}
            ])
        );
        let non_loopback = Router::new()
            .route("/endpoints", get(local_endpoints))
            .layer(Extension(AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            }))
            .layer(Extension(snapshot))
            .layer(Extension(root));
        let response = non_loopback
            .oneshot(
                Request::get("/endpoints")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
