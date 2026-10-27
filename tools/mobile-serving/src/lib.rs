// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-owned orchestration of the production router and authenticated device door.

use axum::{
    body::{Body, to_bytes},
    http::{Request, header::CONTENT_TYPE},
};
use serde_json::{Value, json};
use solstone_core_convey_shell::EmbeddedConveyHandle;
use solstone_core_convey_shell::bind_embedded_device_door_with_root;
use solstone_core_import_web::ImportRouteSlot;
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
mod direct_restore;
#[cfg(unix)]
mod owner_response;
#[cfg(unix)]
pub use owner_response::{OwnerResponseHead, next_owner_request_id};
pub mod owner_bridge;

#[cfg(feature = "audio")]
pub(crate) mod audio;

#[cfg(any(feature = "audio", feature = "processing"))]
mod custody;
#[cfg(feature = "processing")]
mod daily_queue_observation;
#[cfg(feature = "processing")]
pub mod execution;
#[cfg(feature = "processing")]
pub mod foreground;
#[cfg(feature = "processing")]
pub mod import_route;
#[cfg(feature = "processing")]
mod rfdetr;
#[cfg(feature = "processing")]
mod screen_ocr;
#[cfg(feature = "processing")]
mod verification;

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
    handle: Arc<EmbeddedConveyHandle>,
    runtime: tokio::runtime::Runtime,
    root: Arc<JournalRoot>,
    import_route: Arc<ImportRouteSlot>,
    interface_address: SocketAddrV4,
    services: Mutex<Option<AppServices>>,
    stop: MobileServingStop,
    shutdown_complete: std::sync::atomic::AtomicBool,
    shutdown_failed: std::sync::atomic::AtomicBool,
    recovery_hooks: solstone_core_convey_shell::recovery_drain::RecoveryHooks,
    service_failures: Mutex<Vec<tokio::task::JoinError>>,
    #[cfg(unix)]
    owner_responses: owner_response::OwnerResponses,
    #[cfg(all(test, feature = "full-tests"))]
    shutdown_test_fault: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// Clone-only early-stop signal. The serving owner retains every worker and runtime.
#[derive(Clone)]
pub struct MobileServingStop {
    convey: solstone_core_convey_shell::EmbeddedConveyStop,
    services: watch::Sender<bool>,
    backup: solstone_core_backup_web::BackupRouteLifetime,
    imports: std::sync::Weak<ImportRouteSlot>,
}

struct ServingStartup {
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    stop_ready: Arc<dyn Fn(MobileServingStop) + Send + Sync>,
}

impl MobileServingStop {
    pub fn request_stop(&self) {
        if let Some(imports) = self.imports.upgrade() {
            imports.request_stop();
        }
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
    pub fn import_route(&self) -> Arc<ImportRouteSlot> {
        Arc::clone(&self.import_route)
    }

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
        let root = Arc::new(JournalRoot::open(&root).map_err(|e| e.to_string())?);
        Self::start_with_root_owned(root, address, payload_root, startup)
    }

    /// Start with the exact root already captured by the native host.
    pub fn start_with_root_for_retirement_observed(
        root: Arc<JournalRoot>,
        address: SocketAddrV4,
        payload_root: PathBuf,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
        stop_ready: Arc<dyn Fn(MobileServingStop) + Send + Sync>,
    ) -> Result<Self, StartupFailure> {
        Self::start_with_root_owned(
            root,
            address,
            Some(payload_root),
            Some(ServingStartup {
                cancelled,
                stop_ready,
            }),
        )
    }

