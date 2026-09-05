// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use serde_json::{Map, Value};
use solstone_core_journal_io::cortex_use::CortexUseFileIdentity;
use solstone_core_system::process::LaunchAuthority;

use crate::storage::{CortexStore, synthesized_error};

#[derive(Clone, Debug)]
pub(crate) struct Outbound {
    pub(crate) tract: &'static str,
    pub(crate) event: String,
    pub(crate) fields: Map<String, Value>,
}

#[derive(Clone, Debug)]
pub struct Work {
    pub use_id: String,
    pub talent_name: String,
    pub active: PathBuf,
    pub identity: CortexUseFileIdentity,
    pub request: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTalent {
    pub(crate) talent_type: Option<String>,
    pub(crate) declared_cwd: Option<String>,
    pub(crate) timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct RunningUse {
    pub(crate) talent_name: String,
    pub(crate) active: PathBuf,
    pub(crate) identity: CortexUseFileIdentity,
    pub authority: Arc<Mutex<LaunchAuthority>>,
    pub(crate) started: Instant,
    pub(crate) stderr: Arc<Mutex<Vec<String>>>,
}

#[derive(Default)]
struct Inner {
    requests: HashMap<String, Map<String, Value>>,
    resolved: HashMap<String, ResolvedTalent>,
    queued: HashMap<String, Work>,
    running: HashMap<String, RunningUse>,
    finalizers: HashSet<String>,
    // Retain admission through durable completion, after the finalizer is taken.
    admitted: HashSet<String>,
    pending_spawns: usize,
    accepting: bool,
}

struct FinalizedUse {
    running: Option<RunningUse>,
    request: Option<Map<String, Value>>,
}

#[derive(Clone)]
pub struct CortexState {
    store: Arc<CortexStore>,
    inner: Arc<Mutex<Inner>>,
    spawn: mpsc::Sender<Work>,
    cancel: mpsc::Sender<(String, String)>,
    outbound: mpsc::Sender<Outbound>,
}

impl CortexState {
    pub(crate) fn new(
        store: CortexStore,
        spawn: mpsc::Sender<Work>,
        cancel: mpsc::Sender<(String, String)>,
        outbound: mpsc::Sender<Outbound>,
    ) -> Self {
        Self {
            store: Arc::new(store),
            inner: Arc::new(Mutex::new(Inner {
                accepting: true,
                ..Inner::default()
            })),
            spawn,
            cancel,
            outbound,
        }
    }

    pub fn request(&self, request: Map<String, Value>) {
        let Some(use_id) = request
            .get("use_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
        else {
            eprintln!("cortex: request without use_id");
            return;
        };
        let Some(name) = request
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            // Python's callback catches this KeyError. The native equivalent is
            // deliberately silent on the bus and durable surfaces.
            eprintln!("cortex: request without name for {use_id}");
            return;
        };
        let mut inner = self.inner.lock().expect("cortex state lock poisoned");
        if !inner.accepting {
            return;
        }
        if inner.admitted.contains(&use_id) {
            if inner
                .requests
                .get(&use_id)
                .and_then(|request| request.get("name"))
                .and_then(Value::as_str)
                != Some(name)
            {
                eprintln!("cortex: refusing conflicting talent for active use {use_id}");
            }
            return;
        }
        let Ok(Some((active, identity))) = self.store.claim(name, &use_id, &request) else {
            return;
        };
        let work = Work {
            use_id: use_id.clone(),
            talent_name: name.to_owned(),
            active: active.clone(),
            identity,
            request: request.clone(),
        };
        inner.requests.insert(use_id.clone(), request.clone());
        inner.finalizers.insert(use_id.clone());
        inner.admitted.insert(use_id.clone());
        inner.pending_spawns += 1;
        inner.queued.insert(use_id.clone(), work.clone());
        drop(inner);
        if self.spawn.send(work.clone()).is_err() {
            self.abort(work, "Spawn worker error: spawn queue unavailable".into());
            self.spawn_finished();
        }
    }

    pub(crate) fn queue_cancel(&self, message: &Map<String, Value>) {
        let Some(use_id) = message
            .get("use_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        let reason = message
            .get("reason_code")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("talent_watchdog_cancelled");
        let _ = self.cancel.send((use_id.to_owned(), reason.to_owned()));
    }

    pub fn spawn_started(
        &self,
        work: &Work,
        authority: Arc<Mutex<LaunchAuthority>>,
        stderr: Arc<Mutex<Vec<String>>>,
    ) {
        let mut inner = self.inner.lock().expect("cortex state lock poisoned");
        inner.finalizers.insert(work.use_id.clone());
        inner.running.insert(
            work.use_id.clone(),
            RunningUse {
                talent_name: work.talent_name.clone(),
                active: work.active.clone(),
                identity: work.identity,
                authority,
                started: Instant::now(),
                stderr,
            },
        );
    }

    pub fn spawn_begin(&self, use_id: &str) {
        self.inner
            .lock()
            .expect("cortex state lock poisoned")
            .queued
            .remove(use_id);
    }

    pub fn spawn_finished(&self) {
        let mut inner = self.inner.lock().expect("cortex state lock poisoned");
        inner.pending_spawns = inner.pending_spawns.saturating_sub(1);
    }

    pub(crate) fn request_for(&self, use_id: &str) -> Option<Map<String, Value>> {
        self.inner
            .lock()
            .expect("cortex state lock poisoned")
            .requests
            .get(use_id)
            .cloned()
    }

    pub(crate) fn update_resolved_talent(&self, use_id: &str, resolved: ResolvedTalent) {
        let mut inner = self.inner.lock().expect("cortex state lock poisoned");
        if inner.requests.contains_key(use_id) {
            inner.resolved.insert(use_id.to_owned(), resolved);
        }
    }

    pub(crate) fn resolved_talent(&self, use_id: &str) -> Option<ResolvedTalent> {
        self.inner
            .lock()
            .expect("cortex state lock poisoned")
            .resolved
            .get(use_id)
            .cloned()
    }

    pub(crate) fn update_start(&self, use_id: &str, event: &Map<String, Value>) {
        let mut inner = self.inner.lock().expect("cortex state lock poisoned");
        let Some(request) = inner.requests.get_mut(use_id) else {
            return;
        };
        for key in ["model", "provider"] {
            if let Some(value) = event.get(key).filter(|value| !value.is_null()) {
                request.insert(key.into(), value.clone());
            }
        }
    }

    pub(crate) fn append_and_relay(
        &self,
        _use_id: &str,
        active: &std::path::Path,
        event: Map<String, Value>,
    ) {
        if self.store.append_active(active, &event).ok() != Some(true) {
            return;
        }
        let event_name = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let mut fields = event;
        fields.remove("event");
        let _ = self.outbound.send(Outbound {
            tract: "cortex",
            event: event_name,
            fields,
        });
    }

    pub(crate) fn append_and_relay_log_only(
        &self,
        active: &std::path::Path,
        event: Map<String, Value>,
    ) -> std::io::Result<bool> {
        self.store.append_active(active, &event)
    }

    pub(crate) fn journal(&self) -> &std::path::Path {
        self.store.journal()
    }

    fn claim_finalize(&self, use_id: &str) -> Option<FinalizedUse> {
        let mut inner = self.inner.lock().expect("cortex state lock poisoned");
        if !inner.finalizers.remove(use_id) {
            return None;
        }
        inner.queued.remove(use_id);
        let running = inner.running.remove(use_id);
        let request = inner.requests.remove(use_id);
        inner.resolved.remove(use_id);
        Some(FinalizedUse { running, request })
    }

    fn complete(
        &self,
        use_id: &str,
        name: &str,
        identity: CortexUseFileIdentity,
        request: Option<&Map<String, Value>>,
    ) {
        if self.store.complete(use_id, name, identity, request) {
            self.inner
                .lock()
                .expect("cortex state lock poisoned")
                .admitted
                .remove(use_id);
        }
    }

    pub(crate) fn abort(&self, work: Work, message: String) {
        // Abort deliberately uses the same compare-and-take as normal completion:
        // losing the race means another terminal path already removed this use.
        if self.claim_finalize(&work.use_id).is_none() {
            return;
        }
        let mut event = synthesized_error(&work.use_id, message);
        event.insert("reason_code".into(), Value::String("talent_aborted".into()));
        let _ = self.store.append_active(&work.active, &event);
        self.complete(
            &work.use_id,
            &work.talent_name,
            work.identity,
            Some(&work.request),
        );
    }

    pub fn finish(&self, use_id: &str, exit_code: i32) {
        let Some(finalized) = self.claim_finalize(use_id) else {
            return;
        };
        let Some(running) = finalized.running else {
            return;
        };
        if !self.store.has_finish(&running.active) {
            let trace = running
                .stderr
                .lock()
                .expect("stderr lock poisoned")
                .join("\n");
            let mut error = synthesized_error(
                use_id,
                format!("Talent exited with code {exit_code} without finish event"),
            );
            if !trace.is_empty() {
                error.insert("trace".into(), Value::String(trace));
            }
            error.insert("exit_code".into(), Value::from(exit_code));
            error.insert(
                "reason_code".into(),
                Value::String("talent_exited_without_finish".into()),
            );
            self.append_and_relay(use_id, &running.active, error);
        }
        self.complete(
            use_id,
            &running.talent_name,
            running.identity,
            finalized.request.as_ref(),
        );
    }

    pub(crate) fn cancel_running(&self, use_id: &str, reason: &str) -> Option<RunningUse> {
        self.inner
            .lock()
            .expect("cortex state lock poisoned")
            .running
            .contains_key(use_id)
            .then_some(())?;
        let finalized = self.claim_finalize(use_id)?;
        let running = finalized.running?;
        let mut event = synthesized_error(use_id, "Talent cancelled by watchdog");
        event.insert("reason_code".into(), Value::String(reason.to_owned()));
        self.append_and_relay(use_id, &running.active, event);
        self.complete(
            use_id,
            &running.talent_name,
            running.identity,
            finalized.request.as_ref(),
        );
        Some(running)
    }

    pub(crate) fn timeout(&self, use_id: &str, seconds: u64) -> Option<RunningUse> {
        let finalized = self.claim_finalize(use_id)?;
        let running = finalized.running?;
        let mut event =
            synthesized_error(use_id, format!("Talent timed out after {seconds} seconds"));
        event.insert("reason_code".into(), Value::String("talent_timeout".into()));
        self.append_and_relay(use_id, &running.active, event);
        self.complete(
            use_id,
            &running.talent_name,
            running.identity,
            finalized.request.as_ref(),
        );
        Some(running)
    }

    pub(crate) fn status(&self, queue_depth: usize) {
        let inner = self.inner.lock().expect("cortex state lock poisoned");
        if inner.running.is_empty() && queue_depth == 0 {
            return;
        }
        let uses: Vec<Value> = inner.running.iter().map(|(use_id, running)| {
            let request = inner.requests.get(use_id);
            serde_json::json!({"use_id":use_id, "name":request.and_then(|r| r.get("name")).cloned().unwrap_or(Value::String("unknown".into())), "provider":request.and_then(|r| r.get("provider")).cloned().unwrap_or(Value::String("unknown".into())), "elapsed_seconds":running.started.elapsed().as_secs()})
        }).collect();
        let fields = serde_json::from_value(serde_json::json!({"running_uses":inner.running.len(),"uses":uses,"queue_depth":queue_depth})).expect("status fields are objects");
        let _ = self.outbound.send(Outbound {
            tract: "cortex",
            event: "status".into(),
            fields,
        });
    }

    pub(crate) fn queue_depth(&self) -> usize {
        self.inner
            .lock()
            .expect("cortex state lock poisoned")
            .queued
            .len()
    }

    pub fn is_idle(&self) -> bool {
        let inner = self.inner.lock().expect("cortex state lock poisoned");
        inner.running.is_empty() && inner.pending_spawns == 0
    }

    pub fn stop_accepting(&self) {
        self.inner
            .lock()
            .expect("cortex state lock poisoned")
            .accepting = false;
    }

    pub fn stop_immediately(&self) -> Vec<RunningUse> {
        let queued = {
            let mut inner = self.inner.lock().expect("cortex state lock poisoned");
            inner.accepting = false;
            inner.queued.values().cloned().collect::<Vec<_>>()
        };
        for work in queued {
            self.abort(work, "Cortex stopped before spawn".into());
        }
        self.running()
    }

    pub(crate) fn accepting(&self) -> bool {
        self.inner
            .lock()
            .expect("cortex state lock poisoned")
            .accepting
    }

    pub fn running(&self) -> Vec<RunningUse> {
        self.inner
            .lock()
            .expect("cortex state lock poisoned")
            .running
            .values()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn nameless_request_is_silent_except_for_stderr_diagnostic() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let talents = store.talents().to_path_buf();
        let (spawn_tx, spawn_rx) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        state
            .request(serde_json::from_value(serde_json::json!({"use_id":"missing-name"})).unwrap());
        assert!(spawn_rx.try_recv().is_err());
        assert!(outbound_rx.try_recv().is_err());
        assert!(!talents.join("missing-name_active.jsonl").exists());
    }

    #[test]
    fn conflicting_active_id_cannot_claim_a_second_talent_or_replace_state() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (spawn_tx, spawn_rx) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        let original: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "use_id":"1700000000000", "name":"first", "prompt":"original",
        }))
        .unwrap();
        state.request(original.clone());
        let work = spawn_rx.recv().unwrap();
        let conflict = serde_json::from_value(serde_json::json!({
            "use_id":"1700000000000", "name":"second", "prompt":"conflict",
        }))
        .unwrap();
        let retry = state.clone();
        std::thread::spawn(move || retry.request(conflict))
            .join()
            .unwrap();
        state.request(original.clone());
        assert_eq!(state.request_for(&work.use_id), Some(original));
        assert_eq!(state.queue_depth(), 1);
        assert!(spawn_rx.try_recv().is_err());
        assert!(outbound_rx.try_recv().is_err());
        assert!(!directory.path().join("talents/second").exists());
        state.abort(work, "original ends".into());
        let log =
            fs::read_to_string(directory.path().join("talents/first/1700000000000.jsonl")).unwrap();
        assert!(log.contains("original ends"));
        assert!(!log.contains("conflict"));
    }

    #[cfg(unix)]
    #[test]
    fn reserved_ids_isolate_cancellation_finalization_and_recovery() {
        use solstone_core_journal_io::cortex_use::allocate_cortex_use_id;
        use solstone_core_system::process::{self, Disposition, LaunchError};
        use std::time::Duration;
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (spawn_tx, spawn_rx) = mpsc::channel();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        let mut work = Vec::new();
        for name in ["cancelled", "finished", "recovered"] {
            let id = allocate_cortex_use_id(directory.path(), 1_700_000_000_000)
                .unwrap()
                .to_string();
            state.request(
                serde_json::from_value(serde_json::json!({
                    "event":"request", "use_id":id, "name":name, "prompt":name,
                }))
                .unwrap(),
            );
            work.push(spawn_rx.recv().unwrap());
        }
        for item in &work[..2] {
            let mut authority = process::launch(
                Disposition::IndependentBoundedHelper {
                    timeout: Duration::from_secs(2),
                },
                || std::process::Command::new("/bin/true").spawn(),
                Box::new(|child, _| child.kill().map_err(LaunchError::Terminate)),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while authority.poll().unwrap().is_none() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            // Model the interval after child exit, before Cortex handles completion.
            state.spawn_begin(&item.use_id);
            state.spawn_started(
                item,
                Arc::new(Mutex::new(authority)),
                Arc::new(Mutex::new(Vec::new())),
            );
            state.spawn_finished();
        }
        state.queue_cancel(
            &serde_json::from_value(serde_json::json!({
                "use_id":work[0].use_id, "reason_code":"test_cancel",
            }))
            .unwrap(),
        );
        let (cancelled, reason) = cancel_rx.recv().unwrap();
        assert_eq!(cancelled, work[0].use_id);
        assert!(state.cancel_running(&cancelled, &reason).is_some());
        assert!(state.request_for(&work[1].use_id).is_some());
        assert!(state.request_for(&work[2].use_id).is_some());
        state.append_and_relay(&work[1].use_id, &work[1].active,
            serde_json::from_value(serde_json::json!({"event":"finish", "use_id":work[1].use_id, "result":"second result"})).unwrap());
        state.finish(&work[1].use_id, 0);
        state.finish(&work[0].use_id, 0); // A late completion cannot finalize twice.
        assert_eq!(
            state.inner.lock().unwrap().admitted,
            HashSet::from([work[2].use_id.clone()])
        );
        drop(state);
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        store.recover().unwrap();
        for (index, expected) in [(0, "test_cancel"), (1, "second result"), (2, "error")] {
            let log = fs::read_to_string(
                directory
                    .path()
                    .join("talents")
                    .join(&work[index].talent_name)
                    .join(format!("{}.jsonl", work[index].use_id)),
            )
            .unwrap();
            assert!(log.contains(expected), "{log}");
            assert!(!work[index].active.exists());
        }
        let day = chrono::DateTime::from_timestamp_millis(work[0].use_id.parse().unwrap())
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y%m%d")
            .to_string();
        let rows = fs::read_to_string(
            directory
                .path()
                .join("talents")
                .join(format!("{day}.jsonl")),
        )
        .unwrap();
        let rows = rows
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 3);
        for item in work {
            assert_eq!(
                rows.iter()
                    .filter(|row| row["use_id"] == item.use_id)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn compare_and_take_finalization_allows_only_one_terminal_owner() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (spawn_tx, _spawn_rx) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        state.request(
            serde_json::from_value(
                serde_json::json!({"use_id":"one","name":"conversation","day":"20260101"}),
            )
            .unwrap(),
        );
        assert!(state.claim_finalize("one").is_some());
        assert!(state.claim_finalize("one").is_none());
        state.request(
            serde_json::from_value(serde_json::json!({"use_id":"one", "name":"other"})).unwrap(),
        );
        assert!(!directory.path().join("talents/other").exists());
    }

    #[test]
    fn resolved_talent_state_is_available_until_finalization() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (spawn_tx, _spawn_rx) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        state.request(
            serde_json::from_value(serde_json::json!({"use_id":"one","name":"conversation"}))
                .unwrap(),
        );
        assert_eq!(state.resolved_talent("one"), None);
        let resolved = ResolvedTalent {
            talent_type: Some("cogitate".into()),
            declared_cwd: Some("journal".into()),
            timeout_seconds: Some(12),
        };
        state.update_resolved_talent("one", resolved.clone());
        assert_eq!(state.resolved_talent("one"), Some(resolved));
        assert!(state.claim_finalize("one").is_some());
        assert_eq!(state.resolved_talent("one"), None);
    }

    #[test]
    fn status_reports_queue_depth_without_a_running_use() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        // The worker-held item is deliberately not included: this is the queued depth.
        state.status(2);
        let status = outbound_rx.recv().unwrap();
        assert_eq!(status.event, "status");
        assert_eq!(status.fields["running_uses"], 0);
        assert_eq!(status.fields["queue_depth"], 2);
    }

    #[test]
    fn immediate_stop_terminalizes_queued_claim_without_starting_it() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let talents = store.talents().to_path_buf();
        let (spawn_tx, _spawn_rx) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        state.request(
            serde_json::from_value(
                serde_json::json!({"use_id":"one","name":"conversation","day":"20260101"}),
            )
            .unwrap(),
        );
        assert_eq!(state.queue_depth(), 1);
        assert!(state.stop_immediately().is_empty());
        let completed = talents.join("conversation/one.jsonl");
        assert!(
            fs::read_to_string(completed)
                .unwrap()
                .contains("Cortex stopped before spawn")
        );
    }

    #[test]
    fn failed_spawn_send_terminalizes_claim_and_leaves_drain_idle() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let talents = store.talents().to_path_buf();
        let (spawn_tx, spawn_rx) = mpsc::channel();
        drop(spawn_rx);
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        state.request(
            serde_json::from_value(
                serde_json::json!({"use_id":"one","name":"conversation","day":"20260101"}),
            )
            .unwrap(),
        );
        assert!(state.is_idle());
        assert!(
            fs::read_to_string(talents.join("conversation/one.jsonl"))
                .unwrap()
                .contains("Spawn worker error: spawn queue unavailable")
        );
    }
}
