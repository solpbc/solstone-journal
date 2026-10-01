// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Weekly heads-up notification qualification, scheduling, once-record state machine, and delivery.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{Duration as ChronoDuration, NaiveTime, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, JsonWriteOptions, LockOptions, hold_lock,
    write_bytes_exclusive, write_json,
};
use time::OffsetDateTime;

use crate::envelope::{Notification, format_notification_at, parse_notification_at};
use crate::relay::{RelayTransport, UreqRelay};
use crate::router::{DeliveryOutcome, Refused, deliver_notification};

const OWNER_ONLY_MODE: u32 = 0o600;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnceState {
    Pending,
    Retry,
    Done,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WeeklyHeadsUpRecord {
    pub version: u32,
    pub week: String,
    pub state: OnceState,
    pub due: String,
    pub reselect: bool,
    pub failed: Vec<String>,
}

fn heads_up_dir(journal: &Path) -> PathBuf {
    journal.join("health/weekly-heads-up")
}

fn record_path(journal: &Path, week: &str) -> PathBuf {
    heads_up_dir(journal).join(format!("{week}.json"))
}

fn lock_path(journal: &Path) -> PathBuf {
    journal.join("health/weekly-heads-up.lock")
}

fn is_eight_digits(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit())
}

/// Check whether reflections/weekly/<day>.json qualifies for heads-up notification.
fn qualifies(journal: &Path, day: &str) -> bool {
    let week_file = journal.join(format!("reflections/weekly/{day}.json"));
    let content = match fs::read_to_string(&week_file) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let Ok(json_val) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    let Some(memories) = json_val.get("memories").and_then(Value::as_array) else {
        return false;
    };
    !memories.is_empty()
}

/// Compute the initial due timestamp according to the [09:00:00, 21:00:00) owner_zone window.
pub(crate) fn initial_due(journal: &Path, now: OffsetDateTime) -> OffsetDateTime {
    use chrono::TimeZone;
    let zone = solstone_core_journal_config::owner_zone(journal);
    let Some(now_utc_chrono) =
        chrono::DateTime::<chrono::Utc>::from_timestamp(now.unix_timestamp(), now.nanosecond())
    else {
        return now;
    };
    let local = now_utc_chrono.with_timezone(&zone);

    let local_time = local.time();
    let start_window = NaiveTime::from_hms_opt(9, 0, 0).expect("valid time");
    let end_window = NaiveTime::from_hms_opt(21, 0, 0).expect("valid time");

    if local_time >= start_window && local_time < end_window {
        return now;
    }

    let target_date = if local_time < start_window {
        local.date_naive()
    } else {
        local.date_naive() + ChronoDuration::days(1)
    };

    let target_naive = target_date.and_hms_opt(9, 0, 0).expect("valid naive dt");
    let target_local = match zone.from_local_datetime(&target_naive) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(dt, _) => dt,
        chrono::LocalResult::None => local + ChronoDuration::hours(1),
    };

    let target_utc = target_local.with_timezone(&chrono::Utc);
    OffsetDateTime::from_unix_timestamp(target_utc.timestamp())
        .unwrap_or(now)
        .replace_nanosecond(target_utc.timestamp_subsec_nanos())
        .unwrap_or(now)
}

/// Compute the retry due timestamp at the next clock-hour boundary strictly after the attempt.
pub(crate) fn retry_due(journal: &Path, now: OffsetDateTime) -> OffsetDateTime {
    use chrono::TimeZone;
    let zone = solstone_core_journal_config::owner_zone(journal);
    let Some(now_utc_chrono) =
        chrono::DateTime::<chrono::Utc>::from_timestamp(now.unix_timestamp(), now.nanosecond())
    else {
        return now;
    };
    let local = now_utc_chrono.with_timezone(&zone);

    let next_hour_naive = local
        .date_naive()
        .and_hms_opt(local.hour(), 0, 0)
        .expect("valid dt")
        + ChronoDuration::hours(1);

    let start_window = NaiveTime::from_hms_opt(9, 0, 0).expect("valid time");
    let end_window = NaiveTime::from_hms_opt(21, 0, 0).expect("valid time");

    let candidate_time = next_hour_naive.time();

    let target_naive = if candidate_time >= start_window && candidate_time < end_window {
        next_hour_naive
    } else {
        let next_day = if candidate_time < start_window {
            next_hour_naive.date()
        } else {
            next_hour_naive.date() + ChronoDuration::days(1)
        };
        next_day.and_hms_opt(9, 0, 0).expect("valid dt")
    };

    let target_local = match zone.from_local_datetime(&target_naive) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(dt, _) => dt,
        chrono::LocalResult::None => local + ChronoDuration::hours(1),
    };

    let target_utc = target_local.with_timezone(&chrono::Utc);
    OffsetDateTime::from_unix_timestamp(target_utc.timestamp())
        .unwrap_or(now)
        .replace_nanosecond(target_utc.timestamp_subsec_nanos())
        .unwrap_or(now)
}

pub fn observe(journal: &Path, portal: &str, name: &str, day: &str, now: OffsetDateTime) {
    observe_with_transport(journal, portal, name, day, now, &UreqRelay::default());
}

