// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-owned orchestration of the production router and authenticated device door.

use axum::{
    body::{Body, to_bytes},
    http::{Request, header::CONTENT_TYPE},
};
use serde_json::{Value, json};
use solstone_core_convey_shell::EmbeddedConveyHandle;
#[cfg(target_os = "ios")]
use solstone_core_convey_shell::bind_embedded_device_door_with_root;
#[cfg(not(target_os = "ios"))]
use solstone_core_convey_shell::bind_embedded_device_door_with_routes_and_startup;
use solstone_core_journal_io::JournalRoot;
use std::{
    net::SocketAddrV4,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

mod backup;
mod brain;

#[cfg(feature = "audio")]
pub(crate) mod audio;

#[cfg(any(feature = "audio", feature = "processing"))]
mod custody;
#[cfg(feature = "processing")]
pub mod foreground;

pub struct OwnerResponse {
    pub status: u16,
    pub body: String,
}

/// Retain this value in the app's ordinary worker, alongside its private root.
/// Owner requests must be admitted by private in-process or app-owned IPC.
pub struct MobileServing {
    #[cfg(feature = "audio")]
    audio: Option<audio::AudioWorker>,
    bus: solstone_core_callosum::CallosumSocketServer,
    handle: EmbeddedConveyHandle,
    runtime: tokio::runtime::Runtime,
    root: Arc<JournalRoot>,
    interface_address: SocketAddrV4,
    services: Mutex<Option<AppServices>>,
    stop: MobileServingStop,
    shutdown_complete: std::sync::atomic::AtomicBool,
    shutdown_failed: std::sync::atomic::AtomicBool,
    service_failures: Mutex<Vec<tokio::task::JoinError>>,
    #[cfg(all(test, feature = "full-tests"))]
    shutdown_test_fault: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// Clone-only early-stop signal. The serving owner retains every worker and runtime.
#[derive(Clone)]
pub struct MobileServingStop {
    convey: solstone_core_convey_shell::EmbeddedConveyStop,
    services: watch::Sender<bool>,
    backup: solstone_core_backup_web::BackupRouteLifetime,
}

struct ServingStartup {
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    stop_ready: Arc<dyn Fn(MobileServingStop) + Send + Sync>,
}

impl MobileServingStop {
    pub fn request_stop(&self) {
        self.backup.request_stop();
        self.services.send_replace(true);
        self.convey.request_stop();
    }
    pub fn is_stopping(&self) -> bool {
        *self.services.borrow()
    }
}

#[derive(Debug)]
pub struct ShutdownFailure;

/// Startup can expose the authenticated door before a consumer refuses readiness.
/// The caller must retain and retire this actual owner before allowing replacement.
pub struct StartupFailure {
    message: String,
    owner: Option<Box<MobileServing>>,
}

impl StartupFailure {
    pub fn into_parts(self) -> (String, Option<Box<MobileServing>>) {
        (self.message, self.owner)
    }
}

impl From<String> for StartupFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            owner: None,
        }
    }
}

impl From<&str> for StartupFailure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

struct AppServices {
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    backup: JoinHandle<Result<(), tokio::task::JoinError>>,
}

impl MobileServing {
    pub fn stop_handle(&self) -> MobileServingStop {
        self.stop.clone()
    }

    /// Share the backup operation ownership the embedded routes already hold.
    pub fn backup_dependencies(&self) -> solstone_core_backup_web::BackupWebDeps {
        self.handle.backup_dependencies()
    }

    pub fn interface_address(&self) -> SocketAddrV4 {
        self.interface_address
    }

    pub fn start(root: PathBuf, address: SocketAddrV4) -> Result<Self, String> {
        Self::start_using(root, address, None)
    }

    pub fn start_with_payload(
        root: PathBuf,
        address: SocketAddrV4,
        payload_root: PathBuf,
    ) -> Result<Self, String> {
        Self::start_using(root, address, Some(payload_root))
    }

