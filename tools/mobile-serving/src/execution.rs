// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Thinking, maintenance, recurrence and read-only sharing through the
//! authenticated owner router. The service retains the worker through Stop.

use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_generate::{ClientError, GenerateExecutor, GenerateRequest, GenerateResponse};
use solstone_core_journal_io::JournalRoot;
use solstone_core_talent_runtime::prepare::RuntimePaths;
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MatchRequest {
    person: String,
    day: String,
    facet: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageRequest {
    person: String,
    day: String,
    facet: String,
    activity: String,
}

#[path = "execution_schedules.rs"]
mod schedules;

enum Operation {
    Share {
        args: Vec<OsString>,
        package: bool,
    },
    Think(Vec<String>),
    Maintenance(String),
    ListRecurrences,
    PutRecurrence {
        name: String,
        command: Vec<String>,
        every: String,
        enabled: bool,
        max_runtime: Option<String>,
    },
    RemoveRecurrence(String),
}

struct Job {
    operation: Operation,
    cancelled: Arc<AtomicBool>,
    reply: oneshot::Sender<(StatusCode, Value)>,
}

struct CancelRequest(Arc<AtomicBool>);
impl Drop for CancelRequest {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Merge these routes into the authenticated owner router, and retain the
/// returned worker in the same service task collection that is joined at Stop.
/// It is never a public or unauthenticated listener.
pub fn routes_and_worker(
    root: Arc<JournalRoot>,
    payload: Option<PathBuf>,
    mut stop: watch::Receiver<bool>,
) -> (Router, impl Future<Output = ()> + Send + 'static) {
    let (sender, mut receiver) = mpsc::channel::<Job>(1);
    let schedule_routes = schedules::routes(sender.clone());
    let routes = Router::new()
        .route("/api/journal/share", get(discover))
        .route("/api/journal/share/match", post(match_preview))
        .route("/api/journal/share/package", post(package_preview))
        .with_state(sender)
        .merge(schedule_routes);
    let worker = async move {
        loop {
            if *stop.borrow() {
                return;
            }
            let job = tokio::select! {
                biased;
                _ = stop.changed() => return,
                job = receiver.recv() => match job { Some(job) => job, None => return },
            };
            let root = Arc::clone(&root);
            let payload = payload.clone();
            let stopped = stop.clone();
            // Await this task even if its HTTP receiver disconnects or Stop
            // arrives. The root and runtime stay owned until it has retired.
            if let Err(error) = tokio::task::spawn_blocking(move || {
                let journal = root.canonical_path().to_path_buf();
                let admitted =
                    match solstone_core_journal_io::whole_journal_restore::begin_target_use(
                        &journal,
                    ) {
                        Ok(guard) => guard,
                        Err(_) => {
                            let _ = job.reply.send((
                                StatusCode::CONFLICT,
                                json!({"error":"whole-journal restore is pending"}),
                            ));
                            return;
                        }
                    };
                let cancelled = || {
                    *stopped.borrow()
                        || job.cancelled.load(Ordering::Acquire)
                        || root.revalidate_canonical_binding().is_err()
                };
                let result = match job.operation {
                    Operation::Share { args, package } => match payload {
                        Some(payload) => {
                            let generate = OwnerGenerate {
                                root: &root,
                                cancelled: &cancelled,
                            };
                            execute_preview(&root, &payload, &args, package, &generate, &cancelled)
                        }
                        None => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            json!({"error":"payload_unavailable"}),
                        ),
                    },
                    operation => schedules::execute(&root, operation, &cancelled),
                };
                let _ = job.reply.send(result);
                drop(admitted);
            })
            .await
            {
                log::warn!("linked-share preview worker: {error}");
                return;
            }
        }
    };
    (routes, worker)
}

async fn discover() -> Json<Value> {
    Json(
        json!({"read_only":true,"thinking":"/api/journal/think","recurrences":"/api/journal/recurrences","operations":{
            "match":{"method":"POST","path":"/api/journal/share/match","required":["person","day"],"optional":["facet"]},
            "package":{"method":"POST","path":"/api/journal/share/package","required":["person","day","facet","activity"]}
        }}),
    )
}

async fn match_preview(
    State(sender): State<mpsc::Sender<Job>>,
    Json(request): Json<MatchRequest>,
) -> Response {
    let mut args = vec![
        "match".into(),
        "--person".into(),
        request.person.into(),
        "--day".into(),
        request.day.into(),
    ];
    if let Some(facet) = request.facet {
        args.extend(["--facet".into(), facet.into()]);
    }
    submit(sender, args, false).await
}

async fn package_preview(
    State(sender): State<mpsc::Sender<Job>>,
    Json(request): Json<PackageRequest>,
) -> Response {
    let args = vec![
        "package".into(),
        "--person".into(),
        request.person.into(),
        "--day".into(),
        request.day.into(),
        "--facet".into(),
        request.facet.into(),
        "--activity".into(),
        request.activity.into(),
    ];
    submit(sender, args, true).await
}

async fn submit(sender: mpsc::Sender<Job>, args: Vec<OsString>, package: bool) -> Response {
    submit_operation(sender, Operation::Share { args, package }).await
}

async fn submit_operation(sender: mpsc::Sender<Job>, operation: Operation) -> Response {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _request = CancelRequest(Arc::clone(&cancelled));
    let (reply, result) = oneshot::channel();
    match sender.try_send(Job {
        operation,
        cancelled,
        reply,
    }) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            return (StatusCode::CONFLICT, Json(json!({"error":"preview_busy"}))).into_response();
        }
        Err(mpsc::error::TrySendError::Closed(_)) => return stopped_response(),
    }
    match result.await {
        Ok((status, value)) => (status, Json(value)).into_response(),
        Err(_) => stopped_response(),
    }
}

