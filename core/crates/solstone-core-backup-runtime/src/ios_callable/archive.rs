// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Linked retained-source archive, bounded node listing, and exact download.
//! These calls do not go through the ordinary argv decoder or whole-journal
//! snapshot and restore. Settlement is the creator's existing same-owner check.

use std::collections::BTreeSet;
use std::os::fd::AsFd;
use std::time::Duration;

use serde::Deserialize;
use solstone_core_backup::archive_admission::ArchiveCancellation;
use solstone_core_backup::archive_readback::{
    ArchiveNodeRecord, ReadbackError, full_hex_identity, parse_bounded_nodes,
};
use solstone_core_journal_io::media::{ArchiveSource, MediaIdentity, OwnedMediaTarget};
use zeroize::Zeroizing;

use super::AdmittedIosBackupAuthority;
use super::storage::Directory;
use crate::engine::{
    ArchiveError, ArchiveFailure, ArchiveRemoteEffects, ArchiveSnapshot, OwnedAdmittedArchive,
};
use solstone_core_backup::ios_native::{self as native, Call};
use solstone_core_backup::ios_repository::{self as repository, Repository};

const MAX_NAMES: usize = 4096;
const MAX_NAME_PAYLOAD: usize = 1024 * 1024;

struct OpenedSession {
    password: Zeroizing<Vec<u8>>,
    kind: OpenedKind,
}

