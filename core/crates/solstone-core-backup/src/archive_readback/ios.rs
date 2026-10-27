// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixed native reads under the original owned read, never an injected executor.

use crate::bounded_child::Output;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    ReadOperation, ReadbackError, RepositoryBinding, exact_archive_path, full_hex_identity,
    parse_bounded_nodes,
};
use crate::Destination;
use crate::ios_native::{self as native, Call};
use crate::ios_repository::{self, Repository};
use solstone_core_journal_io::JournalRoot;

pub struct IosReadState {
    path: String,
    file: File,
}
impl IosReadState {
    pub fn new(path: String, descriptor: OwnedFd) -> Result<Self, ReadbackError> {
        let state = Self {
            path,
            file: File::from(descriptor),
        };
        state.revalidate()?;
        Ok(state)
    }
    fn revalidate(&self) -> Result<(), ReadbackError> {
        let held = self
            .file
            .metadata()
            .map_err(|_| ReadbackError::ToolChanged)?;
        let named =
            std::fs::symlink_metadata(&self.path).map_err(|_| ReadbackError::ToolChanged)?;
        if !held.is_dir()
            || !named.is_dir()
            || held.dev() != named.dev()
            || held.ino() != named.ino()
        {
            return Err(ReadbackError::ToolChanged);
        }
        Ok(())
    }
}