    fn start_with_root_owned(
        root: Arc<JournalRoot>,
        address: SocketAddrV4,
        payload_root: Option<PathBuf>,
        startup: Option<ServingStartup>,
    ) -> Result<Self, StartupFailure> {
        if address.ip().is_unspecified() || address.ip().is_multicast() || address.port() == 0 {
            return Err("an explicit interface address and nonzero port are required".into());
        }
        root.revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        // A pending restore must stay reachable without materializing config.
        if !solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(
            root.canonical_path(),
        ) {
            direct_restore::persist_bound_port(&root, address.port(), &|| Ok(()))
                .map_err(|e| e.to_string())?;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        // Keep the production ingest notification boundary live. Durable receipt
        // discovery provides restart recovery independently of this transient bus.
        #[cfg(unix)]
        let bus = runtime.block_on(solstone_core_callosum::CallosumSocketServer::bind_journal(
            Arc::clone(&root),
        ));
        #[cfg(windows)]
        let bus = runtime.block_on(solstone_core_callosum::CallosumSocketServer::bind(
            root.canonical_path().join("health/callosum.sock"),
        ));
        let bus = bus.map_err(|e| e.to_string())?;
        let (shutdown, stop) = watch::channel(false);
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
        let owner_routes =
            solstone_core_mcp_endpoint::owner_routes(root.canonical_path().to_path_buf());
        #[cfg(feature = "processing")]
        let (execution_routes, execution_worker, settings_sync) =
            execution::routes_and_worker(Arc::clone(&root), payload_root.clone(), stop.clone());
        #[cfg(feature = "processing")]
        let owner_routes = owner_routes.merge(execution_routes);
        let import_route = Arc::new(ImportRouteSlot::new());
        let route_for_bind = Arc::clone(&import_route);
        let early_imports = Arc::downgrade(&import_route);
        let handle = runtime.block_on(async {
            let startup = solstone_core_convey_shell::EmbeddedStartup {
                #[cfg(feature = "processing")]
                backup_offload_dispatch: Some(verification::offload_dispatch_probe(
                    &root,
                    stop.clone(),
                    Arc::clone(&cancelled),
                )),
                #[cfg(not(feature = "processing"))]
                backup_offload_dispatch: None,
                #[cfg(feature = "processing")]
                backup_verification: Some(verification::submitter(
                    &root,
                    stop.clone(),
                    Arc::clone(&cancelled),
                )),
                #[cfg(not(feature = "processing"))]
                backup_verification: None,
                #[cfg(feature = "processing")]
                settings_sync: Some(settings_sync),
                #[cfg(not(feature = "processing"))]
                settings_sync: None,
                cancelled: Arc::clone(&cancelled),
                stop_ready: Arc::new(move |convey, backup| {
                    let signal = MobileServingStop {
                        convey,
                        services: early_shutdown.clone(),
                        backup,
                        imports: early_imports.clone(),
                    };
                    stop_ready(signal.clone());
                    if early_cancelled() {
                        signal.request_stop();
                    }
                }),
            };
            let binding = bind_embedded_device_door_with_root(
                Arc::clone(&root),
                address,
                payload_root,
                owner_routes,
                startup,
                Some(Arc::clone(&route_for_bind)),
            );
            binding.await
        });
        let handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                // No full serving owner exists yet. Join the live bus before
                // releasing this partial startup's runtime and original root.
                runtime.block_on(bus.stop());
                return Err(error.to_string().into());
            }
        };
        #[cfg(feature = "processing")]
        let execution_task = runtime.spawn(execution_worker);
        let (ready, registered) = oneshot::channel();
        let brain_task = runtime.spawn(brain::run(Arc::clone(&root), stop.clone(), ready));
        let (backup_ready, subscribed) = oneshot::channel();
        let backup_task = runtime.spawn(backup::run(
            Arc::clone(&root),
            handle.backup_dependencies(),
            stop.clone(),
            backup_ready,
        ));
        let direct_control = direct_restore::ServiceControl::new(Arc::clone(&root));
        let mcp_control = direct_restore::ServiceControl::new(Arc::clone(&root));
        let relay_control = direct_restore::ServiceControl::new(Arc::clone(&root));
        let pause_controls = [
            Arc::clone(&direct_control),
            Arc::clone(&mcp_control),
            Arc::clone(&relay_control),
        ];
        let resume_controls = pause_controls.clone();
        let resume_stop = stop.clone();
        let recovery_hooks = solstone_core_convey_shell::recovery_drain::register_recovery_hooks(
            "mobile-serving",
            Arc::clone(&root),
            move |journal| {
                for control in &pause_controls {
                    control.pause_and_wait(journal)?;
                }
                Ok(())
            },
            move |journal| {
                direct_restore::resume_services(
                    &resume_controls,
                    journal,
                    address.port(),
                    &resume_stop,
                )
            },
        );
        let direct_task = runtime.spawn(direct_restore::run(direct_control, stop.clone()));
        let mcp_task = runtime.spawn(direct_restore::run_mcp(mcp_control, stop.clone()));
        let relay_task = runtime.spawn(direct_restore::run_relay(relay_control, stop.clone()));
        let stop_handle = MobileServingStop {
            convey: handle.stop_handle(),
            services: shutdown.clone(),
            backup: handle.backup_dependencies().lifetime(),
            imports: Arc::downgrade(&import_route),
        };
        let owner = Self {
            stop: stop_handle,
            #[cfg(feature = "audio")]
            audio: None,
            bus,
            handle: Arc::new(handle),
            runtime,
            root,
            import_route,
            interface_address: address,
            services: Mutex::new(Some(AppServices {
                shutdown,
                tasks: vec![
                    brain_task,
                    relay_task,
                    mcp_task,
                    direct_task,
                    #[cfg(feature = "processing")]
                    execution_task,
                ],
                backup: backup_task,
            })),
            shutdown_complete: std::sync::atomic::AtomicBool::new(false),
            shutdown_failed: std::sync::atomic::AtomicBool::new(false),
            recovery_hooks,
            service_failures: Mutex::new(Vec::new()),
            #[cfg(unix)]
            owner_responses: owner_response::OwnerResponses::default(),
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
        if solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(
            self.root.canonical_path(),
        ) {
            return Err(
                solstone_core_journal_io::whole_journal_restore::WHOLE_JOURNAL_RESTORE_NOT_READY
                    .into(),
            );
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
        if solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(
            self.root.canonical_path(),
        ) {
            return Err(
                solstone_core_journal_io::whole_journal_restore::WHOLE_JOURNAL_RESTORE_NOT_READY
                    .into(),
            );
        }
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

    /// Private app IPC only. `sink` must be a nonblocking pipe owned by the caller.
    /// Response tasks retain this runtime's router and are canceled and joined at Stop.
    /// A lost response is an unknown mutation outcome, never permission to replay it.
    #[cfg(unix)]
    pub fn open_owner_response(
        &self,
        id: u64,
        method: &str,
        uri: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
        sink: std::fs::File,
    ) -> Result<std::sync::mpsc::Receiver<Result<OwnerResponseHead, String>>, String> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        self.owner_responses.open(
            self.runtime.handle(),
            Arc::clone(&self.handle),
            Arc::clone(&self.root),
            self.stop.services.subscribe(),
            id,
            method,
            uri,
            headers,
            body,
            sink,
        )
    }

    /// Stream a request body from `body` (the read end of a nonblocking pipe).
    /// Returns when the upload is admitted, not when the file has been written.
    /// `body` and `sink` are closed by this runtime on every path.
    #[cfg(unix)]
    pub fn open_owner_upload(
        &self,
        id: u64,
        method: &str,
        uri: &str,
        headers: &[(String, String)],
        body: std::fs::File,
        sink: std::fs::File,
    ) -> Result<std::sync::mpsc::Receiver<Result<OwnerResponseHead, String>>, String> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        self.owner_responses.open_upload(
            self.runtime.handle(),
            Arc::clone(&self.handle),
            Arc::clone(&self.root),
            self.stop.services.subscribe(),
            id,
            method,
            uri,
            headers,
            body,
            sink,
        )
    }

