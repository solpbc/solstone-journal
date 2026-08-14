// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Live cache of relay health reflected by Callosum.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumSocketConnection};

/// The producer-shaped relay health record plus the broker timestamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RelayHealthCache {
    pub(crate) state: String,
    pub(crate) listen_generation: Option<u64>,
    pub(crate) last_successful_relay_tunnel_at: Option<u64>,
    pub(crate) last_relay_tunnel_error: Option<String>,
    pub(crate) last_relay_tunnel_error_at: Option<u64>,
    pub(crate) relay_tunnel_error_status: Option<u16>,
    pub(crate) relay_admission_saturated_count: u64,
    pub(crate) last_relay_listener_ack_at: Option<u64>,
    pub(crate) last_relay_listener_ack_generation: Option<u64>,
    pub(crate) ts: i64,
}

pub(crate) type RelayHealthCacheStore = Arc<Mutex<Option<RelayHealthCache>>>;

#[derive(Deserialize)]
struct RelayHealthPayload {
    state: String,
    #[serde(default)]
    listen_generation: Option<u64>,
    last_successful_relay_tunnel_at: Option<u64>,
    last_relay_tunnel_error: Option<String>,
    last_relay_tunnel_error_at: Option<u64>,
    relay_tunnel_error_status: Option<u16>,
    relay_admission_saturated_count: u64,
    last_relay_listener_ack_at: Option<u64>,
    last_relay_listener_ack_generation: Option<u64>,
}

pub(crate) fn parse_relay_health_event(
    envelope: &CallosumEnvelope,
    now_ms: i64,
) -> Result<RelayHealthCache, serde_json::Error> {
    let payload: RelayHealthPayload =
        serde_json::from_value(Value::Object(envelope.extra.clone()))?;
    Ok(RelayHealthCache {
        state: payload.state,
        listen_generation: payload.listen_generation,
        last_successful_relay_tunnel_at: payload.last_successful_relay_tunnel_at,
        last_relay_tunnel_error: payload.last_relay_tunnel_error,
        last_relay_tunnel_error_at: payload.last_relay_tunnel_error_at,
        relay_tunnel_error_status: payload.relay_tunnel_error_status,
        relay_admission_saturated_count: payload.relay_admission_saturated_count,
        last_relay_listener_ack_at: payload.last_relay_listener_ack_at,
        last_relay_listener_ack_generation: payload.last_relay_listener_ack_generation,
        ts: envelope
            .ts
            .filter(|timestamp| *timestamp != 0)
            .unwrap_or(now_ms),
    })
}

pub(crate) fn replace_if_not_stale(
    store: &RelayHealthCacheStore,
    candidate: RelayHealthCache,
) -> bool {
    let Some(candidate_generation) = candidate.listen_generation else {
        return false;
    };
    let mut cache = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.as_ref().is_some_and(|current| {
        current
            .listen_generation
            .is_some_and(|generation| candidate_generation < generation)
    }) {
        return false;
    }
    *cache = Some(candidate);
    true
}

pub(crate) async fn subscribe_relay_health(journal_root: PathBuf, store: RelayHealthCacheStore) {
    let mut connection =
        CallosumSocketConnection::new(journal_root.join("health/callosum.sock"), Map::new());
    connection.start();
    while let Some(envelope) = connection.next_message().await {
        if envelope.tract != "link" || envelope.event != solstone_core_spl::LINK_HEALTH_EVENT {
            continue;
        }
        match parse_relay_health_event(&envelope, now_ms()) {
            Ok(candidate) => {
                let _ = replace_if_not_stale(&store, candidate);
            }
            Err(error) => log::debug!("ignored malformed relay health event: {error}"),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn envelope(ts: Option<i64>, generation: Option<u64>) -> CallosumEnvelope {
        let mut extra = serde_json::from_value::<Map<String, Value>>(json!({
            "state": "reconnecting",
            "listen_generation": generation,
            "last_successful_relay_tunnel_at": 201,
            "last_relay_tunnel_error": "service_token_rejected",
            "last_relay_tunnel_error_at": 202,
            "relay_tunnel_error_status": 503,
            "relay_admission_saturated_count": 203,
            "last_relay_listener_ack_at": 204,
            "last_relay_listener_ack_generation": 205,
        }))
        .expect("health fields are an object");
        if generation.is_none() {
            extra.remove("listen_generation");
        }
        CallosumEnvelope {
            tract: "link".to_owned(),
            event: solstone_core_spl::LINK_HEALTH_EVENT.to_owned(),
            ts,
            extra,
        }
    }

    #[test]
    fn complete_payload_round_trips_with_producer_spelling() {
        let cache = parse_relay_health_event(&envelope(Some(206), Some(200)), 999)
            .expect("complete health parses");
        assert_eq!(cache.state, "reconnecting");
        assert_eq!(cache.listen_generation, Some(200));
        assert_eq!(cache.last_successful_relay_tunnel_at, Some(201));
        assert_eq!(
            cache.last_relay_tunnel_error.as_deref(),
            Some("service_token_rejected")
        );
        assert_eq!(cache.last_relay_tunnel_error_at, Some(202));
        assert_eq!(cache.relay_tunnel_error_status, Some(503));
        assert_eq!(cache.relay_admission_saturated_count, 203);
        assert_eq!(cache.last_relay_listener_ack_at, Some(204));
        assert_eq!(cache.last_relay_listener_ack_generation, Some(205));
        assert_eq!(cache.ts, 206);
    }

    #[test]
    fn zero_and_absent_timestamp_use_now() {
        assert_eq!(
            parse_relay_health_event(&envelope(Some(0), Some(1)), 700)
                .expect("zero timestamp parses")
                .ts,
            700
        );
        assert_eq!(
            parse_relay_health_event(&envelope(None, Some(1)), 701)
                .expect("absent timestamp naturally parses")
                .ts,
            701
        );
    }

    #[test]
    fn generation_guard_rejects_missing_and_older_but_refreshes_equal() {
        let store = Arc::new(Mutex::new(None));
        assert!(!replace_if_not_stale(
            &store,
            parse_relay_health_event(&envelope(Some(1), None), 1).expect("partial parses"),
        ));
        assert!(replace_if_not_stale(
            &store,
            parse_relay_health_event(&envelope(Some(10), Some(0)), 1).expect("initial parses"),
        ));
        assert!(!replace_if_not_stale(
            &store,
            parse_relay_health_event(&envelope(Some(11), None), 1).expect("partial parses"),
        ));
        assert!(replace_if_not_stale(
            &store,
            parse_relay_health_event(&envelope(Some(12), Some(0)), 1).expect("equal parses"),
        ));
        assert!(replace_if_not_stale(
            &store,
            parse_relay_health_event(&envelope(Some(13), Some(0)), 1).expect("equal parses"),
        ));
        assert!(replace_if_not_stale(
            &store,
            parse_relay_health_event(&envelope(Some(14), Some(1)), 1).expect("newer parses"),
        ));
        assert!(!replace_if_not_stale(
            &store,
            parse_relay_health_event(&envelope(Some(15), Some(0)), 1).expect("older parses"),
        ));
        assert_eq!(
            store
                .lock()
                .expect("cache lock")
                .as_ref()
                .expect("cached")
                .ts,
            14
        );
    }
}
