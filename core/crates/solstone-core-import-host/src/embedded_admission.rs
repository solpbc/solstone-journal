// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One real calendar import through the embedded HTTP route.
//!
//! The route does not admit an attempt. `call::start` does, once, and the
//! joined handle is the completion signal. A second journal that admits first
//! is refused by that same callable.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_generate::{ClientError, GenerateRequest, GenerateResponse};
use solstone_core_import::cli_render::CliRun;
use solstone_core_import::{AttemptState, WorkStop, get_attempt_facts, read_import_metadata};
use solstone_core_import_web::{
    CapturedImport, ImportAdmission, ImportRouteRequest, ImportRouteSlot, RetainedImportRoute,
    mobile_import_contract, routes_with_embedded,
};
use solstone_core_journal_config::work_priority::WorkPriority;
use tower::ServiceExt;

use crate::call::{self, ImportSeams};

const ID: &str = "20261011_120000";
const CALENDAR: &[u8] = b"BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:1\nDTSTART:20260101T120000Z\nSUMMARY:Test\nEND:VEVENT\nEND:VCALENDAR\n";
const ALREADY_RUNNING: &str = "another import of this file is already running";

struct IdleText;

impl solstone_core_import::text::WireClient for IdleText {
    fn execute(&self, _request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Cancelled)
    }
}

fn idle_seams(journal: &Path) -> ImportSeams {
    ImportSeams {
        text: Box::new(IdleText),
        image: Box::new(solstone_core_import_sources::NullWireClient),
        model: Box::new(solstone_core_import_sources::NullDocumentModelClient),
        pdf: Box::new(solstone_core_import_sources::NullPdfWorker),
        reindex: Box::new(crate::registry::IndexScan {
            journal: journal.to_path_buf(),
        }),
    }
}

struct LiveRoute {
    journal: PathBuf,
    me: Mutex<Weak<LiveRoute>>,
    outcome: Arc<(Mutex<Option<CliRun>>, Condvar)>,
    argv: Mutex<Vec<String>>,
    previews: Mutex<Vec<CliRun>>,
}

struct LiveCaptured {
    route: Arc<LiveRoute>,
    request: ImportRouteRequest,
}

impl LiveRoute {
    fn new(journal: PathBuf) -> Arc<Self> {
        let route = Arc::new(Self {
            journal,
            me: Mutex::new(Weak::new()),
            outcome: Arc::new((Mutex::new(None), Condvar::new())),
            argv: Mutex::new(Vec::new()),
            previews: Mutex::new(Vec::new()),
        });
        *route.me.lock().unwrap() = Arc::downgrade(&route);
        route
    }

    fn arc(&self) -> Arc<Self> {
        self.me.lock().unwrap().upgrade().unwrap()
    }

    fn wait_run(&self) -> CliRun {
        let (slot, notify) = &*self.outcome;
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut guard = slot.lock().unwrap();
        loop {
            if let Some(run) = guard.take() {
                return run;
            }
            let now = Instant::now();
            if now >= deadline {
                panic!("embedded import did not finish");
            }
            let (next, _) = notify
                .wait_timeout(guard, deadline.saturating_duration_since(now))
                .unwrap();
            guard = next;
        }
    }
}

impl CapturedImport for LiveCaptured {
    fn preview(&self) -> Result<CliRun, String> {
        let seams = idle_seams(&self.route.journal);
        let run = call::preview(&self.request.args, &self.route.journal, &seams);
        self.route.previews.lock().unwrap().push(CliRun {
            stdout: run.stdout.clone(),
            stderr: run.stderr.clone(),
            exit_code: run.exit_code,
        });
        Ok(run)
    }

    fn run(&self) -> Result<CliRun, String> {
        let args = self.request.args.clone();
        *self.route.argv.lock().unwrap() = args.clone();
        let run = match call::start(
            args,
            self.route.journal.clone(),
            idle_seams(&self.route.journal),
            self.request.stop.clone(),
        ) {
            Ok(handle) => handle.join().0,
            Err(error) => CliRun {
                stdout: String::new(),
                stderr: error.to_string(),
                exit_code: 1,
            },
        };
        let stored = CliRun {
            stdout: run.stdout.clone(),
            stderr: run.stderr.clone(),
            exit_code: run.exit_code,
        };
        let (slot, notify) = &*self.route.outcome;
        *slot.lock().unwrap() = Some(stored);
        notify.notify_all();
        Ok(run)
    }
}

struct LiveAdmission {
    route: Arc<LiveRoute>,
    stop: WorkStop,
    priority: WorkPriority,
}

impl ImportAdmission for LiveAdmission {
    fn checkpoint(&self) -> Result<(), String> {
        if self.stop.is_requested() {
            Err("import stopped".to_owned())
        } else {
            Ok(())
        }
    }