    pub fn canonical_journal_path(&self) -> &std::path::Path {
        self.root.canonical_path()
    }

    #[cfg(unix)]
    pub fn cancel_owner_response(&self, id: u64) {
        self.owner_responses.cancel(id);
    }

    pub fn index(&self) -> Result<Value, String> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let _restore_use = solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(
            Arc::clone(&self.root),
        )
        .map_err(|_| {
            solstone_core_journal_io::whole_journal_restore::WHOLE_JOURNAL_RESTORE_NOT_READY
                .to_owned()
        })?;
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
        solstone_core_journal_io::whole_journal_restore::hold_target_for_restore(
            self.root.canonical_path(),
        );
        let lifetime = self.handle.backup_dependencies().lifetime();
        lifetime.request_stop();
        if self.import_route.retire().is_err() {
            self.shutdown_failed.store(true, Ordering::Release);
            return Err(ShutdownFailure);
        }
        #[cfg(all(test, feature = "full-tests"))]
        {
            let fault = self.shutdown_test_fault.lock().unwrap().take();
            if let Some(fault) = fault {
                fault();
            }
        }
        self.handle.shutdown();
        #[cfg(unix)]
        if self.runtime.block_on(self.owner_responses.drain()).is_err() {
            self.shutdown_failed.store(true, Ordering::Release);
            return Err(ShutdownFailure);
        }
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
        solstone_core_journal_io::whole_journal_restore::wait_for_target_use_to_end(
            self.root.canonical_path(),
        );
        self.runtime.block_on(self.bus.stop());
        self.recovery_hooks.clear();
        solstone_core_journal_io::whole_journal_restore::release_target_for_restore(
            self.root.canonical_path(),
        );
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
    #[cfg(unix)]
    fn owner_pipe(
        server: &MobileServing,
        id: u64,
        method: &str,
        uri: &str,
        content_type: &str,
        body: &str,
    ) -> (OwnerResponseHead, std::os::unix::net::UnixStream) {
        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writer.set_nonblocking(true).unwrap();
        let descriptor: std::os::fd::OwnedFd = writer.into();
        let response = server
            .open_owner_response(
                id,
                method,
                uri,
                &[("content-type".into(), content_type.into())],
                body.as_bytes().to_vec(),
                std::fs::File::from(descriptor),
            )
            .unwrap();
        (
            response
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap(),
            reader,
        )
    }

