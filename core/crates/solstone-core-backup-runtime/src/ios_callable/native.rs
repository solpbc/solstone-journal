// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private ABI ownership. Parent admission must authorize every path/credential
//! before these calls. No callback registry or executable resolution lives here.
#![allow(unsafe_code)] // Limited to the linked, independently reviewed C ABI.

use std::ffi::c_char;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const OK: i32 = 0;
const INVALID_ARGUMENT: i32 = 1;
const ADD_RECOVERY_KEY: i32 = 3;
const SNAPSHOT: i32 = 4;
const RESTORE: i32 = 6;
const REMOVE_KEY: i32 = 10;
const CAP_EXCEEDED: i32 = 5;
const MAX_METADATA: u32 = 4 * 1024 * 1024;
static DRAIN_FAILED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Bytes {
    pub(super) pointer: *const c_char,
    pub(super) length: u32,
}

impl Bytes {
    pub(super) fn new(value: &[u8]) -> Result<Self, i32> {
        Ok(Self {
            pointer: value.as_ptr().cast(),
            length: value.len().try_into().map_err(|_| CAP_EXCEEDED)?,
        })
    }
}

#[repr(C)]
pub(super) struct LocalRequest {
    pub(super) operation: i32,
    pub(super) password: Bytes,
    pub(super) recovery_password: Bytes,
    pub(super) repository: Bytes,
    pub(super) source: Bytes,
    pub(super) restore: Bytes,
    pub(super) snapshot: Bytes,
}

