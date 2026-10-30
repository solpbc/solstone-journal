// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use serde_json::{Map, Value, json};
use solstone_core_brain_owner::{
    BrainObservation, BrainObservationSink, ObservationCaller, ObservationOutcome,
    ObservationPhase, OwnerRefreshOptions, OwnerRefreshOutcome, ProductionSppPrerequisite,
    RespondWithConfigProbe,
};
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumEnvelope, CallosumReceiveEvent, CallosumSocketConnection,
};
use solstone_core_cortex::EmbeddedSppRenewal;
use solstone_core_journal_io::JournalRoot;
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

static NEXT_START: AtomicU64 = AtomicU64::new(1);
static REFRESH_LEASE: AtomicBool = AtomicBool::new(false);

struct RefreshLease;

impl RefreshLease {
    fn try_acquire() -> Option<Self> {
        REFRESH_LEASE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for RefreshLease {
    fn drop(&mut self) {
        REFRESH_LEASE.store(false, Ordering::Release);
    }
}

#[derive(Debug, PartialEq)]
enum Action {
    Refresh(OwnerRefreshOptions),
    Renew(String),
}

fn action(envelope: &CallosumEnvelope) -> Option<Action> {
    if envelope.tract != "supervisor" || envelope.event != "request" {
        return None;
    }
    let command = envelope.extra.get("cmd")?.as_array()?;
    let command: Vec<&str> = command.iter().map(Value::as_str).collect::<Option<_>>()?;
    if command.get(..3)? != ["solstone", "journal", "brain"] {
        return None;
    }
    let mut options = OwnerRefreshOptions {
        expected_fingerprint: None,
        expected_active_fingerprint: false,
        expect_active_fingerprint_absent: false,
    };
    match &command[3..] {
        ["refresh"] => {}
        ["refresh", "--json", "--expect-active-fingerprint-absent"] => {
            options.expect_active_fingerprint_absent = true;
        }
        [
            "refresh",
            "--json",
            "--expected-fingerprint",
            fingerprint,
            "--expected-active-fingerprint",
        ] if !fingerprint.is_empty() => {
            options.expected_fingerprint = Some((*fingerprint).into());
            options.expected_active_fingerprint = true;
        }
        [
            "renew-prerequisites",
            "--json",
            "--expected-fingerprint",
            fingerprint,
        ] if !fingerprint.is_empty() => {
            return Some(Action::Renew((*fingerprint).into()));
        }
        _ => return None,
    }
    Some(Action::Refresh(options))
}

fn execute(
    root: &JournalRoot,
    action: Action,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
    observation: Option<&BrainObservation>,
) -> i32 {
    if cancelled() || root.revalidate_canonical_binding().is_err() {
        if let Some(observation) = observation {
            observation.outcome(
                ObservationPhase::RootBinding,
                ObservationOutcome::ExecutionUnavailable,
            );
        }
        return 2;
    }
    let probe = RespondWithConfigProbe {
        runtime: solstone_core_generate_wire::EndpointRuntime::default(),
    };
    let outcome = match action {
        Action::Refresh(options) => solstone_core_brain_owner::refresh_observed(
            root.canonical_path(),
            &options,
            &probe,
            &ProductionSppPrerequisite,
            cancelled,
            observation,
        ),
        Action::Renew(fingerprint) => solstone_core_brain_owner::renew_prerequisites_observed(
            root.canonical_path(),
            Some(&fingerprint),
            &probe,
            &ProductionSppPrerequisite,
            cancelled,
            observation,
        ),
    };
    if cancelled() || root.revalidate_canonical_binding().is_err() {
        if let Some(observation) = observation {
            observation.outcome(
                ObservationPhase::RootBinding,
                ObservationOutcome::ExecutionUnavailable,
            );
        }
        return 2;
    }
    match outcome {
        OwnerRefreshOutcome::OperationalFailure { .. } | OwnerRefreshOutcome::Cancelled => 2,
        OwnerRefreshOutcome::PersistedReady { view }
        | OwnerRefreshOutcome::PersistedNonReady { view, .. }
        | OwnerRefreshOutcome::Busy { view }
        | OwnerRefreshOutcome::StaleExpectedFingerprint { view }
        | OwnerRefreshOutcome::LostFence { view } => {
            i32::from(solstone_core_brain_owner::owner_exit_code(&view))
        }
    }
}

fn event(
    connection: &CallosumSocketConnection,
    name: &str,
    reference: &str,
    extra: Map<String, Value>,
) {
    let mut fields = extra;
    fields.insert("ref".into(), json!(reference));
    fields.insert("service".into(), json!("brain"));
    connection.emit("supervisor", name, fields);
}

/// One refresh shared with the socket loop. A busy lease is a non-success
/// and does not start a second refresh. The returned code is the worker's.
pub(super) fn run_scheduled(
    root: &JournalRoot,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
    sink: Option<&BrainObservationSink>,
) -> i32 {
    let observation = sink.and_then(|sink| sink.begin(ObservationCaller::ScheduledRefresh));
    let Some(_lease) = RefreshLease::try_acquire() else {
        if let Some(observation) = &observation {
            observation.outcome(ObservationPhase::Admission, ObservationOutcome::Busy);
        }
        return 3;
    };
    execute(
        root,
        Action::Refresh(OwnerRefreshOptions {
            expected_fingerprint: None,
            expected_active_fingerprint: false,
            expect_active_fingerprint_absent: false,
        }),
        cancelled,
        observation.as_ref(),
    )
}

pub(super) fn connection(
    root: &JournalRoot,
) -> (
    CallosumSocketConnection,
    BrainObservationSink,
    Arc<solstone_core_callosum::BrainStartupCapture>,
) {
    let capture = solstone_core_callosum::BrainStartupCapture::new();
    let connection = CallosumSocketConnection::new_with_brain_startup_capture(
        root.canonical_path().join("health/callosum.sock"),
        Map::new(),
        Arc::clone(&capture),
    );
    let sink = BrainObservationSink::new(connection.diagnostic_emitter());
    (connection, sink, capture)
}

fn observed_fields(
    mut fields: Map<String, Value>,
    observation: Option<&BrainObservation>,
) -> Map<String, Value> {
    if let Some(observation) = observation {
        fields.insert("observation_id".into(), json!(observation.id()));
    }
    fields
}

pub(super) async fn run(
    root: Arc<JournalRoot>,
    stop: watch::Receiver<bool>,
    ready: oneshot::Sender<()>,
    connection: CallosumSocketConnection,
) {
    run_with_executor(
        root,
        stop,
        ready,
        Arc::new(|root, action, cancelled, observation| {
            execute(root, action, cancelled, observation)
        }),
        connection,
    )
    .await;
}

type Executor = Arc<
    dyn Fn(
            &JournalRoot,
            Action,
            &(dyn Fn() -> bool + Send + Sync),
            Option<&BrainObservation>,
        ) -> i32
        + Send
        + Sync,
>;

async fn run_with_executor(
    root: Arc<JournalRoot>,
    mut stop: watch::Receiver<bool>,
    ready: oneshot::Sender<()>,
    executor: Executor,
    mut connection: CallosumSocketConnection,
) {
    let sink = BrainObservationSink::new(connection.diagnostic_emitter());
    connection.start();
    let marker = NEXT_START.fetch_add(1, Ordering::Relaxed);
    let mut ready = Some(ready);
    let mut renewal: Option<EmbeddedSppRenewal> = None;
    let mut worker: Option<JoinHandle<i32>> = None;
    let mut active_ref = String::new();
    let mut active_observation: Option<BrainObservation> = None;
    let mut request_sequence = 0_u64;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            finished = async {
                match worker.as_mut() {
                    Some(worker) => Some(worker.await),
                    None => std::future::pending().await,
                }
            } => {
                let code = match finished {
                    Some(Ok(code)) => code,
                    _ => -1,
                };
                event(&connection, "stopped", &active_ref, observed_fields(Map::from_iter([("exit_code".into(), json!(code))]), active_observation.as_ref()));
                worker = None;
                active_ref.clear();
                active_observation = None;
            },
            _ = tick.tick() => {
                if let Some(renewal) = &renewal {
                    for _ in 0..8 {
                        let Some(outbound) = renewal.try_next() else { break };
                        connection.emit(&outbound.tract, &outbound.event, outbound.extra);
                    }
                }
            },
            received = connection.next_event() => {
                let Some(received) = received else { break };
                let envelope = match received {
                    CallosumReceiveEvent::Continuity { phase: CallosumConnectionPhase::Connected, .. } => {
                        connection.emit("supervisor", "status", Map::from_iter([
                            ("service".into(), json!("brain")),
                            ("embedded_brain_start".into(), json!(marker)),
                        ]));
                        continue;
                    },
                    CallosumReceiveEvent::Continuity { .. } => continue,
                    CallosumReceiveEvent::Envelope { envelope, .. } => envelope,
                };
                if *stop.borrow() { break; }
                if envelope.tract == "supervisor" && envelope.event == "status"
                    && envelope.extra.get("embedded_brain_start") == Some(&json!(marker))
                {
                    if let Some(ready) = ready.take() {
                        renewal = Some(EmbeddedSppRenewal::start(root.canonical_path().to_path_buf()));
                        let _ = ready.send(());
                    }
                    continue;
                }
                if let Some(renewal) = &renewal { renewal.observe(&envelope); }
                if ready.is_some() { continue; }
                let Some(action) = action(&envelope) else { continue };
                request_sequence = request_sequence.wrapping_add(1);
                let reference = envelope.extra.get("ref").and_then(Value::as_str)
                    .map(str::to_owned).unwrap_or_else(|| format!("embedded-brain-{marker}-{request_sequence}"));
                let observation = sink.begin(match &action {
                    Action::Refresh(_) => ObservationCaller::MobileRefresh,
                    Action::Renew(_) => ObservationCaller::MobileRenewal,
                });
                if worker.is_some() {
                    if let Some(observation) = &observation { observation.outcome(ObservationPhase::Admission, ObservationOutcome::Busy); }
                    event(&connection, "skipped", &reference, observed_fields(Map::from_iter([
                        ("active_ref".into(), json!(active_ref)), ("reason".into(), json!("busy")),
                    ]), observation.as_ref()));
                    continue;
                }
                let Some(lease) = RefreshLease::try_acquire() else {
                    if let Some(observation) = &observation { observation.outcome(ObservationPhase::Admission, ObservationOutcome::Busy); }
                    event(&connection, "skipped", &reference, observed_fields(Map::from_iter([
                        ("active_ref".into(), json!(active_ref)), ("reason".into(), json!("busy")),
                    ]), observation.as_ref()));
                    continue;
                };
                active_ref = reference;
                active_observation = observation;
                event(&connection, "started", &active_ref, observed_fields(Map::new(), active_observation.as_ref()));
                let observation = active_observation.clone();
                let root = Arc::clone(&root);
                let executor = Arc::clone(&executor);
                let stop_for_worker = stop.clone();
                // The real worker retains the root, the lease, and the fenced
                // refresh until it returns. Dropping a reply observer cannot
                // release that lease.
                worker = Some(tokio::task::spawn_blocking(move || {
                    let _lease = lease;
                    let _restore_use = match solstone_core_journal_io::whole_journal_restore::begin_target_use(
                        root.canonical_path(),
                    ) {
                        Ok(guard) => guard,
                        Err(_) => return 0,
                    };
                    let cancelled = || {
                        *stop_for_worker.borrow()
                            || root.revalidate_canonical_binding().is_err()
                    };
                    executor(&root, action, &cancelled, observation.as_ref())
                }));
            },
        }
    }
    if let Some(renewal) = &mut renewal {
        renewal.stop();
    }
    // Synchronous appraisal has no per-request cancellation API. Stop joins the
    // actual work instead of reporting quiescence after dropping its observer.
    if let Some(worker) = worker {
        let code = worker.await.unwrap_or(-1);
        event(
            &connection,
            "stopped",
            &active_ref,
            observed_fields(
                Map::from_iter([("exit_code".into(), json!(code))]),
                active_observation.as_ref(),
            ),
        );
    }
    connection.stop().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: Value) -> CallosumEnvelope {
        serde_json::from_value(json!({"tract":"supervisor","event":"request","cmd":command}))
            .unwrap()
    }

    #[test]
    fn admits_only_normal_refresh_and_maintained_renewal_commands() {
        assert!(matches!(
            action(&request(json!(["solstone", "journal", "brain", "refresh"]))),
            Some(Action::Refresh(_))
        ));
        assert!(matches!(
            action(&request(json!([
                "solstone",
                "journal",
                "brain",
                "refresh",
                "--json",
                "--expect-active-fingerprint-absent"
            ]))),
            Some(Action::Refresh(OwnerRefreshOptions {
                expect_active_fingerprint_absent: true,
                ..
            }))
        ));
        assert!(matches!(
            action(&request(json!([
                "solstone",
                "journal",
                "brain",
                "refresh",
                "--json",
                "--expected-fingerprint",
                "fp",
                "--expected-active-fingerprint"
            ]))),
            Some(Action::Refresh(OwnerRefreshOptions {
                expected_active_fingerprint: true,
                ..
            }))
        ));
        assert_eq!(
            action(&request(json!([
                "solstone",
                "journal",
                "brain",
                "renew-prerequisites",
                "--json",
                "--expected-fingerprint",
                "fp"
            ]))),
            Some(Action::Renew("fp".into()))
        );
        for command in [
            json!(null),
            json!(["solstone", "journal"]),
            json!(["solstone", "journal", "brain", "refresh", "--extra"]),
            json!(["other", "journal", "brain", "refresh"]),
            json!([
                "solstone",
                "journal",
                "brain",
                "renew-prerequisites",
                "--json",
                "--expected-fingerprint",
                ""
            ]),
        ] {
            assert_eq!(action(&request(command)), None);
        }
        let mut envelope = request(json!(["solstone", "journal", "brain", "refresh"]));
        envelope.event = "status".into();
        assert_eq!(action(&envelope), None);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reflected_consumer_preserves_refs_rejects_overlap_and_joins_actual_work() {
        use solstone_core_callosum::CallosumSocketServer;
        use std::{
            sync::{Mutex, atomic::AtomicUsize},
            time::Duration,
        };

        async fn received(
            connection: &mut CallosumSocketConnection,
            event: &str,
            reference: &str,
        ) -> CallosumEnvelope {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(CallosumReceiveEvent::Envelope { envelope, .. }) =
                        connection.next_event().await
                        && envelope.tract == "supervisor"
                        && envelope.event == event
                        && (reference.is_empty()
                            || envelope.extra.get("ref").and_then(Value::as_str) == Some(reference))
                    {
                        break envelope;
                    }
                }
            })
            .await
            .expect("correlated bus event")
        }

        for observer_loss in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
            let socket = root.canonical_path().join("health/callosum.sock");
            let bus = CallosumSocketServer::bind(socket.clone()).await.unwrap();
            let mut observer = CallosumSocketConnection::new(socket, Map::new());
            observer.start();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match observer.next_event().await {
                        Some(CallosumReceiveEvent::Continuity {
                            phase: CallosumConnectionPhase::Connected,
                            ..
                        }) => {
                            event(&observer, "status", "observer-registered", Map::new());
                        }
                        Some(CallosumReceiveEvent::Envelope { envelope, .. })
                            if envelope.extra.get("ref").and_then(Value::as_str)
                                == Some("observer-registered") =>
                        {
                            break;
                        }
                        _ => {}
                    }
                }
            })
            .await
            .unwrap();
            let (entered, mut entry) = tokio::sync::mpsc::unbounded_channel();
            let (release, released) = std::sync::mpsc::channel();
            let released = Mutex::new(released);
            let calls = Arc::new(AtomicUsize::new(0));
            let worker_calls = Arc::clone(&calls);
            let executor: Executor = Arc::new(move |root, action, _cancelled, observation| {
                assert!(observation.is_some());
                root.revalidate_canonical_binding().unwrap();
                worker_calls.fetch_add(1, Ordering::SeqCst);
                entered.send(action).unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                0
            });
            let (stop, stopping) = watch::channel(false);
            let (ready, registered) = oneshot::channel();
            let (connection, _, _capture) = connection(&root);
            let mut consumer = tokio::spawn(run_with_executor(
                Arc::clone(&root),
                stopping,
                ready,
                executor,
                connection,
            ));
            tokio::time::timeout(Duration::from_secs(5), registered)
                .await
                .unwrap()
                .unwrap();
            // Observe the maintained automatic startup refresh before injecting the
            // explicit owner request, so the test does not race that policy.
            let first = tokio::time::timeout(Duration::from_secs(5), entry.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(first, Action::Refresh(_)));
            let startup = received(&mut observer, "started", "").await;
            let startup_ref = startup.extra["ref"].as_str().unwrap().to_owned();
            assert_eq!(startup.extra["observation_id"].as_str().unwrap().len(), 32);
            let command = json!(["solstone", "journal", "brain", "refresh"]);
            event(
                &observer,
                "request",
                "owner-recheck",
                Map::from_iter([("cmd".into(), command.clone())]),
            );
            let skipped = received(&mut observer, "skipped", "owner-recheck").await;
            assert_eq!(skipped.extra["active_ref"], startup_ref);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            release.send(()).unwrap();
            received(&mut observer, "stopped", &startup_ref).await;
            event(
                &observer,
                "request",
                "owner-check",
                Map::from_iter([("cmd".into(), command)]),
            );
            tokio::time::timeout(Duration::from_secs(5), entry.recv())
                .await
                .unwrap()
                .unwrap();
            let owner_started = received(&mut observer, "started", "owner-check").await;
            if observer_loss {
                observer.stop().await;
            }
            stop.send(true).unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(25), &mut consumer)
                    .await
                    .is_err()
            );
            assert!(
                Arc::strong_count(&root) > 1,
                "actual worker retains root during retirement"
            );
            release.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), consumer)
                .await
                .unwrap()
                .unwrap();
            if !observer_loss {
                let stopped = received(&mut observer, "stopped", "owner-check").await;
                assert_eq!(stopped.extra["exit_code"], 0);
                assert_eq!(
                    stopped.extra["observation_id"],
                    owner_started.extra["observation_id"]
                );
            }
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(Arc::strong_count(&root), 1);
            observer.stop().await;
            bus.stop().await;
        }
    }
}

