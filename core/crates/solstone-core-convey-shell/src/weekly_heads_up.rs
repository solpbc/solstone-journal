// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Weekly heads-up background subscriber in convey-shell.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumSocketConnection};
use solstone_core_push::weekly;
use time::OffsetDateTime;

const RECONNECT_PAUSE: Duration = Duration::from_secs(5);

pub(crate) async fn subscribe_weekly_heads_up(journal_root: PathBuf, portal_base: String) {
    let journal_scan = journal_root.clone();
    let portal_scan = portal_base.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let now = OffsetDateTime::now_utc();
        weekly::scan(&journal_scan, &portal_scan, now);
    })
    .await;

    let mut connection = Some(connect(&journal_root));

    loop {
        let journal_due = journal_root.clone();
        let next_due = tokio::task::spawn_blocking(move || {
            let now = OffsetDateTime::now_utc();
            weekly::next_due(&journal_due, now)
        })
        .await
        .unwrap_or(None);

        let now = OffsetDateTime::now_utc();
        let due_sleep = next_due.map(|due| duration_until(due, now));
        let reconnecting = connection.is_none();

        tokio::select! {
            message = recv_envelope(connection.as_mut()) => {
                let Some(envelope) = message else {
                    connection = None;
                    continue;
                };
                if !is_weekly_finish(&envelope) {
                    continue;
                }
                let Some(day) = finish_day(&envelope) else {
                    continue;
                };
                let journal = journal_root.clone();
                let portal = portal_base.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let now = OffsetDateTime::now_utc();
                    weekly::observe(&journal, &portal, "weekly_reflection", &day, now);
                })
                .await;
            }
            _ = tokio::time::sleep(due_sleep.unwrap_or(Duration::ZERO)), if due_sleep.is_some() => {
                let journal = journal_root.clone();
                let portal = portal_base.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let now = OffsetDateTime::now_utc();
                    weekly::poll(&journal, &portal, now);
                })
                .await;
            }
            _ = tokio::time::sleep(RECONNECT_PAUSE), if reconnecting => {
                connection = Some(connect(&journal_root));
            }
        }
    }
}

fn connect(journal_root: &Path) -> CallosumSocketConnection {
    let mut connection =
        CallosumSocketConnection::new(journal_root.join("health/callosum.sock"), Map::new());
    connection.start();
    connection
}

async fn recv_envelope(
    connection: Option<&mut CallosumSocketConnection>,
) -> Option<CallosumEnvelope> {
    match connection {
        Some(connection) => connection.next_message().await,
        None => std::future::pending().await,
    }
}

fn duration_until(due: OffsetDateTime, now: OffsetDateTime) -> Duration {
    if due <= now {
        return Duration::ZERO;
    }
    let diff = due - now;
    let seconds = u64::try_from(diff.whole_seconds()).unwrap_or(u64::MAX);
    let nanos = u32::try_from(diff.subsec_nanoseconds().max(0)).unwrap_or(0);
    Duration::new(seconds, nanos)
}

fn is_weekly_finish(envelope: &CallosumEnvelope) -> bool {
    envelope.tract == "cortex"
        && envelope.event == "finish"
        && envelope.extra.get("name").and_then(Value::as_str) == Some("weekly_reflection")
}

fn finish_day(envelope: &CallosumEnvelope) -> Option<String> {
    let day = envelope.extra.get("day").and_then(Value::as_str)?;
    if day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()) {
        Some(day.to_owned())
    } else {
        None
    }
}