#[repr(C)]
pub(super) struct RemoteRequest {
    pub(super) version: u32,
    pub(super) operation: i32,
    pub(super) endpoint: Bytes,
    pub(super) region: Bytes,
    pub(super) bucket: Bytes,
    pub(super) prefix: Bytes,
    pub(super) access: Bytes,
    pub(super) secret: Bytes,
    pub(super) token: Bytes,
    pub(super) password: Bytes,
    pub(super) recovery_password: Bytes,
    pub(super) state: Bytes,
    pub(super) source: Bytes,
    pub(super) restore: Bytes,
    pub(super) snapshot: Bytes,
    pub(super) workspace: Bytes,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct Options {
    pub version: u32,
    pub check_read_mode: u32,
    pub retry_lock_ms: u64,
    pub group: u32,
    pub groups: u32,
    pub reserved1: u32,
    pub reserved2: u32,
}

#[repr(C)]
pub(super) struct ResultFields {
    pub status: i32,
    pub key_id: [u8; 65],
    pub snapshot_id: [u8; 65],
    pub snapshot_count: u32,
    pub snapshot_ids: [[u8; 65]; 4096],
    pub diagnostic: [u8; 256],
}

// Only the app's static library links the Go archive, and only it enables
// `ios-linked-bridge`. The workspace's standalone iOS executables do not link
// the archive: there every entry refuses before any native state exists.
#[cfg(feature = "ios-linked-bridge")]
unsafe extern "C" {
    fn restic_bridge_operation_directories_v1_start(
        request: *const LocalRequest,
        options: *const Options,
        source_fd: i32,
        workspace_fd: i32,
        workspace: *const c_char,
        workspace_len: u32,
        policy: *const c_char,
        policy_len: u32,
        out: *mut u64,
    ) -> i32;
    fn restic_bridge_s3_operation_directories_v1_start(
        request: *const RemoteRequest,
        options: *const Options,
        source_fd: i32,
        state_fd: i32,
        workspace_fd: i32,
        policy: *const c_char,
        policy_len: u32,
        out: *mut u64,
    ) -> i32;
    fn restic_bridge_target_admit_v1(
        version: u32,
        target_fd: i32,
        path: *const c_char,
        path_len: u32,
        out: *mut u64,
    ) -> i32;
    fn restic_bridge_target_revalidate_v1(version: u32, id: u64) -> i32;
    fn restic_bridge_target_release_v1(version: u32, id: u64) -> i32;
    fn restic_bridge_s3_restore_admitted_v1_start(
        request: *const RemoteRequest,
        target: u64,
        selector: *const c_char,
        selector_len: u32,
        out: *mut u64,
    ) -> i32;
    fn restic_bridge_cancel(id: u64) -> i32;
    fn restic_bridge_wait(id: u64, result: *mut ResultFields) -> i32;
    fn restic_bridge_result_metadata(
        id: u64,
        buffer: *mut c_char,
        capacity: u32,
        required: *mut u32,
    ) -> i32;
    fn restic_bridge_remove_recovery_key_v1_start(
        request: *const LocalRequest,
        options: *const Options,
        key_id: *const c_char,
        key_id_len: u32,
        maintenance: u32,
        out: *mut u64,
    ) -> i32;
    fn restic_bridge_s3_remove_recovery_key_v1_start(
        request: *const RemoteRequest,
        options: *const Options,
        state_fd: i32,
        key_id: *const c_char,
        key_id_len: u32,
        maintenance: u32,
        out: *mut u64,
    ) -> i32;
    fn restic_bridge_s3_add_recovery_key_v1_start(
        request: *const RemoteRequest,
        options: *const Options,
        state_fd: i32,
        custody: u32,
        out: *mut u64,
    ) -> i32;
    fn restic_bridge_free(id: u64) -> i32;
    pub(super) fn restic_bridge_teardown_v1_start(
        local: *const LocalRequest,
        remote: *const RemoteRequest,
        options: *const Options,
        state_fd: i32,
        admitted_snapshot_ids: *const c_char,
        admitted_snapshot_count: u32,
        admission: *const c_char,
        admission_len: u32,
        out_id: *mut u64,
    ) -> i32;
    pub(crate) fn restic_bridge_http_exchange(
        method: *const c_char,
        method_len: u32,
        url_str: *const c_char,
        url_len: u32,
        header_json: *const c_char,
        header_json_len: u32,
        body: *const c_char,
        body_len: u32,
        cancelled: *const u32,
        out_http_status: *mut i32,
        out_body: *mut c_char,
        out_body_cap: u32,
        out_body_len: *mut u32,
    ) -> i32;
}

#[cfg(not(feature = "ios-linked-bridge"))]
pub(crate) use unlinked::restic_bridge_http_exchange;
#[cfg(not(feature = "ios-linked-bridge"))]
use unlinked::*;

#[cfg(not(feature = "ios-linked-bridge"))]
#[allow(clippy::too_many_arguments)]
mod unlinked {
    use super::{INVALID_ARGUMENT, LocalRequest, Options, RemoteRequest, ResultFields};
    use std::ffi::c_char;

