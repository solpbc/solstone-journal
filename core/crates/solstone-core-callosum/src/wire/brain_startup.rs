// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A bounded selected-source prefix owned by one embedded Brain generation.

use crate::CallosumEnvelope;
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

const ROWS: usize = 128;
const BYTES: usize = 64 * 1024;
const ROW_BYTES: usize = 1024;
const JS_INTEGER: u64 = 9_007_199_254_740_991;
const UNAVAILABLE: u64 = 1;
const SOURCE_LOSS: u64 = 2;
const CONTENTION: u64 = 4;
const OVERFLOW: u64 = 8;
const MALFORMED: u64 = 16;
const IDENTITY: u64 = 32;

struct Prefix {
    rows: Vec<Vec<u8>>,
    bytes: usize,
}

/// No execution resource or transport ownership. Writers never wait for readers.
pub struct BrainStartupCapture {
    generation: Option<String>,
    prefix: Mutex<Prefix>,
    loss: AtomicU64,
    pending: AtomicUsize,
    closed: AtomicBool,
    changed: Notify,
}

/// Only the private response's position in its own selected-source prefix.
#[derive(Default)]
pub struct BrainStartupCursor {
    next: usize,
    marker: Option<(usize, u64, usize, bool)>,
    contended: bool,
}

pub struct BrainStartupEvent {
    pub name: &'static str,
    pub data: String,
    pub terminal: bool,
}

impl BrainStartupCapture {
    pub fn new() -> Arc<Self> {
        let mut identity = [0; 16];
        let generation = getrandom::fill(&mut identity).ok().and_then(|()| {
            let mut generation = String::new();
            generation.try_reserve_exact(32).ok()?;
            const HEX: &[u8; 16] = b"0123456789abcdef";
            for byte in identity {
                generation.push(char::from(HEX[usize::from(byte >> 4)]));
                generation.push(char::from(HEX[usize::from(byte & 15)]));
            }
            Some(generation)
        });
        Self::with_generation(generation, false)
    }

    pub(super) fn with_generation(generation: Option<String>, unavailable: bool) -> Arc<Self> {
        let mut rows = Vec::new();
        let unavailable = unavailable || rows.try_reserve_exact(ROWS).is_err();
        let loss = if unavailable || generation.is_none() {
            UNAVAILABLE
        } else {
            0
        };
        Arc::new(Self {
            generation,
            prefix: Mutex::new(Prefix { rows, bytes: 0 }),
            loss: AtomicU64::new(loss),
            pending: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            changed: Notify::new(),
        })
    }

    fn invalidate(&self, reason: u64) {
        self.loss.fetch_or(reason, Ordering::AcqRel);
        self.changed.notify_waiters();
    }

