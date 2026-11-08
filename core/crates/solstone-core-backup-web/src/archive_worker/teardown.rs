// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Concrete teardown custody in the existing operation mutex.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::Arc,
};

use axum::response::Response;
use serde_json::Value;
use solstone_core_backup::archive_admission::{ArchiveCancellation, Clock};
use solstone_core_backup_runtime::ToolRunner;
use solstone_core_backup_runtime::teardown_admission::{
    TeardownAdmissionRefusal, TeardownCarrierRefusal, TeardownReaderRefusal,
};
#[cfg(target_os = "ios")]
use solstone_core_offload::teardown::run_owned_teardown_with_root;
use solstone_core_offload::teardown::{
    OwnedTeardownOutcome, TeardownRunRefusal, run_owned_teardown,
};

use crate::{
    BackupWebDeps,
    operation::{self, OperationState, SharedOperationSlot, Terminal},
};

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static SETTLED: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
}

struct Reservation {
    generation: u64,
    journal: PathBuf,
    clock: Arc<dyn Clock + Send + Sync>,
    cancellation: ArchiveCancellation,
    version: &'static str,
    #[allow(dead_code)]
    runner: Arc<dyn ToolRunner + Send + Sync>,
}

// Private allocation identity; neither a response nor a generation can mint it.
#[derive(Clone)]
struct Ticket(Arc<Reservation>);

struct Input {
    ticket: Ticket,
}

enum Payload {
    Outcome(OwnedTeardownOutcome),
    Unused(Input),
    // Only original surviving inputs, without a claim about unwound lower owners.
    Unknown(Input),
}

struct Returned {
    ticket: Ticket,
    payload: Payload,
    terminal: Terminal,
    projected: bool,
}

#[derive(Default)]
pub(super) struct Custody {
    reservation: Option<Arc<Reservation>>,
    input: Option<Input>,
    pending: Option<Returned>,
    retained: Vec<Returned>,
}

impl Custody {
    pub(super) fn needs_settlement(&self) -> bool {
        self.pending.is_some() || !self.retained.is_empty()
    }
    pub(super) fn is_busy(&self) -> bool {
        self.reservation.is_some()
            || self.input.is_some()
            || self.pending.is_some()
            || !self.retained.is_empty()
    }

    fn matches(&self, ticket: &Ticket) -> bool {
        self.reservation
            .as_ref()
            .is_some_and(|owner| Arc::ptr_eq(owner, &ticket.0))
    }
}

fn current_matches(state: &OperationState, ticket: &Ticket) -> bool {
    state.archive.teardown.matches(ticket)
        && state.current.as_ref().is_some_and(|current| {
            current.generation == ticket.0.generation
                && current.view.kind == "teardown"
                && current.handoff_poll.is_none()
                && current.pending_handoff_terminal.is_none()
                && {
                    #[cfg(windows)]
                    {
                        current.cleanup.is_none()
                    }
                    #[cfg(not(windows))]
                    {
                        true
                    }
                }
        })
}

fn begin(deps: &BackupWebDeps) -> Result<Ticket, Response> {
    let mut state =
        operation::lock_for_begin(&deps.operations).map_err(|()| operation::busy_response())?;
    let cancellation = ArchiveCancellation::new_child(&state.cancellation);
    let generation = operation::install_slot(
        &mut state.current,
        "teardown",
        None,
        None,
        None,
        Some(cancellation.clone()),
    )
    .generation;
    let ticket = Ticket(Arc::new(Reservation {
        generation,
        journal: deps.journal_root.clone(),
        clock: Arc::clone(&deps.clock),
        cancellation,
        version: deps.version,
        runner: Arc::clone(&deps.runner),
    }));
    state.archive.teardown.reservation = Some(Arc::clone(&ticket.0));
    state.archive.teardown.input = Some(Input {
        ticket: ticket.clone(),
    });
    Ok(ticket)
}

#[allow(dead_code)]
fn start(slot: &SharedOperationSlot, ticket: &Ticket) -> bool {
    let Ok(mut state) = slot.lock() else {
        return false;
    };
    if !current_matches(&state, ticket) {
        return false;
    }
    let current = state.current.as_mut().expect("matching current");
    if current.worker_active || operation::is_terminal(&current.view.phase) {
        return false;
    }
    current.worker_active = true;
    true
}

fn take_input(state: &mut OperationState, ticket: &Ticket) -> Option<Input> {
    if state
        .archive
        .teardown
        .input
        .as_ref()
        .is_some_and(|input| Arc::ptr_eq(&input.ticket.0, &ticket.0))
    {
        state.archive.teardown.input.take()
    } else {
        None
    }
}

fn unused(input: Input, reason: &str) -> Returned {
    Returned {
        ticket: input.ticket.clone(),
        payload: Payload::Unused(input),
        terminal: Terminal::error(reason),
        projected: true,
    }
}

