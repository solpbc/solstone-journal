// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exclusive create, lease, and no-replace publish for one oplog leaf.

use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io::{ErrorKind, Write};
#[cfg(any(test, feature = "test-hooks"))]
use std::time::Duration;

use chrono::{DateTime, FixedOffset};

use super::admission::encode_oplog_admission;
use super::sample_local_instant;

#[cfg(any(test, feature = "test-hooks"))]
use super::lock::acquire_oplog_namespace_lock_with_test_timing;
use super::lock::{OplogNamespaceLockError, acquire_oplog_namespace_lock};
use super::name::{
    OplogFormat, derive_day_key_and_opened_field, file_id_hex, format_oplog_name,
    oplog_name_from_parts, original_is_admissible,
};
use super::namespace::{OplogDayHealth, OplogNamespaceError, admit_day_health_directory};
use super::writer::OplogWriter;
use crate::journal_root::JournalRoot;
use crate::lease::LeaseProbe;

#[cfg(unix)]
use super::unix as platform;
#[cfg(windows)]
use super::windows as platform;

/// Maximum dest-occupied retries, each consuming one pre-drawn file id.
pub const OPLOG_CREATE_ATTEMPTS: usize = 8;
/// Maximum `draw_file_id` calls used to collect [`OPLOG_CREATE_ATTEMPTS`] distinct ids.
pub const OPLOG_FILE_ID_DRAW_BUDGET: usize = 64;

/// Bounded failure while creating an operational log.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OplogCreateError {
    class: OplogCreateClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogCreateClass {
    InvalidField,
    Io,
    EntropySource,
    #[cfg(any(test, feature = "test-hooks"))]
    Sampler,
    LeaseFailed,
    OwnResidue,
    ForeignResidue,
    Aliased,
    RetryExhausted,
    EntropyExhausted,
    LockUnsafe,
    LockIdentityChanged,
    LockBusy,
    LockIo,
    AncestorReplaced,
    NamespaceChronicleUnsafe,
    NamespaceChronicleIdentityChanged,
    NamespaceChronicleIo,
    NamespaceDayUnsafe,
    NamespaceDayIdentityChanged,
    NamespaceDayIo,
    NamespaceHealthUnsafe,
    NamespaceHealthIdentityChanged,
    NamespaceHealthIo,
}

impl OplogCreateClass {
    const fn token(self) -> &'static str {
        match self {
            Self::InvalidField => "oplog_create_invalid_field",
            Self::Io => "oplog_create_io",
            Self::EntropySource => "oplog_create_entropy_source",
            #[cfg(any(test, feature = "test-hooks"))]
            Self::Sampler => "oplog_create_sampler",
            Self::LeaseFailed => "oplog_create_lease_failed",
            Self::OwnResidue => "oplog_create_own_residue",
            Self::ForeignResidue => "oplog_create_foreign_residue",
            Self::Aliased => "oplog_create_aliased",
            Self::RetryExhausted => "oplog_create_retry_exhausted",
            Self::EntropyExhausted => "oplog_create_entropy_exhausted",
            Self::LockUnsafe => "oplog_create_lock_unsafe",
            Self::LockIdentityChanged => "oplog_create_lock_identity_changed",
            Self::LockBusy => "oplog_create_lock_busy",
            Self::LockIo => "oplog_create_lock_io",
            Self::AncestorReplaced => "oplog_create_ancestor_replaced",
            Self::NamespaceChronicleUnsafe => "oplog_create_namespace_chronicle_unsafe",
            Self::NamespaceChronicleIdentityChanged => {
                "oplog_create_namespace_chronicle_identity_changed"
            }
            Self::NamespaceChronicleIo => "oplog_create_namespace_chronicle_io",
            Self::NamespaceDayUnsafe => "oplog_create_namespace_day_unsafe",
            Self::NamespaceDayIdentityChanged => "oplog_create_namespace_day_identity_changed",
            Self::NamespaceDayIo => "oplog_create_namespace_day_io",
            Self::NamespaceHealthUnsafe => "oplog_create_namespace_health_unsafe",
            Self::NamespaceHealthIdentityChanged => {
                "oplog_create_namespace_health_identity_changed"
            }
            Self::NamespaceHealthIo => "oplog_create_namespace_health_io",
        }
    }
}

impl OplogCreateError {
    const fn new(class: OplogCreateClass) -> Self {
        Self { class }
    }

    pub(super) const fn io() -> Self {
        Self::new(OplogCreateClass::Io)
    }

    pub(super) const fn entropy_source() -> Self {
        Self::new(OplogCreateClass::EntropySource)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) const fn sampler() -> Self {
        Self::new(OplogCreateClass::Sampler)
    }

    pub(super) const fn own_residue() -> Self {
        Self::new(OplogCreateClass::OwnResidue)
    }

    fn token(self) -> &'static str {
        self.class.token()
    }
}

impl fmt::Display for OplogCreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

impl fmt::Debug for OplogCreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for OplogCreateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

enum LockTiming {
    Default,
    #[cfg(any(test, feature = "test-hooks"))]
    Explicit(Duration, Duration),
}

/// Create one exclusive append-only operational log under `root`.
pub fn create_oplog(
    root: JournalRoot,
    source_original: &str,
    run_original: &str,
    format: OplogFormat,
) -> Result<OplogWriter, OplogCreateError> {
    if !original_is_admissible(source_original) || !original_is_admissible(run_original) {
        return Err(OplogCreateError::new(OplogCreateClass::InvalidField));
    }
    let instant = sample_local_instant()?;
    create_with_timing(
        root,
        source_original,
        run_original,
        format,
        instant,
        LockTiming::Default,
    )
}