pub(crate) fn observe_with_transport(
    journal: &Path,
    portal: &str,
    name: &str,
    day: &str,
    now: OffsetDateTime,
    transport: &dyn RelayTransport,
) {
    if name != "weekly_reflection" || !is_eight_digits(day) {
        return;
    }

    if !qualifies(journal, day) {
        return;
    }

    let dir = heads_up_dir(journal);
    let _ = fs::create_dir_all(&dir);

    let path = record_path(journal, day);
    if path.exists() {
        return;
    }

    let lock = match hold_lock(
        lock_path(journal),
        LockOptions {
            mode: Some(OWNER_ONLY_MODE),
            ..LockOptions::default()
        },
    ) {
        Ok(l) => l,
        Err(_) => return,
    };

    if path.exists() {
        return;
    }

    let due_time = initial_due(journal, now);
    let due_str = format_notification_at(due_time);

    let initial_record = WeeklyHeadsUpRecord {
        version: 1,
        week: day.to_owned(),
        state: OnceState::Pending,
        due: due_str,
        reselect: false,
        failed: Vec::new(),
    };

    let record_bytes = match serde_json::to_vec_pretty(&initial_record) {
        Ok(b) => b,
        Err(_) => return,
    };

    match write_bytes_exclusive(
        &path,
        &record_bytes,
        AtomicWriteOptions {
            mode: Some(OWNER_ONLY_MODE),
        },
    ) {
        Ok(()) => {}
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists =>
        {
            return;
        }
        Err(_) => return,
    }

    if now < due_time {
        return;
    }

    let notification = Notification {
        at: now,
        kind: "weekly".to_owned(),
        title: "your week is ready".to_owned(),
        body: "".to_owned(),
        open: Some(format!("/app/home/week/{day}")),
    };

    let outcome = deliver_notification(journal, portal, now, &notification, transport, None);
    handle_delivery_outcome(journal, &path, &initial_record, outcome, now);

    drop(lock);
}

fn handle_delivery_outcome(
    journal: &Path,
    path: &Path,
    current: &WeeklyHeadsUpRecord,
    outcome: DeliveryOutcome,
    now: OffsetDateTime,
) {
    let mut updated = current.clone();
    match outcome {
        DeliveryOutcome::Refused(Refused::NoRecipient) => {
            updated.state = OnceState::Done;
            let _ = write_json(
                path,
                &updated,
                JsonWriteOptions {
                    mode: Some(OWNER_ONLY_MODE),
                    ..JsonWriteOptions::default()
                },
            );
        }
        DeliveryOutcome::Refused(Refused::RegistryUnavailable(_))
        | DeliveryOutcome::Refused(Refused::LedgerUnavailable) => {
            updated.state = OnceState::Retry;
            updated.reselect = true;
            updated.failed = Vec::new();
            updated.due = format_notification_at(retry_due(journal, now));
            let _ = write_json(
                path,
                &updated,
                JsonWriteOptions {
                    mode: Some(OWNER_ONLY_MODE),
                    ..JsonWriteOptions::default()
                },
            );
        }
        DeliveryOutcome::Delivered(delivered_items) => {
            if delivered_items.is_empty() {
                updated.state = OnceState::Done;
            } else {
                let mut failed_hashes = Vec::new();
                for item in delivered_items {
                    if item.item.outcome == "failed" {
                        failed_hashes.push(item.device_hash);
                    }
                }

                if failed_hashes.is_empty() {
                    updated.state = OnceState::Done;
                } else {
                    updated.state = OnceState::Retry;
                    updated.reselect = false;
                    updated.failed = failed_hashes;
                    updated.due = format_notification_at(retry_due(journal, now));
                }
            }

            let _ = write_json(
                path,
                &updated,
                JsonWriteOptions {
                    mode: Some(OWNER_ONLY_MODE),
                    ..JsonWriteOptions::default()
                },
            );
        }
    }
}

pub fn poll(journal: &Path, portal: &str, now: OffsetDateTime) {
    poll_with_transport(journal, portal, now, &UreqRelay::default());
}

pub(crate) fn poll_with_transport(
    journal: &Path,
    portal: &str,
    now: OffsetDateTime,
    transport: &dyn RelayTransport,
) {
    let dir = heads_up_dir(journal);
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };

    let lock = match hold_lock(
        lock_path(journal),
        LockOptions {
            mode: Some(OWNER_ONLY_MODE),
            ..LockOptions::default()
        },
    ) {
        Ok(l) => l,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let record: WeeklyHeadsUpRecord = match serde_json::from_str(&content) {
            Ok(r) => r,
            Err(_) => continue,
        };

        if record.state == OnceState::Done {
            continue;
        }

        let Some(due_dt) = parse_notification_at(&record.due) else {
            continue;
        };

        if now < due_dt {
            continue;
        }

        let notification = Notification {
            at: now,
            kind: "weekly".to_owned(),
            title: "your week is ready".to_owned(),
            body: "".to_owned(),
            open: Some(format!("/app/home/week/{}", record.week)),
        };

        match record.state {
            OnceState::Pending => {
                let outcome =
                    deliver_notification(journal, portal, now, &notification, transport, None);
                handle_delivery_outcome(journal, &path, &record, outcome, now);
            }
            OnceState::Retry => {
                let filter = if record.reselect {
                    None
                } else {
                    let hash_set: HashSet<String> = record.failed.iter().cloned().collect();
                    Some(hash_set)
                };

                let _outcome = deliver_notification(
                    journal,
                    portal,
                    now,
                    &notification,
                    transport,
                    filter.as_ref(),
                );

                let mut updated = record.clone();
                updated.state = OnceState::Done;
                let _ = write_json(
                    &path,
                    &updated,
                    JsonWriteOptions {
                        mode: Some(OWNER_ONLY_MODE),
                        ..JsonWriteOptions::default()
                    },
                );
            }
            OnceState::Done => {}
        }
    }

    drop(lock);
}

pub fn next_due(journal: &Path, _now: OffsetDateTime) -> Option<OffsetDateTime> {
    let dir = heads_up_dir(journal);
    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };

    let mut earliest: Option<OffsetDateTime> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<WeeklyHeadsUpRecord>(&content) else {
            continue;
        };

        if record.state == OnceState::Done {
            continue;
        }

        if let Some(due_dt) = parse_notification_at(&record.due) {
            match earliest {
                Some(current) if due_dt < current => earliest = Some(due_dt),
                None => earliest = Some(due_dt),
                _ => {}
            }
        }
    }

    earliest
}

pub fn scan(journal: &Path, portal: &str, now: OffsetDateTime) {
    scan_with_transport(journal, portal, now, &UreqRelay::default());
}