fn return_unused(slot: &SharedOperationSlot, ticket: &Ticket) {
    let returned = {
        // Recovery is custody only. receive() will refuse the poisoned handoff.
        let mut state = slot.lock().unwrap_or_else(|error| error.into_inner());
        take_input(&mut state, ticket).map(|input| unused(input, "failed"))
    };
    if let Some(returned) = returned {
        receive(slot, returned);
    }
}

pub(crate) fn spawn(deps: &BackupWebDeps) -> Result<bool, Response> {
    let ticket = begin(deps)?;
    let mut state = deps.operations.lock().expect("operation slot lock");
    if state.closed || !current_matches(&state, &ticket) {
        drop(state);
        return_unused(&deps.operations, &ticket);
        return Ok(false);
    }
    let current = state.current.as_mut().expect("matching current");
    if current.worker_active || operation::is_terminal(&current.view.phase) {
        drop(state);
        return_unused(&deps.operations, &ticket);
        return Ok(false);
    }
    current.worker_active = true;

    let slot = Arc::clone(&deps.operations);
    let worker_ticket = ticket.clone();
    #[cfg(all(test, feature = "full-tests"))]
    let settled = SETTLED.with(|hook| hook.borrow_mut().take());
    // Original input stays in the admission mutex until the real thread takes it.
    // A refused thread creation therefore cannot drop it in a discarded closure.
    match operation::spawn_thread(move || {
        worker(
            &slot,
            &worker_ticket,
            || {},
            crate::teardown_report::outcome,
        );
        #[cfg(all(test, feature = "full-tests"))]
        if let Some(settled) = settled {
            let _ = settled.send(());
        }
    }) {
        Ok(handle) => {
            state.threads.push(handle);
            Ok(true)
        }
        Err(_) => {
            let current = state.current.as_mut().expect("matching current");
            current.worker_active = false;
            drop(state);
            return_unused(&deps.operations, &ticket);
            Ok(false)
        }
    }
}

fn worker(
    slot: &SharedOperationSlot,
    ticket: &Ticket,
    before_call: impl FnOnce(),
    project: impl FnOnce(&OwnedTeardownOutcome) -> Value,
) {
    let input = {
        let mut state = match slot.lock() {
            Ok(state) => state,
            Err(error) => {
                let input = take_input(&mut error.into_inner(), ticket);
                if let Some(input) = input {
                    receive(slot, unused(input, "failed"));
                }
                return;
            }
        };
        let ready = current_matches(&state, ticket)
            && state
                .current
                .as_ref()
                .is_some_and(|current| current.worker_active);
        let input = take_input(&mut state, ticket);
        drop(state);
        if !ready {
            if let Some(input) = input {
                receive(slot, unused(input, "failed"));
            }
            return;
        }
        input
    };
    if let Some(input) = input {
        receive(slot, execute(input, before_call, project));
    }
}

fn execute(
    input: Input,
    before_call: impl FnOnce(),
    project: impl FnOnce(&OwnedTeardownOutcome) -> Value,
) -> Returned {
    let mut outcome = None;
    let mut report = None;
    let mut invoked = false;
    let result = catch_unwind(AssertUnwindSafe(|| {
        before_call();
        if input.ticket.0.cancellation.is_cancelled() {
            return;
        }
        invoked = true;
        let original = &input.ticket.0;
        #[cfg(target_os = "ios")]
        {
            if let Some(root) = original.runner.ios_journal_root() {
                outcome = Some(run_owned_teardown_with_root(
                    root,
                    Arc::clone(&original.clock),
                    original.cancellation.clone(),
                    original.version,
                ));
            } else {
                outcome = Some(run_owned_teardown(
                    &original.journal,
                    Arc::clone(&original.clock),
                    original.cancellation.clone(),
                    original.version,
                ));
            }
        }
        #[cfg(not(target_os = "ios"))]
        {
            outcome = Some(run_owned_teardown(
                &original.journal,
                Arc::clone(&original.clock),
                original.cancellation.clone(),
                original.version,
            ));
        }
        let returned = outcome.as_ref().expect("actual returned outcome");
        solstone_core_offload::teardown::record_backup_off(
            &original.journal,
            returned,
            solstone_core_removals::Surface::Web {
                page: solstone_core_removals::Page::Backup,
            },
        );
        report = Some(project(returned));
    }));
    let ticket = input.ticket.clone();
    if result.is_ok() && !invoked {
        return unused(input, "cancelled");
    }
    let mut terminal = if result.is_err() {
        Terminal::phase("error", Some("unknown".into()))
    } else {
        let actual = outcome.as_ref().expect("coordinator returned");
        match actual {
            OwnedTeardownOutcome::Finished(settled) => {
                let mut done = Terminal::done();
                // Own storage was emptied, but the owner's bucket may still keep it.
                done.guidance = settled
                    .remote()
                    .storage_retention()
                    .and_then(solstone_core_offload::teardown::storage_retention_sentence)
                    .map(str::to_owned);
                done
            }
            OwnedTeardownOutcome::Held(_) => match unstarted_reason(actual) {
                Some(reason) => Terminal::error(reason),
                None if nothing_deleted(actual) => Terminal::error("storage_unreached"),
                None => Terminal::phase("refused", Some("failed".into())),
            },
            OwnedTeardownOutcome::Unknown(_) => Terminal::error("unknown"),
        }
    };
    terminal.report = report;
    Returned {
        ticket,
        payload: outcome
            .map(Payload::Outcome)
            .unwrap_or(Payload::Unknown(input)),
        terminal,
        projected: result.is_ok(),
    }
}