fn stopped_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error":"preview_stopped"})),
    )
        .into_response()
}

struct OwnerGenerate<'a> {
    root: &'a JournalRoot,
    cancelled: &'a (dyn Fn() -> bool + Send + Sync),
}
impl GenerateExecutor for OwnerGenerate<'_> {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        let cancellation = solstone_core_generate::Cancellation::from_fn(self.cancelled);
        solstone_core_generate_wire::respond_with_context(
            request,
            &solstone_core_generate_wire::EndpointRuntime::default(),
            self.root.canonical_path(),
            |_| None,
            &solstone_core_generate_wire::GenerateCallContext {
                priority: Default::default(),
                cancellation: &cancellation,
            },
        )
        .map_err(ClientError::Decode)
    }
}

fn execute_preview(
    root: &JournalRoot,
    payload: &std::path::Path,
    args: &[OsString],
    package: bool,
    generate: &dyn GenerateExecutor,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> (StatusCode, Value) {
    if cancelled() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"preview_stopped"}),
        );
    }
    let paths = RuntimePaths {
        talent_root: payload.join("solstone/talent"),
        apps_root: payload.join("solstone/apps"),
        templates_dir: payload.join("solstone/think/templates"),
    };
    let talents =
        solstone_core_linked_share::RuntimeTalents::new(root.canonical_path(), paths, generate)
            .with_cancellation(cancelled);
    let failed = std::cell::RefCell::new(None);
    let caller = |name: &str, transcript: &str| {
        use solstone_core_linked_share::TalentCaller;
        let result = talents.call(name, transcript);
        if let Err(error) = &result {
            *failed.borrow_mut() = Some(error.clone());
        }
        result
    };
    let result = solstone_core_linked_share::run_cli(args, root.canonical_path(), &caller);
    if cancelled() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"preview_stopped"}),
        );
    }
    if let Some(error) = failed.into_inner() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error":"preview_failed","detail":error}),
        );
    }
    if result.exit_code != 0 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error":"preview_failed","detail":result.stderr,"exit_code":result.exit_code}),
        );
    }
    let value = if package {
        serde_json::from_str(&result.stdout)
    } else {
        result
            .stdout
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<Vec<Value>, _>>()
            .map(Value::Array)
    };
    match value {
        Ok(value) => (
            StatusCode::OK,
            json!({"read_only":true,"result":value,"warnings":
            result.stderr.lines().filter(|line| !line.is_empty()).collect::<Vec<_>>()}),
        ),
        Err(error) => {
            log::warn!("linked-share preview result: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error":"preview_result_invalid"}),
            )
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "execution_tests.rs"]
mod tests;