    fn start_using(
        root: PathBuf,
        address: SocketAddrV4,
        payload_root: Option<PathBuf>,
    ) -> Result<Self, String> {
        Self::start_using_owned(root, address, payload_root, None).map_err(|failure| {
            let (message, owner) = failure.into_parts();
            if let Some(owner) = owner {
                // Existing synchronous callers keep the complete owner until
                // settlement. JNI uses the owned API and observes it separately.
                owner.shutdown();
            }
            message
        })
    }

    /// Return any exposed startup owner for caller-owned asynchronous retirement.
    pub fn start_with_payload_for_retirement(
        root: PathBuf,
        address: SocketAddrV4,
        payload_root: PathBuf,
    ) -> Result<Self, StartupFailure> {
        Self::start_using_owned(root, address, Some(payload_root), None)
    }

    /// Install the embedding app's stop signal before listener exposure or
    /// registration waits. A refused startup still returns its actual owner.
    pub fn start_with_payload_for_retirement_observed(
        root: PathBuf,
        address: SocketAddrV4,
        payload_root: PathBuf,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
        stop_ready: Arc<dyn Fn(MobileServingStop) + Send + Sync>,
    ) -> Result<Self, StartupFailure> {
        Self::start_using_owned(
            root,
            address,
            Some(payload_root),
            Some(ServingStartup {
                cancelled,
                stop_ready,
            }),
        )
    }

    fn start_using_owned(
        root: PathBuf,
        address: SocketAddrV4,
        payload_root: Option<PathBuf>,
        startup: Option<ServingStartup>,
    ) -> Result<Self, StartupFailure> {
        if address.ip().is_unspecified() || address.ip().is_multicast() || address.port() == 0 {
            return Err("an explicit interface address and nonzero port are required".into());
        }
        let root = Arc::new(JournalRoot::open(&root).map_err(|e| e.to_string())?);
        root.revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        solstone_core_journal_config_write::persist_direct_door_port(
            root.canonical_path(),
            address.port(),
        )
        .map_err(|e| e.to_string())?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        // Keep the production ingest notification boundary live. Durable receipt
        // discovery provides restart recovery independently of this transient bus.
        let bus = runtime
            .block_on(solstone_core_callosum::CallosumSocketServer::bind(
                root.canonical_path().join("health/callosum.sock"),
            ))
            .map_err(|e| e.to_string())?;
        let (shutdown, mut stop) = watch::channel(false);
        let early_shutdown = shutdown.clone();
        let cancelled = startup
            .as_ref()
            .map(|startup| Arc::clone(&startup.cancelled))
            .unwrap_or_else(|| Arc::new(|| false));
        let stop_ready = startup
            .as_ref()
            .map(|startup| Arc::clone(&startup.stop_ready))
            .unwrap_or_else(|| Arc::new(|_| {}));
        let early_cancelled = Arc::clone(&cancelled);
        let handle = runtime
            .block_on(async {
                let startup = solstone_core_convey_shell::EmbeddedStartup {
                    cancelled: Arc::clone(&cancelled),
                    stop_ready: Arc::new(move |convey, backup| {
                        let signal = MobileServingStop {
                            convey,
                            services: early_shutdown.clone(),
                            backup,
                        };
                        stop_ready(signal.clone());
                        if early_cancelled() {
                            signal.request_stop();
                        }
                    }),
                };
                #[cfg(target_os = "ios")]
                let binding = bind_embedded_device_door_with_root(
                    Arc::clone(&root),
                    address,
                    payload_root,
                    solstone_core_mcp_endpoint::owner_routes(root.canonical_path().to_path_buf()),
                    startup,
                );
                #[cfg(not(target_os = "ios"))]
                let binding = bind_embedded_device_door_with_routes_and_startup(
                    root.canonical_path().to_path_buf(),
                    address,
                    payload_root,
                    solstone_core_mcp_endpoint::owner_routes(root.canonical_path().to_path_buf()),
                    startup,
                );
                binding.await
            })
            .map_err(|e| e.to_string())?;
        let (ready, registered) = oneshot::channel();
        let brain_task = runtime.spawn(brain::run(Arc::clone(&root), stop.clone(), ready));
        let (backup_ready, subscribed) = oneshot::channel();
        let backup_task = runtime.spawn(backup::run(
            Arc::clone(&root),
            handle.backup_dependencies(),
            stop.clone(),
            backup_ready,
        ));
        let mcp_root = root.canonical_path().to_path_buf();
        let mut mcp_stop = stop.clone();
        let mcp_task = runtime.spawn(async move {
            loop {
                if *mcp_stop.borrow() {
                    break;
                }
                let result = solstone_core_mcp_endpoint::run_embedded_service(
                    mcp_root.clone(),
                    mcp_stop.clone(),
                )
                .await;
                if *mcp_stop.borrow() {
                    break;
                }
                if let Err(error) = result {
                    log::warn!("embedded agent service stopped: {}", error.class());
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                    _ = mcp_stop.changed() => break,
                }
            }
        });
        let relay_root = root.canonical_path().to_path_buf();
        let relay_task = runtime.spawn(async move {
            loop {
                if *stop.borrow() {
                    break;
                }
                let result = solstone_core_spl::run_embedded_service(
                    relay_root.clone(),
                    solstone_core_spl::Verbosity::Quiet,
                    stop.clone(),
                )
                .await;
                if *stop.borrow() {
                    break;
                }
                if let Err(error) = result {
                    log::warn!("embedded relay service stopped: {}", error.class());
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                    _ = stop.changed() => break,
                }
            }
        });
        let stop_handle = MobileServingStop {
            convey: handle.stop_handle(),
            services: shutdown.clone(),
            backup: handle.backup_dependencies().lifetime(),
        };
        let owner = Self {
            stop: stop_handle,
            #[cfg(feature = "audio")]
            audio: None,
            bus,
            handle,
            runtime,
            root,
            interface_address: address,
            services: Mutex::new(Some(AppServices {
                shutdown,
                tasks: vec![brain_task, relay_task, mcp_task],
                backup: backup_task,
            })),
            shutdown_complete: std::sync::atomic::AtomicBool::new(false),
            shutdown_failed: std::sync::atomic::AtomicBool::new(false),
            service_failures: Mutex::new(Vec::new()),
            #[cfg(all(test, feature = "full-tests"))]
            shutdown_test_fault: Mutex::new(None),
        };
        owner.finish_registration(registered, subscribed)
    }