/// The remote leg stopped before any delete, and no local phase ran.
fn nothing_deleted(outcome: &OwnedTeardownOutcome) -> bool {
    let attempts = outcome.attempts();
    matches!(outcome.refusal(), Some(TeardownRunRefusal::Remote(_)))
        && outcome
            .remote()
            .is_some_and(|remote| remote.delete_attempts() == 0 && remote.abort_attempts() == 0)
        && !attempts.clear()
        && !attempts.binding()
        && !attempts.retirement()
}

/// Owner-actionable refusals that happened before any effect.
fn unstarted_reason(outcome: &OwnedTeardownOutcome) -> Option<&'static str> {
    if !outcome.refused_before_effects() {
        return None;
    }
    let reader = outcome
        .preparation_failure()
        .and_then(|failure| match failure.refusal() {
            TeardownCarrierRefusal::Admission(TeardownAdmissionRefusal::Reader(reader)) => {
                Some(reader)
            }
            _ => None,
        });
    if let Some(reason) = reader.and_then(|reader| match reader {
        TeardownReaderRefusal::BackupOnlyHeld => Some("backup_only_held"),
        TeardownReaderRefusal::BackupOnlyUnplaced => Some("backup_only_unplaced"),
        TeardownReaderRefusal::CatalogUnavailable => Some("offload_record_unavailable"),
        TeardownReaderRefusal::RecoveryUnavailable | TeardownReaderRefusal::RecoveryHeld => None,
    }) {
        Some(reason)
    } else if matches!(
        outcome.refusal(),
        Some(TeardownRunRefusal::UnsupportedDestination)
    ) {
        Some("destination_unsupported")
    } else if matches!(outcome.refusal(), Some(TeardownRunRefusal::ToolUnavailable)) {
        Some("teardown_tool_unavailable")
    } else {
        None
    }
}

impl Returned {
    fn dischargeable(&self) -> bool {
        self.projected
            && match &self.payload {
                Payload::Outcome(OwnedTeardownOutcome::Finished(_)) => true,
                // Dropping an unstarted refusal only releases its admission.
                Payload::Outcome(outcome) => outcome.refused_before_effects(),
                Payload::Unused(input) => Arc::ptr_eq(&input.ticket.0, &self.ticket.0),
                Payload::Unknown(input) => {
                    let _original = &input.ticket;
                    false
                }
            }
    }
}

fn receive(slot: &SharedOperationSlot, returned: Returned) {
    let mut state = match slot.lock() {
        Ok(state) => state,
        Err(error) => {
            error.into_inner().archive.teardown.retained.push(returned);
            return;
        }
    };
    let unused = matches!(&returned.payload, Payload::Unused(_));
    let active = state
        .current
        .as_ref()
        .is_some_and(|current| current.worker_active);
    let cancelled_success = returned.ticket.0.cancellation.is_cancelled()
        && matches!(
            &returned.payload,
            Payload::Outcome(OwnedTeardownOutcome::Finished(_))
        );
    if !current_matches(&state, &returned.ticket)
        || (!active && !unused)
        || state.archive.teardown.pending.is_some()
    {
        state.archive.teardown.retained.push(returned);
        return;
    }
    if cancelled_success {
        // The original coordinator has finished. Retain its complete return,
        // but allow closed-route settlement once the real worker is joined.
        state
            .current
            .as_mut()
            .expect("matching current")
            .worker_active = false;
        state.archive.teardown.retained.push(returned);
        return;
    }
    let dischargeable = returned.dischargeable();
    let current = state.current.as_mut().expect("matching current");
    current.worker_active = false;
    // Move only display facts; the actual owner remains in the same mutex.
    operation::apply_terminal(current, returned.terminal.clone());
    if dischargeable {
        state.archive.teardown.pending = Some(returned);
    } else {
        state.archive.teardown.retained.push(returned);
    }
}

pub(crate) fn response(deps: &BackupWebDeps) -> Option<Response> {
    let mut state = match deps.operations.lock() {
        Ok(state) => state,
        Err(_) => return Some(crate::internal_error()),
    };
    let returned = state.archive.teardown.pending.as_ref()?;
    if !current_matches(&state, &returned.ticket)
        || state
            .current
            .as_ref()
            .is_some_and(|current| current.worker_active)
        || returned.ticket.0.journal != deps.journal_root
        || !Arc::ptr_eq(&returned.ticket.0.clock, &deps.clock)
        || !returned.dischargeable()
    {
        return Some(crate::internal_error());
    }
    // Keep custody outside this catch and through actual JSON response construction.
    let response = catch_unwind(AssertUnwindSafe(|| {
        crate::status::status_with_operation(
            &deps.journal_root,
            state.current.as_ref().map(|current| &current.view),
        )
        .map(crate::response::success)
    }));
    match response {
        Ok(Ok(response)) if response.status().is_success() => {
            // Drop the whole harmless/settled return under the admission mutex.
            drop(state.archive.teardown.pending.take());
            state.archive.teardown.reservation = None;
            Some(response)
        }
        _ => Some(crate::internal_error()),
    }
}