/// Immutable session and actual startup/private-state custody. The check only
/// revalidates that retained custody; every read below calls the fixed C ABI.
/// Normal owner routes cannot construct or replace this context.
pub struct IosReadContext {
    root: Arc<JournalRoot>,
    generation: u64,
    destination: Destination,
    password: String,
    environment: BTreeMap<String, Option<String>>,
    state: Option<IosReadState>,
    check: Arc<dyn Fn() -> Result<(), ReadbackError> + Send + Sync>,
}
impl IosReadContext {
    pub fn new(
        root: Arc<JournalRoot>,
        generation: u64,
        destination: Destination,
        password: String,
        environment: BTreeMap<String, Option<String>>,
        state: Option<IosReadState>,
        check: Arc<dyn Fn() -> Result<(), ReadbackError> + Send + Sync>,
    ) -> Result<Self, ReadbackError> {
        let result = Self {
            root,
            generation,
            destination,
            password,
            environment,
            state,
            check,
        };
        if generation == 0 || result.password.is_empty() {
            return Err(ReadbackError::CredentialsUnavailable);
        }
        result.revalidate()?;
        match result.repository()? {
            Repository::Local(_) if result.state.is_none() => {}
            Repository::S3 { .. } if result.state.is_some() => {}
            _ => return Err(ReadbackError::UnsupportedBackend),
        }
        Ok(result)
    }
    #[cfg(all(
        any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        ),
        feature = "native-archive-host-tests"
    ))]
    pub(super) fn is_local_host_control(&self) -> bool {
        matches!(self.repository(), Ok(Repository::Local(_)))
    }
    fn repository(&self) -> Result<Repository<'_>, ReadbackError> {
        ios_repository::parse(
            &self.destination.repository,
            (!self.environment.is_empty()).then_some(&self.environment),
        )
        .map_err(|_| ReadbackError::CredentialsUnavailable)
    }
    pub(super) fn binds(
        &self,
        root: &Arc<JournalRoot>,
        destination: &Destination,
        password: &str,
    ) -> bool {
        Arc::ptr_eq(root, &self.root)
            && destination == &self.destination
            && password == self.password
    }
    pub(super) fn revalidate(&self) -> Result<(), ReadbackError> {
        if native::drain_failed() {
            return Err(ReadbackError::NativeFailure);
        }
        self.root
            .revalidate_canonical_binding()
            .map_err(|_| ReadbackError::TargetRootChanged)?;
        (self.check)()?;
        if let Some(state) = &self.state {
            state.revalidate()?;
        }
        Ok(())
    }
    fn remote<'a>(
        &'a self,
        destination: &'a crate::teardown_recovery::retained::NativeDestination,
        access: &'a str,
        secret: &'a str,
        token: &'a str,
    ) -> Result<native::Remote<'a>, ReadbackError> {
        let state = self
            .state
            .as_ref()
            .ok_or(ReadbackError::UnsupportedBackend)?;
        Ok(native::Remote {
            endpoint: &destination.endpoint,
            region: &destination.region,
            bucket: &destination.bucket,
            prefix: &destination.prefix,
            access: access.as_bytes(),
            secret: secret.as_bytes(),
            token: token.as_bytes(),
            state: native::PrivateDirectory {
                path: &state.path,
                descriptor: state.file.as_fd(),
            },
        })
    }
    fn preflight(&self, read: &ReadOperation<'_>) -> Result<(), ReadbackError> {
        self.revalidate()?;
        read.revalidate_local()?;
        read.operation.budget().remaining(read.operation.clock())?;
        Ok(())
    }
    fn start_metadata(
        &self,
        read: &ReadOperation<'_>,
        operation: i32,
    ) -> Result<Call, ReadbackError> {
        self.preflight(read)?;
        let options = native::Options {
            version: 1,
            check_read_mode: 0,
            retry_lock_ms: if operation == 2 { 0 } else { 1_800_000 },
            group: 0,
            groups: 0,
            reserved1: 0,
            reserved2: 0,
        };
        let call = match self.repository()? {
            Repository::Local(path) => Call::local(
                operation,
                self.password.as_bytes(),
                &[],
                path,
                None,
                None,
                &[],
                &options,
            ),
            Repository::S3 {
                destination,
                access,
                secret,
                token,
                append_only,
            } => {
                let options = native::Options {
                    version: if append_only { 2 } else { 1 },
                    reserved1: u32::from(append_only),
                    ..options
                };
                Call::remote(
                    operation,
                    self.password.as_bytes(),
                    &[],
                    &self.remote(&destination, access, secret, token)?,
                    None,
                    None,
                    &[],
                    &options,
                )
            }
        };
        call.map_err(|_| ReadbackError::ToolFailed)
    }
    fn start_list(&self, read: &ReadOperation<'_>, snapshot: &str) -> Result<Call, ReadbackError> {
        self.preflight(read)?;
        let call = match self.repository()? {
            Repository::Local(path) => Call::list_local(self.password.as_bytes(), path, snapshot),
            Repository::S3 {
                destination,
                access,
                secret,
                token,
                append_only,
            } => Call::list_remote(
                self.password.as_bytes(),
                &self.remote(&destination, access, secret, token)?,
                snapshot,
                append_only,
            ),
        };
        call.map_err(|_| ReadbackError::ToolFailed)
    }
    fn execute(
        &self,
        read: &ReadOperation<'_>,
        call: Call,
        selection: &str,
        bounded_query: bool,
    ) -> Result<native::Completed, ReadbackError> {
        self.revalidate()?;
        read.revalidate_local()?;
        let remaining = read.operation.budget().remaining(read.operation.clock())?;
        let timeout = if bounded_query {
            remaining.min(std::time::Duration::from_secs(
                crate::archive_admission::ARCHIVE_LS_TIMEOUT_SECONDS,
            ))
        } else {
            remaining
        };
        let handle = call.handle();
        let bound = observation(read, handle, self.generation, selection, false)?;
        read.execution.borrow_mut().attempt();
        let cancellation = read.operation.budget().cancellation().cloned();
        let root = Arc::clone(read.journal_root());
        // Monitor retained thread-safe custody and the original cancellation.
        // The non-Sync read/execution state stays on its owning worker; its
        // configuration and clock are checked before and after the actual call.
        let cancelled = || {
            cancellation
                .as_ref()
                .is_some_and(crate::archive_admission::ArchiveCancellation::is_cancelled)
                || root.revalidate_canonical_binding().is_err()
                || self.revalidate().is_err()
        };
        // wait/free owns this actual frame. Unknown drain never returns.
        let completed = call.wait(Some(timeout), Some(&cancelled), true);
        read.execution.borrow_mut().returned();
        self.revalidate()?;
        let finished = observation(read, handle, self.generation, selection, true)?;
        if !super::same_owner(&bound, &finished) {
            return Err(ReadbackError::NativeFailure);
        }
        if completed.cancellation_requested
            || completed.deadline_requested
            || completed.monitor_failed
        {
            return Err(read.execution_error());
        }
        if completed.fields.status != 0 {
            #[cfg(feature = "native-archive-host-tests")]
            eprintln!(
                "native read returned status={} diagnostic={:?}",
                completed.fields.status,
                std::str::from_utf8(&completed.fields.diagnostic).ok()
            );
            return Err(if completed.fields.status == 5 {
                ReadbackError::IncompleteDiscovery
            } else {
                ReadbackError::ToolFailed
            });
        }
        Ok(completed)
    }
    fn metadata<'a>(&self, completed: &'a native::Completed) -> Result<&'a [u8], ReadbackError> {
        completed
            .metadata
            .as_ref()
            .ok()
            .and_then(Option::as_deref)
            .ok_or(ReadbackError::IncompleteDiscovery)
    }
    fn resolve(&self, read: &ReadOperation<'_>, selector: &str) -> Result<String, ReadbackError> {
        if selector.is_empty()
            || selector.len() > 64
            || !selector.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(ReadbackError::InvalidSelection);
        }
        if full_hex_identity(selector) {
            return Ok(selector.to_owned());
        }
        let completed = self.execute(
            read,
            self.start_metadata(read, 5)?,
            &format!("catalog:{selector}"),
            true,
        )?;
        let value: Value = serde_json::from_slice(self.metadata(&completed)?)
            .map_err(|_| ReadbackError::IncompleteDiscovery)?;
        if value["schema"] != "solstone.backup-metadata.v1" || value["kind"] != "snapshot-catalog" {
            return Err(ReadbackError::IncompleteDiscovery);
        }
        let rows = value["snapshots"]
            .as_array()
            .ok_or(ReadbackError::IncompleteDiscovery)?;
        if rows.len() != completed.fields.snapshot_count as usize || rows.len() > 4096 {
            return Err(ReadbackError::IncompleteDiscovery);
        }
        let mut native_ids = BTreeSet::new();
        for field in completed.fields.snapshot_ids.iter().take(rows.len()) {
            let id = std::str::from_utf8(&field[..64])
                .map_err(|_| ReadbackError::IncompleteDiscovery)?;
            if field[64] != 0 || !full_hex_identity(id) || !native_ids.insert(id) {
                return Err(ReadbackError::IncompleteDiscovery);
            }
        }
        let mut all = BTreeSet::new();
        let mut matches = Vec::new();
        for row in rows {
            let id = row["id"]
                .as_str()
                .filter(|id| full_hex_identity(id))
                .ok_or(ReadbackError::IncompleteDiscovery)?;
            if !all.insert(id) {
                return Err(ReadbackError::IncompleteDiscovery);
            }
            if id.starts_with(selector) {
                matches.push(id);
            }
        }
        if all != native_ids {
            return Err(ReadbackError::IncompleteDiscovery);
        }
        if matches.len() != 1 {
            return Err(ReadbackError::SnapshotNodeUnavailable);
        }
        Ok(matches[0].to_owned())
    }
    pub(super) fn query(
        &self,
        read: &ReadOperation<'_>,
        argv: &[OsString],
    ) -> Result<NativeQuery, ReadbackError> {
        let args: Vec<_> = argv
            .iter()
            .map(|s| s.to_str().ok_or(ReadbackError::InvalidSelection))
            .collect::<Result<_, _>>()?;
        if args.as_slice() == ["cat", "config", "--no-cache"] || args.as_slice() == ["version"] {
            let completed = self.execute(
                read,
                self.start_metadata(read, 2)?,
                "repository-config",
                true,
            )?;
            let value: Value = serde_json::from_slice(self.metadata(&completed)?)
                .map_err(|_| ReadbackError::InvalidRepositoryIdentity)?;
            if value["schema"] != "solstone.backup-metadata.v1"
                || value["kind"] != "repository-config"
                || value["repository"]["id"]
                    .as_str()
                    .is_none_or(|id| !full_hex_identity(id))
            {
                return Err(ReadbackError::InvalidRepositoryIdentity);
            }
            let bytes = if args.as_slice() == ["version"] {
                // The retained startup check verifies this linked recipe. Real
                // repository qualification above cannot be inherited from creation.
                format!("restic {}\n", super::SUPPORTED_RESTIC_VERSION).into_bytes()
            } else {
                serde_json::to_vec(&value["repository"])
                    .map_err(|_| ReadbackError::InvalidRepositoryIdentity)?
            };
            return Ok(NativeQuery::new(completed, bytes));
        }
        let selector = match args.as_slice() {
            ["ls", "--json", selector, "--no-cache"] => *selector,
            ["ls", "--json", selector, path, "--no-cache"] if exact_archive_path(path) => *selector,
            _ => return Err(ReadbackError::Unsupported),
        };
        let snapshot = self.resolve(read, selector)?;
        let completed = self.execute(
            read,
            self.start_list(read, &snapshot)?,
            &format!("nodes:{snapshot}"),
            true,
        )?;
        let nodes = parse_bounded_nodes(self.metadata(&completed)?)?;
        let mut paths = BTreeSet::new();
        let mut bytes = serde_json::to_vec(&json!({"message_type":"snapshot","id":snapshot}))
            .map_err(|_| ReadbackError::IncompleteDiscovery)?;
        bytes.push(b'\n');
        for node in nodes {
            let path = shared_path(&node.path)?;
            if !paths.insert(path.clone()) {
                return Err(ReadbackError::IncompleteDiscovery);
            }
            serde_json::to_writer(
                &mut bytes,
                &json!({"message_type":"node","type":node.node_type,
                "path":path,"name":node.name,"size":node.size}),
            )
            .map_err(|_| ReadbackError::IncompleteDiscovery)?;
            bytes.push(b'\n');
            if bytes.len() > super::MAX_QUERY_BYTES {
                return Err(ReadbackError::IncompleteDiscovery);
            }
        }
        Ok(NativeQuery::new(completed, bytes))
    }
    pub(super) fn download(
        &self,
        read: &ReadOperation<'_>,
        snapshot: &str,
        path: &str,
        target: BorrowedFd<'_>,
        size: u64,
        target_identity: &str,
    ) -> Result<NativeQuery, ReadbackError> {
        if !full_hex_identity(snapshot) {
            return Err(ReadbackError::InvalidSelection);
        }
        let relative = native_path(path)?;
        self.preflight(read)?;
        let call = match self.repository()? {
            Repository::Local(repository) => Call::download_local(
                self.password.as_bytes(),
                repository,
                snapshot,
                relative,
                target,
                size,
            ),
            Repository::S3 {
                destination,
                access,
                secret,
                token,
                append_only,
            } => Call::download_remote(
                self.password.as_bytes(),
                &self.remote(&destination, access, secret, token)?,
                snapshot,
                relative,
                target,
                size,
                append_only,
            ),
        }
        .map_err(|_| ReadbackError::ToolFailed)?;
        let completed = self.execute(
            read,
            call,
            &format!("file:{snapshot}\n{relative}\n{size}\n{target_identity}"),
            false,
        )?;
        let value: Value = serde_json::from_slice(self.metadata(&completed)?)
            .map_err(|_| ReadbackError::IncompleteDiscovery)?;
        if value["drained"].as_u64() != Some(size) {
            return Err(ReadbackError::NativeFailure);
        }
        Ok(NativeQuery::new(completed, Vec::new()))
    }
}