#[cfg(all(test, feature = "full-tests", feature = "processing"))]
mod startup_continuity_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use solstone_core_brain_owner::{
        BundledRuntimePrerequisiteAssessment, OwnerProbe, OwnerSppPrerequisite,
    };
    use solstone_core_generate::{
        Cancellation, ClientError, GenerateRequest, GenerateResponse, GeneratedResponse,
    };
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;
    use tower::ServiceExt;

    struct Probe;
    impl OwnerProbe for Probe {
        fn bundled_runtime_fingerprint(&self, _: &Path, _: &Map<String, Value>) -> Option<String> {
            None
        }
        fn assess_bundled_runtime(
            &self,
            _: &Path,
            _: Option<&str>,
        ) -> BundledRuntimePrerequisiteAssessment {
            BundledRuntimePrerequisiteAssessment {
                reason_code: None,
                desired_fingerprint_sha256: None,
                phase: None,
                runtime_reason: None,
            }
        }
        fn execute_generate(
            &self,
            _: &GenerateRequest,
            _: &Path,
            _: &Map<String, Value>,
            _: &Cancellation,
        ) -> Result<GenerateResponse, ClientError> {
            Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
                id: None,
                text: "OK".into(),
                model: "test-model".into(),
                usage: json!({}),
                finish_reason: "stop".into(),
                thinking: None,
                schema_validation: None,
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: Vec::new(),
            })))
        }
    }
    struct NoSpp;
    impl OwnerSppPrerequisite for NoSpp {
        fn assess(
            &self,
            _: &Path,
            _: &Map<String, Value>,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Value {
            panic!("synthetic cloud fixture must never use SPP")
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delayed_owner_sse_retains_the_actual_startup_relation_without_delaying_work() {
        for mode in [
            "disabled",
            "retained",
            "marked_loss",
            "overflow",
            "identity_unavailable",
            "allocation_unavailable",
        ] {
            let captured = mode != "disabled";
            let available = mode == "retained";
            let temp = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(temp.path().join("config")).unwrap();
            let config_path = temp.path().join("config/journal.json");
            std::fs::write(&config_path,br#"{"setup":{"completed_at":1767225600},"env":{"GOOGLE_API_KEY":"test-key"},"providers":{"active":{"provider":"google","model":"gemini-test"}}}"#).unwrap();
            let principal = temp.path().join("entities/principal");
            std::fs::create_dir_all(&principal).unwrap();
            std::fs::write(principal.join("entity.json"),br#"{"id":"principal","name":"Source Fixture","type":"Person","is_principal":true}"#).unwrap();
            let before = std::fs::read(&config_path).unwrap();
            let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
            let socket = root.canonical_path().join("health/callosum.sock");
            let bus = solstone_core_callosum::CallosumSocketServer::bind(&socket)
                .await
                .unwrap();
            let capture = match mode {
                "identity_unavailable" => {
                    solstone_core_callosum::test_support::unavailable_brain_startup_capture(false)
                }
                "allocation_unavailable" => {
                    solstone_core_callosum::test_support::unavailable_brain_startup_capture(true)
                }
                _ => solstone_core_callosum::BrainStartupCapture::new(),
            };
            if mode == "marked_loss" {
                capture.missing_identity();
            }
            if mode == "overflow" {
                let (emitter, _receiver) =
                    solstone_core_callosum::test_support::diagnostic_fixture_with_startup_capture(
                        256,
                        Arc::clone(&capture),
                    );
                for _ in 0..129 {
                    assert!(emitter.emit("supervisor","brain_observation",serde_json::from_value(json!({
                        "observation_id":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","caller":"generate",
                        "phase":"runtime_establish","outcome":"refused"
                    })).unwrap()));
                }
            }
            let connection = if captured {
                CallosumSocketConnection::new_with_brain_startup_capture(
                    &socket,
                    Map::new(),
                    Arc::clone(&capture),
                )
            } else {
                CallosumSocketConnection::new(&socket, Map::new())
            };
            let (done, complete) = oneshot::channel();
            let done = Mutex::new(Some(done));
            let (release, released) = std::sync::mpsc::channel();
            let released = Mutex::new(released);
            let executor: Executor = Arc::new(move |root, action, cancelled, observation| {
                assert!(matches!(action, Action::Refresh(_)));
                let Action::Refresh(options) = action else {
                    unreachable!()
                };
                let outcome = solstone_core_brain_owner::refresh_observed(
                    root.canonical_path(),
                    &options,
                    &Probe,
                    &NoSpp,
                    cancelled,
                    observation,
                );
                assert!(matches!(
                    outcome,
                    OwnerRefreshOutcome::PersistedReady { .. }
                ));
                // Real decision and writer/lease assertions precede retained
                // evidence assertions, including in the capture-removal control.
                assert!(
                    !solstone_core_brain::probe_file_lease_held(
                        &solstone_core_brain::brain_refresh_lease_path(root.canonical_path())
                    )
                    .unwrap()
                );
                done.lock().unwrap().take().unwrap().send(()).unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                0
            });
            let (stop, stopping) = watch::channel(false);
            let (ready, registered) = oneshot::channel();
            let consumer = tokio::spawn(run_with_executor(
                Arc::clone(&root),
                stopping,
                ready,
                executor,
                connection,
            ));
            tokio::time::timeout(Duration::from_secs(5), registered)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), complete)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(std::fs::read(&config_path).unwrap(), before);
            // Register a test reader after appraisal returns, then release the
            // actual worker and await its reflected stopped row before SSE opens.
            let mut barrier = CallosumSocketConnection::new(&socket, Map::new());
            barrier.start();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match barrier.next_event().await {
                        Some(CallosumReceiveEvent::Continuity {
                            phase: CallosumConnectionPhase::Connected,
                            ..
                        }) => {
                            barrier.emit(
                                "supervisor",
                                "request",
                                Map::from_iter([
                                    (
                                        "cmd".into(),
                                        json!(["solstone", "journal", "brain", "not-an-action"]),
                                    ),
                                    ("ref".into(), json!("barrier")),
                                ]),
                            );
                        }
                        Some(CallosumReceiveEvent::Envelope { envelope, .. })
                            if envelope.extra.get("ref") == Some(&json!("barrier")) =>
                        {
                            break;
                        }
                        _ => {}
                    }
                }
            })
            .await
            .unwrap();
            release.send(()).unwrap();
            let stopped = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(envelope) = barrier.next_message().await
                        && envelope.event == "stopped"
                        && envelope.extra.get("service") == Some(&json!("brain"))
                    {
                        break envelope;
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(stopped.extra["exit_code"], 0);
            // The actual maintained router serializes named SSE events. No
            // listener, device, credential or production request is involved.
            let router = solstone_core_convey_shell::router(root.canonical_path().to_path_buf());
            let router = if captured {
                router.layer(axum::Extension(Arc::clone(&capture)))
            } else {
                router
            };
            let response = router
                .oneshot(
                    Request::builder()
                        .uri("/sse/events")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let mut body = response.into_body();
            let mut wire = String::new();
            let mut capture_complete = false;
            let delivered = tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(frame) = body.frame().await {
                    let frame = frame.unwrap();
                    if let Some(data) = frame.data_ref() {
                        wire.push_str(std::str::from_utf8(data).unwrap());
                    }
                    if captured && wire.contains("\"coverage\":\"complete\"") {
                        capture_complete = true;
                    }
                    if available && capture_complete && wire.contains("\"state\":\"connected\"") {
                        break;
                    }
                    if !available && wire.contains("\"state\":\"connected\"") {
                        break;
                    }
                }
            })
            .await
            .is_ok();
            drop(body);
            stop.send(true).unwrap();
            tokio::time::timeout(Duration::from_secs(5), consumer)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(std::fs::read(&config_path).unwrap(), before);
            assert_eq!(Arc::strong_count(&root), 1);
            assert!(
                !solstone_core_brain::probe_file_lease_held(
                    &solstone_core_brain::brain_refresh_lease_path(root.canonical_path())
                )
                .unwrap()
            );
            barrier.stop().await;
            bus.stop().await;
            println!(
                "STARTUP_CAUSAL_EXECUTION={}",
                json!({"mode":mode,"exit_code":stopped.extra["exit_code"],
                "config_unchanged":true,"permit_released":true,"worker_joined":true,"root_released":true})
            );
            assert!(
                delivered,
                "retained prefix must be available across delayed subscription"
            );
            if available {
                assert!(capture_complete);
                assert!(wire.contains("\"event\":\"started\""));
                assert!(wire.contains("\"phase\":\"permit\""));
                assert!(wire.contains("\"outcome\":\"published\""));
                assert!(wire.contains("\"phase\":\"terminal\""));
                assert!(wire.contains("\"event\":\"stopped\""));
            } else if captured {
                assert!(!capture_complete);
                let expected = if matches!(mode, "marked_loss" | "overflow") {
                    "incomplete"
                } else {
                    "unavailable"
                };
                assert!(wire.contains(&format!("\"coverage\":\"{expected}\"")));
            } else {
                assert!(!wire.contains("brain_startup"));
                assert!(!wire.contains("\"phase\":\"permit\""));
            }
            println!(
                "STARTUP_CAUSAL_WIRE={}",
                json!({"captured":captured,"available":available,"mode":mode,"wire":wire})
            );
            if captured {
                let mut cursor = solstone_core_callosum::BrainStartupCursor::default();
                let closed = capture.next(&mut cursor).await.unwrap();
                assert!(closed.terminal);
                assert!(closed.data.contains("\"coverage\":\"closed\""));
            }
        }
    }
}