// The complete returned owner stays outside the unwind boundary. A failing
// observation must neither release its exclusion nor drop unvisited returns.
fn observe_restart_custody(
    actual: &OwnedTeardownOutcome,
    observe: impl FnOnce(&OwnedTeardownOutcome) -> bool,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| observe(actual))).unwrap_or(false)
}

fn closed_dischargeable(
    returned: &Returned,
    observe: impl FnOnce(&OwnedTeardownOutcome) -> bool,
) -> bool {
    if returned.dischargeable() {
        return true;
    }
    if returned.projected
        && let Payload::Outcome(actual) = &returned.payload
    {
        return observe_restart_custody(actual, observe);
    }
    false
}

pub(super) fn settle_closed(state: &mut OperationState) {
    settle_closed_with(state, |actual| {
        #[cfg(target_os = "ios")]
        {
            actual.confirmed_restart_custody()
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = actual;
            false
        }
    });
}

fn settle_closed_with(
    state: &mut OperationState,
    mut observe: impl FnMut(&OwnedTeardownOutcome) -> bool,
) {
    if !state.closed
        || !state.threads.is_empty()
        || !state.join_batches.is_empty()
        || !state.failed_joins.is_empty()
        || state.writes_in_flight != 0
        || state
            .current
            .as_ref()
            .is_some_and(|current| current.worker_active)
    {
        return;
    }
    let mut returned = std::mem::take(&mut state.archive.teardown.retained);
    returned.extend(state.archive.teardown.pending.take());
    for returned in returned {
        if current_matches(state, &returned.ticket) && closed_dischargeable(&returned, &mut observe)
        {
            let current = state.current.as_mut().expect("matching teardown");
            current.worker_active = false;
            operation::apply_terminal(current, returned.terminal.clone());
            state.archive.teardown.reservation = None;
            drop(returned);
        } else {
            state.archive.teardown.retained.push(returned);
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use solstone_core_backup_runtime::{
        HttpRequest, HttpResponse, HttpTransport, ToolOutput, ToolRequest, ToolRunner,
    };
    use std::time::Instant;
    use std::{
        fs,
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };
    use tower::ServiceExt;

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_unix(&self) -> i64 {
            1_700_000_000
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    struct NoIo(AtomicUsize);
    impl ToolRunner for NoIo {
        fn run(&self, _: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("unstarted teardown launched helper")
        }

        fn run_cancellable(
            &self,
            request: &ToolRequest<'_>,
            _cancellation: &ArchiveCancellation,
        ) -> std::io::Result<ToolOutput> {
            self.run(request)
        }
    }
    impl HttpTransport for NoIo {
        fn execute(
            &self,
            _: &HttpRequest,
        ) -> Result<HttpResponse, solstone_core_backup_runtime::hosted_runtime::HttpError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("unstarted teardown HTTP")
        }
    }

    /// A hosted journal whose catalog records media that exists only in the backup.
    fn fixture() -> (tempfile::TempDir, BackupWebDeps, Arc<NoIo>) {
        let root = crate::test_support::hosted_bound_root();
        solstone_core_backup::save_hosted_binding(
            root.path(),
            &solstone_core_backup::HostedBinding {
                // Never reached: coverage refuses first. A closed local port keeps it so.
                broker_endpoint: "http://127.0.0.1:9".into(),
                ..crate::test_support::hosted_binding()
            },
        )
        .unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260101/010000_001")).unwrap();
        solstone_core_offload::append_offload_event(
            root.path(),
            "20260101",
            "_default",
            "010000_001",
            "snapshot-a",
            &[solstone_core_retention::archive_catalog::OffloadFile {
                name: "clip.webm".into(),
                bytes: 3,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        let cache = crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
            free_bytes: None,
            total_bytes: None,
        });
        let mut deps = BackupWebDeps::production(root.path().to_path_buf(), cache);
        let io = Arc::new(NoIo(AtomicUsize::new(0)));
        deps.clock = Arc::new(FixedClock);
        deps.runner = io.clone();
        deps.http = io.clone();
        deps.executable = root.path().join("missing/bin/solstone");
        (root, deps, io)
    }

    fn actual_return(deps: &BackupWebDeps) -> Returned {
        let ticket = begin(deps).expect("reserve");
        assert!(start(&deps.operations, &ticket));
        let input = take_input(&mut deps.operations.lock().unwrap(), &ticket).unwrap();
        execute(input, || {}, crate::teardown_report::outcome)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn restart_observer_failure_and_panic_keep_effectful_returns_pending() {
        fn effectful() -> (tempfile::TempDir, BackupWebDeps, Returned) {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir(root.path().join("config")).unwrap();
            fs::write(
                root.path().join("config/journal.json"),
                br#"{"backup":{"enabled":true,"mode":"operated"}}"#,
            )
            .unwrap();
            solstone_core_backup::save_hosted_binding(
                root.path(),
                &solstone_core_backup::HostedBinding {
                    broker_endpoint: "http://127.0.0.1:9".into(),
                    ..crate::test_support::hosted_binding()
                },
            )
            .unwrap();
            let mut deps = BackupWebDeps::production(
                root.path().to_path_buf(),
                crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
                    free_bytes: None,
                    total_bytes: None,
                }),
            );
            deps.clock = Arc::new(FixedClock);
            let returned = actual_return(&deps);
            assert!(
                !returned.dischargeable(),
                "actual held after carrier reservation"
            );
            assert!(matches!(
                &returned.payload,
                Payload::Outcome(OwnedTeardownOutcome::Held(_))
            ));
            let Payload::Outcome(actual) = &returned.payload else {
                unreachable!()
            };
            assert!(actual.carrier().is_some());
            (root, deps, returned)
        }
        for panics in [false, true] {
            let (root, deps, returned) = effectful();
            let (other, _, unvisited) = effectful();
            let Payload::Outcome(actual) = &returned.payload else {
                unreachable!()
            };
            let original = Arc::as_ptr(actual.teardown_lease().unwrap());
            receive(&deps.operations, returned);
            receive(&deps.operations, unvisited);
            let lifetime = deps.lifetime();
            lifetime.request_stop();
            {
                let mut state = deps.operations.lock().unwrap();
                let observed = std::cell::Cell::new(0);
                settle_closed_with(&mut state, |_| {
                    observed.set(observed.get() + 1);
                    if panics {
                        panic!("new custody observer");
                    }
                    false
                });
                assert_eq!(
                    observed.get(),
                    1,
                    "matching owner reaches the actual observer"
                );
                assert_eq!(state.archive.teardown.retained.len(), 2);
                let Payload::Outcome(actual) = &state.archive.teardown.retained[0].payload else {
                    unreachable!()
                };
                assert_eq!(Arc::as_ptr(actual.teardown_lease().unwrap()), original);
                assert!(state.archive.teardown.is_busy());
            }
            for root in [&root, &other] {
                assert!(
                    solstone_core_journal_io::activity::ActivityLease::try_acquire(Arc::new(
                        solstone_core_journal_io::JournalRoot::open(root.path()).unwrap()
                    ))
                    .is_err()
                );
            }
            assert_eq!(
                lifetime.drain(
                    crate::test_support::join_executor(),
                    Instant::now() + Duration::from_millis(25)
                ),
                crate::BackupRouteDrain::Pending
            );
            assert_eq!(
                deps.operations
                    .lock()
                    .unwrap()
                    .archive
                    .teardown
                    .retained
                    .len(),
                2
            );
        }
    }

    #[test]
    fn stopped_lifetime_settles_actual_harmless_return_without_status_response() {
        let (root, deps, io) = fixture();
        let before = fs::read(root.path().join("config/journal.json")).unwrap();
        let returned = actual_return(&deps);
        assert!(returned.dischargeable());
        receive(&deps.operations, returned);
        assert!(
            deps.operations
                .lock()
                .unwrap()
                .archive
                .teardown
                .pending
                .is_some()
        );
        let lifetime = deps.lifetime();
        lifetime.request_stop();
        assert_eq!(
            lifetime.drain(
                crate::test_support::join_executor(),
                Instant::now() + Duration::from_secs(5)
            ),
            crate::BackupRouteDrain::Drained
        );
        let state = deps.operations.lock().unwrap();
        assert!(!state.archive.teardown.is_busy());
        assert_eq!(
            state.current.as_ref().unwrap().view.reason_code.as_deref(),
            Some("backup_only_held")
        );
        assert_eq!(io.0.load(Ordering::SeqCst), 0);
        assert_eq!(
            fs::read(root.path().join("config/journal.json")).unwrap(),
            before
        );
    }

    #[test]
    fn stopped_lifetime_keeps_actual_projection_panic_owner_pending() {
        let (_root, deps, _) = fixture();
        let ticket = begin(&deps).unwrap();
        assert!(start(&deps.operations, &ticket));
        worker(
            &deps.operations,
            &ticket,
            || {},
            |actual| {
                assert!(actual.refused_before_effects());
                panic!("lose the returned projection after actual owning execution");
            },
        );
        let lifetime = deps.lifetime();
        lifetime.request_stop();
        assert_eq!(
            lifetime.drain(
                crate::test_support::join_executor(),
                Instant::now() + Duration::from_millis(25)
            ),
            crate::BackupRouteDrain::Pending
        );
        assert!(deps.operations.lock().unwrap().archive.teardown.is_busy());
    }

    fn successor(deps: &BackupWebDeps) {
        assert!(!operation::is_busy(&deps.operations));
        assert!(operation::begin(&deps.operations, "enable", None, None, None).is_ok());
    }

    #[tokio::test]
    async fn teardown_route_discharges_backup_only_refusal_without_effects() {
        let (root, deps, io) = fixture();
        let raw = root.path().join("raw.webm");
        fs::write(&raw, b"owner raw bytes").unwrap();
        let config = root.path().join("config/journal.json");
        let binding = root.path().join("backup/hosted/binding.json");
        let before = [
            fs::read(&config).unwrap(),
            fs::read(&binding).unwrap(),
            fs::read(&raw).unwrap(),
        ];
        let (settled, receiver) = mpsc::channel();
        SETTLED.with(|hook| *hook.borrow_mut() = Some(settled));
        let posted = crate::routes_with_deps(deps.clone())
            .oneshot(
                Request::post("/app/backup/teardown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(posted.status().is_success());
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("actual worker settled");
        let response = crate::routes_with_deps(deps.clone())
            .oneshot(
                Request::get("/app/backup/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap())
                .unwrap();
        assert_eq!(body["operation"]["report"]["state"], "held");
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        {
            assert_eq!(body["operation"]["phase"], "error");
            assert_eq!(body["operation"]["reason_code"], "backup_only_held");
            assert_eq!(body["operation"]["report"]["phase"], "preparation");
            assert_eq!(
                body["operation"]["report"]["preparation_failure"]["refusal"],
                serde_json::json!({"kind":"reader","reason":"backup_only_held"})
            );
            // The discharged refusal no longer holds the journal activity lock.
            drop(
                solstone_core_journal_io::activity::ActivityLease::try_acquire(Arc::new(
                    solstone_core_journal_io::JournalRoot::open(root.path()).unwrap(),
                ))
                .unwrap(),
            );
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            assert_eq!(body["operation"]["phase"], "refused");
            assert_eq!(body["operation"]["report"]["phase"], "prelaunch");
            assert_eq!(
                body["operation"]["report"]["refusal"]["kind"],
                "unsupported_platform"
            );
        }
        assert_eq!(
            before,
            [
                fs::read(config).unwrap(),
                fs::read(binding).unwrap(),
                fs::read(raw).unwrap()
            ]
        );
        assert_eq!(io.0.load(Ordering::SeqCst), 0);
        successor(&deps);
    }

    #[cfg(not(windows))]
    #[test]
    fn teardown_worker_settlement_cancellation_unused_and_projection_panic() {
        let (_root, deps, io) = fixture();
        let ticket = begin(&deps).unwrap();
        assert!(start(&deps.operations, &ticket));
        let original = ticket.0.cancellation.clone();
        let (arrived, arrival) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let worker_slot = Arc::clone(&deps.operations);
        let worker_ticket = ticket.clone();
        let handle = std::thread::spawn(move || {
            worker(
                &worker_slot,
                &worker_ticket,
                || {
                    arrived.send(()).unwrap();
                    resume.recv_timeout(Duration::from_secs(5)).unwrap();
                },
                crate::teardown_report::outcome,
            )
        });
        arrival.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(operation::is_busy(&deps.operations));
        assert!(operation::begin(&deps.operations, "enable", None, None, None).is_err());
        assert_eq!(
            operation::request_archive_cancellation(&deps.operations, ticket.0.generation),
            crate::archive_operation::ArchiveCancellationRequest::Requested
        );
        assert!(original.is_cancelled());
        assert!(
            deps.operations
                .lock()
                .unwrap()
                .current
                .as_ref()
                .unwrap()
                .worker_active
        );
        release.send(()).unwrap();
        handle.join().unwrap();
        assert!(operation::is_busy(&deps.operations));
        assert!(matches!(
            deps.operations
                .lock()
                .unwrap()
                .archive
                .teardown
                .pending
                .as_ref()
                .unwrap()
                .payload,
            Payload::Unused(_)
        ));
        assert!(response(&deps).unwrap().status().is_success());
        successor(&deps);
        assert_eq!(io.0.load(Ordering::SeqCst), 0);

        let (_root, deps, _) = fixture();
        let ticket = begin(&deps).unwrap();
        deps.operations
            .lock()
            .unwrap()
            .current
            .as_mut()
            .unwrap()
            .view
            .phase = "error".into();
        assert!(!start(&deps.operations, &ticket));
        return_unused(&deps.operations, &ticket);
        assert!(operation::is_busy(&deps.operations));
        assert!(response(&deps).unwrap().status().is_success());
        successor(&deps);

        let (_root, deps, _) = fixture();
        operation::fail_next_worker_start();
        assert!(!spawn(&deps).unwrap());
        assert!(operation::is_busy(&deps.operations));
        assert!(response(&deps).unwrap().status().is_success());
        successor(&deps);

        let (_root, deps, _) = fixture();
        let ticket = begin(&deps).unwrap();
        assert!(start(&deps.operations, &ticket));
        let slot = Arc::clone(&deps.operations);
        std::thread::spawn(move || {
            worker(
                &slot,
                &ticket,
                || {},
                |actual| {
                    assert!(actual.refused_before_effects());
                    panic!("projection after actual outcome")
                },
            )
        })
        .join()
        .unwrap();
        let state = deps.operations.lock().unwrap();
        let held = &state.archive.teardown.retained[0];
        assert!(!held.projected);
        assert!(
            matches!(&held.payload, Payload::Outcome(actual) if actual.refused_before_effects())
        );
        assert_eq!(
            state.current.as_ref().unwrap().view.reason_code.as_deref(),
            Some("unknown")
        );
        drop(state);
        assert!(operation::is_busy(&deps.operations));
        assert!(response(&deps).is_none());
    }

    #[test]
    fn teardown_receiver_rejects_whole_returns_and_requires_matching_response() {
        for cut in ["missing", "stale", "inactive", "reservation", "poison"] {
            let (_root, deps, _) = fixture();
            let returned = actual_return(&deps);
            let expected_report = returned.terminal.report.clone();
            {
                let mut state = deps.operations.lock().unwrap();
                match cut {
                    "missing" => state.current = None,
                    "stale" => {
                        operation::install_slot(
                            &mut state.current,
                            "enable",
                            None,
                            None,
                            None,
                            None,
                        );
                    }
                    "inactive" => state.current.as_mut().unwrap().worker_active = false,
                    "reservation" => {
                        let original = &returned.ticket.0;
                        state.archive.teardown.reservation = Some(Arc::new(Reservation {
                            generation: original.generation,
                            journal: original.journal.clone(),
                            clock: Arc::clone(&original.clock),
                            cancellation: original.cancellation.clone(),
                            version: original.version,
                            runner: Arc::clone(&original.runner),
                        }));
                    }
                    "poison" => {}
                    _ => unreachable!(),
                }
            }
            let view = deps
                .operations
                .lock()
                .unwrap()
                .current
                .as_ref()
                .map(|current| current.view.clone());
            if cut == "poison" {
                let slot = Arc::clone(&deps.operations);
                assert!(
                    catch_unwind(AssertUnwindSafe(|| {
                        let _guard = slot.lock().unwrap();
                        panic!("poison");
                    }))
                    .is_err()
                );
            }
            receive(&deps.operations, returned);
            let state = deps
                .operations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            assert_eq!(
                state.current.as_ref().map(|current| &current.view),
                view.as_ref(),
                "{cut}"
            );
            assert_eq!(state.archive.teardown.retained.len(), 1, "{cut}");
            let held = &state.archive.teardown.retained[0];
            assert_eq!(held.terminal.report, expected_report);
            assert!(
                matches!(&held.payload, Payload::Outcome(actual) if actual.refused_before_effects())
            );
            drop(state);
            assert!(operation::is_busy(&deps.operations));
            if cut == "poison" {
                assert!(response(&deps).unwrap().status().is_server_error());
            }
        }
        let (root, deps, _) = fixture();
        receive(&deps.operations, actual_return(&deps));
        let config = root.path().join("config/journal.json");
        let original = fs::read(&config).unwrap();
        fs::write(&config, b"{broken").unwrap();
        assert!(response(&deps).unwrap().status().is_server_error());
        assert!(operation::is_busy(&deps.operations));
        assert!(
            deps.operations
                .lock()
                .unwrap()
                .archive
                .teardown
                .pending
                .is_some()
        );
        fs::write(config, original).unwrap();
        assert!(response(&deps).unwrap().status().is_success());
        successor(&deps);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn teardown_names_unplaced_media_and_a_missing_offload_record() {
        let (root, deps, io) = fixture();
        fs::remove_dir(root.path().join("chronicle/20260101/010000_001")).unwrap();
        let returned = actual_return(&deps);
        assert_eq!(
            returned.terminal.reason_code.as_deref(),
            Some("backup_only_unplaced")
        );
        assert!(returned.dischargeable());
        drop(returned);

        let (root, deps, _) = fixture();
        fs::remove_dir_all(root.path().join("health/offload")).unwrap();
        let config = root.path().join("config/journal.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
        value["backup"]["offload"] = serde_json::json!({"enabled": true});
        fs::write(&config, serde_json::to_vec(&value).unwrap()).unwrap();
        let returned = actual_return(&deps);
        assert_eq!(
            returned.terminal.reason_code.as_deref(),
            Some("offload_record_unavailable")
        );
        assert!(returned.dischargeable());
        assert_eq!(io.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn teardown_closed_types_and_borrowed_codec_preserve_absence_and_redact_errors() {
        fn closed<T: Send + 'static>() {}
        closed::<OwnedTeardownOutcome>();
        closed::<Returned>();
        closed::<Custody>();
        let (_root, deps, _) = fixture();
        let returned = actual_return(&deps);
        let Payload::Outcome(actual) = &returned.payload else {
            panic!("actual owner");
        };
        let report = crate::teardown_report::outcome(actual);
        for field in [
            "publication",
            "binding",
            "retirement",
            "clear_failure",
            "binding_failure",
        ] {
            assert!(report[field].is_null(), "{field}");
        }
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        {
            assert_eq!(report["remote"]["phase"], "not_started");
            assert_eq!(report["remote"]["broker_attempted"], false);
            assert!(report["preparation_failure"]["receipt"].is_null());
            assert!(report["preparation_failure"]["error"].is_null());
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        for field in ["remote", "preparation_failure"] {
            assert!(report[field].is_null(), "{field}");
        }
        assert_eq!(
            report["attempts"],
            serde_json::json!({"clear":false,"binding":false,"retirement":false})
        );
        assert!(
            !report
                .to_string()
                .contains(&deps.journal_root.to_string_lossy().to_string())
        );
        receive(&deps.operations, returned);
    }
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    #[cfg_attr(
        any(target_os = "linux", target_os = "macos"),
        ignore = "requires the copied installed package with the actual owned DNS member"
    )]
    fn stop_after_actual_finished_before_receive_drains_the_joined_worker() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            br#"{"backup":{"enabled":true,"mode":"operated"},"other":7}"#,
        )
        .unwrap();
        fs::write(root.path().join("original.txt"), b"retained original").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        solstone_core_backup::save_hosted_binding(
            root.path(),
            &solstone_core_backup::HostedBinding {
                broker_endpoint: endpoint,
                account_id: "account".into(),
                instance_id: "instance".into(),
                bucket: "bucket".into(),
                prefix: "owner/".into(),
                broker_token: "fixture-token".into(),
            },
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut peer = loop {
                match listener.accept() {
                    Ok((peer, _)) => break peer,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("bounded fixture accept: {error}"),
                }
            };
            // Accepted sockets inherit the listener's nonblocking mode on Windows.
            // Keep accept bounded above, then use the explicit I/O deadlines below.
            peer.set_nonblocking(false).unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            peer.set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                peer.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
                assert!(head.len() < 65536);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.starts_with("POST "));
            let length = head
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            assert!(length < 65536);
            let mut body = vec![0; length];
            peer.read_exact(&mut body).unwrap();
            let body = br#"{"error":"binding_superseded"}"#;
            write!(
                peer,
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            peer.write_all(body).unwrap();
        });
        let mut deps = BackupWebDeps::production(
            root.path().to_path_buf(),
            crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
                free_bytes: None,
                total_bytes: None,
            }),
        );
        deps.clock = Arc::new(FixedClock);
        let ticket = begin(&deps).unwrap();
        assert!(start(&deps.operations, &ticket));
        let input = take_input(&mut deps.operations.lock().unwrap(), &ticket).unwrap();
        let (finished, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let slot = deps.operations.clone();
        let worker = operation::spawn_thread(move || {
            let returned = execute(input, || {}, crate::teardown_report::outcome);
            assert!(
                matches!(
                    &returned.payload,
                    Payload::Outcome(OwnedTeardownOutcome::Finished(_))
                ),
                "actual coordinator: {:?}",
                returned.terminal.report
            );
            assert!(returned.dischargeable());
            finished.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            receive(&slot, returned);
        })
        .unwrap();
        deps.operations.lock().unwrap().threads.push(worker);
        ready
            .recv_timeout(Duration::from_secs(5))
            .expect("actual coordinator Finished");
        server.join().unwrap();
        let lifetime = deps.lifetime();
        lifetime.request_stop();
        assert_eq!(
            lifetime.drain(crate::test_support::join_executor(), Instant::now()),
            crate::BackupRouteDrain::Pending
        );
        assert!(ticket.0.cancellation.is_cancelled());
        release.send(()).unwrap();
        assert_eq!(
            lifetime.drain(
                crate::test_support::join_executor(),
                Instant::now() + Duration::from_secs(5)
            ),
            crate::BackupRouteDrain::Drained
        );
        let state = deps.operations.lock().unwrap();
        assert!(!state.archive.teardown.is_busy());
        assert!(!state.current.as_ref().unwrap().worker_active);
        assert_eq!(state.current.as_ref().unwrap().view.phase, "done");
        assert_eq!(
            fs::read(root.path().join("original.txt")).unwrap(),
            b"retained original"
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod unit_tests {
    use super::*;

    #[test]
    fn runner_whose_ios_journal_root_panics_does_not_panic_on_linux() {
        struct PanickingRunner;
        impl ToolRunner for PanickingRunner {
            fn run(
                &self,
                _: &solstone_core_backup_runtime::runner::ToolRequest<'_>,
            ) -> std::io::Result<solstone_core_backup_runtime::runner::ToolOutput> {
                Err(std::io::Error::other("unused"))
            }
            #[cfg(target_os = "ios")]
            fn ios_journal_root(&self) -> Option<&Arc<solstone_core_journal_io::JournalRoot>> {
                panic!("ios_journal_root must not be called on Linux");
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let mut deps = BackupWebDeps::production(
            dir.path().to_path_buf(),
            crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
                free_bytes: None,
                total_bytes: None,
            }),
        );
        deps.runner = Arc::new(PanickingRunner);
        let ticket = begin(&deps).unwrap();
        let input = take_input(&mut deps.operations.lock().unwrap(), &ticket).unwrap();
        let returned = execute(input, || {}, crate::teardown_report::outcome);
        assert!(returned.projected);
    }
}
