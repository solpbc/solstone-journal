// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A bounded selected-source prefix owned by one embedded Brain generation.

use crate::CallosumEnvelope;
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::sync::{
    Arc, OnceLock,
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

// One monotonic reservation charges both limits. Readers never own writer state.
const BYTE_BITS: u32 = 17;
const BYTE_MASK: u64 = (1 << BYTE_BITS) - 1;
struct Prefix {
    slots: Vec<OnceLock<Option<Vec<u8>>>>,
    reserved: AtomicU64,
}
impl Prefix {
    fn horizon(&self) -> usize {
        (self.reserved.load(Ordering::Acquire) >> BYTE_BITS) as usize
    }
    fn reserve(&self, bytes: usize) -> Option<usize> {
        let mut state = self.reserved.load(Ordering::Acquire);
        loop {
            let count = (state >> BYTE_BITS) as usize;
            let charged = (state & BYTE_MASK) as usize;
            if count >= ROWS || charged + bytes > BYTES {
                return None;
            }
            let next = (((count + 1) as u64) << BYTE_BITS) | (charged + bytes) as u64;
            match self
                .reserved
                .compare_exchange(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Some(count),
                // Strong CAS fails only when another of at most 128 reservations
                // advances. No reset, refund or other writer changes this state.
                Err(actual) => state = actual,
            }
        }
    }
}

/// No execution resource or transport ownership. Writers never wait for readers.
pub struct BrainStartupCapture {
    generation: Option<String>,
    prefix: Prefix,
    loss: AtomicU64,
    pending: AtomicUsize,
    closed: AtomicBool,
    changed: Notify,
}

/// Only the private response's position in its own selected-source prefix.
#[derive(Default)]
pub struct BrainStartupCursor {
    next: usize,
    slot: usize,
    marker: Option<(usize, u64, usize, bool, bool)>,
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
        let mut slots = Vec::new();
        let unavailable = unavailable || slots.try_reserve_exact(ROWS).is_err();
        if !unavailable {
            slots.extend((0..ROWS).map(|_| OnceLock::new()));
        }
        let loss = if unavailable || generation.is_none() {
            UNAVAILABLE
        } else {
            0
        };
        Arc::new(Self {
            generation,
            prefix: Prefix {
                slots,
                reserved: AtomicU64::new(0),
            },
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
            reservation: None,
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
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            changed.await;
        }
    }

    fn try_next(&self, cursor: &mut BrainStartupCursor) -> Option<BrainStartupEvent> {
        if self.closed.load(Ordering::Acquire) {
            return self.marker(cursor, true);
        }
        let horizon = self.prefix.horizon();
        while cursor.slot < horizon {
            let Some(published) = self.prefix.slots[cursor.slot].get() else {
                return self.marker(cursor, false);
            };
            cursor.slot += 1;
            let Some(row) = published else { continue };
            let row: Value = serde_json::from_slice(row).expect("selected source JSON");
            cursor.next += 1;
            return Some(BrainStartupEvent {
                name: "brain_startup",
                data: json!({"generation":self.generation,"sequence":cursor.next,"row":row})
                    .to_string(),
                terminal: false,
            });
        }
        self.marker(cursor, false)
    }

    fn marker(&self, cursor: &mut BrainStartupCursor, closed: bool) -> Option<BrainStartupEvent> {
        // A finalized send sets loss before releasing pending. Acquire of zero
        // therefore cannot certify a send whose failure has not yet been read.
        let closed = closed || self.closed.load(Ordering::Acquire);
        let pending = self.pending.load(Ordering::Acquire);
        // A producer prepared before our scan may publish between the old
        // horizon and pending-zero samples. Acquire its horizon after pending;
        // never certify a prefix that has not delivered those resolved slots.
        let unresolved = cursor.slot < self.prefix.horizon();
        let loss = self.loss.load(Ordering::Acquire);
        let through = cursor.next;
        let status = (through, loss, pending, closed, unresolved);
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
        } else if pending != 0 || unresolved {
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
    reservation: Option<usize>,
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
        if let Some(data) = self.data.take()
            && self.capture.loss.load(Ordering::Acquire) & UNAVAILABLE == 0
        {
            if let Some(slot) = self.capture.prefix.reserve(data.len) {
                // Install the unwind obligation before allocation or publication.
                self.reservation = Some(slot);
                let mut row = Vec::new();
                if row.try_reserve_exact(data.len).is_err() {
                    self.capture.invalidate(UNAVAILABLE);
                    self.publish(None);
                } else {
                    row.extend_from_slice(&data.bytes[..data.len]);
                    self.publish(Some(row));
                }
            } else {
                self.capture.invalidate(OVERFLOW);
            }
        }
        self.finalized = true;
    }

    fn publish(&mut self, row: Option<Vec<u8>>) {
        if let Some(slot) = self.reservation {
            // Only this reservation can initialize its cell; no reader or rival
            // producer initializes it, so set cannot wait on another initializer.
            assert!(self.capture.prefix.slots[slot].set(row).is_ok());
            self.reservation = None;
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.finalized {
            self.capture.invalidate(SOURCE_LOSS);
            self.publish(None);
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
    async fn unresolved_slot_resumes_without_another_emission() {
        let capture = BrainStartupCapture::new();
        let mut first = reserved(&capture, "started");
        capture.prepare(&row("stopped")).unwrap().finish(true);
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut cursor)["coverage"], "pending");
        let mut next = Box::pin(capture.next(&mut cursor));
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        assert!(next.as_mut().poll(&mut cx).is_pending());
        first.publish(Some(projected("started")));
        first.finalized = true;
        drop(first);
        assert_eq!(
            serde_json::from_str::<Value>(&next.await.unwrap().data).unwrap()["sequence"],
            1
        );
        assert_eq!(take(&capture, &mut cursor)["sequence"], 2);
        let marker = take(&capture, &mut cursor);
        assert_eq!(marker["through"], 2);
        assert_eq!(marker["coverage"], "complete");
    }

    fn projected(event: &str) -> Vec<u8> {
        let data = project(&row(event)).unwrap();
        data.bytes[..data.len].to_vec()
    }
    fn reserved(capture: &Arc<BrainStartupCapture>, event: &str) -> Pending {
        let mut pending = capture.prepare(&row(event)).unwrap();
        pending.reservation = capture.prefix.reserve(pending.data.take().unwrap().len);
        assert!(pending.reservation.is_some());
        pending
    }
    #[tokio::test]
    async fn failed_reservation_wakes_reader_and_does_not_count_as_a_row() {
        let capture = BrainStartupCapture::new();
        let first = reserved(&capture, "started");
        capture.prepare(&row("stopped")).unwrap().finish(true);
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut cursor)["coverage"], "pending");
        let mut next = Box::pin(capture.next(&mut cursor));
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        assert!(next.as_mut().poll(&mut cx).is_pending());
        drop(first);
        let surviving: Value = serde_json::from_str(&next.await.unwrap().data).unwrap();
        assert_eq!(surviving["sequence"], 1);
        assert_eq!(surviving["row"]["event"], "stopped");
        let marker = take(&capture, &mut cursor);
        assert_eq!(marker["through"], 1);
        assert_eq!(marker["pending"], 0);
        assert_eq!(marker["reasons"], json!(["source_send_loss"]));
        assert_eq!(marker["coverage"], "incomplete");
    }
    #[test]
    fn settled_marker_rechecks_the_horizon_after_a_prepared_producer_finishes() {
        let capture = BrainStartupCapture::new();
        let pending = capture.prepare(&row("started")).unwrap();
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(capture.prefix.horizon(), 0); // reader's first horizon sample
        pending.finish(true); // reserve/publish/release before marker samples pending
        let marker: Value =
            serde_json::from_str(&capture.marker(&mut cursor, false).unwrap().data).unwrap();
        assert_eq!(marker["coverage"], "pending");
        assert_eq!(marker["through"], 0);
        assert_eq!(take(&capture, &mut cursor)["sequence"], 1);
        assert_eq!(take(&capture, &mut cursor)["coverage"], "complete");
    }
    #[test]
    fn concurrent_reservations_obey_both_budgets_and_never_refund_failures() {
        for (count, bytes, charge) in [(127, 0, 1), (0, BYTES - 7, 7)] {
            let capture = BrainStartupCapture::new();
            capture
                .prefix
                .reserved
                .store((count << BYTE_BITS) | bytes as u64, Ordering::Release);
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let threads: Vec<_> = (0..2)
                .map(|_| {
                    let c = Arc::clone(&capture);
                    let b = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        b.wait();
                        c.prefix.reserve(charge)
                    })
                })
                .collect();
            barrier.wait();
            let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            assert_eq!(results.iter().filter(|v| v.is_some()).count(), 1);
            let state = capture.prefix.reserved.load(Ordering::Acquire);
            assert_eq!(state >> BYTE_BITS, count + 1);
            assert_eq!(state & BYTE_MASK, bytes as u64 + charge as u64);
        }
        let capture = BrainStartupCapture::new();
        let pending = reserved(&capture, "started");
        let state = capture.prefix.reserved.load(Ordering::Acquire);
        let _unwind = std::panic::catch_unwind(move || {
            let _pending = pending;
            panic!("finalization unwind");
        });
        assert_eq!(capture.prefix.reserved.load(Ordering::Acquire), state);
        assert_eq!(capture.pending.load(Ordering::Acquire), 0);
        assert!(capture.prefix.slots[0].get().unwrap().is_none());
        capture.prepare(&row("stopped")).unwrap().finish(true);
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut cursor)["sequence"], 1);
        assert_eq!(
            take(&capture, &mut cursor)["reasons"],
            json!(["source_send_loss"])
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
            assert_eq!(capture.prefix.horizon(), 1);
            assert!(
                !String::from_utf8_lossy(capture.prefix.slots[0].get().unwrap().as_ref().unwrap())
                    .contains("private_token")
            );
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
    fn send_loss_overflow_and_unavailable_never_recover_to_complete() {
        let capture = BrainStartupCapture::new();
        capture.prepare(&row("started")).unwrap().finish(false);
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut cursor)["sequence"], 1);
        assert_eq!(take(&capture, &mut cursor)["coverage"], "incomplete");
        let capture = BrainStartupCapture::new();
        for _ in 0..129 {
            capture.prepare(&row("started")).unwrap().finish(true);
        }
        assert_eq!(capture.prefix.horizon(), 128);
        assert!(capture.prefix.reserved.load(Ordering::Acquire) & BYTE_MASK <= BYTES as u64);
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
        capture
            .prefix
            .reserved
            .store((BYTES - 1) as u64, Ordering::Release);
        capture.prepare(&row("started")).unwrap().finish(true);
        assert_eq!(capture.prefix.horizon(), 0);
        assert_ne!(capture.loss.load(Ordering::Acquire) & OVERFLOW, 0);
        let capture = BrainStartupCapture::new();
        capture.missing_identity();
        assert_eq!(
            take(&capture, &mut BrainStartupCursor::default())["coverage"],
            "incomplete"
        );
    }
}
