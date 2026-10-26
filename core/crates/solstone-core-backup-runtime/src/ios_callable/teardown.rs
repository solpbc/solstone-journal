// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact native destination custody for explicitly confirmed whole-backup teardown.
#![allow(unsafe_code)] // Typed native entry and transfer of a genuinely owned fd.

use std::ffi::c_char;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{AdmittedIosBackupAuthority, ExecuteError, storage};
use crate::teardown::owned::TeardownOperation;
use solstone_core_backup::{ios_native as native, ios_repository as repository};

// Private development progress only. Arguments are fixed labels and numeric
// status/length/flag values; failed output must not affect custody or results.
macro_rules! diagnostic {
    ($stage:literal, $code:expr, $length:expr, $flags:expr) => {{
        #[cfg(feature = "test-hooks")]
        {
            use std::io::Write as _;
            let code: i32 = $code;
            let length: usize = $length;
            let flags: u8 = $flags;
            let _ = writeln!(
                std::io::stderr().lock(),
                "[ios-teardown-diagnostic] stage={} code={} length={} flags={}",
                $stage,
                code,
                length,
                flags
            );
        }
    }};
}

#[cfg(feature = "ios-linked-bridge")]
unsafe extern "C" {
    fn restic_bridge_s3_teardown_observe_v1(
        state: i32,
        destination: *const c_char,
        destination_len: u32,
        pending: *const c_char,
        pending_len: u32,
        out_fd: *mut i32,
        output: *mut c_char,
        capacity: u32,
        out_len: *mut u32,
    ) -> i32;
    fn restic_bridge_s3_teardown_retire_v1(
        state: i32,
        destination: *const c_char,
        destination_len: u32,
        expected: *const c_char,
        expected_len: u32,
    ) -> i32;
    fn restic_bridge_s3_teardown_verify_v1(
        state: i32,
        destination: *const c_char,
        destination_len: u32,
        expected: *const c_char,
        expected_len: u32,
    ) -> i32;
}

#[cfg(not(feature = "ios-linked-bridge"))]
#[allow(clippy::too_many_arguments)]
unsafe fn restic_bridge_s3_teardown_observe_v1(
    _: i32,
    _: *const c_char,
    _: u32,
    _: *const c_char,
    _: u32,
    _: *mut i32,
    _: *mut c_char,
    _: u32,
    _: *mut u32,
) -> i32 {
    1
}
#[cfg(not(feature = "ios-linked-bridge"))]
unsafe fn restic_bridge_s3_teardown_retire_v1(
    _: i32,
    _: *const c_char,
    _: u32,
    _: *const c_char,
    _: u32,
) -> i32 {
    1
}
#[cfg(not(feature = "ios-linked-bridge"))]
unsafe fn restic_bridge_s3_teardown_verify_v1(
    _: i32,
    _: *const c_char,
    _: u32,
    _: *const c_char,
    _: u32,
) -> i32 {
    1
}