pub(super) struct NativeQuery {
    output: Output,
    _completed: native::Completed,
}
impl NativeQuery {
    fn new(completed: native::Completed, stdout: Vec<u8>) -> Self {
        Self {
            output: Output {
                returncode: completed.fields.status,
                stdout,
                stderr: Vec::new(),
            },
            _completed: completed,
        }
    }
    pub(super) fn output(&self) -> &Output {
        &self.output
    }
    pub(super) fn successful(&self) -> bool {
        self._completed.fields.status == 0
    }
}

fn shared_path(relative: &str) -> Result<String, ReadbackError> {
    if relative.is_empty()
        || relative.len() > 4096
        || relative.starts_with('/')
        || relative.contains('\\')
    {
        return Err(ReadbackError::SnapshotNodeUnavailable);
    }
    let absolute = format!("/{relative}");
    if !exact_archive_path(&absolute) || native_path(&absolute)? != relative {
        return Err(ReadbackError::SnapshotNodeUnavailable);
    }
    Ok(absolute)
}
fn native_path(absolute: &str) -> Result<&str, ReadbackError> {
    if !exact_archive_path(absolute) {
        return Err(ReadbackError::SnapshotNodeUnavailable);
    }
    let relative = absolute
        .strip_prefix('/')
        .ok_or(ReadbackError::SnapshotNodeUnavailable)?;
    if relative.is_empty()
        || relative.len() > 4096
        || relative.starts_with('/')
        || relative.contains('\\')
    {
        return Err(ReadbackError::SnapshotNodeUnavailable);
    }
    Ok(relative)
}
fn observation(
    read: &ReadOperation<'_>,
    handle: u64,
    generation: u64,
    selection: &str,
    drained: bool,
) -> Result<super::OwnerObservation, ReadbackError> {
    read.revalidate_local()?;
    let clock = read.owned_clock().ok_or(ReadbackError::Unsupported)?;
    let budget = read.operation.budget();
    let repository = match &read.repository {
        RepositoryBinding::Expected(id) | RepositoryBinding::Confirmed(id) => id.clone(),
        RepositoryBinding::CaptureCurrent => "capture-current".to_owned(),
    };
    Ok(super::OwnerObservation {
        activity: Arc::as_ptr(read.activity()) as usize as u64,
        admission: read as *const ReadOperation<'_> as usize as u64,
        clock: Arc::as_ptr(clock).cast::<()>() as usize,
        deadline: budget.deadline(),
        journal: Arc::as_ptr(read.journal_root()) as usize as u64,
        repository,
        sources: String::new(),
        target: selection.to_owned(),
        call_handle: handle,
        owner_generation: generation,
        drain_ok: drained,
        cancelled: budget
            .cancellation()
            .is_some_and(crate::archive_admission::ArchiveCancellation::is_cancelled),
    })
}

#[cfg(test)]
mod tests {
    use super::{native_path, shared_path};

    #[test]
    fn native_node_paths_round_trip_without_normalizing() {
        for relative in [
            "archive/clip.m4a",
            "archive/sub/derived.txt",
            "archive/é.txt",
        ] {
            let shared = shared_path(relative).unwrap();
            assert_eq!(shared, format!("/{relative}"));
            assert_eq!(native_path(&shared).unwrap(), relative);
        }
    }

    #[test]
    fn ambiguous_or_out_of_bound_node_spellings_refuse() {
        for relative in [
            "",
            "/archive/a",
            "//archive/a",
            "archive//a",
            "archive/../a",
            "archive/./a",
            "archive\\a",
        ] {
            assert!(shared_path(relative).is_err(), "{relative:?}");
        }
        let oversized = format!("archive/{}", "a".repeat(4096));
        assert!(shared_path(&oversized).is_err());
        assert!(native_path(&format!("/{oversized}")).is_err());
    }
}