/// Create with caller-supplied namespace-lock timing.
#[cfg(any(test, feature = "test-hooks"))]
pub fn create_oplog_with_test_timing(
    root: JournalRoot,
    source_original: &str,
    run_original: &str,
    format: OplogFormat,
    instant: DateTime<FixedOffset>,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<OplogWriter, OplogCreateError> {
    create_with_timing(
        root,
        source_original,
        run_original,
        format,
        instant,
        LockTiming::Explicit(timeout, poll_interval),
    )
}

fn create_with_timing(
    root: JournalRoot,
    source_original: &str,
    run_original: &str,
    format: OplogFormat,
    instant: DateTime<FixedOffset>,
    timing: LockTiming,
) -> Result<OplogWriter, OplogCreateError> {
    if !original_is_admissible(source_original) || !original_is_admissible(run_original) {
        return Err(OplogCreateError::new(OplogCreateClass::InvalidField));
    }
    let (day, opened) = derive_day_key_and_opened_field(instant);
    let ids = draw_distinct_file_ids()?;
    let names = ids.map(|file_id_bytes| {
        oplog_name_from_parts(
            source_original,
            run_original,
            opened.clone(),
            file_id_hex(&file_id_bytes),
            format,
        )
    });
    let header = encode_oplog_admission(&names);
    let health = admit_day_health_directory(root, &day).map_err(map_namespace_error)?;
    let _lock = acquire_lock(&health, timing)?;
    for name in &names {
        let dest = format_oplog_name(name);
        let dest_os = OsStr::new(&dest);
        checkpoint(OplogCreatePrimitive::Stage)?;
        let mut staged = platform::stage_exclusive(&health, dest_os)?;
        if let Err(error) = checkpoint(OplogCreatePrimitive::Admission) {
            rollback(&health, &staged)?;
            return Err(error);
        }
        if write_admission(&mut staged.file, &header).is_err() {
            rollback(&health, &staged)?;
            return Err(OplogCreateError::io());
        }
        barrier(OplogCreatePrimitive::AfterStageBeforeLease);
        if let Err(error) = checkpoint(OplogCreatePrimitive::Lease) {
            rollback(&health, &staged)?;
            return Err(error);
        }
        record_event(OplogCreateEvent::Lease);
        let lease = match platform::lease_staged(&staged.file) {
            Ok(Some(lease)) => lease,
            Ok(None) => {
                rollback(&health, &staged)?;
                return Err(OplogCreateError::new(OplogCreateClass::LeaseFailed));
            }
            Err(error) => {
                rollback(&health, &staged)?;
                return Err(error);
            }
        };
        barrier(OplogCreatePrimitive::AfterLeaseBeforePublish);
        if let Err(error) = checkpoint(OplogCreatePrimitive::Publish) {
            rollback(&health, &staged)?;
            return Err(error);
        }
        if health.revalidate_binding().is_err() {
            rollback(&health, &staged)?;
            return Err(OplogCreateError::new(OplogCreateClass::AncestorReplaced));
        }
        record_event(OplogCreateEvent::Publish);
        let published = platform::publish_handle_bound(&health, staged, dest_os);
        match published {
            Ok(file) => {
                if sync_published(&health).is_err() {
                    return Err(OplogCreateError::io());
                }
                if health.revalidate_binding().is_err() {
                    return Err(OplogCreateError::new(OplogCreateClass::AncestorReplaced));
                }
                return Ok(OplogWriter::new(file, lease, dest));
            }
            Err(platform::PublishOutcome::Occupied(staged)) => {
                let foreign = platform::dest_is_foreign(&health, dest_os, staged.identity)?;
                rollback(&health, &staged)?;
                if !foreign {
                    return Err(OplogCreateError::io());
                }
            }
            Err(platform::PublishOutcome::WrongIdentityPublished { file }) => {
                drop(file);
                drop(lease);
                return Err(OplogCreateError::new(OplogCreateClass::ForeignResidue));
            }
            Err(platform::PublishOutcome::Aliased { file }) => {
                drop(file);
                drop(lease);
                return Err(OplogCreateError::new(OplogCreateClass::Aliased));
            }
            Err(platform::PublishOutcome::Io(staged)) => {
                rollback(&health, &staged)?;
                return Err(OplogCreateError::io());
            }
            Err(platform::PublishOutcome::IoAfterPublish { file }) => {
                drop(file);
                return Err(OplogCreateError::own_residue());
            }
        }
    }
    Err(OplogCreateError::new(OplogCreateClass::RetryExhausted))
}

fn write_admission_bytes<W: Write>(writer: &mut W, header: &[u8]) -> Result<(), OplogCreateError> {
    let mut written = 0;
    while written < header.len() {
        match writer.write(&header[written..]) {
            Ok(0) => return Err(OplogCreateError::io()),
            Ok(n) => written += n,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err(OplogCreateError::io()),
        }
    }
    record_event(OplogCreateEvent::AdmissionBytesAccepted);
    Ok(())
}

fn write_admission(file: &mut std::fs::File, header: &[u8]) -> Result<(), OplogCreateError> {
    write_admission_bytes(file, header)?;
    if force_sync_fail() {
        return Err(OplogCreateError::io());
    }
    file.sync_all().map_err(|_| OplogCreateError::io())?;
    record_event(OplogCreateEvent::SyncAll);
    Ok(())
}

fn sync_published(health: &OplogDayHealth) -> Result<(), OplogCreateError> {
    #[cfg(unix)]
    {
        crate::entry::sync_dir_bound(health.health()).map_err(|_| OplogCreateError::io())?;
    }
    #[cfg(windows)]
    {
        let _ = health;
    }
    Ok(())
}

fn rollback(
    health: &OplogDayHealth,
    staged: &platform::StagedFile,
) -> Result<(), OplogCreateError> {
    checkpoint(OplogCreatePrimitive::Rollback)?;
    if force_rollback_fail() {
        return Err(OplogCreateError::own_residue());
    }
    platform::rollback_stage(health, staged)
}

fn acquire_lock(
    health: &OplogDayHealth,
    timing: LockTiming,
) -> Result<super::lock::OplogNamespaceLock, OplogCreateError> {
    let result = match timing {
        LockTiming::Default => acquire_oplog_namespace_lock(health),
        #[cfg(any(test, feature = "test-hooks"))]
        LockTiming::Explicit(timeout, poll_interval) => {
            acquire_oplog_namespace_lock_with_test_timing(health, timeout, poll_interval)
        }
    };
    result.map_err(map_lock_error)
}

fn map_lock_error(error: OplogNamespaceLockError) -> OplogCreateError {
    let token = error.to_string();
    let suffix = token
        .strip_prefix("oplog_namespace_lock_")
        .expect("OplogNamespaceLockError Display is oplog_namespace_lock_{class}");
    let class = match suffix {
        "unsafe" => OplogCreateClass::LockUnsafe,
        "identity_changed" => OplogCreateClass::LockIdentityChanged,
        "busy" => OplogCreateClass::LockBusy,
        "io" => OplogCreateClass::LockIo,
        _ => panic!("unknown OplogNamespaceLockError class {suffix}"),
    };
    OplogCreateError::new(class)
}

fn map_namespace_error(error: OplogNamespaceError) -> OplogCreateError {
    let token = error.to_string();
    let suffix = token
        .strip_prefix("oplog_namespace_")
        .expect("OplogNamespaceError Display is oplog_namespace_{stage}_{class}");
    let class = match suffix {
        "chronicle_unsafe" => OplogCreateClass::NamespaceChronicleUnsafe,
        "chronicle_identity_changed" => OplogCreateClass::NamespaceChronicleIdentityChanged,
        "chronicle_io" => OplogCreateClass::NamespaceChronicleIo,
        "day_unsafe" => OplogCreateClass::NamespaceDayUnsafe,
        "day_identity_changed" => OplogCreateClass::NamespaceDayIdentityChanged,
        "day_io" => OplogCreateClass::NamespaceDayIo,
        "health_unsafe" => OplogCreateClass::NamespaceHealthUnsafe,
        "health_identity_changed" => OplogCreateClass::NamespaceHealthIdentityChanged,
        "health_io" => OplogCreateClass::NamespaceHealthIo,
        _ => panic!("unknown OplogNamespaceError class {suffix}"),
    };
    OplogCreateError::new(class)
}

fn draw_distinct_file_ids() -> Result<[[u8; 16]; OPLOG_CREATE_ATTEMPTS], OplogCreateError> {
    let mut ids = Vec::with_capacity(OPLOG_CREATE_ATTEMPTS);
    let mut seen = HashSet::with_capacity(OPLOG_CREATE_ATTEMPTS);
    for _ in 0..OPLOG_FILE_ID_DRAW_BUDGET {
        let id = draw_file_id()?;
        if seen.insert(id) {
            ids.push(id);
            if ids.len() == OPLOG_CREATE_ATTEMPTS {
                return Ok(ids
                    .try_into()
                    .expect("exactly OPLOG_CREATE_ATTEMPTS distinct ids"));
            }
        }
    }
    Err(OplogCreateError::new(OplogCreateClass::EntropyExhausted))
}

fn draw_file_id() -> Result<[u8; 16], OplogCreateError> {
    fill_oplog_file_id()
}

fn fill_oplog_file_id() -> Result<[u8; 16], OplogCreateError> {
    record_event(OplogCreateEvent::EntropyDraw);
    if take_entropy_source_fault() {
        return Err(OplogCreateError::entropy_source());
    }
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(bytes) = take_injected_file_id() {
        return Ok(bytes);
    }
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| OplogCreateError::entropy_source())?;
    Ok(bytes)
}