pub use solstone_core_backup::teardown_recovery::retained::{
    NativeCustodyObservation as CustodyObservation, NativeRecordObservation as RecordObservation,
    NativeStorageObservation as StorageObservation,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestructiveAdmission {
    pub config_sha256: String,
    pub repository_id: String,
    pub origin: String,
    pub expected: RecordObservation,
}

fn invalid() -> ExecuteError {
    ExecuteError::Entry(1)
}
fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl DestructiveAdmission {
    pub fn scope(
        &self,
        destination: &repository::Destination,
        ids: &[String],
    ) -> Result<String, ExecuteError> {
        solstone_core_backup::teardown_recovery::retained::destructive_scope(
            &self.config_sha256,
            destination,
            &self.repository_id,
            &self.origin,
            ids,
        )
        .map_err(|_| invalid())
    }
}

/// Actual admitted root and existing native parent chain. No decoded observation
/// can construct a root, directory handle or lock.
pub struct DestinationContext {
    authority: Arc<AdmittedIosBackupAuthority>,
    state: storage::Directory,
    destination: repository::Destination,
}

impl DestinationContext {
    pub fn prepare(
        authority: Arc<AdmittedIosBackupAuthority>,
        destination: repository::Destination,
    ) -> Result<Self, ExecuteError> {
        authority.revalidate().map_err(|_| invalid())?;
        let state = authority
            .installed
            .storage
            .create_state(&destination.namespace())
            .map_err(|_| invalid())?;
        let context = Self {
            authority,
            state,
            destination,
        };
        context.revalidate()?;
        Ok(context)
    }

    pub fn reopen(
        authority: Arc<AdmittedIosBackupAuthority>,
        expected: &CustodyObservation,
    ) -> Result<Self, ExecuteError> {
        authority.revalidate().map_err(|_| invalid())?;
        let state = authority
            .installed
            .storage
            .open_state(&expected.destination.namespace())
            .map_err(|_| invalid())?;
        let context = Self {
            authority,
            state,
            destination: expected.destination.clone(),
        };
        if context.storage_observation()? != expected.storage {
            return Err(invalid());
        }
        context.revalidate()?;
        Ok(context)
    }

    pub fn revalidate(&self) -> Result<(), ExecuteError> {
        self.authority.revalidate().map_err(|_| invalid())?;
        self.state.revalidate().map_err(|_| invalid())
    }

    fn storage_observation(&self) -> Result<StorageObservation, ExecuteError> {
        let (installation, family, directory) = self.state.observation().map_err(|_| invalid())?;
        Ok(StorageObservation {
            installation,
            family,
            directory,
        })
    }

    pub fn lock(
        self: &Arc<Self>,
        operation: &TeardownOperation,
        pending_scope: Option<&str>,
        expected: Option<&RecordObservation>,
    ) -> Result<LockedDestination, ExecuteError> {
        diagnostic!("lock-enter", 0, 0, 0);
        operation.remaining().map_err(|_| invalid())?;
        diagnostic!("lock-budget-ready", 0, 0, 0);
        self.revalidate()?;
        diagnostic!("lock-context-ready", 0, 0, 0);
        let destination = serde_json::to_vec(&self.destination).map_err(|_| invalid())?;
        let pending = pending_scope.unwrap_or("");
        if !pending.is_empty() && !hex(pending, 64) {
            return Err(invalid());
        }
        let mut output = [0u8; 4096];
        let mut length = 0;
        let mut fd = -1;
        diagnostic!("lock-observe-enter", 0, 0, 0);
        let status = unsafe {
            restic_bridge_s3_teardown_observe_v1(
                self.state.as_fd().as_raw_fd(),
                destination.as_ptr().cast(),
                destination.len() as u32,
                pending.as_ptr().cast(),
                pending.len() as u32,
                &mut fd,
                output.as_mut_ptr().cast(),
                4096,
                &mut length,
            )
        };
        // The ABI transfers only an actually acquired fd. Own it even on a
        // malformed success/error so no descriptor can escape a refusal.
        let locked = if fd >= 0 {
            Some(unsafe { OwnedFd::from_raw_fd(fd) })
        } else {
            None
        };
        diagnostic!(
            "lock-observe-return",
            status,
            length as usize,
            u8::from(locked.is_some())
        );
        if status != 0 || length == 0 || length > 4096 {
            return Err(ExecuteError::Entry(status.max(1)));
        }
        let locked = locked.ok_or_else(invalid)?;
        diagnostic!("lock-fd-ready", 0, 0, 0);
        let record: RecordObservation =
            serde_json::from_slice(&output[..length as usize]).map_err(|_| invalid())?;
        diagnostic!("lock-observation-decoded", 0, 0, 0);
        if expected.is_some_and(|expected| expected != &record) {
            return Err(invalid());
        }
        diagnostic!("lock-expected-matched", 0, 0, 0);
        let custody = CustodyObservation {
            destination: self.destination.clone(),
            storage: self.storage_observation()?,
            record,
        };
        diagnostic!("lock-storage-observed", 0, 0, 0);
        operation.remaining().map_err(|_| invalid())?;
        diagnostic!("lock-final-budget-ready", 0, 0, 0);
        self.revalidate()?;
        diagnostic!("lock-retained-ready", 0, 0, 0);
        Ok(LockedDestination {
            context: Arc::clone(self),
            locked,
            custody,
        })
    }

    fn remote<'a>(
        &'a self,
        repository: &'a repository::Repository<'_>,
    ) -> Result<native::Remote<'a>, ExecuteError> {
        let repository::Repository::S3 {
            destination,
            access,
            secret,
            token,
            append_only: false,
        } = repository
        else {
            return Err(invalid());
        };
        if destination != &self.destination {
            return Err(invalid());
        }
        Ok(native::Remote {
            endpoint: &self.destination.endpoint,
            region: &self.destination.region,
            bucket: &self.destination.bucket,
            prefix: &self.destination.prefix,
            access: access.as_bytes(),
            secret: secret.as_bytes(),
            token: token.as_bytes(),
            state: native::PrivateDirectory {
                path: self.state.path(),
                descriptor: self.state.as_fd(),
            },
        })
    }

    fn read(
        &self,
        code: i32,
        repository: &repository::Repository<'_>,
        password: &str,
        operation: &TeardownOperation,
    ) -> Result<native::Completed, ExecuteError> {
        let timeout = operation.remaining().map_err(|_| invalid())?;
        self.revalidate()?;
        let secret = Zeroizing::new(password.as_bytes().to_vec());
        let remote = self.remote(repository)?;
        let call = native::Call::remote(code, &secret, &[], &remote, None, None, &[], &options())
            .map_err(ExecuteError::Entry)?;
        let cancellation = operation.cancellation();
        let cancelled = || cancellation.is_cancelled();
        let result = call.wait(Some(timeout), Some(&cancelled), true);
        diagnostic!(
            "read-return",
            result.fields.status,
            0,
            u8::from(result.monitor_failed)
                | (u8::from(result.cancellation_requested) << 1)
                | (u8::from(result.deadline_requested) << 2)
        );
        #[cfg(feature = "test-hooks")]
        match &result.metadata {
            Ok(Some(bytes)) => diagnostic!("read-metadata-present", code, bytes.len(), 0),
            Ok(None) => diagnostic!("read-metadata-absent", code, 0, 0),
            Err(status) => diagnostic!("read-metadata-error", *status, 0, 0),
        }
        self.revalidate()?;
        diagnostic!("read-context-revalidated", code, 0, 0);
        operation.remaining().map_err(|_| invalid())?;
        diagnostic!("read-budget-ready", code, 0, 0);
        if result.fields.status != 0
            || result.monitor_failed
            || result.cancellation_requested
            || result.deadline_requested
        {
            return Err(ExecuteError::Entry(result.fields.status.max(1)));
        }
        Ok(result)
    }

    pub fn inventory(
        &self,
        repository: &repository::Repository<'_>,
        password: &str,
        operation: &TeardownOperation,
    ) -> Result<(String, Vec<String>), ExecuteError> {
        diagnostic!("inventory-open-enter", 0, 0, 0);
        let opened = self.read(2, repository, password, operation)?;
        diagnostic!("inventory-open-ready", 0, 0, 0);
        let raw = opened
            .metadata
            .map_err(ExecuteError::Entry)?
            .ok_or_else(invalid)?;
        diagnostic!("inventory-config-metadata-ready", 0, raw.len(), 0);
        let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        diagnostic!("inventory-config-decoded", 0, 0, 0);
        if value["schema"] != "solstone.backup-metadata.v1" || value["kind"] != "repository-config"
        {
            return Err(invalid());
        }
        diagnostic!("inventory-config-schema-ready", 0, 0, 0);
        let id = value["repository"]["id"]
            .as_str()
            .filter(|id| hex(id, 64))
            .ok_or_else(invalid)?
            .to_owned();
        diagnostic!("inventory-config-id-ready", 0, 0, 0);
        let listed = self.read(5, repository, password, operation)?;
        diagnostic!("inventory-list-ready", 0, 0, 0);
        let raw = listed
            .metadata
            .map_err(ExecuteError::Entry)?
            .ok_or_else(invalid)?;
        diagnostic!("inventory-catalog-metadata-ready", 0, raw.len(), 0);
        let catalogue: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        diagnostic!("inventory-catalog-decoded", 0, 0, 0);
        if catalogue["schema"] != "solstone.backup-metadata.v1"
            || catalogue["kind"] != "snapshot-catalog"
        {
            return Err(invalid());
        }
        diagnostic!("inventory-catalog-schema-ready", 0, 0, 0);
        let rows = catalogue["snapshots"].as_array().ok_or_else(invalid)?;
        if rows.len() > 4096 {
            return Err(invalid());
        }
        let mut ids = rows
            .iter()
            .map(|row| {
                row["id"]
                    .as_str()
                    .filter(|id| hex(id, 64))
                    .map(str::to_owned)
                    .ok_or_else(invalid)
            })
            .collect::<Result<Vec<_>, _>>()?;
        ids.sort();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid());
        }
        diagnostic!("inventory-cohort-ready", 0, ids.len(), 0);
        Ok((id, ids))
    }

    pub fn prune(
        &self,
        repository: &repository::Repository<'_>,
        password: &str,
        ids: &[String],
        admission: &DestructiveAdmission,
        operation: &TeardownOperation,
    ) -> Result<CustodyObservation, ExecuteError> {
        let timeout = operation.remaining().map_err(|_| invalid())?;
        self.revalidate()?;
        let scope = admission.scope(&self.destination, ids)?;
        let remote = self.remote(repository)?;
        let password = Zeroizing::new(password.as_bytes().to_vec());
        let request = native::RemoteRequest {
            version: 1,
            operation: 11,
            endpoint: native::Bytes::new(remote.endpoint.as_bytes())
                .map_err(ExecuteError::Entry)?,
            region: native::Bytes::new(remote.region.as_bytes()).map_err(ExecuteError::Entry)?,
            bucket: native::Bytes::new(remote.bucket.as_bytes()).map_err(ExecuteError::Entry)?,
            prefix: native::Bytes::new(remote.prefix.as_bytes()).map_err(ExecuteError::Entry)?,
            access: native::Bytes::new(remote.access).map_err(ExecuteError::Entry)?,
            secret: native::Bytes::new(remote.secret).map_err(ExecuteError::Entry)?,
            token: native::Bytes::new(remote.token).map_err(ExecuteError::Entry)?,
            password: native::Bytes::new(&password).map_err(ExecuteError::Entry)?,
            recovery_password: native::Bytes::new(&[]).map_err(ExecuteError::Entry)?,
            state: native::Bytes::new(remote.state.path.as_bytes()).map_err(ExecuteError::Entry)?,
            source: native::Bytes::new(&[]).map_err(ExecuteError::Entry)?,
            restore: native::Bytes::new(&[]).map_err(ExecuteError::Entry)?,
            snapshot: native::Bytes::new(&[]).map_err(ExecuteError::Entry)?,
            workspace: native::Bytes::new(&[]).map_err(ExecuteError::Entry)?,
        };
        let contiguous = ids.concat();
        let binding = serde_json::to_vec(admission).map_err(|_| invalid())?;
        let call = native::Call::teardown(
            None,
            Some(&request),
            &options(),
            self.state.as_fd().as_raw_fd(),
            contiguous.as_bytes(),
            ids.len() as u32,
            &binding,
        )
        .map_err(ExecuteError::Entry)?;
        let cancellation = operation.cancellation();
        let cancelled = || cancellation.is_cancelled();
        let completed = call.wait(Some(timeout), Some(&cancelled), true);
        self.revalidate()?;
        operation.remaining().map_err(|_| invalid())?;
        if completed.fields.status != 0
            || completed.fields.snapshot_count != 0
            || completed.monitor_failed
            || completed.cancellation_requested
            || completed.deadline_requested
        {
            return Err(ExecuteError::Entry(completed.fields.status.max(1)));
        }
        let raw = completed
            .metadata
            .map_err(ExecuteError::Entry)?
            .ok_or_else(invalid)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Completion {
            schema: String,
            kind: String,
            completion_id: String,
            state_device: u64,
            state_inode: u64,
            scope: String,
            origin: String,
            reconciled: bool,
        }
        let proof: Completion = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        if proof.schema != "solstone.backup-metadata.v1"
            || proof.kind != "teardown-completed"
            || !hex(&proof.completion_id, 32)
            || proof.scope != scope
            || proof.origin != admission.origin
            || !proof.reconciled
        {
            return Err(invalid());
        }
        Ok(CustodyObservation {
            destination: self.destination.clone(),
            storage: self.storage_observation()?,
            record: RecordObservation {
                version: 1,
                present: true,
                device: proof.state_device,
                inode: proof.state_inode,
                id: proof.completion_id,
                operation: 11,
                active: false,
                scope: proof.scope,
                origin: proof.origin,
                reconciled: proof.reconciled,
            },
        })
    }
}