    /// An upstream optional identity could not be made. Never invent its relation.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn unavailable_for_test(allocation_failure: bool) -> Arc<Self> {
        Self::with_generation(
            allocation_failure.then(|| "a".repeat(32)),
            allocation_failure,
        )
    }

    pub(super) fn missing_emission(&self, tract: &str, event: &str) {
        if tract == "supervisor"
            && matches!(
                event,
                "brain_observation" | "started" | "stopped" | "skipped"
            )
        {
            self.invalidate(SOURCE_LOSS);
        }
    }

    pub fn missing_identity(&self) {
        self.invalidate(IDENTITY);
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }

    pub(super) fn prepare(self: &Arc<Self>, row: &CallosumEnvelope) -> Option<Pending> {
        if !selected(row) {
            return None;
        }
        self.pending.fetch_add(1, Ordering::AcqRel);
        let data = match project(row) {
            Some(data) => Some(data),
            None => {
                self.invalidate(MALFORMED);
                None
            }
        };
        Some(Pending {
            capture: Arc::clone(self),
            data,
            finalized: false,
        })
    }

    /// Notification is armed before inspecting the shared prefix: there is no
    /// replay-to-live registration window and no additional receiver task.
    pub async fn next(&self, cursor: &mut BrainStartupCursor) -> Option<BrainStartupEvent> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(event) = self.try_next(cursor) {
                return Some(event);
            }
            if cursor.contended {
                // Another response can briefly own this lock without emitting
                // or notifying. Retry cooperatively instead of awaiting a
                // source change that may never occur for a settled prefix.
                tokio::task::yield_now().await;
                continue;
            }
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            changed.await;
        }
    }

    fn try_next(&self, cursor: &mut BrainStartupCursor) -> Option<BrainStartupEvent> {
        cursor.contended = false;
        let closed = self.closed.load(Ordering::Acquire);
        if closed {
            return self.marker(cursor, cursor.next, true);
        }
        let prefix = match self.prefix.try_lock() {
            Ok(prefix) => prefix,
            Err(std::sync::TryLockError::WouldBlock) => {
                cursor.contended = true;
                return None;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.invalidate(UNAVAILABLE);
                self.close();
                return self.marker(cursor, cursor.next, true);
            }
        };
        if !closed && let Some(row) = prefix.rows.get(cursor.next) {
            // The retained row is already bounded and valid JSON. This allocation
            // belongs to the reader; producer try_lock never waits for it.
            let row: Value = serde_json::from_slice(row).expect("selected source JSON");
            cursor.next += 1;
            return Some(BrainStartupEvent {
                name: "brain_startup",
                data: json!({"generation":self.generation,"sequence":cursor.next,"row":row})
                    .to_string(),
                terminal: false,
            });
        }
        self.marker(cursor, prefix.rows.len(), closed)
    }

    fn marker(
        &self,
        cursor: &mut BrainStartupCursor,
        through: usize,
        closed: bool,
    ) -> Option<BrainStartupEvent> {
        // A finalized send sets loss before releasing pending. Acquire of zero
        // therefore cannot certify a send whose failure has not yet been read.
        let closed = closed || self.closed.load(Ordering::Acquire);
        let pending = self.pending.load(Ordering::Acquire);
        let loss = self.loss.load(Ordering::Acquire);
        let status = (through, loss, pending, closed);
        if cursor.marker == Some(status) {
            return None;
        }
        cursor.marker = Some(status);
        let coverage = if closed {
            "closed"
        } else if loss & UNAVAILABLE != 0 {
            "unavailable"
        } else if loss != 0 {
            "incomplete"
        } else if pending != 0 {
            "pending"
        } else {
            "complete"
        };
        let reasons: Vec<_> = [
            (UNAVAILABLE, "unavailable"),
            (SOURCE_LOSS, "source_send_loss"),
            (CONTENTION, "contention"),
            (OVERFLOW, "overflow"),
            (MALFORMED, "malformed"),
            (IDENTITY, "identity_unavailable"),
        ]
        .into_iter()
        .filter_map(|(bit, name)| (loss & bit != 0).then_some(name))
        .collect();
        Some(BrainStartupEvent {
            name: "brain_startup_continuity",
            data: json!({"generation":self.generation,"through":through,"coverage":coverage,
                "scope":"selected_source_emission","reasons":reasons,"pending":pending})
            .to_string(),
            terminal: closed,
        })
    }
}

pub(super) struct Pending {
    capture: Arc<BrainStartupCapture>,
    data: Option<FixedRow>,
    finalized: bool,
}