    pub(super) unsafe fn restic_bridge_operation_directories_v1_start(
        _: *const LocalRequest,
        _: *const Options,
        _: i32,
        _: i32,
        _: *const c_char,
        _: u32,
        _: *const c_char,
        _: u32,
        _: *mut u64,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_s3_operation_directories_v1_start(
        _: *const RemoteRequest,
        _: *const Options,
        _: i32,
        _: i32,
        _: i32,
        _: *const c_char,
        _: u32,
        _: *mut u64,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_target_admit_v1(
        _: u32,
        _: i32,
        _: *const c_char,
        _: u32,
        _: *mut u64,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_target_revalidate_v1(_: u32, _: u64) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_target_release_v1(_: u32, _: u64) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_s3_restore_admitted_v1_start(
        _: *const RemoteRequest,
        _: u64,
        _: *const c_char,
        _: u32,
        _: *mut u64,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_cancel(_: u64) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_wait(_: u64, _: *mut ResultFields) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_result_metadata(
        _: u64,
        _: *mut c_char,
        _: u32,
        _: *mut u32,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_remove_recovery_key_v1_start(
        _: *const LocalRequest,
        _: *const Options,
        _: *const c_char,
        _: u32,
        _: u32,
        _: *mut u64,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_s3_remove_recovery_key_v1_start(
        _: *const RemoteRequest,
        _: *const Options,
        _: i32,
        _: *const c_char,
        _: u32,
        _: u32,
        _: *mut u64,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_s3_add_recovery_key_v1_start(
        _: *const RemoteRequest,
        _: *const Options,
        _: i32,
        _: u32,
        _: *mut u64,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_free(_: u64) -> i32 {
        INVALID_ARGUMENT
    }
    pub(super) unsafe fn restic_bridge_teardown_v1_start(
        _: *const LocalRequest,
        _: *const RemoteRequest,
        _: *const Options,
        _: i32,
        _: *const c_char,
        _: u32,
        _: *const c_char,
        _: u32,
        _: *mut u64,
    ) -> i32 {
        INVALID_ARGUMENT
    }
    pub(crate) unsafe fn restic_bridge_http_exchange(
        _: *const c_char,
        _: u32,
        _: *const c_char,
        _: u32,
        _: *const c_char,
        _: u32,
        _: *const c_char,
        _: u32,
        _: *const u32,
        _: *mut i32,
        _: *mut c_char,
        _: u32,
        _: *mut u32,
    ) -> i32 {
        INVALID_ARGUMENT
    }
}

pub(super) struct Remote<'a> {
    pub endpoint: &'a str,
    pub region: &'a str,
    pub bucket: &'a str,
    pub prefix: &'a str,
    pub access: &'a [u8],
    pub secret: &'a [u8],
    pub token: &'a [u8],
    pub state: PrivateDirectory<'a>,
}

/// The parent borrows only its retained JournalRoot. Go captures this identity
/// synchronously and owns its pinned source afterward; it never adopts this fd.
pub(super) struct Source<'a> {
    pub path: &'a str,
    pub descriptor: BorrowedFd<'a>,
}

/// A path and the actual private directory retained by startup storage. Native
/// entry copies its identity synchronously, with no transfer of fd ownership.
#[derive(Clone, Copy)]
pub(super) struct PrivateDirectory<'a> {
    pub path: &'a str,
    pub descriptor: BorrowedFd<'a>,
}

fn snapshot_directories<'a>(
    operation: i32,
    source: Option<Source<'a>>,
    workspace: Option<PrivateDirectory<'a>>,
) -> Result<(&'a str, i32, &'a str, i32), i32> {
    match (operation, source, workspace) {
        (SNAPSHOT, Some(source), Some(workspace)) => Ok((
            source.path,
            source.descriptor.as_raw_fd(),
            workspace.path,
            workspace.descriptor.as_raw_fd(),
        )),
        (1 | 2 | 3 | 5 | 7 | 9 | 12, None, None) => Ok(("", -1, "", -1)),
        _ => Err(INVALID_ARGUMENT),
    }
}

/// Native custody of one existing journal root as a restore target. Go holds
/// the target's activity lease from admission until release; an operation's
/// wait/free never releases it. A refused admission that still names an owner
/// keeps that cleanup custody in native code, so nothing here can reuse it.
pub(super) struct Target {
    id: u64,
}

impl Target {
    pub(super) fn admit(descriptor: BorrowedFd<'_>, path: &str) -> Result<Self, i32> {
        let path = Bytes::new(path.as_bytes())?;
        let mut id = 0;
        let status = unsafe {
            restic_bridge_target_admit_v1(
                1,
                descriptor.as_raw_fd(),
                path.pointer,
                path.length,
                &mut id,
            )
        };
        match (status, id) {
            (OK, 0) => Err(INVALID_ARGUMENT),
            (OK, id) => Ok(Self { id }),
            (status, _) => Err(status),
        }
    }

    pub(super) fn revalidate(&self) -> Result<(), i32> {
        match unsafe { restic_bridge_target_revalidate_v1(1, self.id) } {
            OK => Ok(()),
            status => Err(status),
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        // Every borrowing operation has already returned through wait/free.
        let status = unsafe { restic_bridge_target_release_v1(1, self.id) };
        if status != OK {
            log::warn!("restore target release refused (status {status}); native custody retained");
        }
    }
}

/// A live handle keeps the caller's actual worker/lifetime stack until native
/// wait and free complete. It never carries pointers into Rust request storage.
pub(super) struct Call {
    id: u64,
    began: Instant,
}

pub(super) struct Completed {
    pub fields: Box<ResultFields>,
    pub metadata: Result<Option<Vec<u8>>, i32>,
    pub cancellation_requested: bool,
    pub deadline_requested: bool,
    pub monitor_failed: bool,
}

pub(super) fn drain_failed() -> bool {
    DRAIN_FAILED.load(Ordering::Acquire)
}

fn undrained() -> ! {
    DRAIN_FAILED.store(true, Ordering::Release);
    // Returning would release the surrounding real backup lifetime/Runtime.
    // A caller-owned Stop must remain Pending/failed and refuse replacement.
    loop {
        std::thread::park();
    }
}

fn result_storage() -> Box<ResultFields> {
    let mut result = Box::<ResultFields>::new_uninit();
    // repr(C) consists entirely of integer/byte arrays; all-zero is valid.
    unsafe {
        result.as_mut_ptr().write_bytes(0, 1);
        result.assume_init()
    }
}

impl Call {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn local(
        operation: i32,
        password: &[u8],
        recovery: &[u8],
        repository: &str,
        source: Option<Source<'_>>,
        workspace: Option<PrivateDirectory<'_>>,
        policy: &[u8],
        options: &Options,
    ) -> Result<Self, i32> {
        let began = Instant::now();
        let (source, source_fd, workspace, workspace_fd) =
            snapshot_directories(operation, source, workspace)?;
        let request = LocalRequest {
            operation,
            password: Bytes::new(password)?,
            recovery_password: Bytes::new(recovery)?,
            repository: Bytes::new(repository.as_bytes())?,
            source: Bytes::new(source.as_bytes())?,
            restore: Bytes::new(&[])?,
            snapshot: Bytes::new(&[])?,
        };
        let workspace = Bytes::new(workspace.as_bytes())?;
        let policy = Bytes::new(policy)?;
        let mut id = 0;
        let status = unsafe {
            restic_bridge_operation_directories_v1_start(
                &request,
                options,
                source_fd,
                workspace_fd,
                workspace.pointer,
                workspace.length,
                policy.pointer,
                policy.length,
                &mut id,
            )
        };
        Self::published(id, status, began)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn remote(
        operation: i32,
        password: &[u8],
        recovery: &[u8],
        remote: &Remote<'_>,
        source: Option<Source<'_>>,
        workspace: Option<PrivateDirectory<'_>>,
        policy: &[u8],
        options: &Options,
    ) -> Result<Self, i32> {
        let began = Instant::now();
        let (source, source_fd, workspace, workspace_fd) =
            snapshot_directories(operation, source, workspace)?;
        let request = RemoteRequest {
            version: 1,
            operation,
            endpoint: Bytes::new(remote.endpoint.as_bytes())?,
            region: Bytes::new(remote.region.as_bytes())?,
            bucket: Bytes::new(remote.bucket.as_bytes())?,
            prefix: Bytes::new(remote.prefix.as_bytes())?,
            access: Bytes::new(remote.access)?,
            secret: Bytes::new(remote.secret)?,
            token: Bytes::new(remote.token)?,
            password: Bytes::new(password)?,
            recovery_password: Bytes::new(recovery)?,
            state: Bytes::new(remote.state.path.as_bytes())?,
            source: Bytes::new(source.as_bytes())?,
            restore: Bytes::new(&[])?,
            snapshot: Bytes::new(&[])?,
            workspace: Bytes::new(workspace.as_bytes())?,
        };
        let policy = Bytes::new(policy)?;
        let mut id = 0;
        let status = unsafe {
            restic_bridge_s3_operation_directories_v1_start(
                &request,
                options,
                source_fd,
                remote.state.descriptor.as_raw_fd(),
                workspace_fd,
                policy.pointer,
                policy.length,
                &mut id,
            )
        };
        Self::published(id, status, began)
    }

    /// Restore one snapshot subtree into the admitted target. The snapshot ID,
    /// selector and target path are each checked again natively.
    pub(super) fn remote_restore(
        password: &[u8],
        remote: &Remote<'_>,
        target: &Target,
        target_path: &str,
        snapshot: &str,
        selector: &str,
    ) -> Result<Self, i32> {
        let began = Instant::now();
        let request = RemoteRequest {
            version: 1,
            operation: RESTORE,
            endpoint: Bytes::new(remote.endpoint.as_bytes())?,
            region: Bytes::new(remote.region.as_bytes())?,
            bucket: Bytes::new(remote.bucket.as_bytes())?,
            prefix: Bytes::new(remote.prefix.as_bytes())?,
            access: Bytes::new(remote.access)?,
            secret: Bytes::new(remote.secret)?,
            token: Bytes::new(remote.token)?,
            password: Bytes::new(password)?,
            recovery_password: Bytes::new(&[])?,
            state: Bytes::new(remote.state.path.as_bytes())?,
            source: Bytes::new(&[])?,
            restore: Bytes::new(target_path.as_bytes())?,
            snapshot: Bytes::new(snapshot.as_bytes())?,
            workspace: Bytes::new(&[])?,
        };
        let selector = Bytes::new(selector.as_bytes())?;
        let mut id = 0;
        let status = unsafe {
            restic_bridge_s3_restore_admitted_v1_start(
                &request,
                target.id,
                selector.pointer,
                selector.length,
                &mut id,
            )
        };
        Self::published(id, status, began)
    }

    pub(super) fn local_remove_key(
        password: &[u8],
        repo_path: &str,
        key_id: &str,
        options: &Options,
        maintenance: u32,
    ) -> Result<Self, i32> {
        let began = Instant::now();
        let request = LocalRequest {
            operation: REMOVE_KEY,
            password: Bytes::new(password)?,
            recovery_password: Bytes::new(&[])?,
            repository: Bytes::new(repo_path.as_bytes())?,
            source: Bytes::new(&[])?,
            restore: Bytes::new(&[])?,
            snapshot: Bytes::new(&[])?,
        };
        let key_id_bytes = Bytes::new(key_id.as_bytes())?;
        let mut id = 0;
        let status = unsafe {
            restic_bridge_remove_recovery_key_v1_start(
                &request,
                options,
                key_id_bytes.pointer,
                key_id_bytes.length,
                maintenance,
                &mut id,
            )
        };
        Self::published(id, status, began)
    }

    pub(super) fn remote_remove_key(
        password: &[u8],
        remote: &Remote<'_>,
        key_id: &str,
        options: &Options,
        maintenance: u32,
    ) -> Result<Self, i32> {
        let began = Instant::now();
        let request = RemoteRequest {
            version: 1,
            operation: REMOVE_KEY,
            endpoint: Bytes::new(remote.endpoint.as_bytes())?,
            region: Bytes::new(remote.region.as_bytes())?,
            bucket: Bytes::new(remote.bucket.as_bytes())?,
            prefix: Bytes::new(remote.prefix.as_bytes())?,
            access: Bytes::new(remote.access)?,
            secret: Bytes::new(remote.secret)?,
            token: Bytes::new(remote.token)?,
            password: Bytes::new(password)?,
            recovery_password: Bytes::new(&[])?,
            state: Bytes::new(remote.state.path.as_bytes())?,
            source: Bytes::new(&[])?,
            restore: Bytes::new(&[])?,
            snapshot: Bytes::new(&[])?,
            workspace: Bytes::new(&[])?,
        };
        let key_id_bytes = Bytes::new(key_id.as_bytes())?;
        let mut id = 0;
        let status = unsafe {
            restic_bridge_s3_remove_recovery_key_v1_start(
                &request,
                options,
                remote.state.descriptor.as_raw_fd(),
                key_id_bytes.pointer,
                key_id_bytes.length,
                maintenance,
                &mut id,
            )
        };
        Self::published(id, status, began)
    }

    pub(super) fn remote_add_recovery_key_rotation(
        password: &[u8],
        recovery_password: &[u8],
        remote: &Remote<'_>,
        options: &Options,
        custody: u32,
    ) -> Result<Self, i32> {
        let began = Instant::now();
        let request = RemoteRequest {
            version: 1,
            operation: ADD_RECOVERY_KEY,
            endpoint: Bytes::new(remote.endpoint.as_bytes())?,
            region: Bytes::new(remote.region.as_bytes())?,
            bucket: Bytes::new(remote.bucket.as_bytes())?,
            prefix: Bytes::new(remote.prefix.as_bytes())?,
            access: Bytes::new(remote.access)?,
            secret: Bytes::new(remote.secret)?,
            token: Bytes::new(remote.token)?,
            password: Bytes::new(password)?,
            recovery_password: Bytes::new(recovery_password)?,
            state: Bytes::new(remote.state.path.as_bytes())?,
            source: Bytes::new(&[])?,
            restore: Bytes::new(&[])?,
            snapshot: Bytes::new(&[])?,
            workspace: Bytes::new(&[])?,
        };
        let mut id = 0;
        let status = unsafe {
            restic_bridge_s3_add_recovery_key_v1_start(
                &request,
                options,
                remote.state.descriptor.as_raw_fd(),
                custody,
                &mut id,
            )
        };
        Self::published(id, status, began)
    }

    pub(super) fn teardown(
        local: Option<&LocalRequest>,
        remote: Option<&RemoteRequest>,
        options: &Options,
        state_fd: i32,
        admitted_ids: &[u8],
        admitted_count: u32,
        admission: &[u8],
    ) -> Result<Self, i32> {
        let began = Instant::now();
        let mut id = 0;
        let ids_ptr = if admitted_count == 0 {
            std::ptr::null()
        } else {
            admitted_ids.as_ptr().cast()
        };
        let status = unsafe {
            restic_bridge_teardown_v1_start(
                local.map_or(std::ptr::null(), |r| r),
                remote.map_or(std::ptr::null(), |r| r),
                options,
                state_fd,
                ids_ptr,
                admitted_count,
                admission.as_ptr().cast(),
                admission.len().try_into().map_err(|_| INVALID_ARGUMENT)?,
                &mut id,
            )
        };
        Self::published(id, status, began)
    }

    fn published(id: u64, status: i32, began: Instant) -> Result<Self, i32> {
        match (status, id) {
            (OK, 0) => undrained(),
            (OK, id) => Ok(Self { id, began }),
            (status, 0) => Err(status),
            _ => undrained(),
        }
    }

    pub(super) fn wait(
        mut self,
        timeout: Option<Duration>,
        cancelled: Option<&(dyn Fn() -> bool + Sync)>,
        metadata: bool,
    ) -> Completed {
        let deadline = timeout.map(|timeout| self.began.checked_add(timeout).unwrap_or(self.began));
        let finished = (Mutex::new(false), Condvar::new());
        let cancellation_requested = AtomicBool::new(false);
        let deadline_requested = AtomicBool::new(false);
        let mut fields = result_storage();
        let id = self.id;
        let initial = catch_unwind(AssertUnwindSafe(|| {
            cancelled.is_some_and(|cancelled| cancelled())
        }));
        let initially_cancelled = initial.as_ref().copied().unwrap_or(true);
        let mut panic = initial.err();
        let mut monitor_failed = false;
        let initially_expired = deadline.is_some_and(|deadline| Instant::now() >= deadline);
        let initially_requested = initially_cancelled || initially_expired;
        if initially_requested {
            cancellation_requested.store(initially_cancelled, Ordering::Release);
            deadline_requested.store(initially_expired, Ordering::Release);
            unsafe {
                restic_bridge_cancel(id);
            }
        }
        std::thread::scope(|scope| {
            let monitor = std::thread::Builder::new()
                .name("backup-cancel".into())
                .spawn_scoped(scope, || {
                    let outcome = catch_unwind(AssertUnwindSafe(|| {
                        let (lock, wake) = &finished;
                        let mut done = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                        loop {
                            if *done {
                                break;
                            }
                            if initially_requested {
                                done = wake.wait(done).unwrap_or_else(|poison| poison.into_inner());
                                continue;
                            }
                            let is_cancelled = cancelled.is_some_and(|cancelled| cancelled());
                            let expired =
                                deadline.is_some_and(|deadline| Instant::now() >= deadline);
                            if is_cancelled || expired {
                                cancellation_requested.store(is_cancelled, Ordering::Release);
                                deadline_requested.store(expired, Ordering::Release);
                                unsafe {
                                    restic_bridge_cancel(id);
                                }
                                while !*done {
                                    done = wake
                                        .wait(done)
                                        .unwrap_or_else(|poison| poison.into_inner());
                                }
                                break;
                            }
                            done = wake
                                .wait_timeout(done, Duration::from_millis(25))
                                .unwrap_or_else(|poison| poison.into_inner())
                                .0;
                        }
                    }));
                    if outcome.is_err() {
                        unsafe {
                            restic_bridge_cancel(id);
                        }
                    }
                    outcome
                });
            if monitor.is_err() {
                monitor_failed = true;
                unsafe {
                    restic_bridge_cancel(id);
                }
            }
            let status = unsafe { restic_bridge_wait(id, fields.as_mut()) };
            *finished
                .0
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = true;
            finished.1.notify_all();
            if let Ok(monitor) = monitor {
                let outcome = match monitor.join() {
                    Ok(outcome) => outcome,
                    Err(payload) => Err(payload),
                };
                if let Err(payload) = outcome {
                    panic = Some(payload);
                }
            }
            if status != OK {
                undrained();
            }
        });
        let diagnostic_str = std::str::from_utf8(
            &fields.diagnostic[..fields.diagnostic.iter().position(|b| *b == 0).unwrap_or(0)],
        )
        .unwrap_or("");
        let is_remote_pending = fields.status == 11 && diagnostic_str == "REMOTE_PENDING";

        let metadata = if metadata && fields.status == OK {
            self.metadata().map(Some)
        } else if is_remote_pending {
            match self.metadata() {
                Ok(bytes) if !bytes.is_empty() => Ok(Some(bytes)),
                _ => Ok(None),
            }
        } else {
            Ok(None)
        };
        if unsafe { restic_bridge_free(self.id) } != OK {
            undrained();
        }
        self.id = 0;
        if let Some(payload) = panic {
            resume_unwind(payload);
        }
        Completed {
            fields,
            metadata,
            cancellation_requested: cancellation_requested.load(Ordering::Acquire),
            deadline_requested: deadline_requested.load(Ordering::Acquire),
            monitor_failed,
        }
    }

    fn metadata(&self) -> Result<Vec<u8>, i32> {
        let mut required = 0;
        let status = unsafe {
            restic_bridge_result_metadata(self.id, std::ptr::null_mut(), 0, &mut required)
        };
        if status != CAP_EXCEEDED || required == 0 || required > MAX_METADATA {
            return Err(if status == OK { 1 } else { status });
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(required as usize)
            .map_err(|_| CAP_EXCEEDED)?;
        bytes.resize(required as usize, 0);
        let capacity = required;
        let status = unsafe {
            restic_bridge_result_metadata(
                self.id,
                bytes.as_mut_ptr().cast(),
                capacity,
                &mut required,
            )
        };
        if status != OK || required != capacity {
            return Err(if status == OK { 1 } else { status });
        }
        Ok(bytes)
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        if self.id != 0 {
            unsafe {
                restic_bridge_cancel(self.id);
            }
            let mut result = result_storage();
            if unsafe { restic_bridge_wait(self.id, result.as_mut()) } != OK {
                undrained();
            }
            if unsafe { restic_bridge_free(self.id) } != OK {
                undrained();
            }
            self.id = 0;
        }
    }
}
