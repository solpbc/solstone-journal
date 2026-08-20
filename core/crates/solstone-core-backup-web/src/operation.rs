// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::result_large_err)] // Route helpers return the exact HTTP refusal envelope on the Err path.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::Response;
use serde::Serialize;

use crate::response;

pub const HANDOFF_TTL: Duration = Duration::from_secs(30 * 60);

const TERMINAL: &[&str] = &["done", "error", "degraded", "needs_subscription", "refused"];

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Operation {
    pub kind: String,
    pub phase: String,
    pub reason_code: Option<String>,
    pub portal_url: Option<String>,
}

pub struct Slot {
    pub view: Operation,
    // Hosted wait capability; never serialized (the nonce only appears inside portal_url).
    pub nonce: Option<String>,
    // restore_hosted wait only; never an Operation field so it cannot leak into JSON.
    pub restore_key: Option<String>,
    pub started: Instant,
    pub generation: u64,
}

pub type SharedOperationSlot = Arc<Mutex<Option<Slot>>>;

pub fn new_slot() -> SharedOperationSlot {
    Arc::new(Mutex::new(None))
}

pub fn is_terminal(phase: &str) -> bool {
    TERMINAL.contains(&phase)
}

pub fn running_phase(kind: &str) -> &'static str {
    match kind {
        "enable" | "enable_hosted" => "setting_up",
        "restore" | "restore_hosted" | "offload_restore" => "restoring",
        "rotate" => "rotating",
        "teardown" => "tearing_down",
        _ => "setting_up",
    }
}

pub fn is_busy(slot: &SharedOperationSlot) -> bool {
    slot.lock()
        .expect("operation slot lock")
        .as_ref()
        .is_some_and(|slot| !is_terminal(&slot.view.phase))
}

pub fn current(slot: &SharedOperationSlot) -> Option<Operation> {
    slot.lock()
        .expect("operation slot lock")
        .as_ref()
        .map(|slot| slot.view.clone())
}

pub fn busy_response() -> Response {
    response::error(
        StatusCode::BAD_REQUEST,
        "I couldn't take that action in the current state.",
        "backup_busy",
        "",
    )
}

pub struct Begin {
    pub generation: u64,
}

pub fn begin(
    slot: &SharedOperationSlot,
    kind: &str,
    portal_url: Option<String>,
    nonce: Option<String>,
    restore_key: Option<String>,
) -> Result<Begin, Response> {
    let mut guard = slot.lock().expect("operation slot lock");
    if guard
        .as_ref()
        .is_some_and(|slot| !is_terminal(&slot.view.phase))
    {
        return Err(busy_response());
    }
    let generation = guard
        .as_ref()
        .map(|slot| slot.generation.wrapping_add(1))
        .unwrap_or(1);
    let view = Operation {
        kind: kind.to_owned(),
        phase: running_phase(kind).to_owned(),
        reason_code: None,
        portal_url,
    };
    *guard = Some(Slot {
        view,
        nonce,
        restore_key,
        started: Instant::now(),
        generation,
    });
    Ok(Begin { generation })
}

pub fn finish(
    slot: &SharedOperationSlot,
    generation: u64,
    phase: impl Into<String>,
    reason_code: Option<String>,
) {
    let mut guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.as_mut() else {
        return;
    };
    if current.generation != generation {
        return;
    }
    current.view.phase = phase.into();
    current.view.reason_code = reason_code;
    current.view.portal_url = None;
    current.nonce = None;
    current.restore_key = None;
}

pub struct Terminal {
    pub phase: String,
    pub reason_code: Option<String>,
}

pub fn spawn_worker<F>(slot: SharedOperationSlot, generation: u64, work: F)
where
    F: FnOnce() -> Terminal + Send + 'static,
{
    // ToolRunner/HttpTransport are blocking std process/HTTP calls. axum already
    // owns the tokio runtime; a worker thread must not hold the slot lock across
    // restic/broker, and resolve_operational_tools runs here so POST returns before
    // restic install.
    thread::spawn(move || {
        let terminal = panic::catch_unwind(AssertUnwindSafe(work)).unwrap_or(Terminal {
            phase: "error".into(),
            reason_code: Some("failed".into()),
        });
        finish(&slot, generation, terminal.phase, terminal.reason_code);
    });
}

pub fn mint_hex() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn portal_url(base: &str, nonce: &str, instance: &str) -> String {
    // Origin matches hosted.manage_url; /enable/spb is the external portal path,
    // not a local Convey route. Relative URLs would window.open onto this journal.
    format!(
        "{}/enable/spb?nonce={nonce}&instance={instance}",
        base.trim_end_matches('/')
    )
}

pub struct HandoffMatch {
    pub kind: String,
    pub restore_key: Option<String>,
}

pub enum HandoffError {
    Invalid,
    Expired,
}

pub fn match_handoff(
    slot: &SharedOperationSlot,
    nonce: &str,
) -> Result<HandoffMatch, HandoffError> {
    let guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.as_ref() else {
        return Err(HandoffError::Invalid);
    };
    if !matches!(
        current.view.kind.as_str(),
        "enable_hosted" | "restore_hosted"
    ) || current.nonce.as_deref() != Some(nonce)
        || is_terminal(&current.view.phase)
    {
        return Err(HandoffError::Invalid);
    }
    if current.started.elapsed() > HANDOFF_TTL {
        return Err(HandoffError::Expired);
    }
    Ok(HandoffMatch {
        kind: current.view.kind.clone(),
        restore_key: current.restore_key.clone(),
    })
}

pub fn mark_needs_subscription(slot: &SharedOperationSlot, generation: u64) {
    finish(slot, generation, "needs_subscription", None);
}

pub fn mark_expired(slot: &SharedOperationSlot) {
    let mut guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.as_mut() else {
        return;
    };
    if is_terminal(&current.view.phase) {
        return;
    }
    current.view.phase = "error".into();
    current.view.reason_code = Some("expired".into());
    current.view.portal_url = None;
    current.nonce = None;
    current.restore_key = None;
}

pub fn generation_of(slot: &SharedOperationSlot) -> Option<u64> {
    slot.lock()
        .expect("operation slot lock")
        .as_ref()
        .map(|slot| slot.generation)
}