impl Pending {
    pub(super) fn finish(mut self, sent: bool) {
        if !sent {
            self.capture.invalidate(SOURCE_LOSS);
        }
        if self.capture.closed.load(Ordering::Acquire) {
            self.finalized = true;
            return;
        }
        if let Some(data) = self.data.take() {
            match self.capture.prefix.try_lock() {
                Ok(mut prefix) => {
                    if prefix.rows.len() >= ROWS || prefix.bytes + data.len > BYTES {
                        self.capture.invalidate(OVERFLOW);
                    } else if self.capture.loss.load(Ordering::Acquire) & UNAVAILABLE == 0 {
                        let mut row = Vec::new();
                        if row.try_reserve_exact(data.len).is_err() {
                            self.capture.invalidate(UNAVAILABLE);
                        } else {
                            row.extend_from_slice(&data.bytes[..data.len]);
                            prefix.bytes += row.len();
                            prefix.rows.push(row);
                        }
                    }
                }
                Err(std::sync::TryLockError::WouldBlock) => self.capture.invalidate(CONTENTION),
                Err(std::sync::TryLockError::Poisoned(_)) => self.capture.invalidate(UNAVAILABLE),
            }
        }
        self.finalized = true;
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.finalized {
            self.capture.invalidate(SOURCE_LOSS);
        }
        self.capture.pending.fetch_sub(1, Ordering::AcqRel);
        self.capture.changed.notify_waiters();
    }
}

fn selected(row: &CallosumEnvelope) -> bool {
    row.tract == "supervisor"
        && (row.event == "brain_observation"
            || matches!(row.event.as_str(), "started" | "stopped" | "skipped"))
}