    fn finish_registration(
        self,
        brain_registered: oneshot::Receiver<()>,
        backup_registered: oneshot::Receiver<()>,
    ) -> Result<Self, StartupFailure> {
        if self.stop.is_stopping() {
            return Err(StartupFailure {
                message: "stop requested during startup; journal retained".into(),
                owner: Some(Box::new(self)),
            });
        }
        for (registered, message) in [
            (brain_registered, "embedded Brain consumer did not register"),
            (
                backup_registered,
                "embedded backup consumer did not register",
            ),
        ] {
            let ready = self.runtime.block_on(async {
                matches!(
                    tokio::time::timeout(Duration::from_secs(5), registered).await,
                    Ok(Ok(()))
                )
            });
            if self.stop.is_stopping() {
                return Err(StartupFailure {
                    message: "stop requested during startup; journal retained".into(),
                    owner: Some(Box::new(self)),
                });
            }
            if !ready {
                self.handle.backup_dependencies().lifetime().request_stop();
                self.handle.shutdown();
                if let Some(services) = self
                    .services
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                {
                    services.shutdown.send_replace(true);
                }
                return Err(StartupFailure {
                    message: message.into(),
                    owner: Some(Box::new(self)),
                });
            }
        }
        #[cfg(target_os = "ios")]
        if self.root.revalidate_canonical_binding().is_err() {
            self.handle.backup_dependencies().lifetime().request_stop();
            self.handle.shutdown();
            if let Some(services) = self
                .services
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
            {
                services.shutdown.send_replace(true);
            }
            return Err(StartupFailure {
                message: "embedded journal root changed during startup".into(),
                owner: Some(Box::new(self)),
            });
        }
        Ok(self)
    }