pub struct LockedDestination {
    context: Arc<DestinationContext>,
    locked: OwnedFd,
    custody: CustodyObservation,
}

impl LockedDestination {
    pub fn observation(&self) -> &CustodyObservation {
        &self.custody
    }
    pub fn revalidate(&self, operation: &TeardownOperation) -> Result<(), ExecuteError> {
        operation.remaining().map_err(|_| invalid())?;
        self.verify_custody()?;
        operation.remaining().map(|_| ()).map_err(|_| invalid())
    }

    /// Read-only live-descriptor proof for handing process-local locks back to
    /// an existing durable recovery record. This admits no executing phase.
    pub fn confirm_restart_custody(&self) -> Result<(), ExecuteError> {
        let record = &self.custody.record;
        if record.active || (record.operation == 11 && !record.reconciled) {
            return Err(invalid());
        }
        self.verify_custody()
    }

    fn verify_custody(&self) -> Result<(), ExecuteError> {
        self.context.revalidate()?;
        let destination = serde_json::to_vec(&self.custody.destination).map_err(|_| invalid())?;
        let record = serde_json::to_vec(&self.custody.record).map_err(|_| invalid())?;
        let status = unsafe {
            restic_bridge_s3_teardown_verify_v1(
                self.locked.as_raw_fd(),
                destination.as_ptr().cast(),
                destination.len() as u32,
                record.as_ptr().cast(),
                record.len() as u32,
            )
        };
        if status != 0 {
            return Err(ExecuteError::Entry(status));
        }
        self.context.revalidate()
    }
    pub fn retire(&mut self, operation: &TeardownOperation) -> Result<(), ExecuteError> {
        self.revalidate(operation)?;
        if self.custody.record.operation == 11 && !self.custody.record.reconciled {
            return Err(invalid());
        }
        let destination = serde_json::to_vec(&self.custody.destination).map_err(|_| invalid())?;
        let record = serde_json::to_vec(&self.custody.record).map_err(|_| invalid())?;
        let status = unsafe {
            restic_bridge_s3_teardown_retire_v1(
                self.locked.as_raw_fd(),
                destination.as_ptr().cast(),
                destination.len() as u32,
                record.as_ptr().cast(),
                record.len() as u32,
            )
        };
        if status != 0 {
            return Err(ExecuteError::Entry(status));
        }
        self.custody.record = RecordObservation {
            version: 1,
            present: false,
            device: 0,
            inode: 0,
            id: String::new(),
            operation: 0,
            active: false,
            scope: String::new(),
            origin: String::new(),
            reconciled: false,
        };
        self.context.revalidate()?;
        operation.remaining().map(|_| ()).map_err(|_| invalid())
    }

    /// Re-observe an already confirmed empty destination under this same live
    /// descriptor. This does not create a historical retirement receipt.
    pub fn confirm_retired(&mut self, operation: &TeardownOperation) -> Result<(), ExecuteError> {
        if self.custody.record.present {
            return Err(invalid());
        }
        self.retire(operation)
    }
}

fn options() -> native::Options {
    native::Options {
        version: 1,
        check_read_mode: 0,
        retry_lock_ms: 0,
        group: 0,
        groups: 0,
        reserved1: 0,
        reserved2: 0,
    }
}
