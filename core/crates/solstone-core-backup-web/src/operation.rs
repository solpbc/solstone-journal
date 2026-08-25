// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::{cell::Cell, thread_local};

use axum::http::StatusCode;
use axum::response::Response;
use serde::Serialize;

use crate::{response, validation::RefusedReason};

pub const HANDOFF_TTL: Duration = Duration::from_secs(30 * 60);

const TERMINAL: &[&str] = &["done", "error", "degraded", "needs_subscription", "refused"];

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Operation {
    pub kind: String,
    pub phase: String,
    pub reason_code: Option<String>,
    pub recording_failure: Option<String>,
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

#[cfg(test)]
thread_local! {
    static INSTANCE_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

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

fn hosted_wait_expired(slot: &Slot) -> bool {
    !is_terminal(&slot.view.phase)
        && matches!(slot.view.kind.as_str(), "enable_hosted" | "restore_hosted")
        && slot.nonce.is_some()
        && slot.started.elapsed() > HANDOFF_TTL
}

fn expire_hosted_wait_in_place(slot: &mut Slot) {
    if !hosted_wait_expired(slot) {
        return;
    }
    slot.view.phase = "error".into();
    slot.view.reason_code = Some("expired".into());
    slot.view.recording_failure = None;
    slot.view.portal_url = None;
    slot.nonce = None;
    slot.restore_key = None;
}

fn observe(slot: &mut Option<Slot>) -> Option<&Slot> {
    if let Some(current) = slot.as_mut() {
        expire_hosted_wait_in_place(current);
    }
    slot.as_ref()
}

pub fn is_busy(slot: &SharedOperationSlot) -> bool {
    observe(&mut slot.lock().expect("operation slot lock"))
        .is_some_and(|slot| !is_terminal(&slot.view.phase))
}

pub fn current(slot: &SharedOperationSlot) -> Option<Operation> {
    observe(&mut slot.lock().expect("operation slot lock")).map(|slot| slot.view.clone())
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
    if observe(&mut guard).is_some_and(|slot| !is_terminal(&slot.view.phase)) {
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
        recording_failure: None,
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
    recording_failure: Option<String>,
) {
    let mut guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.as_mut() else {
        return;
    };
    if current.generation != generation || is_terminal(&current.view.phase) {
        return;
    }
    current.view.phase = phase.into();
    current.view.reason_code = reason_code;
    current.view.recording_failure = recording_failure;
    current.view.portal_url = None;
    current.nonce = None;
    current.restore_key = None;
}

pub struct Terminal {
    pub phase: String,
    pub reason_code: Option<String>,
    pub recording_failure: Option<String>,
}

impl Terminal {
    pub fn done() -> Self {
        Self::phase("done", None)
    }

    pub fn error(reason_code: impl Into<String>) -> Self {
        Self::phase("error", Some(reason_code.into()))
    }

    pub fn phase(phase: impl Into<String>, reason_code: Option<String>) -> Self {
        Self {
            phase: phase.into(),
            reason_code,
            recording_failure: None,
        }
    }

    pub fn restore(
        phase: impl Into<String>,
        reason_code: Option<String>,
        recording_failure: Option<String>,
    ) -> Self {
        Self {
            phase: phase.into(),
            reason_code,
            recording_failure,
        }
    }
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
        let terminal = panic::catch_unwind(AssertUnwindSafe(work))
            .unwrap_or_else(|_| Terminal::error("failed"));
        finish(
            &slot,
            generation,
            terminal.phase,
            terminal.reason_code,
            terminal.recording_failure,
        );
    });
}

pub fn mint_hex() -> Result<String, getrandom::Error> {
    #[cfg(test)]
    INSTANCE_ALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
    mint_hex_from_csprng()
}

pub fn mint_capability() -> Result<String, getrandom::Error> {
    mint_hex_from_csprng()
}

fn mint_hex_from_csprng() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
pub fn reset_instance_allocations() {
    INSTANCE_ALLOCATIONS.with(|count| count.set(0));
}

#[cfg(test)]
pub fn instance_allocations() -> usize {
    INSTANCE_ALLOCATIONS.with(Cell::get)
}

pub fn portal_url(base: &str, nonce: &str, instance: &str) -> String {
    // /enable/backup is the external services-portal handoff endpoint, not a local
    // Convey route. Keep this URL absolute so the browser does not target this journal.
    format!(
        "{}/enable/backup?nonce={nonce}&instance={instance}",
        base.trim_end_matches('/')
    )
}

pub fn restore_portal_url(base: &str, nonce: &str) -> String {
    format!(
        "{}/enable/backup?nonce={nonce}&intent=restore",
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
    let mut guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.as_mut() else {
        return Err(HandoffError::Invalid);
    };
    expire_hosted_wait_in_place(current);
    if is_terminal(&current.view.phase) {
        return if current.view.reason_code.as_deref() == Some("expired") {
            Err(HandoffError::Expired)
        } else {
            Err(HandoffError::Invalid)
        };
    }
    if !matches!(
        current.view.kind.as_str(),
        "enable_hosted" | "restore_hosted"
    ) || current.nonce.as_deref() != Some(nonce)
    {
        return Err(HandoffError::Invalid);
    }
    let _ = current.nonce.take();
    Ok(HandoffMatch {
        kind: current.view.kind.clone(),
        restore_key: current.restore_key.clone(),
    })
}

pub fn mark_needs_subscription(slot: &SharedOperationSlot, generation: u64) {
    finish(slot, generation, "needs_subscription", None, None);
}

pub fn mark_refused(slot: &SharedOperationSlot, generation: u64, reason: RefusedReason) {
    finish(
        slot,
        generation,
        "refused",
        Some(reason.code().into()),
        None,
    );
}

pub fn mark_expired(slot: &SharedOperationSlot, generation: u64) {
    finish(slot, generation, "error", Some("expired".into()), None);
}

pub fn mark_cancelled(slot: &SharedOperationSlot, generation: u64) {
    finish(slot, generation, "error", Some("cancelled".into()), None);
}

pub fn nonce_for_generation(slot: &SharedOperationSlot, generation: u64) -> Option<String> {
    let guard = slot.lock().expect("operation slot lock");
    let current = guard.as_ref()?;
    (current.generation == generation
        && !is_terminal(&current.view.phase)
        && current.view.kind == "restore_hosted")
        .then(|| current.nonce.clone())
        .flatten()
}

pub fn generation_of(slot: &SharedOperationSlot) -> Option<u64> {
    slot.lock()
        .expect("operation slot lock")
        .as_ref()
        .map(|slot| slot.generation)
}

#[cfg(test)]
pub fn backdate_started(slot: &SharedOperationSlot, age: Duration) {
    let mut guard = slot.lock().expect("operation slot lock");
    if let Some(current) = guard.as_mut() {
        current.started = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
    }
}

#[cfg(test)]
mod tests {
    use super::{portal_url, restore_portal_url};

    fn parse_portal_url(url: &str) -> (&str, &str, &str, Vec<(&str, &str)>) {
        let (scheme, remainder) = url.split_once("://").expect("scheme separator");
        let authority_end = remainder.find('/').expect("authority/path separator");
        let (authority, path_and_query) = remainder.split_at(authority_end);
        let (path, query) = path_and_query
            .split_once('?')
            .expect("path/query separator");

        let mut query_pairs = query
            .split('&')
            .map(|pair| pair.split_once('=').expect("query key/value separator"))
            .collect::<Vec<_>>();
        query_pairs.sort_unstable();

        assert!(
            query_pairs.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "duplicate query key"
        );

        (scheme, authority, path, query_pairs)
    }

    #[test]
    fn portal_url_uses_exact_backup_handoff_route_and_query() {
        let nonce = "alpha-nonce-1";
        let instance = "beta-instance-2";
        let url = portal_url("http://portal.example.test:8123/", nonce, instance);

        let (scheme, authority, path, query_pairs) = parse_portal_url(&url);

        assert_eq!(scheme, "http");
        assert_eq!(authority, "portal.example.test:8123");
        assert_eq!(path, "/enable/backup");
        assert_eq!(query_pairs, vec![("instance", instance), ("nonce", nonce)]);
    }

    #[test]
    fn old_spb_handoff_path_is_not_the_backup_handoff_path() {
        let old_url = "http://portal.example.test:8123/enable/spb?nonce=alpha-nonce-1&instance=beta-instance-2";

        let (_, _, path, _) = parse_portal_url(old_url);

        assert_ne!(path, "/enable/backup");
        assert_eq!(path, "/enable/spb");
    }

    #[test]
    fn restore_portal_url_uses_restore_intent_without_instance() {
        let nonce = "alpha-nonce-1";
        let url = restore_portal_url("http://portal.example.test:8123/", nonce);

        let (_, _, path, query_pairs) = parse_portal_url(&url);

        assert_eq!(path, "/enable/backup");
        assert_eq!(query_pairs, vec![("intent", "restore"), ("nonce", nonce)]);
    }
}