    pub fn status(&self) -> Value {
        #[allow(unused_mut)]
        let mut value = json!({"root_current": self.root.revalidate_canonical_binding().is_ok(),
            "door_address": self.handle.live_door_addr().map(|a| a.to_string())});
        #[cfg(feature = "audio")]
        {
            value["audio"] = self
                .audio
                .as_ref()
                .map_or(json!({"state":"disabled"}), |a| a.status());
        }
        value
    }

    /// Enable the dedicated foreground app's local processing pass. Successful
    /// input never needs a private command to trigger transcription.
    #[cfg(feature = "audio")]
    pub fn enable_audio(
        &mut self,
        models: PathBuf,
        model: PathBuf,
        scratch: PathBuf,
    ) -> Result<(), String> {
        if self.audio.is_some() {
            return Err("audio worker already enabled".into());
        }
        self.enable_audio_with_cancellation(models, model, scratch, std::sync::Arc::new(|| false))
    }

    #[cfg(feature = "audio")]
    pub fn enable_audio_with_cancellation(
        &mut self,
        models: PathBuf,
        model: PathBuf,
        scratch: PathBuf,
        cancelled: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<(), String> {
        if self.stop.is_stopping() || cancelled() {
            return Err("server stop pending".into());
        }
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        if self.audio.is_some() {
            return Ok(());
        }
        self.audio = Some(audio::AudioWorker::start_with_cancellation(
            self.root.canonical_path().to_path_buf(),
            models,
            model,
            scratch,
            cancelled,
        )?);
        Ok(())
    }

    #[cfg(feature = "audio")]
    pub fn cancel_audio(&self) -> Result<(), String> {
        self.audio.as_ref().ok_or("audio worker disabled")?.cancel();
        Ok(())
    }

    #[cfg(feature = "audio")]
    pub fn resume_audio(&self) -> Result<(), String> {
        self.audio.as_ref().ok_or("audio worker disabled")?.resume();
        Ok(())
    }

    /// Dispatch only a request already admitted by the app's private owner boundary.
    pub fn request_owner(
        &self,
        method: &str,
        uri: &str,
        body: &str,
    ) -> Result<OwnerResponse, String> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let uri: axum::http::Uri = uri
            .parse()
            .map_err(|e: axum::http::uri::InvalidUri| e.to_string())?;
        if uri.scheme().is_some() || uri.authority().is_some() || !uri.path().starts_with('/') {
            return Err("owner request requires an origin-form path".into());
        }
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .map_err(|e| e.to_string())?;
        self.runtime.block_on(async {
            let response = self.handle.request_owner(request).await;
            let status = response.status().as_u16();
            let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .map_err(|e| e.to_string())?;
            let body = String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?;
            Ok(OwnerResponse { status, body })
        })
    }