pub(crate) fn scan_with_transport(
    journal: &Path,
    portal: &str,
    now: OffsetDateTime,
    transport: &dyn RelayTransport,
) {
    let talents_dir = journal.join("talents/weekly_reflection");
    let Ok(entries) = fs::read_dir(&talents_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };

        if !file_name.ends_with(".jsonl") || file_name.ends_with("_active.jsonl") {
            continue;
        }

        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(json_val) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if json_val.get("event").and_then(Value::as_str) == Some("finish")
                && json_val.get("name").and_then(Value::as_str) == Some("weekly_reflection")
                && let Some(day) = json_val.get("day").and_then(Value::as_str)
                && is_eight_digits(day)
            {
                observe_with_transport(journal, portal, "weekly_reflection", day, now, transport);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{PushKey, open_envelope};
    use crate::relay::{RelayFault, RelayReply};
    use base64::Engine;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;
    use time::Month;

    struct MockTransport {
        calls: AtomicUsize,
        reply_status: u16,
        reply_body: Vec<u8>,
        fail_timeout: bool,
    }

    impl MockTransport {
        fn new(status: u16, body: Vec<u8>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                reply_status: status,
                reply_body: body,
                fail_timeout: false,
            }
        }
    }

    impl RelayTransport for MockTransport {
        fn post_json(
            &self,
            _url: &str,
            _body_json: &[u8],
            _bearer_token: Option<&str>,
        ) -> Result<RelayReply, RelayFault> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_timeout {
                return Err(RelayFault::Timeout);
            }
            Ok(RelayReply {
                status: self.reply_status,
                body: self.reply_body.clone(),
            })
        }

        fn post_bytes(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
            _body_bytes: &[u8],
        ) -> Result<u16, RelayFault> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_timeout {
                return Err(RelayFault::Timeout);
            }
            Ok(self.reply_status)
        }
    }

    fn setup_test_journal() -> TempDir {
        let root = TempDir::new_in("/var/tmp").expect("temp dir");
        let config_dir = root.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            r#"{"identity":{"timezone":"America/Phoenix"}}"#,
        )
        .unwrap();
        root
    }

    fn write_week_file(root: &Path, day: &str, memories_count: usize, sentinel: Option<&str>) {
        let dir = root.join("reflections/weekly");
        fs::create_dir_all(&dir).unwrap();
        let memories: Vec<Value> = (0..memories_count)
            .map(|i| {
                serde_json::json!({
                    "id": format!("mem-{i}"),
                    "key": format!("k-{i}"),
                    "day": day,
                    "text": sentinel.unwrap_or("test memory"),
                    "source": { "kind": "briefing", "uri": "briefing:test" }
                })
            })
            .collect();
        let json = serde_json::json!({
            "version": 1,
            "week": { "start": day, "end": day },
            "memories": memories,
        });
        fs::write(
            dir.join(format!("{day}.json")),
            serde_json::to_string(&json).unwrap(),
        )
        .unwrap();
    }

    fn write_device_and_ledger(root: &Path, cid: &str, token: &str, reg_time: &str) {
        let ca_dir = root.join("link/ca");
        fs::create_dir_all(&ca_dir).unwrap();
        let ca = solstone_core_sol_link::ca::generate_ca().expect("ca");
        let spki_der = ca.spki_der().to_vec();
        let instance_id = solstone_core_sol_link::ca::jid_from_spki(&spki_der).expect("jid");
        fs::write(ca_dir.join("cert.pem"), ca.certificate_pem()).unwrap();
        fs::write(ca_dir.join("private.pem"), ca.private_key_pem()).unwrap();
        fs::write(
            root.join("link/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"Test Home"}}"#),
        )
        .unwrap();

        let mut ledger = solstone_core_sol_link::ledger::AuthorizationLedger::new(root);
        let entry = solstone_core_sol_link::ledger::ClientEntry::new(
            cid,
            "Test Device",
            "2026-08-01T00:00:00Z",
            "inst-1",
            solstone_core_sol_link::ledger::ClientRole::Roleless,
        );
        let _ = ledger.add(entry);

        let reg = serde_json::json!({
            "version": 2,
            "devices": [
                {
                    "platform": "ios",
                    "cid": cid,
                    "device_token": token,
                    "bundle_id": "com.solstone.test",
                    "environment": "development",
                    "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
                    "registered_at": reg_time
                }
            ]
        });
        fs::write(
            root.join("config/push-registry.json"),
            serde_json::to_string(&reg).unwrap(),
        )
        .unwrap();
    }

    fn utc_dt(year: i32, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> OffsetDateTime {
        let month = Month::try_from(month).expect("valid month");
        let date = time::Date::from_calendar_date(year, month, day).expect("valid date");
        let time = time::Time::from_hms(hour, minute, second).expect("valid time");
        OffsetDateTime::new_utc(date, time)
    }

    #[test]
    fn test_zone_window_and_due_calculations() {
        let root = setup_test_journal();

        // 2026-08-16 03:15 MST = 2026-08-16T10:15:00Z -> due 2026-08-16T16:00:00Z (09:00 MST)
        let t1 = utc_dt(2026, 8, 16, 10, 15, 0);
        let due1 = initial_due(root.path(), t1);
        assert_eq!(format_notification_at(due1), "2026-08-16T16:00:00Z");

        // 2026-08-16 09:00:00 MST = 2026-08-16T16:00:00Z -> due now
        let t2 = utc_dt(2026, 8, 16, 16, 0, 0);
        let due2 = initial_due(root.path(), t2);
        assert_eq!(format_notification_at(due2), "2026-08-16T16:00:00Z");

        // 2026-08-16 14:00 MST = 2026-08-16T21:00:00Z -> due now
        let t3 = utc_dt(2026, 8, 16, 21, 0, 0);
        let due3 = initial_due(root.path(), t3);
        assert_eq!(format_notification_at(due3), "2026-08-16T21:00:00Z");

        // 2026-08-16 21:00:00 MST = 2026-08-17T04:00:00Z -> due next day 09:00 MST = 2026-08-17T16:00:00Z
        let t4 = utc_dt(2026, 8, 17, 4, 0, 0);
        let due4 = initial_due(root.path(), t4);
        assert_eq!(format_notification_at(due4), "2026-08-17T16:00:00Z");

        // 2026-08-16 22:30 MST = 2026-08-17T05:30:00Z -> due next day 09:00 MST = 2026-08-17T16:00:00Z
        let t5 = utc_dt(2026, 8, 17, 5, 30, 0);
        let due5 = initial_due(root.path(), t5);
        assert_eq!(format_notification_at(due5), "2026-08-17T16:00:00Z");

        // Failure at 2026-08-16 14:30 MST = 2026-08-16T21:30:00Z -> retry at 15:00 MST = 2026-08-16T22:00:00Z
        let r1 = utc_dt(2026, 8, 16, 21, 30, 0);
        let retry1 = retry_due(root.path(), r1);
        assert_eq!(format_notification_at(retry1), "2026-08-16T22:00:00Z");

        // Failure at 2026-08-16 20:30 MST = 2026-08-17T03:30:00Z -> retry at next day 09:00 MST = 2026-08-17T16:00:00Z
        let r2 = utc_dt(2026, 8, 17, 3, 30, 0);
        let retry2 = retry_due(root.path(), r2);
        assert_eq!(format_notification_at(retry2), "2026-08-17T16:00:00Z");
    }

    #[test]
    fn test_qualification_and_non_qualifying() {
        let root = setup_test_journal();
        let transport = MockTransport::new(200, vec![]);
        let now = utc_dt(2026, 8, 16, 21, 0, 0);

        // Missing week file
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );
        assert!(!record_path(root.path(), "20260809").exists());

        // Empty memories
        write_week_file(root.path(), "20260809", 0, None);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );
        assert!(!record_path(root.path(), "20260809").exists());

        // Name mismatch
        write_week_file(root.path(), "20260809", 2, None);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "other_talent",
            "20260809",
            now,
            &transport,
        );
        assert!(!record_path(root.path(), "20260809").exists());

        // Invalid day format
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "2026080",
            now,
            &transport,
        );
        assert!(!record_path(root.path(), "2026080").exists());
    }

    #[test]
    fn test_heads_up_envelope_privacy_and_delivery() {
        let root = setup_test_journal();
        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token = "0123456789abcdef";
        let reg_time = "2026-08-10T12:00:00Z";
        write_device_and_ledger(root.path(), cid, token, reg_time);

        let sentinel = "TOP_SECRET_MEMORIES_CONTENT_SENTINEL";
        write_week_file(root.path(), "20260809", 1, Some(sentinel));

        // Setup transport returning relay enrollment + dispatch responses
        let enroll_resp = serde_json::json!({
            "token": "valid_bearer_token",
            "token_type": "Bearer",
            "instance_id": solstone_core_sol_link::committed::load_committed_identity(root.path()).unwrap().instance_id()
        });
        let dispatch_resp = serde_json::json!({
            "results": [
                { "token": token, "outcome": "sent" }
            ]
        });

        struct MultiTransport {
            calls: AtomicUsize,
            enroll_body: Vec<u8>,
            dispatch_body: Vec<u8>,
            dispatched_envelope: Mutex<Option<String>>,
        }

        impl RelayTransport for MultiTransport {
            fn post_json(
                &self,
                url: &str,
                body: &[u8],
                _token: Option<&str>,
            ) -> Result<RelayReply, RelayFault> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if url.ends_with("/relay-token") {
                    Ok(RelayReply {
                        status: 200,
                        body: self.enroll_body.clone(),
                    })
                } else {
                    let parsed: Value = serde_json::from_slice(body).unwrap();
                    let b64 = parsed["devices"][0]["envelope"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    *self.dispatched_envelope.lock().unwrap() = Some(b64);
                    Ok(RelayReply {
                        status: 200,
                        body: self.dispatch_body.clone(),
                    })
                }
            }

            fn post_bytes(
                &self,
                _url: &str,
                _headers: &[(&str, &str)],
                _body: &[u8],
            ) -> Result<u16, RelayFault> {
                Ok(200)
            }
        }

        let transport = MultiTransport {
            calls: AtomicUsize::new(0),
            enroll_body: serde_json::to_vec(&enroll_resp).unwrap(),
            dispatch_body: serde_json::to_vec(&dispatch_resp).unwrap(),
            dispatched_envelope: Mutex::new(None),
        };

        let now = utc_dt(2026, 8, 16, 21, 0, 0);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );

        let rec_path = record_path(root.path(), "20260809");
        assert!(rec_path.exists());
        let rec_content = fs::read_to_string(&rec_path).unwrap();
        let record: WeeklyHeadsUpRecord = serde_json::from_str(&rec_content).unwrap();
        assert_eq!(record.state, OnceState::Done);

        // Assert record bytes contain NO tokens, endpoints, keys, or sentinel
        assert!(!rec_content.contains(sentinel));
        assert!(!rec_content.contains(token));
        assert!(!rec_content.contains("KioqKio"));

        // Assert envelope contents
        let envelope_b64 = transport
            .dispatched_envelope
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        let envelope_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(envelope_b64)
            .unwrap();
        let key = PushKey::from_base64url("KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio").unwrap();
        let notif = open_envelope(&key, &envelope_bytes).expect("open envelope");
        assert_eq!(notif.kind, "weekly");
        assert_eq!(notif.title, "your week is ready");
        assert_eq!(notif.body, "");
        assert_eq!(notif.open.as_deref(), Some("/app/home/week/20260809"));
        assert_eq!(notif.at, now);

        // Second observe is a no-op
        let calls_before = transport.calls.load(Ordering::SeqCst);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), calls_before);

        // Poll is a no-op
        poll_with_transport(root.path(), "https://portal.test", now, &transport);
        assert_eq!(transport.calls.load(Ordering::SeqCst), calls_before);
    }

    #[test]
    fn test_deferred_schedule_and_poll() {
        let root = setup_test_journal();
        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token = "0123456789abcdef";
        let reg_time = "2026-08-10T12:00:00Z";
        write_device_and_ledger(root.path(), cid, token, reg_time);
        write_week_file(root.path(), "20260809", 1, None);

        let enroll_resp = serde_json::json!({
            "token": "valid_bearer_token",
            "token_type": "Bearer",
            "instance_id": solstone_core_sol_link::committed::load_committed_identity(root.path()).unwrap().instance_id()
        });
        let dispatch_resp = serde_json::json!({
            "results": [
                { "token": token, "outcome": "sent" }
            ]
        });

        struct TestTransport {
            calls: AtomicUsize,
            enroll_body: Vec<u8>,
            dispatch_body: Vec<u8>,
        }

        impl RelayTransport for TestTransport {
            fn post_json(
                &self,
                url: &str,
                _body: &[u8],
                _token: Option<&str>,
            ) -> Result<RelayReply, RelayFault> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if url.ends_with("/relay-token") {
                    Ok(RelayReply {
                        status: 200,
                        body: self.enroll_body.clone(),
                    })
                } else {
                    Ok(RelayReply {
                        status: 200,
                        body: self.dispatch_body.clone(),
                    })
                }
            }

            fn post_bytes(
                &self,
                _url: &str,
                _headers: &[(&str, &str)],
                _body: &[u8],
            ) -> Result<u16, RelayFault> {
                Ok(200)
            }
        }

        let transport = TestTransport {
            calls: AtomicUsize::new(0),
            enroll_body: serde_json::to_vec(&enroll_resp).unwrap(),
            dispatch_body: serde_json::to_vec(&dispatch_resp).unwrap(),
        };

        // Trigger at 03:15 MST = 2026-08-16T10:15:00Z -> pending, no send
        let t1 = utc_dt(2026, 8, 16, 10, 15, 0);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            t1,
            &transport,
        );

        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
        let rec: WeeklyHeadsUpRecord = serde_json::from_str(
            &fs::read_to_string(record_path(root.path(), "20260809")).unwrap(),
        )
        .unwrap();
        assert_eq!(rec.state, OnceState::Pending);
        assert_eq!(rec.due, "2026-08-16T16:00:00Z");

        // Next due check
        assert_eq!(
            format_notification_at(next_due(root.path(), t1).unwrap()),
            "2026-08-16T16:00:00Z"
        );

        // Poll before due at 15:59:59Z -> no send
        let t_before = utc_dt(2026, 8, 16, 15, 59, 59);
        poll_with_transport(root.path(), "https://portal.test", t_before, &transport);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);

        // Poll at 16:00:00Z -> sends once
        let t_due = utc_dt(2026, 8, 16, 16, 0, 0);
        poll_with_transport(root.path(), "https://portal.test", t_due, &transport);
        assert!(transport.calls.load(Ordering::SeqCst) > 0);

        let rec2: WeeklyHeadsUpRecord = serde_json::from_str(
            &fs::read_to_string(record_path(root.path(), "20260809")).unwrap(),
        )
        .unwrap();
        assert_eq!(rec2.state, OnceState::Done);
        assert_eq!(next_due(root.path(), t_due), None);
    }

    #[test]
    fn test_retry_and_partial_failure() {
        let root = setup_test_journal();
        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token1 = "0123456789abcdef";
        let token2 = "fedcba9876543210";
        let reg_time = "2026-08-10T12:00:00Z";

        let ca_dir = root.path().join("link/ca");
        fs::create_dir_all(&ca_dir).unwrap();
        let ca = solstone_core_sol_link::ca::generate_ca().expect("ca");
        let spki_der = ca.spki_der().to_vec();
        let instance_id = solstone_core_sol_link::ca::jid_from_spki(&spki_der).expect("jid");
        fs::write(ca_dir.join("cert.pem"), ca.certificate_pem()).unwrap();
        fs::write(ca_dir.join("private.pem"), ca.private_key_pem()).unwrap();
        fs::write(
            root.path().join("link/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"Test Home"}}"#),
        )
        .unwrap();

        let mut ledger = solstone_core_sol_link::ledger::AuthorizationLedger::new(root.path());
        let entry = solstone_core_sol_link::ledger::ClientEntry::new(
            cid,
            "Test Device",
            "2026-08-01T00:00:00Z",
            "inst-1",
            solstone_core_sol_link::ledger::ClientRole::Roleless,
        );
        let _ = ledger.add(entry);

        let reg = serde_json::json!({
            "version": 2,
            "devices": [
                {
                    "platform": "ios",
                    "cid": cid,
                    "device_token": token1,
                    "bundle_id": "com.solstone.test",
                    "environment": "development",
                    "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
                    "registered_at": reg_time
                },
                {
                    "platform": "ios",
                    "cid": cid,
                    "device_token": token2,
                    "bundle_id": "com.solstone.test",
                    "environment": "development",
                    "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
                    "registered_at": reg_time
                }
            ]
        });
        fs::write(
            root.path().join("config/push-registry.json"),
            serde_json::to_string(&reg).unwrap(),
        )
        .unwrap();
        write_week_file(root.path(), "20260809", 1, None);

        let enroll_resp = serde_json::json!({
            "token": "valid_bearer_token",
            "token_type": "Bearer",
            "instance_id": instance_id
        });

        // First attempt: token1 fails, token2 sent
        let dispatch_resp1 = serde_json::json!({
            "results": [
                { "token": token1, "outcome": "failed", "reason": "Unregistered" },
                { "token": token2, "outcome": "sent" }
            ]
        });

        // Second attempt: token1 sent
        let dispatch_resp2 = serde_json::json!({
            "results": [
                { "token": token1, "outcome": "sent" }
            ]
        });

        struct StepTransport {
            attempt: AtomicUsize,
            enroll_body: Vec<u8>,
            dispatch1_body: Vec<u8>,
            dispatch2_body: Vec<u8>,
            last_dispatch_tokens: Mutex<Vec<String>>,
        }

        impl RelayTransport for StepTransport {
            fn post_json(
                &self,
                url: &str,
                body: &[u8],
                _token: Option<&str>,
            ) -> Result<RelayReply, RelayFault> {
                if url.ends_with("/relay-token") {
                    Ok(RelayReply {
                        status: 200,
                        body: self.enroll_body.clone(),
                    })
                } else {
                    let att = self.attempt.fetch_add(1, Ordering::SeqCst);
                    let parsed: Value = serde_json::from_slice(body).unwrap();
                    let tokens: Vec<String> = parsed["devices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|d| d["token"].as_str().unwrap().to_owned())
                        .collect();
                    *self.last_dispatch_tokens.lock().unwrap() = tokens;
                    if att == 0 {
                        Ok(RelayReply {
                            status: 200,
                            body: self.dispatch1_body.clone(),
                        })
                    } else {
                        Ok(RelayReply {
                            status: 200,
                            body: self.dispatch2_body.clone(),
                        })
                    }
                }
            }

            fn post_bytes(
                &self,
                _url: &str,
                _headers: &[(&str, &str)],
                _body: &[u8],
            ) -> Result<u16, RelayFault> {
                Ok(200)
            }
        }

        let transport = StepTransport {
            attempt: AtomicUsize::new(0),
            enroll_body: serde_json::to_vec(&enroll_resp).unwrap(),
            dispatch1_body: serde_json::to_vec(&dispatch_resp1).unwrap(),
            dispatch2_body: serde_json::to_vec(&dispatch_resp2).unwrap(),
            last_dispatch_tokens: Mutex::new(Vec::new()),
        };

        // 14:30 MST = 21:30 UTC -> initial attempt
        let t1 = utc_dt(2026, 8, 16, 21, 30, 0);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            t1,
            &transport,
        );

        let rec: WeeklyHeadsUpRecord = serde_json::from_str(
            &fs::read_to_string(record_path(root.path(), "20260809")).unwrap(),
        )
        .unwrap();
        assert_eq!(rec.state, OnceState::Retry);
        assert_eq!(rec.reselect, false);
        assert_eq!(rec.failed.len(), 1);
        assert_eq!(rec.due, "2026-08-16T22:00:00Z");

        // Poll at 22:00:00Z -> retries ONLY the failed device
        let t2 = utc_dt(2026, 8, 16, 22, 0, 0);
        poll_with_transport(root.path(), "https://portal.test", t2, &transport);

        let tokens_in_second_attempt = transport.last_dispatch_tokens.lock().unwrap().clone();
        assert_eq!(tokens_in_second_attempt, vec![token1.to_owned()]);

        let rec2: WeeklyHeadsUpRecord = serde_json::from_str(
            &fs::read_to_string(record_path(root.path(), "20260809")).unwrap(),
        )
        .unwrap();
        assert_eq!(rec2.state, OnceState::Done);

        // One more poll does not dispatch
        let attempts_before = transport.attempt.load(Ordering::SeqCst);
        let t3 = utc_dt(2026, 8, 16, 23, 0, 0);
        poll_with_transport(root.path(), "https://portal.test", t3, &transport);
        let attempts_after = transport.attempt.load(Ordering::SeqCst);
        assert_eq!(attempts_before, attempts_after);
    }

    #[test]
    fn unparseable_week_file_writes_no_record_and_does_not_call_transport() {
        let root = setup_test_journal();
        let weekly_dir = root.path().join("reflections/weekly");
        fs::create_dir_all(&weekly_dir).unwrap();
        fs::write(weekly_dir.join("20260809.json"), "{").unwrap();

        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token = "0123456789abcdef";
        let reg_time = "2026-08-10T12:00:00Z";
        write_device_and_ledger(root.path(), cid, token, reg_time);

        let transport = MockTransport::new(200, vec![]);
        // 12:00 Phoenix (in window) = 19:00 UTC
        let now = utc_dt(2026, 8, 16, 19, 0, 0);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );

        assert!(!record_path(root.path(), "20260809").exists());
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn missing_push_registry_qualifying_in_window_ends_done_no_transport_call() {
        let root = setup_test_journal();
        write_week_file(root.path(), "20260809", 1, None);

        let transport = MockTransport::new(200, vec![]);
        // 12:00 Phoenix (in window) = 19:00 UTC
        let now = utc_dt(2026, 8, 16, 19, 0, 0);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );

        let rec_path = record_path(root.path(), "20260809");
        assert!(rec_path.exists());
        let rec: WeeklyHeadsUpRecord =
            serde_json::from_str(&fs::read_to_string(rec_path).unwrap()).unwrap();
        assert_eq!(rec.state, OnceState::Done);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn device_age_30_vs_31_and_ledger_filtering() {
        let root = setup_test_journal();
        let cid_authorized =
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let cid_unauthorized =
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let token_31_days = "11111111111111111111111111111111";
        let token_30_days = "22222222222222222222222222222222";
        let token_unauthorized = "33333333333333333333333333333333";

        write_device_and_ledger(
            root.path(),
            cid_authorized,
            token_30_days,
            "2026-07-17T19:00:00Z",
        );

        // now = 2026-08-16T19:00:00Z (12:00 Phoenix)
        // 30 days before = 2026-07-17T19:00:00Z
        // 31 days before = 2026-07-16T19:00:00Z
        let reg = serde_json::json!({
            "version": 2,
            "devices": [
                {
                    "platform": "ios",
                    "cid": cid_authorized,
                    "device_token": token_31_days,
                    "bundle_id": "com.solstone.test",
                    "environment": "development",
                    "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
                    "registered_at": "2026-07-16T19:00:00Z"
                },
                {
                    "platform": "ios",
                    "cid": cid_authorized,
                    "device_token": token_30_days,
                    "bundle_id": "com.solstone.test",
                    "environment": "development",
                    "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
                    "registered_at": "2026-07-17T19:00:00Z"
                },
                {
                    "platform": "ios",
                    "cid": cid_unauthorized,
                    "device_token": token_unauthorized,
                    "bundle_id": "com.solstone.test",
                    "environment": "development",
                    "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
                    "registered_at": "2026-08-10T19:00:00Z"
                }
            ]
        });
        fs::write(
            root.path().join("config/push-registry.json"),
            serde_json::to_string(&reg).unwrap(),
        )
        .unwrap();
        write_week_file(root.path(), "20260809", 1, None);

        let instance_id = solstone_core_sol_link::committed::load_committed_identity(root.path())
            .unwrap()
            .instance_id()
            .to_owned();
        let enroll_resp = serde_json::json!({
            "token": "valid_bearer_token",
            "token_type": "Bearer",
            "instance_id": instance_id
        });
        let dispatch_resp = serde_json::json!({
            "results": [
                { "token": token_30_days, "outcome": "sent" }
            ]
        });

        struct CheckDeviceTransport {
            calls: AtomicUsize,
            enroll_body: Vec<u8>,
            dispatch_body: Vec<u8>,
            dispatched_tokens: Mutex<Vec<String>>,
        }

        impl RelayTransport for CheckDeviceTransport {
            fn post_json(
                &self,
                url: &str,
                body: &[u8],
                _token: Option<&str>,
            ) -> Result<RelayReply, RelayFault> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if url.ends_with("/relay-token") {
                    Ok(RelayReply {
                        status: 200,
                        body: self.enroll_body.clone(),
                    })
                } else {
                    let parsed: Value = serde_json::from_slice(body).unwrap();
                    let tokens: Vec<String> = parsed["devices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|d| d["token"].as_str().unwrap().to_owned())
                        .collect();
                    *self.dispatched_tokens.lock().unwrap() = tokens;
                    Ok(RelayReply {
                        status: 200,
                        body: self.dispatch_body.clone(),
                    })
                }
            }

            fn post_bytes(
                &self,
                _url: &str,
                _headers: &[(&str, &str)],
                _body: &[u8],
            ) -> Result<u16, RelayFault> {
                Ok(200)
            }
        }

        let transport = CheckDeviceTransport {
            calls: AtomicUsize::new(0),
            enroll_body: serde_json::to_vec(&enroll_resp).unwrap(),
            dispatch_body: serde_json::to_vec(&dispatch_resp).unwrap(),
            dispatched_tokens: Mutex::new(Vec::new()),
        };

        let now = utc_dt(2026, 8, 16, 19, 0, 0);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );

        let rec_path = record_path(root.path(), "20260809");
        assert!(rec_path.exists());
        let rec: WeeklyHeadsUpRecord =
            serde_json::from_str(&fs::read_to_string(rec_path).unwrap()).unwrap();
        assert_eq!(rec.state, OnceState::Done);

        let sent = transport.dispatched_tokens.lock().unwrap().clone();
        assert_eq!(sent, vec![token_30_days.to_owned()]);
    }

    #[test]
    fn registry_directory_read_error_retries_with_reselect_and_sends_after_repair() {
        let root = setup_test_journal();
        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token = "0123456789abcdef";
        let reg_time = "2026-08-10T12:00:00Z";

        // Push registry is a directory -> read error
        fs::create_dir_all(root.path().join("config/push-registry.json")).unwrap();

        let ca_dir = root.path().join("link/ca");
        fs::create_dir_all(&ca_dir).unwrap();
        let ca = solstone_core_sol_link::ca::generate_ca().expect("ca");
        let spki_der = ca.spki_der().to_vec();
        let instance_id = solstone_core_sol_link::ca::jid_from_spki(&spki_der).expect("jid");
        fs::write(ca_dir.join("cert.pem"), ca.certificate_pem()).unwrap();
        fs::write(ca_dir.join("private.pem"), ca.private_key_pem()).unwrap();
        fs::write(
            root.path().join("link/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"Test Home"}}"#),
        )
        .unwrap();

        let mut ledger = solstone_core_sol_link::ledger::AuthorizationLedger::new(root.path());
        let entry = solstone_core_sol_link::ledger::ClientEntry::new(
            cid,
            "Test Device",
            "2026-08-01T00:00:00Z",
            "inst-1",
            solstone_core_sol_link::ledger::ClientRole::Roleless,
        );
        let _ = ledger.add(entry);

        write_week_file(root.path(), "20260809", 1, None);

        let enroll_resp = serde_json::json!({
            "token": "valid_bearer_token",
            "token_type": "Bearer",
            "instance_id": instance_id
        });
        let dispatch_resp = serde_json::json!({
            "results": [
                { "token": token, "outcome": "sent" }
            ]
        });

        struct RegistryErrorTransport {
            calls: AtomicUsize,
            enroll_body: Vec<u8>,
            dispatch_body: Vec<u8>,
        }

        impl RelayTransport for RegistryErrorTransport {
            fn post_json(
                &self,
                url: &str,
                _body: &[u8],
                _token: Option<&str>,
            ) -> Result<RelayReply, RelayFault> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if url.ends_with("/relay-token") {
                    Ok(RelayReply {
                        status: 200,
                        body: self.enroll_body.clone(),
                    })
                } else {
                    Ok(RelayReply {
                        status: 200,
                        body: self.dispatch_body.clone(),
                    })
                }
            }

            fn post_bytes(
                &self,
                _url: &str,
                _headers: &[(&str, &str)],
                _body: &[u8],
            ) -> Result<u16, RelayFault> {
                Ok(200)
            }
        }

        let transport = RegistryErrorTransport {
            calls: AtomicUsize::new(0),
            enroll_body: serde_json::to_vec(&enroll_resp).unwrap(),
            dispatch_body: serde_json::to_vec(&dispatch_resp).unwrap(),
        };

        // 12:30 Phoenix = 19:30 UTC -> observe during window
        let now = utc_dt(2026, 8, 16, 19, 30, 0);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );

        let rec_path = record_path(root.path(), "20260809");
        assert!(rec_path.exists());
        let rec: WeeklyHeadsUpRecord =
            serde_json::from_str(&fs::read_to_string(&rec_path).unwrap()).unwrap();
        assert_eq!(rec.state, OnceState::Retry);
        assert_eq!(rec.reselect, true);
        assert!(rec.failed.is_empty());
        assert_eq!(rec.due, "2026-08-16T20:00:00Z"); // next hour
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);

        // Repair registry
        fs::remove_dir(root.path().join("config/push-registry.json")).unwrap();
        let reg = serde_json::json!({
            "version": 2,
            "devices": [
                {
                    "platform": "ios",
                    "cid": cid,
                    "device_token": token,
                    "bundle_id": "com.solstone.test",
                    "environment": "development",
                    "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
                    "registered_at": reg_time
                }
            ]
        });
        fs::write(
            root.path().join("config/push-registry.json"),
            serde_json::to_string(&reg).unwrap(),
        )
        .unwrap();

        // Poll at due time (20:00:00Z)
        let t_due = utc_dt(2026, 8, 16, 20, 0, 0);
        poll_with_transport(root.path(), "https://portal.test", t_due, &transport);

        let rec2: WeeklyHeadsUpRecord =
            serde_json::from_str(&fs::read_to_string(&rec_path).unwrap()).unwrap();
        assert_eq!(rec2.state, OnceState::Done);
        let calls_after_send = transport.calls.load(Ordering::SeqCst);
        assert!(calls_after_send > 0);

        // Later poll at 21:00:00Z does not send
        let t_later = utc_dt(2026, 8, 16, 21, 0, 0);
        poll_with_transport(root.path(), "https://portal.test", t_later, &transport);
        assert_eq!(transport.calls.load(Ordering::SeqCst), calls_after_send);
    }

    #[test]
    fn android_device_410_revoked_ends_done_later_poll_does_not_call_post_bytes() {
        let root = setup_test_journal();
        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let reg_time = "2026-08-10T12:00:00Z";

        let ca_dir = root.path().join("link/ca");
        fs::create_dir_all(&ca_dir).unwrap();
        let ca = solstone_core_sol_link::ca::generate_ca().expect("ca");
        let spki_der = ca.spki_der().to_vec();
        let instance_id = solstone_core_sol_link::ca::jid_from_spki(&spki_der).expect("jid");
        fs::write(ca_dir.join("cert.pem"), ca.certificate_pem()).unwrap();
        fs::write(ca_dir.join("private.pem"), ca.private_key_pem()).unwrap();
        fs::write(
            root.path().join("link/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"Test Home"}}"#),
        )
        .unwrap();

        let mut ledger = solstone_core_sol_link::ledger::AuthorizationLedger::new(root.path());
        let entry = solstone_core_sol_link::ledger::ClientEntry::new(
            cid,
            "Android Device",
            "2026-08-01T00:00:00Z",
            "inst-1",
            solstone_core_sol_link::ledger::ClientRole::Roleless,
        );
        let _ = ledger.add(entry);

        crate::vapid::create_vapid_key(root.path(), "2026-08-01T00:00:00Z".to_owned()).unwrap();

        let reg = serde_json::json!({
            "version": 2,
            "devices": [
                {
                    "platform": "android",
                    "cid": cid,
                    "endpoint": "https://push.example.com/sub/android1",
                    "p256dh": "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
                    "auth": "BTBZMqHH6r4Tts7J_aSIgg",
                    "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
                    "registered_at": reg_time
                }
            ]
        });
        fs::write(
            root.path().join("config/push-registry.json"),
            serde_json::to_string(&reg).unwrap(),
        )
        .unwrap();
        write_week_file(root.path(), "20260809", 1, None);

        struct Android410Transport {
            post_bytes_calls: AtomicUsize,
        }

        impl RelayTransport for Android410Transport {
            fn post_json(
                &self,
                _url: &str,
                _body: &[u8],
                _token: Option<&str>,
            ) -> Result<RelayReply, RelayFault> {
                panic!("post_json should not be called for android web push");
            }

            fn post_bytes(
                &self,
                _url: &str,
                _headers: &[(&str, &str)],
                _body: &[u8],
            ) -> Result<u16, RelayFault> {
                self.post_bytes_calls.fetch_add(1, Ordering::SeqCst);
                Ok(410)
            }
        }

        let transport = Android410Transport {
            post_bytes_calls: AtomicUsize::new(0),
        };

        // 12:00 Phoenix = 19:00 UTC
        let now = utc_dt(2026, 8, 16, 19, 0, 0);
        observe_with_transport(
            root.path(),
            "https://portal.test",
            "weekly_reflection",
            "20260809",
            now,
            &transport,
        );

        let rec_path = record_path(root.path(), "20260809");
        assert!(rec_path.exists());
        let rec: WeeklyHeadsUpRecord =
            serde_json::from_str(&fs::read_to_string(&rec_path).unwrap()).unwrap();
        assert_eq!(rec.state, OnceState::Done);
        assert_eq!(transport.post_bytes_calls.load(Ordering::SeqCst), 1);

        // Later poll does not call post_bytes
        let later = utc_dt(2026, 8, 16, 20, 0, 0);
        poll_with_transport(root.path(), "https://portal.test", later, &transport);
        assert_eq!(transport.post_bytes_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_scan_startup() {
        let root = setup_test_journal();
        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token = "0123456789abcdef";
        let reg_time = "2026-08-10T12:00:00Z";
        write_device_and_ledger(root.path(), cid, token, reg_time);
        write_week_file(root.path(), "20260809", 1, None);

        let talents_dir = root.path().join("talents/weekly_reflection");
        fs::create_dir_all(&talents_dir).unwrap();

        // Write finished log and active log
        let log_content = "{\"event\":\"start\",\"day\":\"20260809\",\"name\":\"weekly_reflection\"}\n{\"event\":\"finish\",\"day\":\"20260809\",\"name\":\"weekly_reflection\",\"output\":\"done\"}\n";
        fs::write(talents_dir.join("1786800000000.jsonl"), log_content).unwrap();
        fs::write(talents_dir.join("1786800000000_active.jsonl"), log_content).unwrap();

        let transport = MockTransport::new(200, vec![]);
        // Now at 14:00 MST = 21:00 UTC
        let now = utc_dt(2026, 8, 16, 21, 0, 0);

        scan_with_transport(root.path(), "https://portal.test", now, &transport);

        assert!(record_path(root.path(), "20260809").exists());
    }
}