enum OpenedKind {
    Local {
        path: String,
    },
    S3 {
        destination: repository::Destination,
        access: Zeroizing<Vec<u8>>,
        secret: Zeroizing<Vec<u8>>,
        token: Zeroizing<Vec<u8>>,
        append_only: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DrainReport {
    drained: u64,
}

fn not_attempted(error: ArchiveError) -> ArchiveFailure {
    ArchiveFailure {
        error,
        remote_effects: ArchiveRemoteEffects::NotAttempted,
    }
}

fn attempted(
    error: ArchiveError,
    status: Option<i32>,
    snapshot_id: Option<String>,
) -> ArchiveFailure {
    ArchiveFailure {
        error,
        remote_effects: ArchiveRemoteEffects::Attempted {
            returncode: status,
            observed_snapshot_id: snapshot_id.filter(|id| full_hex_identity(id)),
        },
    }
}

fn from_readback(reason: ReadbackError) -> ArchiveError {
    match reason {
        ReadbackError::Admission(reason) => reason.into(),
        ReadbackError::InvalidSelection => ArchiveError::InvalidSelection,
        ReadbackError::RepositoryIdentityChanged => ArchiveError::RepositoryIdentityChanged,
        ReadbackError::SnapshotNodeUnavailable => ArchiveError::SnapshotIdentityInvalid,
        ReadbackError::ToolUnavailable | ReadbackError::Unsupported => {
            ArchiveError::ToolUnavailable
        }
        other => ArchiveError::Readback(other),
    }
}

fn failed() -> ArchiveError {
    ArchiveError::ToolFailed { reason: "failed" }
}

fn logical_root_ok(root: &str) -> bool {
    !root.is_empty()
        && root != "."
        && root != ".."
        && root.len() <= 4096
        && !root.contains(['/', '\\', '\0'])
}

fn leaf_ok(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 4096
        && !name.contains(['/', '\\', '\0'])
}

fn relative_ok(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.starts_with('/')
        && !path.contains(['\\', '\0'])
        && !path.contains("//")
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn field_text(bytes: &[u8]) -> Option<&str> {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(0);
    let text = std::str::from_utf8(&bytes[..end]).ok()?;
    if text.is_empty() { None } else { Some(text) }
}

fn snapshot_text(fields: &native::ResultFields) -> Option<String> {
    let text = field_text(&fields.snapshot_id)?;
    full_hex_identity(text).then(|| text.to_owned())
}

fn diagnostic_token(fields: &native::ResultFields) -> &str {
    field_text(&fields.diagnostic).unwrap_or("")
}

fn effect_error(completed: &native::Completed, listing: bool) -> Option<ArchiveError> {
    if completed.monitor_failed {
        return Some(failed());
    }
    if completed.deadline_requested {
        return Some(ArchiveError::DeadlineExceeded);
    }
    let token = diagnostic_token(&completed.fields);
    let status = completed.fields.status;
    if listing && (status == 5 || token == "INCOMPLETE_DISCOVERY") {
        return Some(ArchiveError::Readback(ReadbackError::IncompleteDiscovery));
    }
    if token == "SNAPSHOT_NODE_UNAVAILABLE" {
        return Some(ArchiveError::SnapshotIdentityInvalid);
    }
    if completed.cancellation_requested || status == 3 {
        return Some(ArchiveError::Cancelled);
    }
    if status != 0 {
        return Some(failed());
    }
    None
}

fn open_session(
    creator: &solstone_core_backup::archive_readback::OwnedArchiveCreator,
) -> Result<OpenedSession, ArchiveError> {
    let (destination, password, env) = creator.linked_session().map_err(from_readback)?;
    if password.is_empty() {
        return Err(ArchiveError::ConfigurationUnavailable);
    }
    let parsed = repository::parse(&destination.repository, (!env.is_empty()).then_some(&env))
        .map_err(|_| ArchiveError::ConfigurationUnavailable)?;
    let password = Zeroizing::new(password.into_bytes());
    let kind = match parsed {
        Repository::Local(path) => OpenedKind::Local {
            path: path.to_owned(),
        },
        Repository::S3 {
            destination,
            access,
            secret,
            token,
            append_only,
        } => OpenedKind::S3 {
            destination,
            access: Zeroizing::new(access.as_bytes().to_vec()),
            secret: Zeroizing::new(secret.as_bytes().to_vec()),
            token: Zeroizing::new(token.as_bytes().to_vec()),
            append_only,
        },
    };
    Ok(OpenedSession { password, kind })
}

fn fresh_timeout(
    creator: &solstone_core_backup::archive_readback::OwnedArchiveCreator,
) -> Result<(Duration, Option<ArchiveCancellation>), ArchiveError> {
    let prepared = creator.prepared_operation();
    let cancellation = prepared.budget().cancellation().cloned();
    if cancellation
        .as_ref()
        .is_some_and(ArchiveCancellation::is_cancelled)
    {
        return Err(ArchiveError::Cancelled);
    }
    let timeout = prepared
        .budget()
        .remaining(prepared.clock())
        .map_err(ArchiveError::from)?;
    Ok((timeout, cancellation))
}

fn source_names(sources: &[ArchiveSource<'_>]) -> Result<Vec<String>, ArchiveError> {
    if sources.is_empty() || sources.len() > MAX_NAMES {
        return Err(ArchiveError::InvalidSelection);
    }
    let mut names = Vec::with_capacity(sources.len());
    let mut seen = BTreeSet::new();
    for source in sources {
        source
            .revalidate()
            .map_err(|_| ArchiveError::SourceUnavailable)?;
        let name = source
            .name()
            .to_str()
            .filter(|name| leaf_ok(name))
            .ok_or(ArchiveError::InvalidSelection)?;
        if !seen.insert(name.to_owned()) {
            return Err(ArchiveError::InvalidSelection);
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

fn stage_fd<'a>(
    sources: &'a [ArchiveSource<'a>],
) -> Result<std::os::fd::BorrowedFd<'a>, ArchiveError> {
    let first = sources.first().ok_or(ArchiveError::InvalidSelection)?;
    let expected = nix::sys::stat::fstat(first.stage_directory())
        .map_err(|_| ArchiveError::SourceUnavailable)?;
    for source in sources.iter().skip(1) {
        let actual = nix::sys::stat::fstat(source.stage_directory())
            .map_err(|_| ArchiveError::SourceUnavailable)?;
        if actual.st_dev != expected.st_dev || actual.st_ino != expected.st_ino {
            return Err(ArchiveError::InvalidSelection);
        }
    }
    Ok(first.stage_directory())
}

fn revalidate_sources(sources: &[ArchiveSource<'_>]) -> Result<(), ArchiveError> {
    for source in sources {
        source
            .revalidate()
            .map_err(|_| ArchiveError::SourceUnavailable)?;
    }
    Ok(())
}

fn destination_state(
    authority: &AdmittedIosBackupAuthority,
    session: &OpenedSession,
) -> Result<Option<Directory>, ArchiveError> {
    match &session.kind {
        OpenedKind::Local { .. } => Ok(None),
        OpenedKind::S3 { destination, .. } => {
            let state = authority
                .open_destination_state(&destination.namespace())
                .map_err(|_| ArchiveError::ConfigurationChanged)?;
            state
                .revalidate()
                .map_err(|_| ArchiveError::ConfigurationChanged)?;
            Ok(Some(state))
        }
    }
}

fn remote<'a>(
    session: &'a OpenedSession,
    state: &'a Directory,
) -> Result<native::Remote<'a>, ArchiveError> {
    let OpenedKind::S3 {
        destination,
        access,
        secret,
        token,
        ..
    } = &session.kind
    else {
        return Err(ArchiveError::ConfigurationUnavailable);
    };
    Ok(native::Remote {
        endpoint: &destination.endpoint,
        region: &destination.region,
        bucket: &destination.bucket,
        prefix: &destination.prefix,
        access: access.as_slice(),
        secret: secret.as_slice(),
        token: token.as_slice(),
        state: native::PrivateDirectory {
            path: state.path(),
            descriptor: state.as_fd(),
        },
    })
}

fn append_only(session: &OpenedSession) -> bool {
    match &session.kind {
        OpenedKind::Local { .. } => false,
        OpenedKind::S3 { append_only, .. } => *append_only,
    }
}

fn start_archive(
    session: &OpenedSession,
    state: Option<&Directory>,
    logical_root: &str,
    source: std::os::fd::BorrowedFd<'_>,
    names: &[u8],
) -> Result<Call, i32> {
    match &session.kind {
        OpenedKind::Local { path } => Call::archive_local(
            session.password.as_slice(),
            path,
            logical_root,
            source,
            names,
        ),
        OpenedKind::S3 { .. } => {
            let state = state.ok_or(1)?;
            let remote = remote(session, state).map_err(|_| 1)?;
            Call::archive_remote(
                session.password.as_slice(),
                &remote,
                logical_root,
                source,
                names,
                append_only(session),
            )
        }
    }
}

fn start_list(
    session: &OpenedSession,
    state: Option<&Directory>,
    snapshot_id: &str,
) -> Result<Call, i32> {
    match &session.kind {
        OpenedKind::Local { path } => {
            Call::list_local(session.password.as_slice(), path, snapshot_id)
        }
        OpenedKind::S3 { .. } => {
            let state = state.ok_or(1)?;
            let remote = remote(session, state).map_err(|_| 1)?;
            Call::list_remote(
                session.password.as_slice(),
                &remote,
                snapshot_id,
                append_only(session),
            )
        }
    }
}

fn start_download(
    session: &OpenedSession,
    state: Option<&Directory>,
    snapshot_id: &str,
    node_path: &str,
    target: std::os::fd::BorrowedFd<'_>,
    size_bound: u64,
) -> Result<Call, i32> {
    match &session.kind {
        OpenedKind::Local { path } => Call::download_local(
            session.password.as_slice(),
            path,
            snapshot_id,
            node_path,
            target,
            size_bound,
        ),
        OpenedKind::S3 { .. } => {
            let state = state.ok_or(1)?;
            let remote = remote(session, state).map_err(|_| 1)?;
            Call::download_remote(
                session.password.as_slice(),
                &remote,
                snapshot_id,
                node_path,
                target,
                size_bound,
                append_only(session),
            )
        }
    }
}

struct BoundCall {
    call: Call,
    handle: u64,
    generation: u64,
    timeout: Duration,
    cancellation: Option<ArchiveCancellation>,
}

fn bind_call(
    archive: &mut OwnedAdmittedArchive,
    authority: &AdmittedIosBackupAuthority,
    sources: &[ArchiveSource<'_>],
    target_identity: &str,
    call: Call,
    timeout: Duration,
    cancellation: Option<ArchiveCancellation>,
) -> Result<BoundCall, ArchiveFailure> {
    let handle = call.handle();
    let generation = authority.generation();
    if let Err(reason) = archive.linked_creator_mut().begin_linked_effect(
        sources,
        handle,
        generation,
        target_identity,
    ) {
        drop(call);
        return Err(attempted(from_readback(reason), None, None));
    }
    Ok(BoundCall {
        call,
        handle,
        generation,
        timeout,
        cancellation,
    })
}

fn wait_bound(
    archive: &OwnedAdmittedArchive,
    authority: &AdmittedIosBackupAuthority,
    state: Option<&Directory>,
    sources: &[ArchiveSource<'_>],
    bound: BoundCall,
    metadata: bool,
    listing: bool,
    observe_snapshot: bool,
) -> Result<(native::Completed, u64, u64), ArchiveFailure> {
    let BoundCall {
        call,
        handle,
        generation,
        timeout,
        cancellation,
    } = bound;
    let cancelled = || {
        cancellation
            .as_ref()
            .is_some_and(ArchiveCancellation::is_cancelled)
    };
    let completed = call.wait(Some(timeout), Some(&cancelled), metadata);
    let status = completed.fields.status;
    let snapshot = observe_snapshot
        .then(|| snapshot_text(&completed.fields))
        .flatten();
    if let Some(error) = effect_error(&completed, listing) {
        return Err(attempted(error, Some(status), snapshot));
    }
    if let Err(error) = authority
        .revalidate()
        .map(|_| ())
        .map_err(|_| ArchiveError::ConfigurationChanged)
        .and_then(|()| fresh_timeout(archive.linked_creator()).map(|_| ()))
        .and_then(|()| revalidate_sources(sources))
        .and_then(|()| {
            state
                .map(Directory::revalidate)
                .transpose()
                .map(|_| ())
                .map_err(|_| ArchiveError::ConfigurationChanged)
        })
    {
        return Err(attempted(error, Some(status), snapshot));
    }
    Ok((completed, handle, generation))
}

fn settle(
    archive: &mut OwnedAdmittedArchive,
    sources: &[ArchiveSource<'_>],
    snapshot_id: &str,
    handle: u64,
    generation: u64,
    target_identity: &str,
    status: i32,
) -> Result<ArchiveSnapshot, ArchiveFailure> {
    match archive.linked_creator_mut().finish_linked_effect(
        sources,
        snapshot_id,
        handle,
        generation,
        true,
        target_identity,
    ) {
        Ok(snapshot) => Ok(ArchiveSnapshot {
            repository_id: snapshot.repository_id,
            snapshot_id: snapshot.snapshot_id,
        }),
        Err(reason) => Err(attempted(
            from_readback(reason),
            Some(status),
            Some(snapshot_id.to_owned()),
        )),
    }
}

fn prepare(
    archive: &OwnedAdmittedArchive,
    authority: &AdmittedIosBackupAuthority,
) -> Result<
    (
        OpenedSession,
        Option<Directory>,
        Duration,
        Option<ArchiveCancellation>,
    ),
    ArchiveFailure,
> {
    let creator = archive.linked_creator();
    let session = open_session(creator).map_err(not_attempted)?;
    authority
        .revalidate_archive_root(archive.journal_root())
        .map_err(|_| not_attempted(ArchiveError::ConfigurationChanged))?;
    let (timeout, cancellation) = fresh_timeout(creator).map_err(not_attempted)?;
    let state = destination_state(authority, &session).map_err(not_attempted)?;
    Ok((session, state, timeout, cancellation))
}

impl OwnedAdmittedArchive {
    /// Archive the exact retained leaves under one logical root.
    /// Opcode 20. A confirmed drain settles this same owner; a failed drain does not.
    pub fn archive_retained_sources(
        &mut self,
        authority: &AdmittedIosBackupAuthority,
        logical_root: &str,
        sources: &[ArchiveSource<'_>],
    ) -> Result<ArchiveSnapshot, ArchiveFailure> {
        if !logical_root_ok(logical_root) {
            return Err(not_attempted(ArchiveError::InvalidSelection));
        }
        let names = source_names(sources).map_err(not_attempted)?;
        let source = stage_fd(sources).map_err(not_attempted)?;
        let payload = serde_json::to_vec(&names)
            .map_err(|_| not_attempted(ArchiveError::InvalidSelection))?;
        if payload.is_empty() || payload.len() > MAX_NAME_PAYLOAD {
            return Err(not_attempted(ArchiveError::InvalidSelection));
        }
        let (session, state, timeout, cancellation) = prepare(self, authority)?;
        let call = start_archive(&session, state.as_ref(), logical_root, source, &payload)
            .map_err(|_| not_attempted(failed()))?;
        let bound = bind_call(
            self,
            authority,
            sources,
            logical_root,
            call,
            timeout,
            cancellation,
        )?;
        let (completed, handle, generation) = wait_bound(
            self,
            authority,
            state.as_ref(),
            sources,
            bound,
            false,
            false,
            true,
        )?;
        let Some(snapshot_id) = snapshot_text(&completed.fields) else {
            return Err(attempted(
                ArchiveError::SnapshotIdentityInvalid,
                Some(completed.fields.status),
                None,
            ));
        };
        settle(
            self,
            sources,
            &snapshot_id,
            handle,
            generation,
            logical_root,
            completed.fields.status,
        )
    }

    /// List every node in one snapshot. An incomplete walk is a refusal.
    /// Opcode 21.
    pub fn list_retained_nodes(
        &mut self,
        authority: &AdmittedIosBackupAuthority,
        snapshot_id: &str,
        sources: &[ArchiveSource<'_>],
    ) -> Result<Vec<ArchiveNodeRecord>, ArchiveFailure> {
        if !full_hex_identity(snapshot_id) {
            return Err(not_attempted(ArchiveError::SnapshotIdentityInvalid));
        }
        revalidate_sources(sources).map_err(not_attempted)?;
        let (session, state, timeout, cancellation) = prepare(self, authority)?;
        let call = start_list(&session, state.as_ref(), snapshot_id)
            .map_err(|_| not_attempted(failed()))?;
        let bound = bind_call(
            self,
            authority,
            sources,
            snapshot_id,
            call,
            timeout,
            cancellation,
        )?;
        let (completed, handle, generation) = wait_bound(
            self,
            authority,
            state.as_ref(),
            sources,
            bound,
            true,
            true,
            false,
        )?;
        let status = completed.fields.status;
        let bytes = completed
            .metadata
            .as_ref()
            .ok()
            .and_then(Option::as_deref)
            .ok_or_else(|| {
                attempted(
                    ArchiveError::Readback(ReadbackError::IncompleteDiscovery),
                    Some(status),
                    None,
                )
            })?;
        let nodes = parse_bounded_nodes(bytes)
            .map_err(|reason| attempted(from_readback(reason), Some(status), None))?;
        settle(
            self,
            sources,
            snapshot_id,
            handle,
            generation,
            snapshot_id,
            status,
        )?;
        Ok(nodes)
    }

    /// Download one archived file into an already issued target.
    /// Opcode 22. Returns the drained byte count after same-owner settlement.
    pub fn read_retained_file(
        &mut self,
        authority: &AdmittedIosBackupAuthority,
        snapshot_id: &str,
        node_path: &str,
        target: &mut OwnedMediaTarget,
        size_bound: u64,
        sources: &[ArchiveSource<'_>],
    ) -> Result<u64, ArchiveFailure> {
        if !full_hex_identity(snapshot_id) {
            return Err(not_attempted(ArchiveError::SnapshotIdentityInvalid));
        }
        if !relative_ok(node_path) {
            return Err(not_attempted(ArchiveError::InvalidSelection));
        }
        revalidate_sources(sources).map_err(not_attempted)?;
        let identity = target.identity();
        let MediaIdentity::Unix { device, inode } = identity else {
            return Err(not_attempted(ArchiveError::InvalidSelection));
        };
        let target_identity = format!("{snapshot_id}\n{node_path}\n{device}:{inode}");
        let (session, state, timeout, cancellation) = prepare(self, authority)?;
        let transport = target
            .helper_transport()
            .map_err(|_| not_attempted(ArchiveError::SourceUnavailable))?;
        let call = start_download(
            &session,
            state.as_ref(),
            snapshot_id,
            node_path,
            transport.descriptor,
            size_bound,
        )
        .map_err(|_| not_attempted(failed()))?;
        let bound = bind_call(
            self,
            authority,
            sources,
            &target_identity,
            call,
            timeout,
            cancellation,
        )?;
        let (completed, handle, generation) = wait_bound(
            self,
            authority,
            state.as_ref(),
            sources,
            bound,
            true,
            false,
            false,
        )?;
        let status = completed.fields.status;
        let bytes = completed
            .metadata
            .as_ref()
            .ok()
            .and_then(Option::as_deref)
            .ok_or_else(|| attempted(failed(), Some(status), None))?;
        let report: DrainReport =
            serde_json::from_slice(bytes).map_err(|_| attempted(failed(), Some(status), None))?;
        if report.drained > size_bound {
            return Err(attempted(failed(), Some(status), None));
        }
        settle(
            self,
            sources,
            snapshot_id,
            handle,
            generation,
            &target_identity,
            status,
        )?;
        drop(transport);
        Ok(report.drained)
    }
}