    pub fn index(&self) -> Result<Value, String> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let report =
            solstone_core_indexer_store::scan::scan_journal(self.root.canonical_path(), false)
                .map_err(|e| e.to_string())?;
        if report.failed != 0 {
            return Err(format!("index scan failed: {:?}", report.failures));
        }
        Ok(
            json!({"indexed":report.indexed,"removed":report.removed,"skipped":report.skipped,
            "failed":report.failed,"warnings":report.warnings}),
        )
    }

    /// Close and settle the actual app-owned services before destroying this owner.
    /// On failure the caller must retain this value, including its Runtime.
    pub fn shutdown_for_retirement(&self) -> Result<(), ShutdownFailure> {
        use std::sync::atomic::Ordering;
        if self.shutdown_failed.load(Ordering::Acquire) {
            return Err(ShutdownFailure);
        }
        if self.shutdown_complete.load(Ordering::Acquire) {
            return Ok(());
        }
        self.stop.request_stop();
        #[cfg(feature = "audio")]
        if let Some(audio) = &self.audio {
            audio.stop();
        }
        let mut service_owner = self.services.lock().unwrap_or_else(|p| p.into_inner());
        if self.shutdown_failed.load(Ordering::Acquire) {
            return Err(ShutdownFailure);
        }
        if self.shutdown_complete.load(Ordering::Acquire) {
            return Ok(());
        }
        let lifetime = self.handle.backup_dependencies().lifetime();
        lifetime.request_stop();
        #[cfg(all(test, feature = "full-tests"))]
        {
            let fault = self.shutdown_test_fault.lock().unwrap().take();
            if let Some(fault) = fault {
                fault();
            }
        }
        self.handle.shutdown();
        if let Some(services) = service_owner.take() {
            let _ = services.shutdown.send(true);
            self.runtime.block_on(async {
                for task in services.tasks {
                    if let Err(failure) = task.await {
                        self.service_failures
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(failure);
                        self.shutdown_failed.store(true, Ordering::Release);
                    }
                }
                let failure = match services.backup.await {
                    Ok(Ok(())) => None,
                    Ok(Err(failure)) | Err(failure) => Some(failure),
                };
                if let Some(failure) = failure {
                    self.service_failures
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(failure);
                    self.shutdown_failed.store(true, Ordering::Release);
                }
            });
            if self.shutdown_failed.load(Ordering::Acquire) {
                return Err(ShutdownFailure);
            }
        }
        self.runtime.block_on(self.handle.shutdown_and_wait());
        loop {
            match lifetime.drain(
                self.runtime.handle(),
                Instant::now() + Duration::from_secs(3600),
            ) {
                solstone_core_backup_web::BackupRouteDrain::Drained => break,
                solstone_core_backup_web::BackupRouteDrain::Pending => continue,
                solstone_core_backup_web::BackupRouteDrain::Failure => {
                    self.shutdown_failed.store(true, Ordering::Release);
                    return Err(ShutdownFailure);
                }
            }
        }
        self.runtime.block_on(self.bus.stop());
        self.shutdown_complete.store(true, Ordering::Release);
        drop(service_owner);
        Ok(())
    }

    pub fn shutdown(&self) {
        if self.shutdown_for_retirement().is_err() {
            log::error!("embedded service shutdown failed; actual journal owner retained");
            loop {
                std::thread::park();
            }
        }
    }
}