/// Bound no-follow lease probe of one leaf under the admitted day-health directory.
pub fn probe_oplog_lease(health: &OplogDayHealth, leaf: &OsStr) -> LeaseProbe {
    if force_probe_indeterminate() {
        return LeaseProbe::Indeterminate;
    }
    platform::probe_named(health, leaf)
}

/// Ordered checkpoints for one create call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogCreatePrimitive {
    /// Exclusive stage allocation.
    Stage,
    /// Admission-record write and file sync.
    Admission,
    /// After staging, before taking the self-lease.
    AfterStageBeforeLease,
    /// Self-lease acquisition.
    Lease,
    /// After lease, before publish.
    AfterLeaseBeforePublish,
    /// No-replace publish.
    Publish,
    /// Stage rollback.
    Rollback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogCreateEvent {
    EntropyDraw,
    AdmissionBytesAccepted,
    SyncAll,
    Lease,
    Publish,
}

#[cfg(any(test, feature = "test-hooks"))]
struct OplogCreateTraceState {
    fault: Option<(OplogCreatePrimitive, usize)>,
    fault_consumed: bool,
    attempted: Vec<OplogCreatePrimitive>,
    barriers: Vec<(OplogCreatePrimitive, Box<dyn FnOnce()>)>,
    file_ids: std::collections::VecDeque<[u8; 16]>,
    rollback_fail: bool,
    probe_indeterminate: bool,
    dest_identity_io: bool,
    publish_io: bool,
    sync_fail: bool,
    entropy_fault: Option<usize>,
    entropy_fault_consumed: bool,
    entropy_draws: usize,
    events: Vec<OplogCreateEvent>,
    sampled_instant: Option<DateTime<FixedOffset>>,
    sampler_fail: bool,
    sampler_calls: usize,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static OPLOG_CREATE_TRACE: std::cell::RefCell<Option<OplogCreateTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn checkpoint(_primitive: OplogCreatePrimitive) -> Result<(), OplogCreateError> {
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint(primitive: OplogCreatePrimitive) -> Result<(), OplogCreateError> {
    let fault = OPLOG_CREATE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return false;
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        if state.fault == Some((primitive, ordinal)) {
            state.fault = None;
            state.fault_consumed = true;
            true
        } else {
            false
        }
    });
    if fault {
        return Err(match primitive {
            OplogCreatePrimitive::Lease => OplogCreateError::new(OplogCreateClass::LeaseFailed),
            OplogCreatePrimitive::Rollback => OplogCreateError::own_residue(),
            _ => OplogCreateError::io(),
        });
    }
    Ok(())
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn barrier(_primitive: OplogCreatePrimitive) {}

#[cfg(any(test, feature = "test-hooks"))]
fn barrier(primitive: OplogCreatePrimitive) {
    let callback = OPLOG_CREATE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state
            .barriers
            .iter()
            .position(|(candidate, _)| *candidate == primitive)
            .map(|index| state.barriers.remove(index).1)
    });
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn force_rollback_fail() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn force_rollback_fail() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.rollback_fail)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn force_probe_indeterminate() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn force_probe_indeterminate() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.probe_indeterminate)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
pub(super) fn force_dest_identity_io() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn force_dest_identity_io() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.dest_identity_io)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
pub(super) fn force_publish_io() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn force_publish_io() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.publish_io)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn force_sync_fail() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn force_sync_fail() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| trace.borrow().as_ref().is_some_and(|state| state.sync_fail))
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn take_entropy_source_fault() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn take_entropy_source_fault() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return false;
        };
        state.entropy_draws += 1;
        if state.entropy_fault == Some(state.entropy_draws) {
            state.entropy_fault = None;
            state.entropy_fault_consumed = true;
            true
        } else {
            false
        }
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn record_event(_event: OplogCreateEvent) {}

#[cfg(any(test, feature = "test-hooks"))]
fn record_event(event: OplogCreateEvent) {
    OPLOG_CREATE_TRACE.with(|trace| {
        if let Some(state) = trace.borrow_mut().as_mut() {
            state.events.push(event);
        }
    });
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn take_sampler_override() -> Option<Result<DateTime<FixedOffset>, OplogCreateError>> {
    OPLOG_CREATE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state.sampler_calls += 1;
        if state.sampler_fail {
            Some(Err(OplogCreateError::sampler()))
        } else {
            state.sampled_instant.map(Ok)
        }
    })
}

#[cfg(any(test, feature = "test-hooks"))]
fn take_injected_file_id() -> Option<[u8; 16]> {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .as_mut()
            .and_then(|state| state.file_ids.pop_front())
    })
}

#[cfg(any(test, feature = "test-hooks"))]
fn empty_trace() -> OplogCreateTraceState {
    OplogCreateTraceState {
        fault: None,
        fault_consumed: false,
        attempted: Vec::new(),
        barriers: Vec::new(),
        file_ids: std::collections::VecDeque::new(),
        rollback_fail: false,
        probe_indeterminate: false,
        dest_identity_io: false,
        publish_io: false,
        sync_fail: false,
        entropy_fault: None,
        entropy_fault_consumed: false,
        entropy_draws: 0,
        events: Vec::new(),
        sampled_instant: None,
        sampler_fail: false,
        sampler_calls: 0,
    }
}

/// Run `operation` with one injected create fault at the first occurrence.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_create_fault<T>(
    primitive: OplogCreatePrimitive,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    run_with_oplog_create_fault_at(primitive, 1, operation)
}

/// Run `operation` with one injected create fault at a 1-based primitive ordinal.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_create_fault_at<T>(
    primitive: OplogCreatePrimitive,
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, state) = with_trace(
        OplogCreateTraceState {
            fault: Some((primitive, ordinal)),
            ..empty_trace()
        },
        operation,
    );
    (result, state.fault_consumed)
}

