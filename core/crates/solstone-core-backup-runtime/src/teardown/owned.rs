// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Original teardown budget and joined remote execution. No local finish authority.

use solstone_core_backup::teardown_recovery::{RemoteEffect, RemoteFacts};
use std::sync::Arc;
use std::time::{Duration, Instant};

use solstone_core_backup::archive_admission::{
    AdmissionError, ArchiveBudget, ArchiveCancellation, Clock,
};

use crate::hosted_runtime::{HostedCredsUnavailable, HttpError};
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    windows
))]
use crate::hosted_runtime::{HttpRequest, HttpResponse};
use crate::teardown_admission::{
    CarrierTeardownAdmission, TeardownAdmissionRefusal, TeardownReader,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteTeardownPhase {
    NotStarted,
    Broker,
    Inventory,
    Deleting,
    Aborting,
    Confirming,
    Wiped,
    /// BYO restic snapshots were forgotten with prune; config and keys remain.
    SnapshotsPruned,
    BindingSuperseded,
}

/// Attempted requests and confirmed responses are independent observations.
/// This report contains no namespace, credentials, response bytes or local authority.
#[derive(Debug)]
pub struct RemoteTeardownReport {
    pub(crate) phase: RemoteTeardownPhase,
    pub(crate) broker_attempted: bool,
    pub(crate) delete_attempts: u64,
    pub(crate) delete_confirmations: u64,
    pub(crate) abort_attempts: u64,
    pub(crate) abort_confirmations: u64,
    pub(crate) objects_empty: bool,
    pub(crate) uploads_empty: bool,
    /// Own storage only: whether the owner's bucket keeps what was deleted.
    pub(crate) storage_retention: Option<StorageRetention>,
    pub(crate) admitted_snapshot_count: Option<u32>,
    pub(crate) previous: Option<RemoteFacts>,
}

impl RemoteTeardownReport {
    pub fn phase(&self) -> RemoteTeardownPhase {
        self.phase
    }
    pub fn broker_attempted(&self) -> bool {
        self.broker_attempted
    }
    pub fn delete_attempts(&self) -> u64 {
        self.delete_attempts
    }
    pub fn delete_confirmations(&self) -> u64 {
        self.delete_confirmations
    }
    pub fn abort_attempts(&self) -> u64 {
        self.abort_attempts
    }
    pub fn abort_confirmations(&self) -> u64 {
        self.abort_confirmations
    }
    pub fn objects_empty(&self) -> bool {
        self.objects_empty
    }
    pub fn uploads_empty(&self) -> bool {
        self.uploads_empty
    }
    pub fn storage_retention(&self) -> Option<StorageRetention> {
        self.storage_retention
            .or_else(|| self.previous.as_ref().and_then(|p| p.storage_retention))
    }
    pub fn current_storage_retention(&self) -> Option<StorageRetention> {
        self.storage_retention
    }
    pub fn admitted_snapshot_count(&self) -> Option<u32> {
        self.admitted_snapshot_count
    }
    pub fn previous(&self) -> Option<&RemoteFacts> {
        self.previous.as_ref()
    }

    /// Only observations from this invocation, never its restored prior receipt.
    pub fn confirmed_facts(&self) -> Option<RemoteFacts> {
        let effect = match self.phase {
            RemoteTeardownPhase::SnapshotsPruned => RemoteEffect::SnapshotsPruned {
                admitted_snapshot_count: self.admitted_snapshot_count?,
                remaining_snapshots: 0,
            },
            RemoteTeardownPhase::Wiped => RemoteEffect::OperatedWiped {
                delete_attempts: self.delete_attempts,
                delete_confirmations: self.delete_confirmations,
                abort_attempts: self.abort_attempts,
                abort_confirmations: self.abort_confirmations,
                objects_empty: self.objects_empty,
                uploads_empty: self.uploads_empty,
            },
            _ => return None,
        };
        Some(RemoteFacts {
            version: 1,
            effect,
            storage_retention: self.storage_retention,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeardownProtocolRefusal {
    Malformed,
    Limit,
    Namespace,
    Pagination,
    Acknowledgment,
}

#[derive(Debug)]
pub enum TeardownRemoteRefusal {
    Unsupported,
    Operation(AdmissionError),
    Admission(TeardownAdmissionRefusal),
    BindingUnavailable,
    Timestamp,
    Broker(HostedCredsUnavailable),
    Http(HttpError),
    HttpStatus(u16),
    Protocol(TeardownProtocolRefusal),
    Residual,
    AlreadyAttempted,
    /// Own storage only: no destination or key to address the repository with.
    DestinationUnavailable,
    /// Own storage only: restic stopped with this reason.
    Restic(&'static str),
}

/// Returned only by the real joined remote path; descriptive, not a cleanup token.
pub enum RemoteTeardownDisposition {
    Wiped,
    BindingSuperseded,
}

/// Retain this original value outside actor unwind and throughout local completion.
/// Its single budget cannot be renewed by progress, another request or credential fetch.
pub struct TeardownOperation {
    clock: Arc<dyn Clock + Send + Sync>,
    budget: ArchiveBudget,
    cancellation: ArchiveCancellation,
    deadline: Instant,
    report: RemoteTeardownReport,
}

impl TeardownOperation {
    pub fn new(
        clock: Arc<dyn Clock + Send + Sync>,
        cancellation: ArchiveCancellation,
    ) -> Result<Self, AdmissionError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(super::TEARDOWN_TIMEOUT_SECONDS))
            .ok_or(AdmissionError::DeadlineExceeded)?;
        let mut budget = ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone())?;
        let now = clock.now_unix();
        budget.remaining(clock.as_ref())?;
        budget.cap_credential_expiry(
            now.checked_add(super::TEARDOWN_TIMEOUT_SECONDS as i64)
                .ok_or(AdmissionError::DeadlineExceeded)?,
        );
        Ok(Self {
            clock,
            budget,
            cancellation,
            deadline,
            report: RemoteTeardownReport {
                phase: RemoteTeardownPhase::NotStarted,
                broker_attempted: false,
                delete_attempts: 0,
                delete_confirmations: 0,
                abort_attempts: 0,
                abort_confirmations: 0,
                objects_empty: false,
                uploads_empty: false,
                storage_retention: None,
                admitted_snapshot_count: None,
                previous: None,
            },
        })
    }

    pub fn report(&self) -> &RemoteTeardownReport {
        &self.report
    }

    /// Close operation resources and move the original observations unchanged.
    /// This grants no local completion or cleanup authority.
    pub fn into_report(self) -> RemoteTeardownReport {
        self.report
    }

    /// Historical reporting only. This changes no phase, counters or execution authority.
    #[cfg(any(test, target_os = "ios"))]
    pub fn restore_prior_facts(&mut self, previous: RemoteFacts) {
        self.report.previous = Some(previous);
    }
    pub fn cancellation(&self) -> &ArchiveCancellation {
        &self.cancellation
    }

    pub fn remaining(&self) -> Result<Duration, AdmissionError> {
        let wall = self.budget.remaining(self.clock.as_ref())?;
        let monotonic = self.deadline.saturating_duration_since(Instant::now());
        if monotonic.is_zero() {
            return Err(AdmissionError::DeadlineExceeded);
        }
        Ok(wall.min(monotonic))
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "android",
        target_os = "ios",
        windows
    ))]
    pub(crate) fn date(&self) -> Result<String, TeardownRemoteRefusal> {
        self.remaining().map_err(TeardownRemoteRefusal::Operation)?;
        let date = chrono::DateTime::<chrono::Utc>::from_timestamp(self.clock.now_unix(), 0)
            .ok_or(TeardownRemoteRefusal::Timestamp)?;
        self.remaining().map_err(TeardownRemoteRefusal::Operation)?;
        Ok(date.format("%Y%m%dT%H%M%SZ").to_string())
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "android",
        windows
    ))]
    pub(crate) fn request(
        &self,
        mut request: HttpRequest,
    ) -> Result<HttpResponse, TeardownRemoteRefusal> {
        request.timeout = request
            .timeout
            .min(self.remaining().map_err(TeardownRemoteRefusal::Operation)?);
        let response = crate::controlled_http::execute_teardown(&request, &self.cancellation);
        // The request worker/runtime/socket has settled. Android's one permanent
        // resolver may still hold only its hostname job; it cannot dispatch HTTP
        // or retain teardown authority after this call returns.
        self.remaining().map_err(TeardownRemoteRefusal::Operation)?;
        response.map_err(TeardownRemoteRefusal::Http)
    }

    #[cfg(target_os = "ios")]
    #[allow(unsafe_code)] // Bounded native HTTP ABI; the scoped watcher joins before return.
    pub(crate) fn request(
        &self,
        mut request: crate::hosted_runtime::HttpRequest,
    ) -> Result<crate::hosted_runtime::HttpResponse, TeardownRemoteRefusal> {
        use std::collections::BTreeMap;
        use std::ffi::CString;

        request.timeout = request
            .timeout
            .min(self.remaining().map_err(TeardownRemoteRefusal::Operation)?);

        let deadline = std::time::Instant::now() + request.timeout;

        let mut header_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (k, v) in &request.headers {
            header_map.entry(k.clone()).or_default().push(v.clone());
        }
        let header_json = serde_json::to_vec(&header_map)
            .map_err(|_| TeardownRemoteRefusal::Http(HttpError::Other))?;

        let method_c = CString::new(request.method)
            .map_err(|_| TeardownRemoteRefusal::Http(HttpError::Other))?;
        let url_c =
            CString::new(request.url).map_err(|_| TeardownRemoteRefusal::Http(HttpError::Other))?;

        let mut out_status: i32 = 0;
        let mut out_body_len: u32 = 0;
        let mut out_body: Vec<u8> = vec![0u8; 1024 * 1024];

        let finished = (std::sync::Mutex::new(false), std::sync::Condvar::new());
        let cancelled_atomic =
            std::sync::atomic::AtomicU32::new(if self.cancellation.is_cancelled() {
                1
            } else {
                0
            });
        let cancellation = &self.cancellation;

        let status = std::thread::scope(|scope| {
            scope.spawn(|| {
                let (lock, wake) = &finished;
                let mut done = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                loop {
                    if *done {
                        break;
                    }
                    if cancellation.is_cancelled() || std::time::Instant::now() >= deadline {
                        cancelled_atomic.store(1, std::sync::atomic::Ordering::Release);
                        break;
                    }
                    done = wake
                        .wait_timeout(done, std::time::Duration::from_millis(25))
                        .unwrap_or_else(|poison| poison.into_inner())
                        .0;
                }
            });
            let status = unsafe {
                solstone_core_backup::ios_native::restic_bridge_http_exchange(
                    method_c.as_ptr(),
                    method_c.as_bytes().len() as u32,
                    url_c.as_ptr(),
                    url_c.as_bytes().len() as u32,
                    header_json.as_ptr().cast(),
                    header_json.len() as u32,
                    request.body.as_ptr().cast(),
                    request.body.len() as u32,
                    cancelled_atomic.as_ptr(),
                    &mut out_status,
                    out_body.as_mut_ptr().cast(),
                    out_body.len() as u32,
                    &mut out_body_len,
                )
            };
            let (lock, wake) = &finished;
            let mut done = lock.lock().unwrap_or_else(|poison| poison.into_inner());
            *done = true;
            wake.notify_all();
            status
        });

        if status == 3 {
            return Err(TeardownRemoteRefusal::Http(HttpError::Timeout));
        }
        if status != 0 {
            return Err(TeardownRemoteRefusal::Http(HttpError::Other));
        }

        out_body.truncate(out_body_len as usize);
        self.remaining().map_err(TeardownRemoteRefusal::Operation)?;

        Ok(crate::hosted_runtime::HttpResponse {
            status: out_status as u16,
            headers: vec![],
            body: out_body,
        })
    }

    /// Run once under the actual retained admission. The installed coordinator supplies
    /// the sole fresh Catalog/raw reader. No request transport or settlement callback
    /// can be injected here, and successful remote work grants no local finish.
    pub fn run_remote(
        &mut self,
        admission: &CarrierTeardownAdmission,
        reader: &mut TeardownReader<'_>,
        version: &str,
    ) -> Result<RemoteTeardownDisposition, TeardownRemoteRefusal> {
        #[cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "android",
            target_os = "ios",
            windows
        ))]
        {
            use crate::hosted_runtime::{decode_hosted_credentials, hosted_credentials_request};
            if self.report.phase != RemoteTeardownPhase::NotStarted {
                return Err(TeardownRemoteRefusal::AlreadyAttempted);
            }
            self.remaining().map_err(TeardownRemoteRefusal::Operation)?;
            admission
                .check(reader)
                .map_err(TeardownRemoteRefusal::Admission)?;
            let binding = admission
                .configuration()
                .binding()
                .ok_or(TeardownRemoteRefusal::BindingUnavailable)?;
            let request = hosted_credentials_request(binding, "maintenance", version);
            self.report.phase = RemoteTeardownPhase::Broker;
            self.report.broker_attempted = true;
            let response = self.request(request)?;
            admission
                .revalidate()
                .map_err(TeardownRemoteRefusal::Admission)?;
            let credentials = match decode_hosted_credentials(&response) {
                Ok(credentials) => credentials,
                Err(refusal) if refusal.reason_code == "binding_superseded" => {
                    self.report.phase = RemoteTeardownPhase::BindingSuperseded;
                    return Ok(RemoteTeardownDisposition::BindingSuperseded);
                }
                Err(refusal) => return Err(TeardownRemoteRefusal::Broker(refusal)),
            };
            let expiry = chrono::DateTime::parse_from_rfc3339(&credentials.expires_at)
                .map_err(|_| TeardownRemoteRefusal::Timestamp)?
                .timestamp();
            self.budget.cap_credential_expiry(expiry);
            self.remaining().map_err(TeardownRemoteRefusal::Operation)?;
            admission
                .check(reader)
                .map_err(TeardownRemoteRefusal::Admission)?;
            let s3 = crate::s3_wipe::S3Credentials {
                endpoint: credentials.endpoint,
                access_key_id: credentials.access_key_id,
                secret_access_key: credentials.secret_access_key,
                session_token: credentials.session_token,
                region: "auto".into(),
            };
            crate::s3_wipe::owned::wipe(self, admission, reader, &s3)?;
            self.report.phase = RemoteTeardownPhase::Wiped;
            Ok(RemoteTeardownDisposition::Wiped)
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "android",
            target_os = "ios",
            windows
        )))]
        {
            let _ = (admission, reader, version);
            Err(TeardownRemoteRefusal::Unsupported)
        }
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "android",
        target_os = "ios",
        windows
    ))]
    pub(crate) fn report_mut(&mut self) -> &mut RemoteTeardownReport {
        &mut self.report
    }
}

#[cfg(target_os = "ios")]
mod ios;
mod own_storage;
mod retention;
#[cfg(target_os = "ios")]
pub use ios::IosOperatedPreparation;
pub use own_storage::snapshot_ids;
pub use retention::{StorageKeeps, StorageRetention};

#[cfg(test)]
mod tests;