    #[cfg(unix)]
    fn owner_bytes(
        server: &MobileServing,
        id: u64,
        method: &str,
        uri: &str,
        content_type: &str,
        body: &str,
    ) -> (OwnerResponseHead, Vec<u8>) {
        use std::io::Read;
        let (head, mut reader) = owner_pipe(server, id, method, uri, content_type, body);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        (head, bytes)
    }

    #[cfg(unix)]
    #[test]
    fn owner_pipe_preserves_setup_json_guard_redirect_assets_and_live_events() {
        use std::io::{BufRead, BufReader};
        let (server, _directory) = fixture();
        let (head, _) = owner_bytes(&server, 1, "GET", "/app/backup/", "application/json", "");
        assert!(matches!(head.status, 301 | 302 | 303 | 307 | 308));
        assert!(
            head.headers
                .iter()
                .any(|(key, value)| key == "location" && value.starts_with("/init"))
        );
        let (head, bytes) = owner_bytes(&server, 2, "GET", "/init/mark", "application/json", "");
        assert_eq!(head.status, 200);
        let mark: Value = serde_json::from_slice(&bytes).unwrap();
        let body = json!({"proposal_id":mark["proposal_id"]}).to_string();
        let (head, _) = owner_bytes(&server, 3, "POST", "/init/mark/lock", "text/plain", &body);
        assert_eq!(head.status, 415);
        let (head, _) = owner_bytes(
            &server,
            4,
            "POST",
            "/init/mark/lock",
            "application/json",
            &body,
        );
        assert_eq!(head.status, 200);
        let (head, bytes) = owner_bytes(
            &server,
            5,
            "POST",
            "/init/finalize",
            "application/json",
            "{}",
        );
        assert_eq!(head.status, 200, "{}", String::from_utf8_lossy(&bytes));
        let (head, bytes) = owner_bytes(
            &server,
            6,
            "GET",
            "/static/Comfortaa-Variable.woff2",
            "application/json",
            "",
        );
        assert_eq!(head.status, 200);
        assert_eq!(
            bytes,
            include_bytes!(
                "../../../core/crates/solstone-core-convey-shell/assets/static/Comfortaa-Variable.woff2"
            )
        );
        let (head, reader) = owner_pipe(&server, 7, "GET", "/sse/events", "application/json", "");
        assert_eq!(head.status, 200);
        assert!(head.headers.iter().any(|(key,value)|key=="content-type" && value.starts_with("text/event-stream")));
        let mut reader = BufReader::new(reader);
        let mut connected = false;
        for _ in 0..20 {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line.contains("connected") {
                connected = true;
                break;
            }
        }
        assert!(connected, "actual SSE connection must become live");
        let client = solstone_core_callosum::CallosumOneShotSender::new(
            server.root.canonical_path().join("health/callosum.sock"),
            Duration::from_secs(5),
        );
        client
            .send_line_confirmed(concat!(
                r#"{"tract":"owner-pipe-check","event":"changed","text":"café 🐦"}"#,
                "\n"
            ))
            .unwrap();
        let mut delivered = false;
        for _ in 0..20 {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line.contains("owner-pipe-check") {
                assert!(line.contains("café 🐦"));
                delivered = true;
                break;
            }
        }
        assert!(delivered, "actual bus event must cross the response pipe");
        server.cancel_owner_response(7);
        let mut tail = String::new();
        std::io::Read::read_to_string(&mut reader, &mut tail).unwrap();
        server.shutdown_for_retirement().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unread_owner_pipes_do_not_block_stop_and_navigation_slots_are_reclaimed() {
        let (server, _directory) = fixture();
        for id in 1..=70 {
            let (_, reader) = owner_pipe(&server, id, "GET", "/init", "text/html", "");
            server.cancel_owner_response(id);
            drop(reader);
            // Join the owned tasks at this lifetime boundary; no timed poll is needed.
            server
                .runtime
                .block_on(server.owner_responses.drain())
                .unwrap();
        }
        let mut readers = Vec::new();
        for id in 100..=115 {
            readers.push(owner_pipe(&server, id, "GET", "/sse/events", "text/event-stream", "").1);
        }
        let started = Instant::now();
        server.stop_handle().request_stop();
        server.shutdown_for_retirement().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "non-reading views must not hold retirement"
        );
        drop(readers);
    }

