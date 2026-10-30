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

/// Capture-only encoded fixed metadata. Never part of an ordinary envelope.
#[derive(Clone)]
pub struct PrivateProofReceive {
    bytes: [u8; 512],
    len: Option<usize>,
}
impl PrivateProofReceive {
    /// Encoding failure remains a present, invalid observation, so it cannot
    /// silently turn a lost summary into complete private evidence.
    pub fn encode(summary: &impl Serialize) -> Self {
        let mut fixed = FixedRow {
            bytes: [0; ROW_BYTES],
            len: 0,
        };
        let encoded = serde_json::to_writer(&mut fixed, summary).is_ok() && fixed.len <= 512;
        let mut result = Self {
            bytes: [0; 512],
            len: encoded.then_some(fixed.len),
        };
        if encoded {
            result.bytes[..fixed.len].copy_from_slice(&fixed.bytes[..fixed.len]);
        }
        result
    }

    fn value(&self) -> Option<Value> {
        let value: Value = serde_json::from_slice(&self.bytes[..self.len?]).ok()?;
        valid_proof_receive(&value).then_some(value)
    }
}

fn valid_proof_receive(value: &Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/proof_receive_schema.json"
    ))
    .expect("generated private proof schema");
    let required = schema["required"].as_array().expect("required fields");
    let optional = schema["optional"].as_array().expect("optional fields");
    if !required
        .iter()
        .all(|key| fields.contains_key(key.as_str().unwrap()))
        || !fields.keys().all(|key| {
            required
                .iter()
                .chain(optional)
                .any(|k| k.as_str() == Some(key))
        })
    {
        return false;
    }
    if !value["destination"]
        .as_str()
        .is_some_and(|s| s.len() <= 65 && s.parse::<std::net::SocketAddr>().is_ok())
    {
        return false;
    }
    for (key, choices) in schema["enums"].as_object().unwrap() {
        if !choices.as_array().unwrap().contains(&value[key]) {
            return false;
        }
    }
    for key in ["saw_eof", "tls_failed", "close_observed"] {
        if !value[key].is_boolean() {
            return false;
        }
    }
    let Some(ordinal) = value["ordinal"].as_u64() else {
        return false;
    };
    let Some(attempted) = value["attempted"].as_u64() else {
        return false;
    };
    if ordinal == 0
        || ordinal > attempted
        || attempted > schema["limits"]["attempted"].as_u64().unwrap()
    {
        return false;
    }
    for key in ["header_bytes", "body_bytes", "declared_body_bytes"] {
        if let Some(v) = fields.get(key)
            && !v
                .as_u64()
                .is_some_and(|n| n <= schema["limits"][key].as_u64().unwrap())
        {
            return false;
        }
    }
    if !schema["stage_categories"][value["stage"].as_str().unwrap()]
        .as_array()
        .unwrap()
        .contains(&value["category"])
    {
        return false;
    }
    if let Some(flag) = schema["branch_flags"]
        .get(value["branch"].as_str().unwrap())
        .and_then(Value::as_str)
        && value[flag] != true
    {
        return false;
    }
    if let (Some(body), Some(declared)) = (
        value["body_bytes"].as_u64(),
        value["declared_body_bytes"].as_u64(),
    ) && body > declared
        && value["category"] != "framing"
    {
        return false;
    }
    if schema["complete_branches"]
        .as_array()
        .unwrap()
        .contains(&value["branch"])
        && value["stage"] != "complete"
    {
        return false;
    }
    let header = fields.get("header_bytes").and_then(Value::as_u64);
    let body = fields.get("body_bytes").and_then(Value::as_u64);
    let declared = fields.get("declared_body_bytes").and_then(Value::as_u64);
    match value["stage"].as_str() {
        Some("before_receive") => {
            header.is_none()
                && body.is_none()
                && declared.is_none()
                && value["category"] == "before_receive"
                && value["branch"] == "unknown"
                && value["saw_eof"] == false
                && value["tls_failed"] == false
                && value["close_observed"] == false
        }
        Some("headers") => {
            header.is_some()
                && body.is_none()
                && declared.is_none()
                && value["category"] != "before_receive"
        }
        Some("body" | "complete") => {
            header.is_some_and(|n| n >= 4)
                && body.is_some()
                && declared.is_some()
                && (value["stage"] != "complete" || body == declared)
                && value["category"] != "before_receive"
        }
        _ => false,
    }
}

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

    pub(super) fn prepare(
        self: &Arc<Self>,
        row: &CallosumEnvelope,
        proof: Option<&PrivateProofReceive>,
    ) -> Option<Pending> {
        if !selected(row) {
            return None;
        }
        self.pending.fetch_add(1, Ordering::AcqRel);
        let data = match project(row, proof) {
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

struct Selected<'a>(&'a CallosumEnvelope, Option<&'a Value>);
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
        if let Some(proof) = self.1 {
            map.serialize_entry("proof_receive", proof)?;
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

fn project(row: &CallosumEnvelope, proof: Option<&PrivateProofReceive>) -> Option<FixedRow> {
    let proof = match proof {
        Some(proof) => {
            if row.event != "brain_observation"
                || row.extra.get("caller")?.as_str()? != "generate"
                || row.extra.get("phase")?.as_str()? != "runtime_establish"
                || !matches!(
                    row.extra.get("outcome")?.as_str()?,
                    "published" | "unchanged" | "refused" | "publication_uncertain"
                )
            {
                return None;
            }
            let value = proof.value()?;
            let schema: Value = serde_json::from_str(include_str!(
                "../../../../fixtures/proof_receive_schema.json"
            ))
            .ok()?;
            if let Some(expected) = schema["category_reasons"]
                .get(value["category"].as_str()?)
                .and_then(Value::as_str)
            {
                let verification = row.extra.get("verification_reason").and_then(Value::as_str);
                let reason = row.extra.get("reason_code").and_then(Value::as_str);
                if verification.is_some_and(|r| r != expected)
                    || reason.is_some_and(|r| r != "attestation_rejected")
                    || (row.extra.get("outcome")?.as_str()? == "published"
                        && (verification != Some(expected)
                            || reason != Some("attestation_rejected")))
                {
                    return None;
                }
            }
            Some(value)
        }
        None => None,
    };
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
    serde_json::to_writer(&mut data, &Selected(row, proof.as_ref())).ok()?;
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
    #[test]
    fn private_proof_rejects_conflicts_and_keeps_existing_loss_limits() {
        let base = serde_json::json!({"destination":"127.0.0.1:443","ordinal":1,"attempted":2,
            "stage":"body","category":"response_body_eof","header_bytes":64,"body_bytes":2,
            "declared_body_bytes":3,"branch":"mapped_unexpected_eof","saw_eof":true,"tls_failed":false,"close_observed":false});
        assert!(valid_proof_receive(&base));
        for (key, value) in [
            ("ordinal", json!(3)),
            ("attempted", json!(9)),
            ("destination", json!("service.example:443")),
            ("header_bytes", json!(16385)),
            ("body_bytes", json!(8388609)),
            ("stage", json!("complete")),
            ("branch", json!("trailing_reset")),
            ("category", json!("response_eof")),
            ("saw_eof", json!(false)),
            ("secret", json!("not allowed")),
        ] {
            let mut bad = base.clone();
            bad[key] = value;
            assert!(!valid_proof_receive(&bad), "{key}");
        }
        let make_row = || {
            serde_json::from_value::<CallosumEnvelope>(json!({"tract":"supervisor","event":"brain_observation",
            "observation_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","caller":"generate","phase":"runtime_establish","outcome":"unchanged"})).unwrap()
        };
        let proof = PrivateProofReceive::encode(&base);
        let capture = BrainStartupCapture::new();
        for _ in 0..=ROWS {
            capture
                .prepare(&make_row(), Some(&proof))
                .unwrap()
                .finish(true);
        }
        let mut cursor = BrainStartupCursor::default();
        let mut retained = 0;
        loop {
            let event = take(&capture, &mut cursor);
            if event.get("coverage").is_some() {
                assert_eq!(event["coverage"], "incomplete");
                break;
            }
            retained += 1;
        }
        assert!(retained > 0 && retained <= ROWS);
        let closed = BrainStartupCapture::new();
        let pending = closed.prepare(&make_row(), Some(&proof)).unwrap();
        closed.close();
        pending.finish(true);
        assert_eq!(
            take(&closed, &mut BrainStartupCursor::default())["coverage"],
            "closed"
        );
        let mut wrong = make_row();
        wrong.extra.insert("caller".into(), json!("mobile_renewal"));
        assert!(project(&wrong, Some(&proof)).is_none());
        let too_large = PrivateProofReceive::encode(&json!({"secret":"x".repeat(2048)}));
        assert!(too_large.value().is_none());
    }

    #[test]
    fn private_proof_reason_binding_and_all_closed_publication_row_bounds() {
        let base = json!({"destination":"[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff%4294967295]:65535","ordinal":8,"attempted":8,
            "stage":"complete","category":"channel_state","header_bytes":16384,"body_bytes":8388608,"declared_body_bytes":8388608,
            "branch":"buffered_before_tls_failure","saw_eof":false,"tls_failed":true,"close_observed":false});
        let mut row:CallosumEnvelope=serde_json::from_value(json!({"tract":"supervisor","event":"brain_observation",
            "observation_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","caller":"generate","phase":"runtime_establish","outcome":"published",
            "revision":9007199254740991u64,"runtime_failure_marker":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "reason_code":"attestation_rejected","verification_reason":"proof_http_eof"})).unwrap();
        let proof = PrivateProofReceive::encode(&base);
        for outcome in ["published", "unchanged", "refused", "publication_uncertain"] {
            row.extra.insert("outcome".into(), json!(outcome));
            assert!(project(&row, Some(&proof)).is_none(), "{outcome}");
        }
        row.extra.remove("verification_reason");
        row.extra.remove("reason_code");
        assert!(project(&row, Some(&proof)).is_some());
        row.extra.insert("outcome".into(), json!("published"));
        assert!(project(&row, Some(&proof)).is_none());
        let mut summary = base;
        summary["category"] = json!("proof_rejected");
        let proof = PrivateProofReceive::encode(&summary);
        row.extra
            .insert("reason_code".into(), json!("attestation_rejected"));
        let mut max = 0;
        let mut cases = 0;
        for line in include_str!("../../../../fixtures/brain_observation_vocabulary.tsv").lines() {
            if let Some(("verification_reason", reason)) = line.split_once('\t') {
                row.extra
                    .insert("verification_reason".into(), json!(reason));
                let data = project(&row, Some(&proof))
                    .expect("every closed reason fits a maximal private publication");
                max = max.max(data.len);
                cases += 1;
            }
        }
        assert!(cases > 0 && max <= ROW_BYTES);
        println!(
            "PROOF_ROW_BOUND={}",
            json!({"cases":cases,"max_bytes":max,"limit":ROW_BYTES,
            "construction":"Every verification_reason in exact brain_observation_vocabulary.tsv, maximal numeric IPv6/ordinal/count/header/body/length/publication IDs/revision and longest allowed terminal branch; conservative allowed combinations."})
        );
    }

    #[tokio::test]
    async fn unresolved_slot_resumes_without_another_emission() {
        let capture = BrainStartupCapture::new();
        let mut first = reserved(&capture, "started");
        capture.prepare(&row("stopped"), None).unwrap().finish(true);
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
        let data = project(&row(event), None).unwrap();
        data.bytes[..data.len].to_vec()
    }
    fn reserved(capture: &Arc<BrainStartupCapture>, event: &str) -> Pending {
        let mut pending = capture.prepare(&row(event), None).unwrap();
        pending.reservation = capture.prefix.reserve(pending.data.take().unwrap().len);
        assert!(pending.reservation.is_some());
        pending
    }
    #[tokio::test]
    async fn failed_reservation_wakes_reader_and_does_not_count_as_a_row() {
        let capture = BrainStartupCapture::new();
        let first = reserved(&capture, "started");
        capture.prepare(&row("stopped"), None).unwrap().finish(true);
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
        let pending = capture.prepare(&row("started"), None).unwrap();
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
        capture.prepare(&row("stopped"), None).unwrap().finish(true);
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
        let pending = capture.prepare(&row("started"), None).unwrap();
        assert_eq!(take(&capture, &mut cursor)["coverage"], "pending");
        pending.finish(true);
        assert_eq!(take(&capture, &mut cursor)["sequence"], 1);
        assert_eq!(take(&capture, &mut cursor)["coverage"], "complete");
        let pending = capture.prepare(&row("stopped"), None).unwrap();
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
            capture.prepare(&bad, None).unwrap().finish(true);
            capture.prepare(&row("started"), None).unwrap().finish(true);
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
        capture
            .prepare(&row("started"), None)
            .unwrap()
            .finish(false);
        let mut cursor = BrainStartupCursor::default();
        assert_eq!(take(&capture, &mut cursor)["sequence"], 1);
        assert_eq!(take(&capture, &mut cursor)["coverage"], "incomplete");
        let capture = BrainStartupCapture::new();
        for _ in 0..129 {
            capture.prepare(&row("started"), None).unwrap().finish(true);
        }
        assert_eq!(capture.prefix.horizon(), 128);
        assert!(capture.prefix.reserved.load(Ordering::Acquire) & BYTE_MASK <= BYTES as u64);
        assert_ne!(capture.loss.load(Ordering::Acquire) & OVERFLOW, 0);
        for capture in [
            BrainStartupCapture::with_generation(None, false),
            BrainStartupCapture::with_generation(Some("a".repeat(32)), true),
        ] {
            capture.prepare(&row("started"), None).unwrap().finish(true);
            let mut cursor = BrainStartupCursor::default();
            assert_eq!(take(&capture, &mut cursor)["coverage"], "unavailable");
        }
    }
    #[test]
    fn close_supersedes_undelivered_rows_and_new_runtime_has_its_own_identity() {
        let capture = BrainStartupCapture::new();
        capture.prepare(&row("started"), None).unwrap().finish(true);
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
        let projection = project(&original, None).unwrap();
        let value: Value = serde_json::from_slice(&projection.bytes[..projection.len]).unwrap();
        assert_eq!(value["revision"], 9_007_199_254_740_991_u64);
        assert!(value.get("secret").is_none());
        original
            .extra
            .insert("revision".into(), json!(9_007_199_254_740_992_u64));
        assert!(project(&original, None).is_none());
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
        capture.prepare(&row("started"), None).unwrap().finish(true);
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
        capture.prepare(&row("started"), None).unwrap().finish(true);
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