fn opaque_str(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn opaque(value: &Value) -> bool {
    value.as_str().is_some_and(opaque_str)
}

fn reference(value: &Value) -> bool {
    let Some(s) = value.as_str() else {
        return false;
    };
    if s.len() > 128 {
        return false;
    }
    if let Some(id) = s.strip_prefix("spp-renewal-") {
        return opaque_str(id);
    }
    let Some(tail) = s.strip_prefix("embedded-brain-") else {
        return false;
    };
    let Some((marker, sequence)) = tail.split_once('-') else {
        return false;
    };
    [marker, sequence].into_iter().all(|part| {
        !part.starts_with('0')
            && part.bytes().all(|b| b.is_ascii_digit())
            && part.parse::<u64>().is_ok_and(|n| n > 0)
    })
}

const FIELDS: &[&str] = &[
    "observation_id",
    "caller",
    "phase",
    "outcome",
    "failure_category",
    "run_id",
    "checking_revision",
    "revision",
    "reason_code",
    "verification_reason",
    "runtime_failure_marker",
    "ref",
    "service",
    "exit_code",
    "reason",
    "active_ref",
];

struct Selected<'a>(&'a CallosumEnvelope);
impl Serialize for Selected<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("tract", "supervisor")?;
        map.serialize_entry("event", &self.0.event)?;
        for key in FIELDS {
            if let Some(value) = self.0.extra.get(*key) {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

struct FixedRow {
    bytes: [u8; ROW_BYTES],
    len: usize,
}
impl Write for FixedRow {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > ROW_BYTES - self.len {
            return Err(io::Error::other("selected row too large"));
        }
        self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn project(row: &CallosumEnvelope) -> Option<FixedRow> {
    if !opaque(row.extra.get("observation_id")?) {
        return None;
    }
    if row.event == "brain_observation" {
        for key in ["caller", "phase", "outcome"] {
            row.extra.get(key)?.as_str()?;
        }
    } else {
        if row.extra.get("service")?.as_str()? != "brain" {
            return None;
        }
        if row.event == "stopped" {
            row.extra.get("exit_code")?.as_i64()?;
        }
        if !reference(row.extra.get("ref")?) {
            return None;
        }
        if row.event == "skipped" && row.extra.get("reason")?.as_str()? != "busy" {
            return None;
        }
        if let Some(active) = row.extra.get("active_ref")
            && active.as_str() != Some("")
            && !reference(active)
        {
            return None;
        }
    }
    for key in FIELDS {
        if let Some(value) = row.extra.get(*key) {
            let valid = match *key {
                "observation_id" | "run_id" | "runtime_failure_marker" => opaque(value),
                "revision" | "checking_revision" => value.as_u64().is_some_and(|n| n <= JS_INTEGER),
                "exit_code" => value.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
                "caller"
                | "phase"
                | "outcome"
                | "failure_category"
                | "reason_code"
                | "verification_reason" => value.as_str().is_some_and(|value| {
                    include_str!("../../../../fixtures/brain_observation_vocabulary.tsv")
                        .lines()
                        .any(|line| line.split_once('\t') == Some((*key, value)))
                }),
                "ref" => reference(value),
                "active_ref" => value.as_str() == Some("") || reference(value),
                "service" => value.as_str() == Some("brain"),
                "reason" => value.as_str() == Some("busy"),
                _ => false,
            };
            if !valid {
                return None;
            }
        }
    }
    let mut data = FixedRow {
        bytes: [0; ROW_BYTES],
        len: 0,
    };
    serde_json::to_writer(&mut data, &Selected(row)).ok()?;
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    fn row(event: &str) -> CallosumEnvelope {
        serde_json::from_value(json!({"tract":"supervisor","event":event,
            "observation_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "service":"brain",
            "ref":"embedded-brain-1-1", "exit_code":3}))
        .unwrap()
    }
    fn take(capture: &BrainStartupCapture, cursor: &mut BrainStartupCursor) -> Value {
        serde_json::from_str(&capture.try_next(cursor).unwrap().data).unwrap()
    }
    #[tokio::test]
    async fn reader_lock_contention_retries_without_another_source_emission() {
        let capture = BrainStartupCapture::new();
        capture.prepare(&row("started")).unwrap().finish(true);
        let held = capture.prefix.lock().unwrap();
        let mut cursor = BrainStartupCursor::default();
        let mut next = Box::pin(capture.next(&mut cursor));
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        assert!(next.as_mut().poll(&mut context).is_pending());
        drop(held);
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), next)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&event.data).unwrap()["sequence"],
            1
        );
    }
    #[test]
    fn delayed_cursor_and_pending_send_have_distinct_watermarks() {
        let capture = BrainStartupCapture::new();
        let mut cursor = BrainStartupCursor::default();
        let pending = capture.prepare(&row("started")).unwrap();
        assert_eq!(take(&capture, &mut cursor)["coverage"], "pending");
        pending.finish(true);
        assert_eq!(take(&capture, &mut cursor)["sequence"], 1);
        assert_eq!(take(&capture, &mut cursor)["coverage"], "complete");
        let pending = capture.prepare(&row("stopped")).unwrap();
        let unresolved = take(&capture, &mut cursor);
        assert_eq!(unresolved["through"], 1);
        assert_eq!(unresolved["coverage"], "pending");
        pending.finish(false);
        assert_eq!(take(&capture, &mut cursor)["sequence"], 2);
        assert_eq!(take(&capture, &mut cursor)["coverage"], "incomplete");
        let mut reconnect = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut reconnect)["sequence"], 1);
    }
    #[test]
    fn rejects_caller_text_without_retaining_it_and_keeps_loss_sticky() {
        for (key, value) in [
            ("ref", json!("private_token")),
            ("service", json!("private_token")),
            ("active_ref", json!("private_token")),
            ("reason", json!("private_token")),
            ("observation_id", json!("wrong")),
            ("revision", json!(9_007_199_254_740_992_u64)),
        ] {
            let capture = BrainStartupCapture::new();
            let mut bad = row("started");
            bad.extra.insert(key.into(), value);
            capture.prepare(&bad).unwrap().finish(true);
            capture.prepare(&row("started")).unwrap().finish(true);
            let prefix = capture.prefix.lock().unwrap();
            assert_eq!(prefix.rows.len(), 1);
            assert!(!String::from_utf8_lossy(&prefix.rows[0]).contains("private_token"));
            drop(prefix);
            let mut cursor = BrainStartupCursor::default();
            take(&capture, &mut cursor);
            assert_eq!(take(&capture, &mut cursor)["coverage"], "incomplete");
        }
        assert!(reference(&json!(
            "spp-renewal-0123456789abcdef0123456789abcdef"
        )));
        for reference_value in [
            "embedded-brain-0-1",
            "embedded-brain-1-0",
            "embedded-brain-01-2",
            "spp-renewal-secret",
            "embedded-brain-1-2-extra",
        ] {
            assert!(!reference(&json!(reference_value)));
        }
    }
    #[test]
    fn contention_overflow_and_unavailable_never_recover_to_complete() {
        let capture = BrainStartupCapture::new();
        let held = capture.prefix.lock().unwrap();
        capture.prepare(&row("started")).unwrap().finish(true);
        drop(held);
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut cursor)["coverage"], "incomplete");
        let capture = BrainStartupCapture::new();
        for _ in 0..129 {
            capture.prepare(&row("started")).unwrap().finish(true);
        }
        let prefix = capture.prefix.lock().unwrap();
        assert_eq!(prefix.rows.len(), 128);
        assert!(prefix.bytes <= 65536);
        drop(prefix);
        assert_ne!(capture.loss.load(Ordering::Acquire) & OVERFLOW, 0);
        for capture in [
            BrainStartupCapture::with_generation(None, false),
            BrainStartupCapture::with_generation(Some("a".repeat(32)), true),
        ] {
            capture.prepare(&row("started")).unwrap().finish(true);
            let mut cursor = BrainStartupCursor::default();
            assert_eq!(take(&capture, &mut cursor)["coverage"], "unavailable");
        }
    }
    #[test]
    fn close_supersedes_undelivered_rows_and_new_runtime_has_its_own_identity() {
        let capture = BrainStartupCapture::new();
        capture.prepare(&row("started")).unwrap().finish(true);
        capture.close();
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut cursor)["coverage"], "closed");
        let next = BrainStartupCapture::new();
        assert_ne!(capture.generation, next.generation);
        assert_eq!(
            take(&next, &mut BrainStartupCursor::default())["through"],
            0
        );
    }
    #[test]
    fn exact_numeric_boundary_and_bounded_unknown_extras() {
        let mut original = row("started");
        original
            .extra
            .insert("revision".into(), json!(9_007_199_254_740_991_u64));
        original.extra.insert("secret".into(), json!("excluded"));
        let projection = project(&original).unwrap();
        let value: Value = serde_json::from_slice(&projection.bytes[..projection.len]).unwrap();
        assert_eq!(value["revision"], 9_007_199_254_740_991_u64);
        assert!(value.get("secret").is_none());
        original
            .extra
            .insert("revision".into(), json!(9_007_199_254_740_992_u64));
        assert!(project(&original).is_none());
    }
    #[tokio::test]
    async fn cursor_notification_and_closure_cover_the_handoff() {
        let capture = BrainStartupCapture::new();
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut cursor)["coverage"], "complete");
        let mut next = Box::pin(capture.next(&mut cursor));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(next.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        capture.prepare(&row("started")).unwrap().finish(true);
        assert_eq!(next.await.unwrap().name, "brain_startup");
        assert_eq!(take(&capture, &mut cursor)["coverage"], "complete");
        let mut next = Box::pin(capture.next(&mut cursor));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(next.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        capture.close();
        assert!(next.await.unwrap().terminal);
    }
    #[test]
    fn byte_budget_and_identity_failure_are_sticky() {
        let capture = BrainStartupCapture::new();
        capture.prefix.lock().unwrap().bytes = BYTES - 1;
        capture.prepare(&row("started")).unwrap().finish(true);
        assert_eq!(capture.prefix.lock().unwrap().rows.len(), 0);
        assert_ne!(capture.loss.load(Ordering::Acquire) & OVERFLOW, 0);
        let capture = BrainStartupCapture::new();
        capture.missing_identity();
        assert_eq!(
            take(&capture, &mut BrainStartupCursor::default())["coverage"],
            "incomplete"
        );
    }
}