    #[cfg(target_os = "linux")]
    fn os_pipe() -> (std::fs::File, std::fs::File) {
        use std::os::fd::FromRawFd;
        let mut descriptors = [-1; 2];
        unsafe extern "C" {
            fn pipe(fds: *mut i32) -> i32;
            fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
        }
        assert_eq!(unsafe { pipe(descriptors.as_mut_ptr()) }, 0);
        let flags = unsafe { fcntl(descriptors[0], 3, 0) };
        assert_eq!(unsafe { fcntl(descriptors[0], 4, flags | 0o4000) }, 0);
        (
            unsafe { std::fs::File::from_raw_fd(descriptors[0]) },
            unsafe { std::fs::File::from_raw_fd(descriptors[1]) },
        )
    }

    #[cfg(target_os = "linux")]
    fn multipart(client_item_id: &str, filename: &str, bytes: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend(b"--bound\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\n");
        body.extend(client_item_id.as_bytes());
        body.extend(b"\r\n--bound\r\nContent-Disposition: form-data; name=\"file\"; filename=\"");
        body.extend(filename.as_bytes());
        body.extend(b"\"\r\nContent-Type: application/octet-stream\r\n\r\n");
        body.extend(bytes);
        body.extend(b"\r\n--bound--\r\n");
        body
    }