impl Drop for MobileServing {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "../../android-runtime/native/src/retirement.rs"]
// JNI uses additional registry accessors outside this partial host test import.
#[allow(dead_code)]
mod android_retirement;

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    fn fixture() -> (MobileServing, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
            unreachable!()
        };
        drop(probe);
        let server = MobileServing::start_with_payload(
            directory.path().to_path_buf(),
            address,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload"),
        )
        .unwrap();
        (server, directory)
    }

    #[test]
    fn startup_readiness_refusal_returns_actual_owner_until_admitted_work_settles() {
        use crate::android_retirement::{Outcome, Retirement};
        let (server, directory) = fixture();
        let runtime = server.runtime.handle().clone();
        let lifetime = server.handle.backup_dependencies().lifetime();
        let permit = lifetime.admit_maintenance().unwrap();
        let (release, held) = std::sync::mpsc::channel();
        let worker = runtime.spawn_blocking(move || {
            let _permit = permit;
            held.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let (brain, brain_ready) = oneshot::channel();
        let (backup, backup_ready) = oneshot::channel();
        brain.send(()).unwrap();
        drop(backup); // A real failed registration receiver, after door exposure.
        let failure = match server.finish_registration(brain_ready, backup_ready) {
            Ok(_) => panic!("registration must fail"),
            Err(failure) => failure,
        };
        let (message, owner) = failure.into_parts();
        assert_eq!(message, "embedded backup consumer did not register");
        let owner = owner.expect("the exposed original owner must be returned");
        assert!(lifetime.admit_maintenance().is_err());
        let root = owner.root.clone();
        let mut retirement = Retirement::start_isolated((*owner, directory), root, |owner| {
            owner.0.shutdown_for_retirement().map_err(|_| ())
        })
        .ok()
        .unwrap();
        assert_eq!(retirement.wait(Duration::from_millis(25)), Outcome::Pending);
        assert_eq!(runtime.block_on(runtime.spawn(async { 46 })).unwrap(), 46);
        release.send(()).unwrap();
        runtime.block_on(worker).unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Stopped);
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn stop_before_listener_exposure_retains_startup_owner_until_real_join() {
        use crate::android_retirement::{Outcome, Retirement};
        use std::sync::atomic::{AtomicBool, Ordering};
        let (server, directory) = fixture();
        let address = server.interface_address();
        let mark = server.request_owner("GET", "/init/mark", "").unwrap();
        let proposal: Value = serde_json::from_str(&mark.body).unwrap();
        let lock = json!({"proposal_id":proposal["proposal_id"]}).to_string();
        assert_eq!(
            server
                .request_owner("POST", "/init/mark/lock", &lock)
                .unwrap()
                .status,
            200
        );
        assert_eq!(
            server
                .request_owner("POST", "/init/finalize", "{}")
                .unwrap()
                .status,
            200
        );
        assert_eq!(server.status()["door_address"], address.to_string());
        server.shutdown();
        drop(server);

        let (published, observed) = std::sync::mpsc::channel();
        let (release_start, start_held) = std::sync::mpsc::channel();
        let start_held = Mutex::new(start_held);
        let (release_work, work_held) = std::sync::mpsc::channel();
        let work_held = Mutex::new(Some(work_held));
        let first = AtomicBool::new(true);
        let cancelled = Arc::new(AtomicBool::new(false));
        let input_cancelled = cancelled.clone();
        let root = directory.path().to_path_buf();
        let constructor = std::thread::spawn(move || {
            MobileServing::start_with_payload_for_retirement_observed(
                root,
                address,
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload"),
                Arc::new(move || input_cancelled.load(Ordering::SeqCst)),
                Arc::new(move |stop| {
                    if first.swap(false, Ordering::SeqCst) {
                        let permit = stop.backup.admit_maintenance().unwrap();
                        let held = work_held.lock().unwrap().take().unwrap();
                        let runtime = tokio::runtime::Handle::current();
                        let worker = runtime.spawn_blocking(move || {
                            let _permit = permit;
                            held.recv_timeout(Duration::from_secs(10)).unwrap();
                        });
                        published.send((stop, runtime, worker)).unwrap();
                        start_held
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(10))
                            .unwrap();
                    }
                }),
            )
        });
        let (stop, runtime, worker) = observed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            std::net::TcpStream::connect(address).is_err(),
            "hook precedes exposure"
        );
        cancelled.store(true, Ordering::SeqCst);
        let requested = Instant::now();
        stop.request_stop();
        assert!(requested.elapsed() < Duration::from_millis(500));
        assert!(stop.is_stopping());
        assert!(stop.backup.admit_maintenance().is_err());
        assert!(
            !constructor.is_finished(),
            "actual startup remains owned while held"
        );
        assert_eq!(runtime.block_on(runtime.spawn(async { 47 })).unwrap(), 47);
        release_start.send(()).unwrap();
        let failure = match constructor.join().unwrap() {
            Ok(_) => panic!("cancelled startup must refuse"),
            Err(failure) => failure,
        };
        let (message, owner) = failure.into_parts();
        assert_eq!(message, "stop requested during startup; journal retained");
        let owner = owner.expect("actual exposed startup owner retained");
        assert_eq!(owner.status()["door_address"], Value::Null);
        assert_eq!(
            owner.request_owner("GET", "/init/mark", "").unwrap().status,
            503
        );
        assert!(!owner.shutdown_complete.load(Ordering::Acquire));
        let root = owner.root.clone();
        let mut retirement = Retirement::start_isolated((*owner, directory), root, |owner| {
            owner.0.shutdown_for_retirement().map_err(|_| ())
        })
        .ok()
        .unwrap();
        assert_eq!(retirement.wait(Duration::from_millis(25)), Outcome::Pending);
        release_work.send(()).unwrap();
        runtime.block_on(worker).unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Stopped);
    }

    #[test]
    fn maintenance_join_panic_retains_actual_runtime_and_opaque_payload() {
        use crate::android_retirement::{Outcome, Retirement};
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Payload(Arc<AtomicBool>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
                panic!("failure payload must remain owned");
            }
        }
        let (server, directory) = fixture();
        let runtime = server.runtime.handle().clone();
        let lifetime = server.handle.backup_dependencies().lifetime();
        let dropped = Arc::new(AtomicBool::new(false));
        let payload = Payload(dropped.clone());
        let failure = runtime.spawn(async move {
            tokio::task::spawn_blocking(move || std::panic::panic_any(payload)).await
        });
        let old_backup = {
            let mut services = server.services.lock().unwrap();
            let services = services.as_mut().unwrap();
            services.shutdown.send_replace(true);
            std::mem::replace(&mut services.backup, failure)
        };
        runtime.block_on(old_backup).unwrap().unwrap();
        let root = server.root.clone();
        let mut retirement = Retirement::start_isolated((server, directory), root, |owner| {
            owner.0.shutdown_for_retirement().map_err(|_| ())
        })
        .ok()
        .unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Failed);
        assert_eq!(retirement.wait(Duration::ZERO), Outcome::Failed);
        assert_eq!(runtime.block_on(runtime.spawn(async { 47 })).unwrap(), 47);
        assert!(lifetime.admit_maintenance().is_err());
        assert!(!dropped.load(Ordering::Acquire));
        std::mem::forget(retirement);
    }
    #[test]
    fn original_runtime_survives_actual_route_drain_failure() {
        use crate::android_retirement::{Outcome, Retirement};
        let directory = tempfile::tempdir().unwrap();
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
            unreachable!()
        };
        drop(probe);
        let server = MobileServing::start_with_payload(
            directory.path().to_path_buf(),
            address,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload"),
        )
        .unwrap();
        let runtime = server.runtime.handle().clone();
        let deps = server.handle.backup_dependencies();
        let lifetime = deps.lifetime();
        assert!(
            std::thread::spawn(move || deps.poison_operation_mutex_for_test())
                .join()
                .is_err()
        );
        let root = server.root.clone();
        let identity = Arc::downgrade(&root);
        let mut retirement = Retirement::start_isolated((server, directory), root, |owner| {
            owner.0.shutdown_for_retirement().map_err(|_| ())
        })
        .ok()
        .unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Failed);
        assert_eq!(retirement.wait(Duration::ZERO), Outcome::Failed);
        assert_eq!(runtime.block_on(runtime.spawn(async { 44 })).unwrap(), 44);
        assert!(lifetime.admit_maintenance().is_err());
        assert!(
            identity
                .upgrade()
                .unwrap()
                .revalidate_canonical_binding()
                .is_ok()
        );
        // Retain this explicit test parent and the actual failed original owner.
        // Production's corresponding parent is process-lifetime storage.
        std::mem::forget(retirement);
    }

    #[test]
    fn original_runtime_survives_pre_drained_shutdown_panic() {
        use crate::android_retirement::{Outcome, Retirement, admission};
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Payload(Arc<AtomicBool>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
                panic!("retained panic payload must not drop during observation");
            }
        }
        struct Fixture {
            server: MobileServing,
            _directory: tempfile::TempDir,
        }
        fn fixture() -> Fixture {
            let directory = tempfile::tempdir().unwrap();
            let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
                unreachable!()
            };
            drop(probe);
            let server = MobileServing::start_with_payload(
                directory.path().to_path_buf(),
                address,
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload"),
            )
            .unwrap();
            Fixture {
                server,
                _directory: directory,
            }
        }
        // Real service stop/start twins consume actual route and native joins.
        for _ in 0..3 {
            assert_eq!(admission(), Outcome::Stopped);
            let owner = fixture();
            let root = owner.server.root.clone();
            let mut retirement = Retirement::start(owner, root, |owner| {
                owner.server.shutdown_for_retirement().map_err(|_| ())
            })
            .ok()
            .unwrap();
            assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Stopped);
            drop(retirement);
            assert_eq!(admission(), Outcome::Stopped);
        }
        let owner = fixture();
        let runtime = owner.server.runtime.handle().clone();
        let lifetime = owner.server.handle.backup_dependencies().lifetime();
        let root = owner.server.root.clone();
        let identity = Arc::downgrade(&root);
        let dropped = Arc::new(AtomicBool::new(false));
        let payload = Payload(dropped.clone());
        *owner.server.shutdown_test_fault.lock().unwrap() = Some(Box::new(move || {
            std::panic::panic_any(payload);
        }));
        let mut retirement = Retirement::start(owner, root, |owner| {
            owner.server.shutdown_for_retirement().map_err(|_| ())
        })
        .ok()
        .unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Failed);
        assert_eq!(retirement.wait(Duration::ZERO), Outcome::Failed);
        assert_eq!(runtime.block_on(runtime.spawn(async { 42 })).unwrap(), 42);
        assert!(lifetime.admit_maintenance().is_err());
        assert!(
            identity
                .upgrade()
                .unwrap()
                .revalidate_canonical_binding()
                .is_ok()
        );
        assert!(!dropped.load(Ordering::Acquire));
        drop(retirement);
        assert_eq!(admission(), Outcome::Failed);
        assert_eq!(
            crate::android_retirement::wait_for_process(Duration::ZERO),
            Outcome::Failed
        );
        assert_eq!(runtime.block_on(runtime.spawn(async { 43 })).unwrap(), 43);
        assert!(!dropped.load(Ordering::Acquire));
        // The intentional Failed process record keeps this actual owner/fixture.
    }

    #[test]
    fn synchronous_app_start_setup_and_stop() {
        let root = tempfile::tempdir().unwrap();
        let payload = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload");
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = match probe.local_addr().unwrap() {
            std::net::SocketAddr::V4(v) => v,
            _ => unreachable!(),
        };
        drop(probe);
        assert!(
            MobileServing::start_with_payload(
                root.path().to_path_buf(),
                address,
                root.path().to_path_buf()
            )
            .is_err()
        );
        let server =
            MobileServing::start_with_payload(root.path().to_path_buf(), address, payload).unwrap();
        assert_eq!(server.status()["door_address"], Value::Null);
        let mark_response = server.request_owner("GET", "/init/mark", "").unwrap();
        assert_eq!(mark_response.status, 200);
        let mark_json: Value = serde_json::from_str(&mark_response.body).unwrap();
        let proposal_id = mark_json["proposal_id"].as_str().expect("proposal_id");
        let lock_body = serde_json::json!({ "proposal_id": proposal_id }).to_string();
        assert_eq!(
            server
                .request_owner("POST", "/init/mark/lock", &lock_body)
                .unwrap()
                .status,
            200
        );
        let finalized = server
            .request_owner("POST", "/init/finalize", "{}")
            .unwrap();
        assert_eq!(finalized.status, 200, "{}", finalized.body);
        assert_eq!(server.status()["door_address"], address.to_string());
        let early_stop = server.stop_handle();
        let requested_at = std::time::Instant::now();
        early_stop.request_stop();
        assert!(requested_at.elapsed() < std::time::Duration::from_millis(500));
        assert!(early_stop.is_stopping());
        assert_eq!(server.status()["door_address"], Value::Null);
        assert!(
            server
                .handle
                .backup_dependencies()
                .lifetime()
                .admit_maintenance()
                .is_err()
        );
        assert!(
            !server
                .shutdown_complete
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert_eq!(
            server
                .runtime
                .block_on(server.runtime.spawn(async { 43 }))
                .unwrap(),
            43
        );
        server.shutdown();
        assert_eq!(server.status()["door_address"], Value::Null);
    }
}
