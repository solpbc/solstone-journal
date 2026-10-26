// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Weekly heads-up background subscriber in convey-shell.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumEnvelope, CallosumReceiveEvent, CallosumSocketConnection,
};
use solstone_core_push::weekly;
use time::OffsetDateTime;

const RECONNECT_PAUSE: Duration = Duration::from_secs(5);
static NEXT_SUBSCRIBER: AtomicU64 = AtomicU64::new(1);

pub(crate) async fn subscribe_weekly_heads_up(journal_root: PathBuf, portal_base: String) {
    let (_owner, stop) = tokio::sync::watch::channel(false);
    subscribe_weekly_heads_up_until_stopped(journal_root, portal_base, stop, Arc::new(|| true))
        .await;
}

/// Retain and await blocking passes: aborting their async parent would detach
/// delivery from the embedding owner's checked Stop.
pub(crate) async fn subscribe_weekly_heads_up_until_stopped(
    journal_root: PathBuf,
    portal_base: String,
    mut stop: tokio::sync::watch::Receiver<bool>,
    active: Arc<dyn Fn() -> bool + Send + Sync>,
) {
    if *stop.borrow() || !active() {
        return;
    }
    // Subscribe before catch-up can block on a delivery. Re-scan on each
    // reflected subscription epoch as well, covering completions before
    // registration and events lost during a later connection gap.
    let marker = NEXT_SUBSCRIBER.fetch_add(1, Ordering::Relaxed);
    let mut connection = Some(connect(&journal_root));
    let journal_scan = journal_root.clone();
    let portal_scan = portal_base.clone();
    let scan_active = Arc::clone(&active);
    let scan_stop = stop.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || {
        if !*scan_stop.borrow() && scan_active() {
            let now = OffsetDateTime::now_utc();
            weekly::scan(&journal_scan, &portal_scan, now);
        }
    })
    .await
    {
        log::warn!("weekly heads-up scan: {error}");
        return;
    }

    loop {
        if *stop.borrow() || !active() {
            return;
        }
        let journal_due = journal_root.clone();
        let next_due = match tokio::task::spawn_blocking(move || {
            let now = OffsetDateTime::now_utc();
            weekly::next_due(&journal_due, now)
        })
        .await
        {
            Ok(due) => due,
            Err(error) => {
                log::warn!("weekly heads-up due read: {error}");
                return;
            }
        };

        let now = OffsetDateTime::now_utc();
        let due_sleep = next_due.map(|due| duration_until(due, now));
        let reconnecting = connection.is_none();

        tokio::select! {
            _ = stop.changed() => return,
            message = recv_event(connection.as_mut()) => {
                let envelope = match message {
                    Some(CallosumReceiveEvent::Envelope { envelope, .. }) => envelope,
                    Some(CallosumReceiveEvent::Continuity {
                        phase: CallosumConnectionPhase::Connected, ..
                    }) => {
                        if let Some(connection) = connection.as_ref() {
                            connection.emit("convey", "subscriber-ready", Map::from_iter([
                                ("weekly_heads_up_start".into(), json!(marker)),
                            ]));
                        }
                        continue;
                    }
                    Some(CallosumReceiveEvent::Continuity { .. }) => continue,
                    None => {
                        connection = None;
                        continue;
                    }
                };
                if envelope.tract == "convey" && envelope.event == "subscriber-ready"
                    && envelope.extra.get("weekly_heads_up_start") == Some(&json!(marker))
                {
                    // Reflection proves the server registered this listener;
                    // client-side Connected alone does not prove that.
                    let journal = journal_root.clone();
                    let portal = portal_base.clone();
                    let active = Arc::clone(&active);
                    let stop = stop.clone();
                    if let Err(error) = tokio::task::spawn_blocking(move || {
                        if *stop.borrow() || !active() { return; }
                        weekly::scan(&journal, &portal, OffsetDateTime::now_utc());
                    }).await {
                        log::warn!("weekly heads-up reconnect scan: {error}");
                        return;
                    }
                    continue;
                }
                if !is_weekly_finish(&envelope) {
                    continue;
                }
                let Some(day) = finish_day(&envelope) else {
                    continue;
                };
                let journal = journal_root.clone();
                let portal = portal_base.clone();
                let active = Arc::clone(&active);
                let stop = stop.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || {
                    if *stop.borrow() || !active() { return; }
                    let now = OffsetDateTime::now_utc();
                    weekly::observe(&journal, &portal, "weekly_reflection", &day, now);
                }).await {
                    log::warn!("weekly heads-up completion: {error}");
                    return;
                }
            }
            _ = tokio::time::sleep(due_sleep.unwrap_or(Duration::ZERO)), if due_sleep.is_some() => {
                let journal = journal_root.clone();
                let portal = portal_base.clone();
                let active = Arc::clone(&active);
                let stop = stop.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || {
                    if *stop.borrow() || !active() { return; }
                    let now = OffsetDateTime::now_utc();
                    weekly::poll(&journal, &portal, now);
                }).await {
                    log::warn!("weekly heads-up delivery: {error}");
                    return;
                }
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

async fn recv_event(
    connection: Option<&mut CallosumSocketConnection>,
) -> Option<CallosumReceiveEvent> {
    match connection {
        Some(connection) => connection.next_event().await,
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