    #[cfg(target_os = "linux")]
    fn write_all_blocking(mut sink: std::fs::File, bytes: &[u8]) {
        use std::io::Write;
        sink.write_all(bytes).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn upload(
        server: &MobileServing,
        id: u64,
        client_item_id: &str,
        bytes: &[u8],
    ) -> (OwnerResponseHead, Vec<u8>) {
        use std::io::Read;
        let (body_read, body_write) = os_pipe();
        let (response_read, response_write) = std::os::unix::net::UnixStream::pair().unwrap();
        response_read
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        response_write.set_nonblocking(true).unwrap();
        let descriptor: std::os::fd::OwnedFd = response_write.into();
        let response_write = std::fs::File::from(descriptor);
        let receiver = server
            .open_owner_upload(
                id,
                "POST",
                "/app/import/api/save",
                &[(
                    "content-type".into(),
                    "multipart/form-data; boundary=bound".into(),
                )],
                body_read,
                response_write,
            )
            .unwrap();
        let payload = multipart(client_item_id, "notes.bin", bytes);
        let writer = std::thread::spawn(move || write_all_blocking(body_write, &payload));
        let head = receiver
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap();
        let mut response = Vec::new();
        let mut reader = response_read;
        reader.read_to_end(&mut response).unwrap();
        writer.join().unwrap();
        (head, response)
    }

    #[cfg(target_os = "linux")]
    fn admit_journal(server: &MobileServing) {
        let (head, bytes) = owner_bytes(server, 1, "GET", "/init/mark", "application/json", "");
        assert_eq!(head.status, 200, "{}", String::from_utf8_lossy(&bytes));
        let mark: Value = serde_json::from_slice(&bytes).unwrap();
        let body = json!({"proposal_id": mark["proposal_id"]}).to_string();
        let (head, bytes) = owner_bytes(
            server,
            2,
            "POST",
            "/init/mark/lock",
            "application/json",
            &body,
        );
        assert_eq!(head.status, 200, "{}", String::from_utf8_lossy(&bytes));
        let (head, bytes) = owner_bytes(
            server,
            3,
            "POST",
            "/init/finalize",
            "application/json",
            "{}",
        );
        assert_eq!(head.status, 200, "{}", String::from_utf8_lossy(&bytes));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owner_upload_streams_exact_bytes_into_the_existing_save() {
        use std::io::Write;
        let (server, directory) = fixture();
        admit_journal(&server);
        let mut first = vec![0u8; 300 * 1024];
        first[0] = 0;
        first[1] = 0xff;
        first[4096] = 0;
        let last = first.len() - 1;
        first[last] = 0xff;
        let mut second = first.clone();
        second[2] = 0x7f;
        let (head, bytes) = upload(&server, 21, "item-nul", &first);
        assert_eq!(head.status, 200, "{}", String::from_utf8_lossy(&bytes));
        let saved: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved["client_item_id"], "item-nul");
        let path = saved["path"].as_str().unwrap();
        let timestamp = saved["timestamp"].as_str().unwrap();
        assert!(!path.is_empty());
        assert!(!timestamp.is_empty());
        assert_eq!(std::fs::read(path).unwrap(), first);
        let metadata: Value = serde_json::from_str(
            &std::fs::read_to_string(
                directory
                    .path()
                    .join("imports")
                    .join(timestamp)
                    .join("import.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let hashed = solstone_core_import::hash_source(std::path::Path::new(path)).unwrap();
        assert_eq!(metadata["source_hash"], hashed.as_str());
        assert_eq!(metadata["file_size"], first.len() as u64);
        let (other_head, other_bytes) = upload(&server, 22, "item-other", &second);
        assert_eq!(
            other_head.status,
            200,
            "{}",
            String::from_utf8_lossy(&other_bytes)
        );
        let other: Value = serde_json::from_slice(&other_bytes).unwrap();
        assert_ne!(other["path"].as_str().unwrap(), path);
        let other_hash = solstone_core_import::hash_source(std::path::Path::new(
            other["path"].as_str().unwrap(),
        ))
        .unwrap();
        assert_ne!(other_hash.as_str(), hashed.as_str());
        let identity = json!({
            "path": path,
            "timestamp": saved["source_timestamp"],
            "import_id": timestamp,
            "setting": "",
        });
        let (meta, _) = owner_bytes(
            &server,
            23,
            "POST",
            "/app/import/api/meta",
            "application/json",
            &json!({"path": path, "setting": "", "import_id": timestamp}).to_string(),
        );
        assert_eq!(meta.status, 200);
        let (preview, preview_bytes) = owner_bytes(
            &server,
            24,
            "POST",
            "/app/import/api/preview",
            "application/json",
            &identity.to_string(),
        );
        assert!(preview.status >= 200);
        assert!(!preview_bytes.is_empty());
        let (start, start_bytes) = owner_bytes(
            &server,
            25,
            "POST",
            "/app/import/api/start",
            "application/json",
            &identity.to_string(),
        );
        assert!(start.status >= 200);
        assert!(!start_bytes.is_empty());
        let (list, list_bytes) = owner_bytes(
            &server,
            26,
            "GET",
            "/app/import/api/list?page=1&per_page=50",
            "application/json",
            "",
        );
        assert_eq!(list.status, 200);
        let listed = String::from_utf8(list_bytes).unwrap();
        assert!(listed.contains(timestamp));
        let mut truncated = multipart("item-cut", "cut.bin", &first);
        truncated.truncate(80);
        let (body_read, body_write) = os_pipe();
        let (response_read, response_write) = std::os::unix::net::UnixStream::pair().unwrap();
        response_write.set_nonblocking(true).unwrap();
        let descriptor: std::os::fd::OwnedFd = response_write.into();
        let receiver = server
            .open_owner_upload(
                27,
                "POST",
                "/app/import/api/save",
                &[(
                    "content-type".into(),
                    "multipart/form-data; boundary=bound".into(),
                )],
                body_read,
                std::fs::File::from(descriptor),
            )
            .unwrap();
        write_all_blocking(body_write, &truncated);
        let cut = receiver
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap();
        assert_ne!(cut.status, 200);
        drop(response_read);
        let imports = directory.path().join("imports");
        for entry in std::fs::read_dir(&imports).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.path().join("import.json");
            if let Ok(text) = std::fs::read_to_string(metadata) {
                assert!(!text.contains("item-cut"));
            }
        }
        assert!(
            !imports.join(".save-tmp").exists()
                || std::fs::read_dir(imports.join(".save-tmp"))
                    .map(|reader| reader.count() == 0)
                    .unwrap_or(true)
        );
        let (body_read, body_write) = os_pipe();
        let (_response_read, response_write) = std::os::unix::net::UnixStream::pair().unwrap();
        response_write.set_nonblocking(true).unwrap();
        let descriptor: std::os::fd::OwnedFd = response_write.into();
        let receiver = server
            .open_owner_upload(
                28,
                "POST",
                "/app/import/api/save",
                &[(
                    "content-type".into(),
                    "multipart/form-data; boundary=bound".into(),
                )],
                body_read,
                std::fs::File::from(descriptor),
            )
            .unwrap();
        server.cancel_owner_response(28);
        drop(body_write);
        assert!(receiver.recv_timeout(Duration::from_secs(2)).is_err());
        let _ = Write::flush(&mut std::io::stdout());
        server.shutdown_for_retirement().unwrap();
    }

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
    fn native_startup_keeps_original_root_before_config_or_ipc_effects() {
        let parent = tempfile::tempdir().unwrap();
        let journal = parent.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        let root = Arc::new(JournalRoot::open(&journal).unwrap());
        std::fs::rename(&journal, parent.path().join("original")).unwrap();
        std::fs::create_dir(&journal).unwrap();
        let result = MobileServing::start_with_root_owned(
            root,
            SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 42420),
            None,
            None,
        );
        assert!(result.is_err());
        assert!(std::fs::read_dir(&journal).unwrap().next().is_none());
    }

    #[test]
    fn native_serving_and_deferred_backup_share_the_original_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
            unreachable!()
        };
        drop(probe);
        let server = MobileServing::start_with_root_owned(
            Arc::clone(&root),
            address,
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload")),
            None,
        )
        .unwrap_or_else(|_| panic!("ordinary bound startup"));
        let backup_root = server.backup_dependencies().retained_root().unwrap();
        assert!(Arc::ptr_eq(&root, &backup_root));
        assert!(Arc::ptr_eq(&root, &server.root));
        server.shutdown_for_retirement().unwrap();
    }