/// Run `operation` with one barrier callback.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_create_barrier<T>(
    primitive: OplogCreatePrimitive,
    callback: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    with_trace(
        OplogCreateTraceState {
            barriers: vec![(primitive, Box::new(callback))],
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Inject file ids consumed in order before falling back to `getrandom`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_file_ids<T>(ids: Vec<[u8; 16]>, operation: impl FnOnce() -> T) -> T {
    with_trace(
        OplogCreateTraceState {
            file_ids: ids.into(),
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Force `probe_oplog_lease` to return `Indeterminate`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_probe_indeterminate<T>(operation: impl FnOnce() -> T) -> T {
    with_trace(
        OplogCreateTraceState {
            probe_indeterminate: true,
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Run `operation` with one entropy-adapter fault at the first draw.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_entropy_source_fault<T>(operation: impl FnOnce() -> T) -> (T, bool) {
    run_with_oplog_entropy_source_fault_at(1, operation)
}

/// Run `operation` with one entropy-adapter fault at a 1-based draw ordinal.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_entropy_source_fault_at<T>(
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, state) = with_trace(
        OplogCreateTraceState {
            entropy_fault: Some(ordinal),
            ..empty_trace()
        },
        operation,
    );
    (result, state.entropy_fault_consumed)
}

/// Freeze the production sampler to `instant`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_sampled_instant<T>(
    instant: DateTime<FixedOffset>,
    operation: impl FnOnce() -> T,
) -> T {
    with_trace(
        OplogCreateTraceState {
            sampled_instant: Some(instant),
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Fail the production sampler before any entropy draw.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_sampler_fault<T>(operation: impl FnOnce() -> T) -> (T, bool) {
    let (result, state) = with_trace(
        OplogCreateTraceState {
            sampler_fail: true,
            ..empty_trace()
        },
        operation,
    );
    (result, state.sampler_calls > 0)
}

/// Fail `sync_all` after the admission bytes are written.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_sync_fail<T>(operation: impl FnOnce() -> T) -> T {
    with_trace(
        OplogCreateTraceState {
            sync_fail: true,
            ..empty_trace()
        },
        operation,
    )
    .0
}

#[cfg(any(test, feature = "test-hooks"))]
fn with_trace<T>(
    state: OplogCreateTraceState,
    operation: impl FnOnce() -> T,
) -> (T, OplogCreateTraceState) {
    OPLOG_CREATE_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "oplog create trace is already active"
        );
        *trace.borrow_mut() = Some(state);
    });
    let result = operation();
    let state = OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("oplog create trace remains active")
    });
    (result, state)
}

#[cfg(all(test, unix))]
fn spawn_sleep_holding_oplog_stdout(stdio: std::process::Stdio) -> std::process::Child {
    use std::process::{Command, Stdio};

    Command::new("sleep")
        .arg("0.3")
        .stdout(stdio)
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[cfg(all(test, unix))]
mod tests {
    use std::error::Error;
    use std::ffi::OsStr;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    use chrono::DateTime;
    use nix::sys::stat::{Mode, umask};

    use super::*;
    use crate::journal_root::JournalRoot;
    use crate::lease::{DEFAULT_LEASE_RETRY_MAX, probe_file_lease};
    use crate::operational_log::name::{OplogNameClassification, classify_oplog_name};
    use crate::operational_log::{
        OplogFormat, OplogNamespacePrimitive, acquire_oplog_namespace_lock_with_test_timing,
        admit_day_health_directory, run_with_oplog_namespace_barrier,
        run_with_oplog_namespace_fault, validate_oplog_admission,
    };

    const ZERO: Duration = Duration::ZERO;
    const SOURCE: &str = "cortex";
    const RUN: &str = "daily-think";

    fn instant() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-09-01T16:42:33.381904Z").unwrap()
    }

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir_in("/var/tmp").unwrap()
    }

    fn health_at(root: &Path) -> crate::operational_log::OplogDayHealth {
        let (day, _) = derive_day_key_and_opened_field(instant());
        admit_day_health_directory(JournalRoot::open(root).unwrap(), &day).unwrap()
    }

    fn create(root: &Path) -> Result<OplogWriter, OplogCreateError> {
        create_oplog_with_test_timing(
            JournalRoot::open(root).unwrap(),
            SOURCE,
            RUN,
            OplogFormat::Log,
            instant(),
            ZERO,
            ZERO,
        )
    }

    fn dest_for(file_id: [u8; 16]) -> String {
        let (_, opened) = derive_day_key_and_opened_field(instant());
        format_oplog_name(&oplog_name_from_parts(
            SOURCE,
            RUN,
            opened,
            file_id_hex(&file_id),
            OplogFormat::Log,
        ))
    }

    fn health_dir(root: &Path) -> std::path::PathBuf {
        let (day, _) = derive_day_key_and_opened_field(instant());
        root.join("chronicle").join(day).join("health")
    }

    fn expect_token(error: OplogCreateError, token: &str) {
        assert_eq!(error.to_string(), token);
        assert_eq!(format!("{error:?}"), token);
        assert!(error.source().is_none());
    }

    fn count_event(state: &OplogCreateTraceState, event: OplogCreateEvent) -> usize {
        state
            .events
            .iter()
            .filter(|candidate| **candidate == event)
            .count()
    }

    // A concurrent test's forked child can briefly inherit this writer's OFD across
    // fork-to-exec (CLOEXEC applies at exec, not fork); the lock self-releases.
    fn assert_lease_released(health: &OplogDayHealth, leaf: &OsStr) {
        let deadline = std::time::Instant::now() + DEFAULT_LEASE_RETRY_MAX;
        loop {
            if probe_oplog_lease(health, leaf) == LeaseProbe::Released {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "lease did not converge to Released within {DEFAULT_LEASE_RETRY_MAX:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn listing(path: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn canonical_leaves(path: &Path) -> Vec<String> {
        listing(path)
            .into_iter()
            .filter(|name| {
                matches!(
                    classify_oplog_name(OsStr::new(name)),
                    OplogNameClassification::Candidate(Ok(_))
                )
            })
            .collect()
    }

    fn leftover_unrelated(path: &Path) -> Vec<String> {
        listing(path)
            .into_iter()
            .filter(|name| *name != ".oplog-namespace.lock")
            .filter(|name| {
                matches!(
                    classify_oplog_name(OsStr::new(name)),
                    OplogNameClassification::Unrelated
                )
            })
            .collect()
    }

    fn payload_after_admission(path: &Path, leaf: &str) -> Vec<u8> {
        let bytes = fs::read(path).unwrap();
        let record = validate_oplog_admission(OsStr::new(leaf), &bytes).unwrap();
        bytes[record.header_len()..].to_vec()
    }

    #[test]
    fn two_creates_at_the_same_instant_get_distinct_file_ids() {
        let temporary = temp();
        let first = create(temporary.path()).unwrap();
        let second = create(temporary.path()).unwrap();
        assert_ne!(first.leaf_name(), second.leaf_name());
        let dir = health_dir(temporary.path());
        assert_eq!(canonical_leaves(&dir).len(), 2);
    }

    #[test]
    fn injected_file_id_collision_retries_without_touching_incumbent() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let first_id = [0x11; 16];
        let second_id = [0x22; 16];
        let incumbent = dest_for(first_id);
        let path = health_dir(temporary.path()).join(&incumbent);
        fs::write(&path, b"incumbent").unwrap();
        let writer =
            run_with_oplog_file_ids(vec![first_id, second_id], || create(temporary.path()))
                .unwrap();
        assert_eq!(writer.leaf_name(), dest_for(second_id));
        assert_eq!(fs::read(&path).unwrap(), b"incumbent");
    }

    #[test]
    fn exhausted_collisions_leave_incumbents_byte_identical() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let dir = health_dir(temporary.path());
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [index as u8; 16])
            .collect();
        let incumbents: Vec<String> = ids.iter().copied().map(dest_for).collect();
        for incumbent in &incumbents {
            fs::write(dir.join(incumbent), b"same-bytes").unwrap();
        }
        let error = run_with_oplog_file_ids(ids, || create(temporary.path())).unwrap_err();
        expect_token(error, "oplog_create_retry_exhausted");
        for incumbent in &incumbents {
            assert_eq!(fs::read(dir.join(incumbent)).unwrap(), b"same-bytes");
        }
        assert_eq!(canonical_leaves(&dir), incumbents);
        assert!(!leftover_unrelated(&dir).is_empty());
    }

    #[test]
    fn random_source_failure_does_not_retry() {
        let temporary = temp();
        let (result, consumed) = run_with_oplog_entropy_source_fault(|| create(temporary.path()));
        assert!(consumed);
        expect_token(result.unwrap_err(), "oplog_create_entropy_source");
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn sixty_four_duplicate_ids_are_entropy_exhausted_with_zero_side_effects() {
        let temporary = temp();
        let id = [0x11; 16];
        let error = run_with_oplog_file_ids(vec![id; OPLOG_FILE_ID_DRAW_BUDGET], || {
            create(temporary.path())
        })
        .unwrap_err();
        expect_token(error, "oplog_create_entropy_exhausted");
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn non_collision_errors_return_immediately() {
        let temporary = temp();
        for primitive in [OplogCreatePrimitive::Stage, OplogCreatePrimitive::Publish] {
            let (result, consumed) =
                run_with_oplog_create_fault(primitive, || create(temporary.path()));
            assert!(consumed);
            expect_token(result.unwrap_err(), "oplog_create_io");
            assert!(canonical_leaves(&health_dir(temporary.path())).is_empty());
        }
    }

    #[test]
    fn lease_failure_rolls_back_only_the_stage() {
        let temporary = temp();
        let (result, consumed) =
            run_with_oplog_create_fault(OplogCreatePrimitive::Lease, || create(temporary.path()));
        assert!(consumed);
        expect_token(result.unwrap_err(), "oplog_create_lease_failed");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert!(listing(&dir).contains(&".oplog-namespace.lock".to_owned()));
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn injected_rollback_failure_is_own_residue_and_unrelated_native_name() {
        let temporary = temp();
        let error = with_trace(
            OplogCreateTraceState {
                fault: Some((OplogCreatePrimitive::Lease, 1)),
                rollback_fail: true,
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(error, "oplog_create_own_residue");
        let names = listing(&health_dir(temporary.path()));
        let residue = names
            .iter()
            .find(|name| *name != ".oplog-namespace.lock")
            .expect("stage residue remains");
        assert!(matches!(
            classify_oplog_name(OsStr::new(residue)),
            OplogNameClassification::Unrelated
        ));
        assert!(canonical_leaves(&health_dir(temporary.path())).is_empty());
    }

    #[test]
    fn barriers_see_no_canonical_candidate_and_lock_blocks_a_second_publisher() {
        for primitive in [
            OplogCreatePrimitive::AfterStageBeforeLease,
            OplogCreatePrimitive::AfterLeaseBeforePublish,
        ] {
            let isolated = temp();
            let root = isolated.path().to_path_buf();
            run_with_oplog_create_barrier(
                primitive,
                {
                    let root = root.clone();
                    move || {
                        let dir = health_dir(&root);
                        assert!(canonical_leaves(&dir).is_empty());
                        let second = health_at(&root);
                        let error =
                            acquire_oplog_namespace_lock_with_test_timing(&second, ZERO, ZERO)
                                .unwrap_err();
                        assert_eq!(error.to_string(), "oplog_namespace_lock_busy");
                    }
                },
                {
                    let root = root.clone();
                    move || create(&root)
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn probe_is_active_after_publish_and_released_after_drop() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let writer = create(temporary.path()).unwrap();
        let leaf = writer.leaf_name().to_owned();
        assert_eq!(
            probe_oplog_lease(&health, OsStr::new(&leaf)),
            LeaseProbe::Active
        );
        drop(writer);
        assert_lease_released(&health, OsStr::new(&leaf));
    }

    #[test]
    fn published_file_starts_with_admission_header_then_payload() {
        let temporary = temp();
        let mut writer = create(temporary.path()).unwrap();
        writer.write_all(b"payload-line\n").unwrap();
        writer.flush().unwrap();
        let leaf = writer.leaf_name().to_owned();
        drop(writer);
        let bytes = fs::read(health_dir(temporary.path()).join(&leaf)).unwrap();
        let record = validate_oplog_admission(OsStr::new(&leaf), &bytes).unwrap();
        assert_eq!(&bytes[record.header_len()..], b"payload-line\n");
    }

    #[test]
    fn admission_fault_leaves_unrelated_stage() {
        let temporary = temp();
        let (result, consumed) =
            run_with_oplog_create_fault(OplogCreatePrimitive::Admission, || {
                create(temporary.path())
            });
        assert!(consumed);
        expect_token(result.unwrap_err(), "oplog_create_io");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn extra_hard_link_before_publish_is_aliased() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterLeaseBeforePublish,
            {
                let dir = health_dir(&root);
                move || {
                    let stage = fs::read_dir(&dir)
                        .unwrap()
                        .map(|entry| entry.unwrap().file_name())
                        .find(|name| name.to_string_lossy().contains(".tmp"))
                        .expect("stage name");
                    fs::hard_link(dir.join(&stage), dir.join("alias-link")).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(error, "oplog_create_aliased");
        let dir = health_dir(&root);
        assert_eq!(canonical_leaves(&dir).len(), 1);
        assert!(dir.join("alias-link").exists());
    }

    #[test]
    fn stage_pathname_replacement_is_foreign_residue() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let id = [0x44; 16];
        let dest = dest_for(id);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: vec![id].into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterLeaseBeforePublish,
                    Box::new({
                        let dir = health_dir(&root);
                        move || {
                            let stage = fs::read_dir(&dir)
                                .unwrap()
                                .map(|entry| entry.unwrap().file_name())
                                .find(|name| name.to_string_lossy().contains(".tmp"))
                                .expect("stage name");
                            let from = dir.join(&stage);
                            fs::rename(&from, dir.join("displaced-stage")).unwrap();
                            fs::write(&from, b"replacement").unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(error, "oplog_create_foreign_residue");
        let dir = health_dir(&root);
        assert_eq!(fs::read(dir.join(&dest)).unwrap(), b"replacement");
        let displaced = fs::read(dir.join("displaced-stage")).unwrap();
        assert!(displaced.starts_with(b"{\"_solstone_oplog_v\":1"));
    }

    #[test]
    fn dest_identity_io_after_publish_is_own_residue_and_preserves_dest() {
        let temporary = temp();
        let id = [0x88; 16];
        let dest = dest_for(id);
        let error = with_trace(
            OplogCreateTraceState {
                dest_identity_io: true,
                file_ids: vec![id].into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(error, "oplog_create_own_residue");
        let path = health_dir(temporary.path()).join(&dest);
        let bytes = fs::read(&path).unwrap();
        let record = validate_oplog_admission(OsStr::new(&dest), &bytes).unwrap();
        assert_eq!(&bytes[record.header_len()..], b"");
    }

    #[test]
    fn publish_io_leaves_unrelated_stage_residue() {
        let temporary = temp();
        let error = with_trace(
            OplogCreateTraceState {
                publish_io: true,
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(error, "oplog_create_io");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn occupied_retries_leave_unrelated_stage_residue() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let dir = health_dir(temporary.path());
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x66 + index as u8; 16])
            .collect();
        let incumbents: Vec<String> = ids.iter().copied().map(dest_for).collect();
        for incumbent in &incumbents {
            fs::write(dir.join(incumbent), b"preexisting").unwrap();
        }
        let error = run_with_oplog_file_ids(ids, || create(temporary.path())).unwrap_err();
        expect_token(error, "oplog_create_retry_exhausted");
        for incumbent in &incumbents {
            assert_eq!(fs::read(dir.join(incumbent)).unwrap(), b"preexisting");
        }
        assert_eq!(canonical_leaves(&dir), incumbents);
        assert_eq!(leftover_unrelated(&dir).len(), OPLOG_CREATE_ATTEMPTS);
    }

    #[test]
    fn namespace_lock_excludes_and_releases() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let first = acquire_oplog_namespace_lock_with_test_timing(&health, ZERO, ZERO).unwrap();
        assert_eq!(
            acquire_oplog_namespace_lock_with_test_timing(&health, ZERO, ZERO)
                .unwrap_err()
                .to_string(),
            "oplog_namespace_lock_busy"
        );
        drop(first);
        drop(acquire_oplog_namespace_lock_with_test_timing(&health, ZERO, ZERO).unwrap());
    }

    #[test]
    fn unsafe_lock_fails_before_stage() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let lock_path = health_dir(temporary.path()).join(".oplog-namespace.lock");
        std::os::unix::fs::symlink("outside", &lock_path).unwrap();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x55 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.clone().into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        expect_token(result.unwrap_err(), "oplog_create_lock_unsafe");
        assert_eq!(state.file_ids.len(), 0);
        for id in ids {
            assert!(!health_dir(temporary.path()).join(dest_for(id)).exists());
        }
    }

    #[test]
    fn wrong_mode_lock_fails_before_stage() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let lock_path = health_dir(temporary.path()).join(".oplog-namespace.lock");
        fs::write(&lock_path, b"unchanged").unwrap();
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x56 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.clone().into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        expect_token(result.unwrap_err(), "oplog_create_lock_unsafe");
        assert_eq!(state.file_ids.len(), 0);
        for id in ids {
            assert!(!health_dir(temporary.path()).join(dest_for(id)).exists());
        }
        assert_eq!(fs::read(&lock_path).unwrap(), b"unchanged");
    }

    #[test]
    fn replaced_lock_parent_fails_before_stage() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let dir = health_dir(temporary.path());
        let lock_path = dir.join(".oplog-namespace.lock");
        fs::write(&lock_path, b"original-lock").unwrap();
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(&lock_path).unwrap();
        fs::create_dir(&lock_path).unwrap();
        // Bound acquire never re-opens the day-health parent by path, so a
        // pathname swap of that directory cannot produce ParentChanged. The
        // replacement the primitive *can* refuse before stage is a lock
        // entry whose identity/kind no longer matches a 0o600 regular file
        // — here the lock name is replaced with a directory, the same
        // shape as cortex_use::lock's "directory" unsafe-entry fixture.
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x77 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.clone().into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        expect_token(result.unwrap_err(), "oplog_create_lock_unsafe");
        assert_eq!(state.file_ids.len(), 0);
        for id in ids {
            assert!(!dir.join(dest_for(id)).exists());
        }
        assert!(lock_path.is_dir());
    }

    #[test]
    fn mid_flight_ancestor_replacement_does_not_escape() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let outside = root.join("escape-target");
        fs::create_dir(&outside).unwrap();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterLeaseBeforePublish,
            {
                let root = root.clone();
                let outside = outside.clone();
                move || {
                    let dir = health_dir(&root);
                    fs::rename(&dir, root.join("health-displaced")).unwrap();
                    std::os::unix::fs::symlink(&outside, &dir).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(error, "oplog_create_ancestor_replaced");
        let displaced = root.join("health-displaced");
        assert!(canonical_leaves(&displaced).is_empty());
        assert!(canonical_leaves(&outside).is_empty());
        assert_eq!(leftover_unrelated(&displaced).len(), 1);
        assert!(
            fs::symlink_metadata(health_dir(&root))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn production_create_rejects_invalid_originals_without_side_effects() {
        let temporary = temp();
        expect_token(
            create_oplog(
                JournalRoot::open(temporary.path()).unwrap(),
                "",
                RUN,
                OplogFormat::Log,
            )
            .unwrap_err(),
            "oplog_create_invalid_field",
        );
        expect_token(
            create_oplog(
                JournalRoot::open(temporary.path()).unwrap(),
                "ok",
                "bad\0name",
                OplogFormat::Log,
            )
            .unwrap_err(),
            "oplog_create_invalid_field",
        );
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn invalid_originals_and_wrong_kind_namespace_do_not_create() {
        let temporary = temp();
        expect_token(
            create_oplog_with_test_timing(
                JournalRoot::open(temporary.path()).unwrap(),
                "",
                RUN,
                OplogFormat::Log,
                instant(),
                ZERO,
                ZERO,
            )
            .unwrap_err(),
            "oplog_create_invalid_field",
        );
        expect_token(
            create_oplog_with_test_timing(
                JournalRoot::open(temporary.path()).unwrap(),
                "ok",
                "bad\0name",
                OplogFormat::Log,
                instant(),
                ZERO,
                ZERO,
            )
            .unwrap_err(),
            "oplog_create_invalid_field",
        );
        assert!(!temporary.path().join("chronicle").exists());
        let root = temporary.path().join("other");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink("elsewhere", root.join("chronicle")).unwrap();
        let error =
            admit_day_health_directory(JournalRoot::open(&root).unwrap(), "20260901").unwrap_err();
        assert_eq!(error.to_string(), "oplog_namespace_chronicle_unsafe");
        assert!(!root.join("elsewhere").exists());
    }

    #[test]
    fn reader_survives_pathname_replacement_and_drop_releases() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let mut writer = create(temporary.path()).unwrap();
        writer.write_all(b"hello\n").unwrap();
        writer.flush().unwrap();
        let dir = health_dir(temporary.path());
        let leaf = writer.leaf_name().to_owned();
        let path = dir.join(&leaf);
        let retained = fs::File::open(&path).unwrap();
        fs::rename(&path, dir.join("moved")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        writer.write_all(b"late\n").unwrap();
        writer.flush().unwrap();
        assert_eq!(probe_file_lease(&retained), LeaseProbe::Active);
        assert_eq!(
            payload_after_admission(&dir.join("moved"), &leaf),
            b"hello\nlate\n"
        );
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        drop(writer);
        assert_lease_released(&health, OsStr::new("moved"));
    }

    #[test]
    fn owner_only_mode_under_permissive_umask() {
        let temporary = temp();
        let previous = umask(Mode::from_bits_truncate(0o022));
        let writer = create(temporary.path()).unwrap();
        umask(previous);
        let mode = fs::metadata(health_dir(temporary.path()).join(writer.leaf_name()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn stdio_duplicates_append_exactly_once() {
        let temporary = temp();
        let mut writer = create(temporary.path()).unwrap();
        let mut dup = writer.try_clone_for_stdio().unwrap();
        writer.write_all(b"one\n").unwrap();
        dup.write_all(b"two\n").unwrap();
        writer.write_all(b"three\n").unwrap();
        writer.flush().unwrap();
        dup.flush().unwrap();
        let bytes = payload_after_admission(
            &health_dir(temporary.path()).join(writer.leaf_name()),
            writer.leaf_name(),
        );
        assert_eq!(bytes, b"one\ntwo\nthree\n");
    }

    #[test]
    fn dropping_writer_keeps_duplicate_active() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let writer = create(temporary.path()).unwrap();
        let mut dup = writer.try_clone_for_stdio().unwrap();
        let leaf = writer.leaf_name().to_owned();
        drop(writer);
        assert_eq!(
            probe_oplog_lease(&health, OsStr::new(&leaf)),
            LeaseProbe::Active
        );
        dup.write_all(b"still\n").unwrap();
        dup.flush().unwrap();
        drop(dup);
        assert_lease_released(&health, OsStr::new(&leaf));
        assert_eq!(
            payload_after_admission(&health_dir(temporary.path()).join(&leaf), &leaf),
            b"still\n"
        );
    }

    #[test]
    fn child_process_exit_releases_lease() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let writer = create(temporary.path()).unwrap();
        let leaf = writer.leaf_name().to_owned();
        let stdio = writer.duplicate_locked_stdio().unwrap();
        drop(writer);
        let mut child = super::spawn_sleep_holding_oplog_stdout(stdio);
        assert_eq!(
            probe_oplog_lease(&health, OsStr::new(&leaf)),
            LeaseProbe::Active
        );
        let status = child.wait().unwrap();
        assert!(status.success());
        drop(child);
        assert_lease_released(&health, OsStr::new(&leaf));
    }

    #[test]
    fn injected_probe_failure_is_indeterminate_without_companion_files() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let writer = create(temporary.path()).unwrap();
        let leaf = OsStr::new(writer.leaf_name());
        let probe = run_with_oplog_probe_indeterminate(|| probe_oplog_lease(&health, leaf));
        assert_eq!(probe, LeaseProbe::Indeterminate);
        let names = listing(&health_dir(temporary.path()));
        assert!(names.contains(&".oplog-namespace.lock".to_owned()));
        assert!(names.contains(&writer.leaf_name().to_owned()));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn day_and_utc_come_from_the_same_instant() {
        let temporary = temp();
        let offset = DateTime::parse_from_rfc3339("2026-09-01T23:30:00-05:00").unwrap();
        let (day, opened) = derive_day_key_and_opened_field(offset);
        let writer = create_oplog_with_test_timing(
            JournalRoot::open(temporary.path()).unwrap(),
            SOURCE,
            RUN,
            OplogFormat::Log,
            offset,
            ZERO,
            ZERO,
        )
        .unwrap();
        assert!(writer.leaf_name().contains(&opened));
        assert!(
            temporary
                .path()
                .join("chronicle")
                .join(&day)
                .join("health")
                .exists()
        );
    }

    #[test]
    fn entropy_fault_at_every_draw_ordinal_leaves_no_chronicle() {
        for ordinal in 1..=OPLOG_FILE_ID_DRAW_BUDGET {
            let temporary = temp();
            let (result, state) = with_trace(
                OplogCreateTraceState {
                    entropy_fault: Some(ordinal),
                    file_ids: vec![[0x11; 16]; OPLOG_FILE_ID_DRAW_BUDGET].into(),
                    ..empty_trace()
                },
                || create(temporary.path()),
            );
            assert!(state.entropy_fault_consumed, "ordinal {ordinal}");
            expect_token(result.unwrap_err(), "oplog_create_entropy_source");
            assert_eq!(
                state.file_ids.len(),
                OPLOG_FILE_ID_DRAW_BUDGET - (ordinal - 1),
                "ordinal {ordinal} must not consume a queued id after the fault"
            );
            assert!(
                !temporary.path().join("chronicle").exists(),
                "ordinal {ordinal} must not admit the namespace"
            );
        }
    }

    #[test]
    fn preexisting_symlink_chronicle_maps_to_create_namespace_unsafe() {
        let temporary = temp();
        let root = temporary.path();
        std::os::unix::fs::symlink("elsewhere", root.join("chronicle")).unwrap();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [index as u8 + 1; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                sampled_instant: Some(instant()),
                file_ids: ids.into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(root).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                )
            },
        );
        expect_token(
            result.unwrap_err(),
            "oplog_create_namespace_chronicle_unsafe",
        );
        assert_eq!(state.sampler_calls, 1);
        assert_eq!(
            count_event(&state, OplogCreateEvent::EntropyDraw),
            OPLOG_CREATE_ATTEMPTS
        );
        assert_eq!(
            count_event(&state, OplogCreateEvent::AdmissionBytesAccepted),
            0
        );
        assert_eq!(count_event(&state, OplogCreateEvent::Lease), 0);
        assert_eq!(count_event(&state, OplogCreateEvent::Publish), 0);
        assert!(state.attempted.is_empty());
        assert!(!root.join("elsewhere").exists());
        assert!(
            fs::symlink_metadata(root.join("chronicle"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!root.join("chronicle").join("20260901").exists());
    }

    #[test]
    fn after_chronicle_wrong_kind_day_maps_to_create_namespace_day_unsafe() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterChronicle,
            {
                let root = root.clone();
                move || {
                    fs::write(health_dir(&root).parent().unwrap(), b"not-a-directory").unwrap();
                }
            },
            {
                let root = root.clone();
                move || create(&root)
            },
        )
        .unwrap_err();
        expect_token(error, "oplog_create_namespace_day_unsafe");
        assert!(root.join("chronicle").is_dir());
        assert!(root.join("chronicle").join("20260901").is_file());
        assert!(!health_dir(&root).exists());
    }

    #[test]
    fn after_day_symlink_health_maps_to_create_namespace_health_unsafe() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterDay,
            {
                let root = root.clone();
                move || {
                    std::os::unix::fs::symlink("outside", health_dir(&root)).unwrap();
                }
            },
            {
                let root = root.clone();
                move || create(&root)
            },
        )
        .unwrap_err();
        expect_token(error, "oplog_create_namespace_health_unsafe");
        assert!(
            fs::symlink_metadata(health_dir(&root))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!root.join("outside").exists());
    }

    #[test]
    fn namespace_fault_after_chronicle_maps_to_create_io_without_leaves() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let (result, consumed) =
            run_with_oplog_namespace_fault(OplogNamespacePrimitive::AfterChronicle, {
                let root = root.clone();
                move || create(&root)
            });
        assert!(consumed);
        expect_token(result.unwrap_err(), "oplog_create_namespace_chronicle_io");
        assert!(root.join("chronicle").is_dir());
        assert!(!root.join("chronicle").join("20260901").exists());
    }

    #[test]
    fn sampled_near_midnight_instant_controls_day_and_utc_fields() {
        let temporary = temp();
        let offset = DateTime::parse_from_rfc3339("2026-09-01T23:30:00-05:00").unwrap();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x21 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                sampled_instant: Some(offset),
                file_ids: ids.into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(temporary.path()).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                )
            },
        );
        let writer = result.unwrap();
        assert_eq!(state.sampler_calls, 1);
        assert!(writer.leaf_name().contains("20260902T043000.000000Z"));
        assert!(
            temporary
                .path()
                .join("chronicle")
                .join("20260901")
                .join("health")
                .exists()
        );
    }

    #[test]
    fn production_factory_log_and_jsonl_headers_match_hand_authored_literals() {
        const LOG_HEADER: &[u8] = b"{\"_solstone_oplog_v\":1,\"candidates\":[\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--a1b2c3d4e5f60718293a4b5c6d7e8f90--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--b0c1d2e3f405162738495a6b7c8d9e0f--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--c1d2e3f405162738495a6b7c8d9e0f10--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--d2e3f405162738495a6b7c8d9e0f1011--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--e3f405162738495a6b7c8d9e0f101112--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--f405162738495a6b7c8d9e0f10111213--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.log\"]}\n";
        const JSONL_HEADER: &[u8] = b"{\"_solstone_oplog_v\":1,\"candidates\":[\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--a1b2c3d4e5f60718293a4b5c6d7e8f90--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--b0c1d2e3f405162738495a6b7c8d9e0f--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--c1d2e3f405162738495a6b7c8d9e0f10--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--d2e3f405162738495a6b7c8d9e0f1011--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--e3f405162738495a6b7c8d9e0f101112--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--f405162738495a6b7c8d9e0f10111213--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\"]}\n";
        let ids: Vec<[u8; 16]> = vec![
            [
                0x8f, 0x03, 0xca, 0xbe, 0xad, 0x7e, 0x44, 0x1d, 0x83, 0xf6, 0xc9, 0x2b, 0x2d, 0x89,
                0xa0, 0x21,
            ],
            [
                0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e,
                0x8f, 0x90,
            ],
            [
                0xb0, 0xc1, 0xd2, 0xe3, 0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d,
                0x9e, 0x0f,
            ],
            [
                0xc1, 0xd2, 0xe3, 0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e,
                0x0f, 0x10,
            ],
            [
                0xd2, 0xe3, 0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f,
                0x10, 0x11,
            ],
            [
                0xe3, 0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f, 0x10,
                0x11, 0x12,
            ],
            [
                0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f, 0x10, 0x11,
                0x12, 0x13,
            ],
            [
                0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f, 0x10, 0x11, 0x12,
                0x13, 0x14,
            ],
        ];

        let log_root = temp();
        let (log_result, _) = with_trace(
            OplogCreateTraceState {
                sampled_instant: Some(instant()),
                file_ids: ids.clone().into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(log_root.path()).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                )
            },
        );
        let log_writer = log_result.unwrap();
        let log_path = health_dir(log_root.path()).join(log_writer.leaf_name());
        drop(log_writer);
        assert_eq!(fs::read(&log_path).unwrap(), LOG_HEADER);

        let jsonl_root = temp();
        let (jsonl_result, _) = with_trace(
            OplogCreateTraceState {
                sampled_instant: Some(instant()),
                file_ids: ids.into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(jsonl_root.path()).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Jsonl,
                )
            },
        );
        let jsonl_writer = jsonl_result.unwrap();
        let jsonl_path = health_dir(jsonl_root.path()).join(jsonl_writer.leaf_name());
        drop(jsonl_writer);
        assert_eq!(fs::read(&jsonl_path).unwrap(), JSONL_HEADER);
    }

    #[test]
    fn sampler_fault_occurs_before_any_entropy_draw() {
        let temporary = temp();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                sampler_fail: true,
                file_ids: vec![[0x11; 16]; OPLOG_CREATE_ATTEMPTS].into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(temporary.path()).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                )
            },
        );
        expect_token(result.unwrap_err(), "oplog_create_sampler");
        assert_eq!(state.sampler_calls, 1);
        assert_eq!(count_event(&state, OplogCreateEvent::EntropyDraw), 0);
        assert!(state.file_ids.len() == OPLOG_CREATE_ATTEMPTS);
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn invalid_originals_do_not_call_the_sampler() {
        let temporary = temp();
        let (result, state) = with_trace(empty_trace(), || {
            create_oplog(
                JournalRoot::open(temporary.path()).unwrap(),
                "",
                RUN,
                OplogFormat::Log,
            )
        });
        expect_token(result.unwrap_err(), "oplog_create_invalid_field");
        assert_eq!(state.sampler_calls, 0);
        assert_eq!(count_event(&state, OplogCreateEvent::EntropyDraw), 0);
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn success_records_admission_sync_lease_publish_after_entropy_draws() {
        let temporary = temp();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x31 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        result.unwrap();
        assert_eq!(
            count_event(&state, OplogCreateEvent::EntropyDraw),
            OPLOG_CREATE_ATTEMPTS
        );
        assert_eq!(
            &state.events[OPLOG_CREATE_ATTEMPTS..],
            &[
                OplogCreateEvent::AdmissionBytesAccepted,
                OplogCreateEvent::SyncAll,
                OplogCreateEvent::Lease,
                OplogCreateEvent::Publish,
            ]
        );
    }

    #[test]
    fn sync_fault_writes_bytes_and_invokes_neither_lease_nor_publish() {
        let temporary = temp();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x41 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                sync_fail: true,
                file_ids: ids.into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        expect_token(result.unwrap_err(), "oplog_create_io");
        assert_eq!(
            count_event(&state, OplogCreateEvent::AdmissionBytesAccepted),
            1
        );
        assert_eq!(count_event(&state, OplogCreateEvent::SyncAll), 0);
        assert_eq!(count_event(&state, OplogCreateEvent::Lease), 0);
        assert_eq!(count_event(&state, OplogCreateEvent::Publish), 0);
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn first_distinct_draw_order_binds_admission_and_publish_attempts() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let first = [0x51; 16];
        let second = [0x52; 16];
        let rest: Vec<[u8; 16]> = (0..6).map(|index| [0x53 + index as u8; 16]).collect();
        let mut ids = vec![first, first, second];
        ids.extend(rest);
        fs::write(
            health_dir(temporary.path()).join(dest_for(first)),
            b"incumbent",
        )
        .unwrap();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        let writer = result.unwrap();
        assert_eq!(writer.leaf_name(), dest_for(second));
        assert_eq!(count_event(&state, OplogCreateEvent::EntropyDraw), 9);
        assert_eq!(count_event(&state, OplogCreateEvent::Publish), 2);
        let bytes = fs::read(health_dir(temporary.path()).join(writer.leaf_name())).unwrap();
        let record = validate_oplog_admission(OsStr::new(writer.leaf_name()), &bytes).unwrap();
        assert_eq!(record.candidates()[0].file_id(), file_id_hex(&first));
        assert_eq!(record.candidates()[1].file_id(), file_id_hex(&second));
        assert_eq!(
            fs::read(health_dir(temporary.path()).join(dest_for(first))).unwrap(),
            b"incumbent"
        );
    }
}

#[cfg(test)]
mod derivation_tests {
    use chrono::DateTime;

    use super::*;

    /// DST here is two independent `FixedOffset` instants, not an IANA fold.
    ///
    /// Production only ever receives `DateTime<FixedOffset>`. This crate
    /// depends on plain `chrono`, not `chrono-tz`, so a real zone database
    /// transition is not available in tests. A caller who sampled
    /// `Local::now()` across a spring-forward hands in two nearby instants
    /// with different offsets; this checks that each call derives day-key
    /// and UTC field from that instant alone, with no persistent state.
    #[test]
    fn dst_boundary_instants_derive_independently() {
        let before = DateTime::parse_from_rfc3339("2026-03-08T01:30:00-05:00").unwrap();
        let after = DateTime::parse_from_rfc3339("2026-03-08T03:30:00-04:00").unwrap();
        let (day_before, opened_before) = derive_day_key_and_opened_field(before);
        let (day_after, opened_after) = derive_day_key_and_opened_field(after);
        assert_eq!(day_before, "20260308");
        assert_eq!(opened_before, "20260308T063000.000000Z");
        assert_eq!(day_after, "20260308");
        assert_eq!(opened_after, "20260308T073000.000000Z");
        let (day_before_again, opened_before_again) = derive_day_key_and_opened_field(before);
        assert_eq!(day_before, day_before_again);
        assert_eq!(opened_before, opened_before_again);
    }
}

#[cfg(test)]
mod write_admission_tests {
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind, Write};

    use super::*;

    struct ScriptedWrite {
        plan: VecDeque<io::Result<usize>>,
        sink: Vec<u8>,
    }

    impl Write for ScriptedWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match self.plan.pop_front() {
                Some(Ok(n)) => {
                    let n = n.min(buf.len());
                    self.sink.extend_from_slice(&buf[..n]);
                    Ok(n)
                }
                Some(Err(error)) => Err(error),
                None => {
                    self.sink.extend_from_slice(buf);
                    Ok(buf.len())
                }
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn interrupted_write_retries_until_the_header_is_accepted() {
        let header = b"{\"_solstone_oplog_v\":1}\n";
        let mut writer = ScriptedWrite {
            plan: VecDeque::from([
                Err(io::Error::from(ErrorKind::Interrupted)),
                Ok(4),
                Err(io::Error::from(ErrorKind::Interrupted)),
                Ok(header.len() - 4),
            ]),
            sink: Vec::new(),
        };
        write_admission_bytes(&mut writer, header).unwrap();
        assert_eq!(writer.sink, header);
    }

    #[test]
    fn zero_progress_write_is_io() {
        let header = b"{\"_solstone_oplog_v\":1}\n";
        let mut writer = ScriptedWrite {
            plan: VecDeque::from([Ok(0)]),
            sink: Vec::new(),
        };
        expect_io(write_admission_bytes(&mut writer, header));
        assert!(writer.sink.is_empty());
    }

    #[test]
    fn non_interrupted_write_error_is_io() {
        let header = b"{\"_solstone_oplog_v\":1}\n";
        let mut writer = ScriptedWrite {
            plan: VecDeque::from([Err(io::Error::from(ErrorKind::BrokenPipe))]),
            sink: Vec::new(),
        };
        expect_io(write_admission_bytes(&mut writer, header));
        assert!(writer.sink.is_empty());
    }

    fn expect_io(result: Result<(), OplogCreateError>) {
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "oplog_create_io");
        assert_eq!(format!("{error:?}"), "oplog_create_io");
        assert!(std::error::Error::source(&error).is_none());
    }
}