    fn bind(self: Arc<Self>, args: Vec<String>) -> Result<Arc<dyn CapturedImport>, String> {
        Ok(Arc::new(LiveCaptured {
            route: Arc::clone(&self.route),
            request: ImportRouteRequest {
                args,
                stop: self.stop.clone(),
                priority: self.priority,
            },
        }))
    }
}

impl RetainedImportRoute for LiveRoute {
    fn prepare(
        &self,
        stop: WorkStop,
        priority: WorkPriority,
    ) -> Result<Arc<dyn ImportAdmission>, String> {
        Ok(Arc::new(LiveAdmission {
            route: self.arc(),
            stop,
            priority,
        }))
    }
}

fn stage(root: &Path) -> (Value, PathBuf) {
    let directory = root.join("imports").join(ID);
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("calendar.ics");
    std::fs::write(&source, CALENDAR).unwrap();
    let hash = solstone_core_import::hash_source(&source).unwrap();
    let metadata = json!({
        "file_path": source,
        "source_hint": "ics",
        "source_hash": hash.as_str(),
    });
    solstone_core_import::write_import_metadata(root, ID, metadata.as_object().unwrap()).unwrap();
    (
        json!({"path": source, "timestamp": ID, "import_id": ID}),
        source,
    )
}

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn control_argv(recorded: &[String], media: &Path) -> Vec<String> {
    let mut args = vec![media.display().to_string()];
    let mut skip_value = false;
    for arg in recorded.iter().skip(1) {
        if skip_value {
            skip_value = false;
            continue;
        }
        if arg == "--preview-token" || arg == "--snapshot-digest" {
            skip_value = true;
            continue;
        }
        args.push(arg.clone());
    }
    args
}

#[tokio::test]
async fn embedded_ics_start_admits_once_and_pre_admission_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let (data, _) = stage(root.path());
    let route = LiveRoute::new(root.path().to_path_buf());
    let slot = Arc::new(ImportRouteSlot::new());
    slot.install(route.clone()).unwrap();
    let app = routes_with_embedded(root.path().to_path_buf(), slot.clone());

    let preview = app
        .clone()
        .oneshot(post_json("/app/import/api/preview", &data))
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let previewed = json_body(preview).await;
    let (preview_exit, preview_stdout, preview_stderr) = {
        let previews = route.previews.lock().unwrap();
        let run = previews.first().unwrap();
        (run.exit_code, run.stdout.clone(), run.stderr.clone())
    };
    assert_eq!(preview_exit, 0, "{preview_stdout}{preview_stderr}");
    let size: Value = serde_json::from_str(preview_stdout.trim()).unwrap();
    assert_eq!(size["schema"], "solstone.import.size.v1");
    assert!(get_attempt_facts(&read_import_metadata(root.path(), ID).unwrap()).is_none());
    let mut start_body = data;
    start_body["preview_token"] = previewed["preview_token"].clone();
    let started = app
        .oneshot(post_json(mobile_import_contract::START_PATH, &start_body))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let run = route.wait_run();
    assert_eq!(run.exit_code, 0, "{}{}", run.stdout, run.stderr);
    assert!(!run.stdout.contains(ALREADY_RUNNING));
    assert!(!run.stderr.contains(ALREADY_RUNNING));
    let metadata = read_import_metadata(root.path(), ID).unwrap();
    let facts = get_attempt_facts(&metadata).unwrap();
    assert_eq!(facts.generation, 1);
    assert_eq!(facts.state, AttemptState::Completed);
    let recorded = route.argv.lock().unwrap().clone();
    assert!(!recorded.iter().any(|arg| arg == "solstone"
        || arg == "journal"
        || arg == "importer"
        || arg == "--dry-run"));
    assert!(recorded.iter().any(|arg| arg == ID));
    assert!(
        recorded
            .windows(2)
            .any(|pair| pair[0] == "--source" && pair[1] == "ics")
    );
    let digest = read_import_metadata(root.path(), ID).unwrap()["preview"]["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = previewed["preview_token"].as_str().unwrap();
    assert!(
        recorded
            .windows(2)
            .any(|pair| pair[0] == "--snapshot-digest" && pair[1] == digest)
    );
    assert!(
        recorded
            .windows(2)
            .any(|pair| pair[0] == "--preview-token" && pair[1] == token)
    );
    slot.retire().expect("checked import retirement");

    let other = tempfile::tempdir().unwrap();
    let (_, source) = stage(other.path());
    solstone_core_import::admit_running_attempt(other.path(), ID, 1, Some("ics")).unwrap();
    let refused = call::start(
        control_argv(&recorded, &source),
        other.path().to_path_buf(),
        idle_seams(other.path()),
        WorkStop::new(),
    )
    .unwrap()
    .join()
    .0;
    assert_eq!(refused.exit_code, 1);
    assert!(
        refused.stderr.contains(ALREADY_RUNNING) || refused.stdout.contains(ALREADY_RUNNING),
        "{}{}",
        refused.stdout,
        refused.stderr
    );
}