    #[test]
    fn convey_startup_failure_retires_its_live_bus_before_returning() {
        let directory = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
        let result = MobileServing::start_with_root_owned(
            Arc::clone(&root),
            SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 42420),
            Some(directory.path().join("missing-payload")),
            None,
        );
        let failure = match result {
            Ok(_) => panic!("missing payload must refuse"),
            Err(failure) => failure,
        };
        let (_, owner) = failure.into_parts();
        assert!(
            owner.is_none(),
            "partial startup has completed its joined cleanup"
        );
        assert!(!directory.path().join("health/callosum.sock").exists());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let bus = runtime
            .block_on(solstone_core_callosum::CallosumSocketServer::bind_journal(
                root,
            ))
            .unwrap();
        runtime.block_on(bus.stop());
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

    #[cfg(unix)]
    #[test]
    fn pending_restore_serves_recovery_without_indexing_and_clearing_it_indexes() {
        #[cfg(all(feature = "processing", feature = "audio"))]
        let _foreground_lock = foreground::TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let record =
            solstone_core_journal_io::whole_journal_restore::publish_whole_journal_restore(
                directory.path(),
                &solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreSource::Byo {
                    repository: "repo".into(),
                    backend: "s3".into(),
                },
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "/journal",
            )
            .unwrap();
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
        // This is the native owner's startup order: the recovery router and
        // paused foreground host exist before its audio readiness handshake.
        #[cfg(all(feature = "processing", feature = "audio"))]
        let foreground = {
            let payload = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload");
            let mut host = foreground::ForegroundProcessingHost::open_with_cancellation(
                Arc::clone(&server.root),
                &payload,
                Arc::new(|| false),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                server.backup_dependencies(),
            )
            .unwrap();
            host.start_workers().unwrap();
            let pending = directory
                .path()
                .join(solstone_core_journal_io::whole_journal_restore::WHOLE_JOURNAL_RESTORE_FILE);
            let before = std::fs::read(&pending).unwrap();
            let models = directory.path().join("unprepared-models");
            let scratch = directory.path().join("unused-audio-scratch");
            let configured = host.enable_audio(
                models.clone(),
                models.join("model-does-not-exist.gguf"),
                scratch.clone(),
            );
            assert!(
                configured.is_ok(),
                "cold recovery handshake refused: {configured:?}"
            );
            assert_eq!(std::fs::read(&pending).unwrap(), before);
            assert!(!models.exists() && !scratch.exists());
            assert_eq!(host.audio_status()["generation"], 0);
            assert!(host.resume_audio().is_err(), "ordinary audio stays fenced");
            assert!(
                foreground::submit_owner_maintenance(&server.root, "health:prune-logs").is_err()
            );
            assert!(
                solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(
                    Arc::clone(&server.root)
                )
                .is_err()
            );
            host
        };
        let config = directory.path().join("config/journal.json");
        assert!(!config.exists());
        let db = solstone_core_indexer_store::db::db_path(directory.path());
        assert!(!db.is_file());
        let (head, bytes) = owner_bytes(
            &server,
            1,
            "GET",
            "/init/api/recovery",
            "application/json",
            "",
        );
        assert_eq!(head.status, 200);
        let recovery: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(recovery["recovery"], "pending");
        assert_eq!(recovery["snapshot_id"], record.snapshot_id);
        let (head, bytes) = owner_bytes(
            &server,
            2,
            "GET",
            "/static/Comfortaa-Variable.woff2",
            "application/json",
            "",
        );
        assert_eq!(head.status, 200);
        assert!(!bytes.is_empty());
        let (head, bytes) = owner_bytes(
            &server,
            3,
            "POST",
            "/init/finalize",
            "application/json",
            "{}",
        );
        assert_eq!(head.status, 409, "{}", String::from_utf8_lossy(&bytes));
        assert!(!config.exists());
        assert_eq!(
            server.index().unwrap_err(),
            solstone_core_journal_io::whole_journal_restore::WHOLE_JOURNAL_RESTORE_NOT_READY
        );
        assert!(!db.is_file());
        solstone_core_journal_io::whole_journal_restore::clear_whole_journal_restore(
            directory.path(),
            &record,
        )
        .unwrap();
        #[cfg(all(feature = "processing", feature = "audio"))]
        {
            solstone_core_convey_shell::recovery_drain::resume_after_whole_journal_restore(
                directory.path(),
            );
            foreground.resume_audio().unwrap();
            // The same configured worker advances its ordinary resume epoch.
            assert_eq!(foreground.audio_status()["generation"], 1);
        }
        let indexed = server.index().unwrap();
        assert_eq!(indexed["failed"], 0);
        assert!(db.is_file());
        #[cfg(all(feature = "processing", feature = "audio"))]
        {
            foreground.shutdown().unwrap();
            assert!(
                foreground
                    .enable_audio(
                        directory.path().join("unprepared-models"),
                        directory.path().join("missing-model.gguf"),
                        directory.path().join("unused-audio-scratch"),
                    )
                    .is_err(),
                "joined Stop cannot be reopened by audio configuration"
            );
        }
        server.shutdown_for_retirement().unwrap();
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
