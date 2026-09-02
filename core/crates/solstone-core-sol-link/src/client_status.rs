// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only client status projection over the authorization and activity ledgers.

use std::collections::BTreeMap;
use std::path::Path;

use crate::ledger::{
    AcceptedSegment, AuthorizedClientsRead, ClientActivity, ClientEntry, DeviceActivityRead,
    IngestRejection, SourceRecord, parse_rfc3339_utc, read_authorized_clients,
    read_device_activity,
};

pub const CLIENT_ACTIVE_MS: i64 = 30_000;
pub const CLIENT_STALE_MS: i64 = 120_000;
pub const CLIENT_FUTURE_CLOCK_DRIFT_TOLERANCE_MS: i64 = 300_000;

/// Capture freshness measures accepted-ingest age against the five-minute segment
/// seal/upload cadence with a delivery margin for upload lag. Connection freshness
/// measures `last_seen_at` against per-request liveness, so these thresholds must
/// not be merged with the connection thresholds above.
const CAPTURE_SEGMENT_CADENCE_MS: i64 = 300_000;
const CAPTURE_DELIVERY_MARGIN_MS: i64 = 60_000;
pub const CLIENT_CAPTURE_ACTIVE_MS: i64 = CAPTURE_SEGMENT_CADENCE_MS + CAPTURE_DELIVERY_MARGIN_MS;
pub const CLIENT_CAPTURE_STALE_MS: i64 = CLIENT_CAPTURE_ACTIVE_MS + CAPTURE_SEGMENT_CADENCE_MS;

/// Whether `link/devices.json` supplied activity data for an inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientActivityState {
    Present,
    Missing,
    Unreadable,
    Malformed,
}

/// Why the authoritative client ledger could not be inspected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientLedgerUnavailable {
    Unreadable,
    Malformed,
    DuplicateCid,
}

/// Result of reading the client-ledger projection.
///
/// An unavailable authorization ledger is intentionally distinct from an empty
/// client list. Activity-file failure remains visible on every variant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientInspection {
    Empty {
        clients: Vec<ClientAssessment>,
        activity: ClientActivityState,
    },
    Ready {
        clients: Vec<ClientAssessment>,
        activity: ClientActivityState,
    },
    LedgerUnavailable {
        reason: ClientLedgerUnavailable,
        activity: ClientActivityState,
    },
}

/// A paired client enriched with non-authoritative connection and capture state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientAssessment {
    pub cid: String,
    pub client_entry: ClientEntry,
    pub last_seen_at: Option<String>,
    pub last_accepted_ingest_at: Option<String>,
    pub last_accepted_segment: Option<AcceptedSegment>,
    pub ingest_rejection: Option<IngestRejection>,
    pub connection: ConnectionFreshness,
    pub capture_state: ClientCaptureState,
    pub capture_elapsed_ms: Option<i64>,
    pub source_delivery: BTreeMap<String, SourceDeliveryRow>,
}

/// Per-source delivery classification, distinct from device-level [`ClientCaptureState`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceDelivery {
    Current,
    NeedsAttention,
    Unknown,
}

/// Classified delivery for one source on one device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDeliveryRow {
    pub state: SourceDelivery,
    pub elapsed_ms: Option<i64>,
    pub last_accepted_ingest_at: Option<String>,
    pub last_accepted_segment: Option<AcceptedSegment>,
    pub ingest_rejection: Option<IngestRejection>,
}

/// Connection freshness derived from `last_seen_at`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionFreshness {
    Unknown,
    Known {
        state: ConnectionState,
        group: ConnectionGroup,
        elapsed_ms: Option<i64>,
        clock_skew: bool,
        label: &'static str,
        reach: ClientReach,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Connected,
    Stale,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionGroup {
    Active,
    Stale,
    Inactive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientReach {
    Active,
    Stale,
    Offline,
}

/// Capture health derived from accepted ingest activity and any active rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientCaptureState {
    Unknown,
    NoCapture,
    Degraded,
    Active,
    Stale,
    Offline,
}

/// Build the shared paired-client projection at a caller-supplied millisecond time.
pub fn inspect_clients_at(journal_root: &Path, now_ms: i64) -> ClientInspection {
    let activity_read = read_device_activity(&journal_root.join("link").join("devices.json"));
    let activity = activity_state(&activity_read);
    match read_authorized_clients(&journal_root.join("link").join("authorized_clients.json")) {
        AuthorizedClientsRead::Missing => ClientInspection::Empty {
            clients: Vec::new(),
            activity,
        },
        AuthorizedClientsRead::Unreadable => ClientInspection::LedgerUnavailable {
            reason: ClientLedgerUnavailable::Unreadable,
            activity,
        },
        AuthorizedClientsRead::Malformed => ClientInspection::LedgerUnavailable {
            reason: ClientLedgerUnavailable::Malformed,
            activity,
        },
        AuthorizedClientsRead::DuplicateCid => ClientInspection::LedgerUnavailable {
            reason: ClientLedgerUnavailable::DuplicateCid,
            activity,
        },
        AuthorizedClientsRead::Present(entries) => {
            let mut clients = entries
                .into_iter()
                .map(|entry| assessment_for(entry, activity_for(&activity_read), activity, now_ms))
                .collect::<Vec<_>>();
            clients.sort_by(|left, right| left.cid.cmp(&right.cid));
            if clients.is_empty() {
                ClientInspection::Empty { clients, activity }
            } else {
                ClientInspection::Ready { clients, activity }
            }
        }
    }
}

/// Classify each observed source on one device.
///
/// Never consults another device's sources or connection. `Current` is the
/// capture Active or Stale band (`elapsed < CLIENT_CAPTURE_STALE_MS`). Quiet
/// sources escalate only with signal (a) (this device is not fully offline) or
/// signal (b) (a sibling source on this device is `Current`).
pub fn classify_source_deliveries(
    sources: &BTreeMap<String, SourceRecord>,
    connection: &ConnectionFreshness,
    now_ms: i64,
) -> BTreeMap<String, SourceDeliveryRow> {
    let preliminary = sources
        .iter()
        .map(|(source, record)| (source.clone(), classify_source_pass1(record, now_ms)))
        .collect::<Vec<_>>();
    let any_current = preliminary.iter().any(|(_, pass)| {
        matches!(
            pass,
            SourcePass::Decided(row) if row.state == SourceDelivery::Current
        )
    });
    let signal_a = matches!(
        connection,
        ConnectionFreshness::Known {
            reach: ClientReach::Active | ClientReach::Stale,
            ..
        }
    );
    preliminary
        .into_iter()
        .map(|(source, pass)| {
            let row = match pass {
                SourcePass::Decided(row) => row,
                SourcePass::Undecided(mut row) => {
                    row.state = if signal_a || any_current {
                        SourceDelivery::NeedsAttention
                    } else {
                        SourceDelivery::Unknown
                    };
                    row
                }
            };
            (source, row)
        })
        .collect()
}

enum SourcePass {
    Decided(SourceDeliveryRow),
    Undecided(SourceDeliveryRow),
}

fn classify_source_pass1(record: &SourceRecord, now_ms: i64) -> SourcePass {
    let SourceRecord::Valid(activity) = record else {
        return SourcePass::Decided(malformed_source_row());
    };
    let row = SourceDeliveryRow {
        state: SourceDelivery::Unknown,
        elapsed_ms: None,
        last_accepted_ingest_at: activity.last_accepted_ingest_at.clone(),
        last_accepted_segment: activity.last_accepted_segment.clone(),
        ingest_rejection: activity.ingest_rejection.clone(),
    };
    if activity.ingest_rejection.is_some() {
        let elapsed_ms = activity
            .last_accepted_ingest_at
            .as_deref()
            .and_then(|value| timestamp_age_ms(value, now_ms));
        return SourcePass::Decided(SourceDeliveryRow {
            state: SourceDelivery::NeedsAttention,
            elapsed_ms,
            ..row
        });
    }
    let (capture_state, elapsed_ms) =
        capture_freshness(activity.last_accepted_ingest_at.as_deref(), false, now_ms);
    let row = SourceDeliveryRow { elapsed_ms, ..row };
    match capture_state {
        ClientCaptureState::Active | ClientCaptureState::Stale => {
            SourcePass::Decided(SourceDeliveryRow {
                state: SourceDelivery::Current,
                ..row
            })
        }
        ClientCaptureState::Unknown => SourcePass::Decided(SourceDeliveryRow {
            state: SourceDelivery::Unknown,
            ..row
        }),
        ClientCaptureState::Offline | ClientCaptureState::NoCapture => SourcePass::Undecided(row),
        // Rejection is handled above; capture_freshness(..., false, ...) never
        // returns Degraded.
        ClientCaptureState::Degraded => {
            unreachable!("capture_freshness with rejecting=false cannot return Degraded")
        }
    }
}

fn malformed_source_row() -> SourceDeliveryRow {
    SourceDeliveryRow {
        state: SourceDelivery::Unknown,
        elapsed_ms: None,
        last_accepted_ingest_at: None,
        last_accepted_segment: None,
        ingest_rejection: None,
    }
}

/// Aggregate assessed capture rows, with a rejection always winning.
pub fn rollup_client_capture_states(rows: &[ClientAssessment]) -> Option<ClientCaptureState> {
    let states = rows.iter().map(|row| row.capture_state).collect::<Vec<_>>();
    if states.contains(&ClientCaptureState::Degraded) {
        Some(ClientCaptureState::Degraded)
    } else if states.contains(&ClientCaptureState::Stale) {
        Some(ClientCaptureState::Stale)
    } else if states.contains(&ClientCaptureState::Offline) {
        Some(ClientCaptureState::Offline)
    } else if states.contains(&ClientCaptureState::Active) {
        Some(ClientCaptureState::Active)
    } else {
        None
    }
}

fn activity_state(read: &DeviceActivityRead) -> ClientActivityState {
    match read {
        DeviceActivityRead::Present(_) => ClientActivityState::Present,
        DeviceActivityRead::Missing => ClientActivityState::Missing,
        DeviceActivityRead::Unreadable => ClientActivityState::Unreadable,
        DeviceActivityRead::Malformed => ClientActivityState::Malformed,
    }
}

fn activity_for(
    read: &DeviceActivityRead,
) -> Option<&std::collections::BTreeMap<String, ClientActivity>> {
    match read {
        DeviceActivityRead::Present(activity) => Some(activity),
        DeviceActivityRead::Missing
        | DeviceActivityRead::Unreadable
        | DeviceActivityRead::Malformed => None,
    }
}

fn assessment_for(
    client_entry: ClientEntry,
    activities: Option<&std::collections::BTreeMap<String, ClientActivity>>,
    activity_state: ClientActivityState,
    now_ms: i64,
) -> ClientAssessment {
    let cid = client_entry.fingerprint.clone();
    let activity = activities.and_then(|activities| activities.get(&cid));
    let last_seen_at = activity.map(|activity| activity.last_seen_at.clone());
    let last_accepted_ingest_at =
        activity.and_then(|activity| activity.last_accepted_ingest_at.clone());
    let last_accepted_segment =
        activity.and_then(|activity| activity.last_accepted_segment.clone());
    let ingest_rejection = activity.and_then(|activity| activity.ingest_rejection.clone());
    let connection = match activity_state {
        ClientActivityState::Unreadable | ClientActivityState::Malformed => {
            ConnectionFreshness::Unknown
        }
        ClientActivityState::Present | ClientActivityState::Missing => {
            connection_freshness(last_seen_at.as_deref(), now_ms)
        }
    };
    let (capture_state, capture_elapsed_ms) = match activity_state {
        ClientActivityState::Unreadable | ClientActivityState::Malformed => {
            (ClientCaptureState::Unknown, None)
        }
        ClientActivityState::Present | ClientActivityState::Missing => capture_freshness(
            last_accepted_ingest_at.as_deref(),
            ingest_rejection.is_some(),
            now_ms,
        ),
    };
    let source_delivery = match activity_state {
        ClientActivityState::Unreadable | ClientActivityState::Malformed => BTreeMap::new(),
        ClientActivityState::Present | ClientActivityState::Missing => match activity {
            Some(activity) => classify_source_deliveries(&activity.sources, &connection, now_ms),
            None => BTreeMap::new(),
        },
    };
    ClientAssessment {
        cid,
        client_entry,
        last_seen_at,
        last_accepted_ingest_at,
        last_accepted_segment,
        ingest_rejection,
        connection,
        capture_state,
        capture_elapsed_ms,
        source_delivery,
    }
}

fn connection_freshness(last_seen_at: Option<&str>, now_ms: i64) -> ConnectionFreshness {
    let Some(last_seen_at) = last_seen_at else {
        return ConnectionFreshness::Known {
            state: ConnectionState::Disconnected,
            group: ConnectionGroup::Inactive,
            elapsed_ms: None,
            clock_skew: false,
            label: "offline",
            reach: ClientReach::Offline,
        };
    };
    let Some(age_ms) = timestamp_age_ms(last_seen_at, now_ms) else {
        return ConnectionFreshness::Unknown;
    };
    if age_ms < -CLIENT_FUTURE_CLOCK_DRIFT_TOLERANCE_MS {
        ConnectionFreshness::Known {
            state: ConnectionState::Disconnected,
            group: ConnectionGroup::Inactive,
            elapsed_ms: Some(age_ms),
            clock_skew: true,
            label: "offline",
            reach: ClientReach::Offline,
        }
    } else if age_ms < CLIENT_ACTIVE_MS {
        ConnectionFreshness::Known {
            state: ConnectionState::Connected,
            group: ConnectionGroup::Active,
            elapsed_ms: Some(age_ms.max(0)),
            clock_skew: false,
            label: "connected",
            reach: ClientReach::Active,
        }
    } else if age_ms < CLIENT_STALE_MS {
        ConnectionFreshness::Known {
            state: ConnectionState::Stale,
            group: ConnectionGroup::Stale,
            elapsed_ms: Some(age_ms),
            clock_skew: false,
            label: "not reporting",
            reach: ClientReach::Stale,
        }
    } else {
        ConnectionFreshness::Known {
            state: ConnectionState::Disconnected,
            group: ConnectionGroup::Inactive,
            elapsed_ms: Some(age_ms),
            clock_skew: false,
            label: "offline",
            reach: ClientReach::Offline,
        }
    }
}

fn capture_freshness(
    last_accepted_ingest_at: Option<&str>,
    rejecting: bool,
    now_ms: i64,
) -> (ClientCaptureState, Option<i64>) {
    if rejecting {
        return (
            ClientCaptureState::Degraded,
            last_accepted_ingest_at.and_then(|value| timestamp_age_ms(value, now_ms)),
        );
    }
    let Some(last_accepted_ingest_at) = last_accepted_ingest_at else {
        return (ClientCaptureState::NoCapture, None);
    };
    let Some(age_ms) = timestamp_age_ms(last_accepted_ingest_at, now_ms) else {
        return (ClientCaptureState::Unknown, None);
    };
    let state = if age_ms < -CLIENT_FUTURE_CLOCK_DRIFT_TOLERANCE_MS {
        ClientCaptureState::Offline
    } else if age_ms < CLIENT_CAPTURE_ACTIVE_MS {
        ClientCaptureState::Active
    } else if age_ms < CLIENT_CAPTURE_STALE_MS {
        ClientCaptureState::Stale
    } else {
        ClientCaptureState::Offline
    };
    (state, Some(age_ms.max(0)))
}

fn timestamp_age_ms(timestamp: &str, now_ms: i64) -> Option<i64> {
    let timestamp = parse_rfc3339_utc(timestamp)?;
    let timestamp_ms = i64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).ok()?;
    Some(now_ms.saturating_sub(timestamp_ms))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;

    const NOW_MS: i64 = 1_776_508_800_000;
    const NOW: &str = "2026-04-19T18:03:12Z";

    #[test]
    fn missing_ledger_is_an_explicit_empty_inspection() {
        let root = TempDir::new();
        assert_eq!(
            inspect_clients_at(root.path(), NOW_MS),
            ClientInspection::Empty {
                clients: Vec::new(),
                activity: ClientActivityState::Missing,
            }
        );
    }

    #[test]
    fn unreadable_and_malformed_ledgers_are_not_empty() {
        let root = TempDir::new();
        fs::create_dir_all(root.path().join("link/authorized_clients.json")).unwrap();
        assert!(matches!(
            inspect_clients_at(root.path(), NOW_MS),
            ClientInspection::LedgerUnavailable {
                reason: ClientLedgerUnavailable::Unreadable,
                ..
            }
        ));
        fs::remove_dir_all(root.path().join("link/authorized_clients.json")).unwrap();
        fs::create_dir_all(root.path().join("link")).unwrap();
        fs::write(root.path().join("link/authorized_clients.json"), b"{").unwrap();
        assert!(matches!(
            inspect_clients_at(root.path(), NOW_MS),
            ClientInspection::LedgerUnavailable {
                reason: ClientLedgerUnavailable::Malformed,
                ..
            }
        ));
        fs::write(
            root.path().join("link/authorized_clients.json"),
            br#"[{"fingerprint":"a"},{"fingerprint":"a"}]"#,
        )
        .unwrap();
        assert!(matches!(
            inspect_clients_at(root.path(), NOW_MS),
            ClientInspection::LedgerUnavailable {
                reason: ClientLedgerUnavailable::DuplicateCid,
                ..
            }
        ));
    }

    #[test]
    fn missing_activity_is_an_honest_no_capture_state() {
        let root = TempDir::new();
        write_clients(root.path(), &["a"]);
        let ClientInspection::Ready { clients, activity } = inspect_clients_at(root.path(), NOW_MS)
        else {
            panic!("ready inspection");
        };
        assert_eq!(activity, ClientActivityState::Missing);
        assert_eq!(clients[0].capture_state, ClientCaptureState::NoCapture);
        assert_eq!(clients[0].last_accepted_ingest_at, None);
    }

    #[test]
    fn unreadable_or_malformed_activity_is_unknown_for_every_client() {
        for contents in [None, Some(b"{".as_slice())] {
            let root = TempDir::new();
            write_clients(root.path(), &["a", "b"]);
            let activity_path = root.path().join("link/devices.json");
            match contents {
                None => fs::create_dir_all(&activity_path).unwrap(),
                Some(contents) => fs::write(&activity_path, contents).unwrap(),
            }
            let ClientInspection::Ready { clients, activity } =
                inspect_clients_at(root.path(), NOW_MS)
            else {
                panic!("ready inspection");
            };
            assert!(matches!(
                activity,
                ClientActivityState::Unreadable | ClientActivityState::Malformed
            ));
            assert!(clients.iter().all(|client| {
                client.capture_state == ClientCaptureState::Unknown
                    && client.connection == ConnectionFreshness::Unknown
            }));
        }
    }

    #[test]
    fn client_without_activity_is_present_but_has_no_capture_evidence() {
        let root = TempDir::new();
        write_clients(root.path(), &["a"]);
        fs::write(root.path().join("link/devices.json"), json!({}).to_string()).unwrap();
        let ClientInspection::Ready { clients, activity } = inspect_clients_at(root.path(), NOW_MS)
        else {
            panic!("ready inspection");
        };
        assert_eq!(activity, ClientActivityState::Present);
        assert_eq!(clients[0].capture_state, ClientCaptureState::NoCapture);
        assert_eq!(clients[0].last_seen_at, None);
    }

    #[test]
    fn orphan_activity_is_not_projected_as_a_client() {
        let root = TempDir::new();
        write_clients(root.path(), &["a"]);
        fs::write(
            root.path().join("link/devices.json"),
            json!({"orphan": {"last_seen_at": NOW}}).to_string(),
        )
        .unwrap();
        let ClientInspection::Ready { clients, .. } = inspect_clients_at(root.path(), NOW_MS)
        else {
            panic!("ready inspection");
        };
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].cid, "a");
    }

    #[test]
    fn capture_freshness_is_active_at_thirty_seconds() {
        assert_eq!(
            capture_freshness_at_age(30_000, false),
            (ClientCaptureState::Active, Some(30_000))
        );
    }

    #[test]
    fn capture_freshness_is_active_at_two_minutes() {
        assert_eq!(
            capture_freshness_at_age(120_000, false),
            (ClientCaptureState::Active, Some(120_000))
        );
    }

    #[test]
    fn capture_freshness_is_active_at_299_999_milliseconds() {
        assert_eq!(
            capture_freshness_at_age(299_999, false),
            (ClientCaptureState::Active, Some(299_999))
        );
    }

    #[test]
    fn capture_freshness_is_active_at_359_999_milliseconds() {
        assert_eq!(
            capture_freshness_at_age(359_999, false),
            (ClientCaptureState::Active, Some(359_999))
        );
    }

    #[test]
    fn capture_freshness_is_stale_at_six_minutes() {
        assert_eq!(
            capture_freshness_at_age(360_000, false),
            (ClientCaptureState::Stale, Some(360_000))
        );
    }

    #[test]
    fn capture_freshness_is_stale_at_659_999_milliseconds() {
        assert_eq!(
            capture_freshness_at_age(659_999, false),
            (ClientCaptureState::Stale, Some(659_999))
        );
    }

    #[test]
    fn capture_freshness_is_offline_at_eleven_minutes() {
        assert_eq!(
            capture_freshness_at_age(660_000, false),
            (ClientCaptureState::Offline, Some(660_000))
        );
    }

    #[test]
    fn connection_freshness_is_stale_at_thirty_seconds() {
        assert!(matches!(
            connection_freshness_at_age(30_000),
            ConnectionFreshness::Known {
                state: ConnectionState::Stale,
                group: ConnectionGroup::Stale,
                elapsed_ms: Some(30_000),
                clock_skew: false,
                label: "not reporting",
                reach: ClientReach::Stale,
            }
        ));
    }

    #[test]
    fn connection_freshness_is_disconnected_at_two_minutes() {
        assert!(matches!(
            connection_freshness_at_age(120_000),
            ConnectionFreshness::Known {
                state: ConnectionState::Disconnected,
                group: ConnectionGroup::Inactive,
                elapsed_ms: Some(120_000),
                clock_skew: false,
                label: "offline",
                reach: ClientReach::Offline,
            }
        ));
    }

    #[test]
    fn capture_rejection_wins_past_offline_threshold() {
        assert_eq!(
            capture_freshness_at_age(660_001, true),
            (ClientCaptureState::Degraded, Some(660_001))
        );
    }

    #[test]
    fn quiet_sources_need_attention_when_the_device_is_not_fully_offline() {
        let sources = BTreeMap::from([
            (
                "audio".to_owned(),
                valid_source_at_age(CLIENT_CAPTURE_STALE_MS),
            ),
            (
                "location".to_owned(),
                valid_source_at_age(CLIENT_CAPTURE_STALE_MS + 1),
            ),
        ]);
        let classified =
            classify_source_deliveries(&sources, &known_connection(ClientReach::Active), NOW_MS);
        assert_eq!(classified["audio"].state, SourceDelivery::NeedsAttention);
        assert_eq!(classified["location"].state, SourceDelivery::NeedsAttention);
    }

    #[test]
    fn quiet_source_needs_attention_when_a_sibling_is_current_even_if_offline() {
        let sources = BTreeMap::from([
            ("audio".to_owned(), valid_source_at_age(30_000)),
            (
                "location".to_owned(),
                valid_source_at_age(CLIENT_CAPTURE_STALE_MS),
            ),
        ]);
        let classified =
            classify_source_deliveries(&sources, &known_connection(ClientReach::Offline), NOW_MS);
        assert_eq!(classified["audio"].state, SourceDelivery::Current);
        assert_eq!(classified["location"].state, SourceDelivery::NeedsAttention);
    }

    #[test]
    fn quiet_source_is_unknown_when_offline_without_a_current_sibling() {
        let sources = BTreeMap::from([(
            "location".to_owned(),
            valid_source_at_age(CLIENT_CAPTURE_STALE_MS),
        )]);
        let classified =
            classify_source_deliveries(&sources, &known_connection(ClientReach::Offline), NOW_MS);
        assert_eq!(classified["location"].state, SourceDelivery::Unknown);
        assert_ne!(classified["location"].state, SourceDelivery::NeedsAttention);
    }

    #[test]
    fn stale_connection_reach_still_satisfies_signal_a() {
        let sources = BTreeMap::from([(
            "audio".to_owned(),
            valid_source_at_age(CLIENT_CAPTURE_STALE_MS),
        )]);
        let classified =
            classify_source_deliveries(&sources, &known_connection(ClientReach::Stale), NOW_MS);
        assert_eq!(classified["audio"].state, SourceDelivery::NeedsAttention);
    }

    #[test]
    fn source_rejection_wins_outright_even_with_recent_accept() {
        let sources = BTreeMap::from([(
            "audio".to_owned(),
            SourceRecord::Valid(crate::ledger::SourceActivity {
                last_accepted_ingest_at: Some(timestamp_at_age(30_000)),
                last_accepted_segment: None,
                ingest_rejection: Some(IngestRejection {
                    reason_code: "event_append_failed".to_owned(),
                    first: timestamp_at_age(1_000),
                    latest: timestamp_at_age(1_000),
                    active_count: 1,
                }),
            }),
        )]);
        let classified =
            classify_source_deliveries(&sources, &known_connection(ClientReach::Offline), NOW_MS);
        assert_eq!(classified["audio"].state, SourceDelivery::NeedsAttention);
        assert_eq!(classified["audio"].elapsed_ms, Some(30_000));
    }

    #[test]
    fn source_classification_is_scoped_to_one_device() {
        let quiet = BTreeMap::from([(
            "audio".to_owned(),
            valid_source_at_age(CLIENT_CAPTURE_STALE_MS),
        )]);
        let current = BTreeMap::from([("audio".to_owned(), valid_source_at_age(30_000))]);
        let a = classify_source_deliveries(&quiet, &known_connection(ClientReach::Offline), NOW_MS);
        let b =
            classify_source_deliveries(&current, &known_connection(ClientReach::Active), NOW_MS);
        assert_eq!(a["audio"].state, SourceDelivery::Unknown);
        assert_eq!(b["audio"].state, SourceDelivery::Current);
        let a_again =
            classify_source_deliveries(&quiet, &known_connection(ClientReach::Offline), NOW_MS);
        assert_eq!(a_again, a);

        let root = TempDir::new();
        write_clients(root.path(), &["a", "b"]);
        fs::write(
            root.path().join("link/devices.json"),
            json!({
                "a": {
                    "last_seen_at": timestamp_at_age(CLIENT_STALE_MS),
                    "sources": {
                        "audio": {"last_accepted_ingest_at": timestamp_at_age(CLIENT_CAPTURE_STALE_MS)}
                    }
                },
                "b": {
                    "last_seen_at": timestamp_at_age(1_000),
                    "last_accepted_ingest_at": timestamp_at_age(1_000),
                    "sources": {
                        "audio": {"last_accepted_ingest_at": timestamp_at_age(1_000)}
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let ClientInspection::Ready { clients, .. } = inspect_clients_at(root.path(), NOW_MS)
        else {
            panic!("ready inspection");
        };
        let device_a = clients.iter().find(|row| row.cid == "a").expect("device a");
        let device_b = clients.iter().find(|row| row.cid == "b").expect("device b");
        assert_eq!(
            device_a.source_delivery["audio"].state,
            SourceDelivery::Unknown
        );
        assert_eq!(
            device_b.source_delivery["audio"].state,
            SourceDelivery::Current
        );
        assert!(!device_a.source_delivery.contains_key("b"));
        assert_eq!(device_a.source_delivery.len(), 1);
    }

    fn capture_freshness_at_age(age_ms: i64, rejecting: bool) -> (ClientCaptureState, Option<i64>) {
        let timestamp = timestamp_at_age(age_ms);
        capture_freshness(Some(&timestamp), rejecting, NOW_MS)
    }

    fn connection_freshness_at_age(age_ms: i64) -> ConnectionFreshness {
        let timestamp = timestamp_at_age(age_ms);
        connection_freshness(Some(&timestamp), NOW_MS)
    }

    fn timestamp_at_age(age_ms: i64) -> String {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(NOW_MS - age_ms) * 1_000_000)
            .expect("test timestamp")
            .format(&Rfc3339)
            .expect("RFC3339 timestamp")
    }

    fn valid_source_at_age(age_ms: i64) -> SourceRecord {
        SourceRecord::Valid(crate::ledger::SourceActivity {
            last_accepted_ingest_at: Some(timestamp_at_age(age_ms)),
            last_accepted_segment: None,
            ingest_rejection: None,
        })
    }

    fn known_connection(reach: ClientReach) -> ConnectionFreshness {
        match reach {
            ClientReach::Active => ConnectionFreshness::Known {
                state: ConnectionState::Connected,
                group: ConnectionGroup::Active,
                elapsed_ms: Some(0),
                clock_skew: false,
                label: "connected",
                reach,
            },
            ClientReach::Stale => ConnectionFreshness::Known {
                state: ConnectionState::Stale,
                group: ConnectionGroup::Stale,
                elapsed_ms: Some(CLIENT_STALE_MS - 1),
                clock_skew: false,
                label: "not reporting",
                reach,
            },
            ClientReach::Offline => ConnectionFreshness::Known {
                state: ConnectionState::Disconnected,
                group: ConnectionGroup::Inactive,
                elapsed_ms: Some(CLIENT_STALE_MS),
                clock_skew: false,
                label: "offline",
                reach,
            },
        }
    }

    fn write_clients(root: &Path, cids: &[&str]) {
        let link = root.join("link");
        fs::create_dir_all(&link).unwrap();
        let clients = cids
            .iter()
            .map(|cid| {
                json!({
                    "fingerprint": cid,
                    "device_label": "phone",
                    "paired_at": NOW,
                    "instance_id": "instance",
                })
            })
            .collect::<Vec<_>>();
        fs::write(
            link.join("authorized_clients.json"),
            serde_json::to_vec(&clients).unwrap(),
        )
        .unwrap();
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = PathBuf::from("/var/tmp").join(format!(
                "sol-link-client-status-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
