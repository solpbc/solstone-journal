// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Descriptor-bound Cortex namespace census and leaf-name lifecycle parsing.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
#[cfg(any(test, feature = "test-hooks"))]
use std::time::Duration;

use super::lock::{CortexNamespaceLock, CortexNamespaceLockError, acquire_cortex_namespace_lock};
use super::namespace::CortexNamespaceAuthority;
use crate::errors::FlatDirectoryError;
use crate::journal_root::{JournalEntryKind, JournalRootError};
use crate::observation::{FlatDirectoryEntry, NativeMtime};

#[cfg(any(test, feature = "test-hooks"))]
use super::lock::acquire_cortex_namespace_lock_with_test_timing;
#[cfg(unix)]
use crate::flat_directory::{
    FlatDirectory, NativeListed, list_native_entries, open_flat_directory_bound,
};
#[cfg(unix)]
use crate::journal_root::ObjectIdentity;
#[cfg(windows)]
use crate::windows_identity::WindowsFileIdentity;
#[cfg(windows)]
use crate::windows_sync_dir::{
    WindowsFlatDirectory, WindowsNativeListed, list_windows_native_entries,
    native_child_directory_identity, open_windows_flat_directory_bound,
};

/// Inclusive listing of one admitted Cortex namespace.
///
/// ```compile_fail
/// let _ = solstone_core_journal_io::cortex_use::CortexCensus {
///     authority: todo!(),
///     lock: todo!(),
///     root_entries: todo!(),
///     talents: todo!(),
/// };
/// ```
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<solstone_core_journal_io::cortex_use::CortexCensus>();
/// needs_clone::<solstone_core_journal_io::cortex_use::CortexTalentCensus>();
/// ```
///
/// ```compile_fail
/// fn steal(census: &solstone_core_journal_io::cortex_use::CortexCensus) {
///     let _owned: solstone_core_journal_io::cortex_use::CortexNamespaceAuthority =
///         *census.authority();
/// }
/// ```
pub struct CortexCensus {
    authority: CortexNamespaceAuthority,
    #[allow(dead_code)]
    lock: CortexNamespaceLock,
    root_entries: Vec<CortexCensusLeaf>,
    talents: Vec<CortexTalentCensus>,
    refused_talents: usize,
}

/// One retained talent directory and its direct children.
pub struct CortexTalentCensus {
    name: OsString,
    #[cfg(unix)]
    directory: FlatDirectory,
    #[cfg(windows)]
    directory: WindowsFlatDirectory,
    entries: Vec<CortexCensusLeaf>,
}

/// One no-follow census leaf with parsed lifecycle projections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CortexCensusLeaf {
    name: OsString,
    kind: JournalEntryKind,
    size: u64,
    mtime: NativeMtime,
    projections: CortexLifecycleProjections,
}

/// Zero, one, or two filename-derived lifecycle projections.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CortexLifecycleProjections {
    active: Option<String>,
    completed: Option<String>,
}

impl CortexCensus {
    /// Borrow the admitted namespace authority retained by this census.
    pub fn authority(&self) -> &CortexNamespaceAuthority {
        &self.authority
    }

    /// Direct `talents/` children, including non-directories.
    pub fn root_entries(&self) -> &[CortexCensusLeaf] {
        &self.root_entries
    }

    /// Retained talent groups in native-name order.
    pub fn talents(&self) -> &[CortexTalentCensus] {
        &self.talents
    }

    /// Inclusive count of every observed root and talent-child entry.
    pub fn observed_entry_count(&self) -> usize {
        self.root_entries
            .len()
            .saturating_add(self.talents.iter().map(|talent| talent.entries.len()).sum())
    }

    /// Talent directories skipped because listing or opening them failed as I/O.
    pub fn refused_talent_count(&self) -> usize {
        self.refused_talents
    }

    /// Re-open every bound talent name and the fixed authority slots.
    pub fn revalidate_bindings(&self) -> Result<(), CortexCensusError> {
        for talent in &self.talents {
            bind_talent(&self.authority, &talent.name, &talent.directory)?;
        }
        bind_authority(&self.authority)
    }
}

impl CortexTalentCensus {
    /// Native directory name as listed under `talents/`.
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Direct children of this talent directory.
    pub fn entries(&self) -> &[CortexCensusLeaf] {
        &self.entries
    }

    /// Borrow the retained talent-directory capability.
    #[cfg(unix)]
    pub fn directory(&self) -> &FlatDirectory {
        &self.directory
    }

    /// Borrow the retained talent-directory capability.
    #[cfg(windows)]
    pub fn directory(&self) -> &WindowsFlatDirectory {
        &self.directory
    }
}

impl CortexCensusLeaf {
    /// Native entry name.
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// No-follow kind.
    pub fn kind(&self) -> JournalEntryKind {
        self.kind
    }

    /// Observed size.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Observed native mtime.
    pub fn mtime(&self) -> NativeMtime {
        self.mtime
    }

    /// Parsed lifecycle projections for this name.
    pub fn projections(&self) -> &CortexLifecycleProjections {
        &self.projections
    }
}

impl CortexLifecycleProjections {
    /// Active-use id when the name is `{id}_active.jsonl`.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// Completed-use id when the name is `{id}.jsonl`.
    pub fn completed(&self) -> Option<&str> {
        self.completed.as_deref()
    }
}

/// Closed checkpoints for one census walk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CortexCensusPrimitive {
    /// After collecting `talents/` names, before observing them.
    PostRootList,
    /// After classifying a real talent directory, before opening it.
    PreTalentOpen,
    /// After collecting one talent directory's names, before observing them.
    PostLeafEnumeration,
    /// After parsing one talent's leaves, before its interleaved binding check.
    PostTalentList,
    /// Before the final parent-relative authority and talent binding pass.
    PreFinalAuthorityCheck,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CortexCensusStage {
    Root,
    TalentOpen,
    TalentList,
    TalentBinding,
    Authority,
}

impl CortexCensusStage {
    const fn token(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::TalentOpen => "talent_open",
            Self::TalentList => "talent_list",
            Self::TalentBinding => "talent_binding",
            Self::Authority => "authority",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CortexCensusClass {
    IdentityChanged,
    Io,
}

impl CortexCensusClass {
    const fn token(self) -> &'static str {
        match self {
            Self::IdentityChanged => "identity_changed",
            Self::Io => "io",
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CortexCensusErrorKind {
    LimitExceeded,
    Stage {
        stage: CortexCensusStage,
        class: CortexCensusClass,
    },
    LockUnsafe,
    LockIdentityChanged,
    LockBusy,
    LockIo,
}

/// Bounded failure while taking a Cortex namespace census.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CortexCensusError {
    kind: CortexCensusErrorKind,
}

impl CortexCensusError {
    const fn limit_exceeded() -> Self {
        Self {
            kind: CortexCensusErrorKind::LimitExceeded,
        }
    }

    const fn stage(stage: CortexCensusStage, class: CortexCensusClass) -> Self {
        Self {
            kind: CortexCensusErrorKind::Stage { stage, class },
        }
    }

    fn from_lock(error: CortexNamespaceLockError) -> Self {
        let kind = match error.to_string().as_str() {
            "cortex_namespace_lock_unsafe" => CortexCensusErrorKind::LockUnsafe,
            "cortex_namespace_lock_identity_changed" => CortexCensusErrorKind::LockIdentityChanged,
            "cortex_namespace_lock_busy" => CortexCensusErrorKind::LockBusy,
            _ => CortexCensusErrorKind::LockIo,
        };
        Self { kind }
    }

    fn token(self) -> String {
        match self.kind {
            CortexCensusErrorKind::LimitExceeded => "cortex_census_limit_exceeded".into(),
            CortexCensusErrorKind::Stage { stage, class } => {
                format!("cortex_census_{}_{}", stage.token(), class.token())
            }
            CortexCensusErrorKind::LockUnsafe => "cortex_namespace_lock_unsafe".into(),
            CortexCensusErrorKind::LockIdentityChanged => {
                "cortex_namespace_lock_identity_changed".into()
            }
            CortexCensusErrorKind::LockBusy => "cortex_namespace_lock_busy".into(),
            CortexCensusErrorKind::LockIo => "cortex_namespace_lock_io".into(),
        }
    }

    fn is_io_at(self, stage: CortexCensusStage) -> bool {
        matches!(
            self.kind,
            CortexCensusErrorKind::Stage {
                stage: found,
                class: CortexCensusClass::Io,
            } if found == stage
        )
    }
}

impl fmt::Display for CortexCensusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.token())
    }
}

impl fmt::Debug for CortexCensusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for CortexCensusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

pub(crate) fn map_listing(
    stage: CortexCensusStage,
    error: FlatDirectoryError,
) -> CortexCensusError {
    let class = match error {
        FlatDirectoryError::IdentityChanged { .. }
        | FlatDirectoryError::EnumerationChanged { .. }
        | FlatDirectoryError::NotDirectory { .. }
        | FlatDirectoryError::SymlinkRefused { .. }
        | FlatDirectoryError::NotRegular { .. } => CortexCensusClass::IdentityChanged,
        _ => CortexCensusClass::Io,
    };
    CortexCensusError::stage(stage, class)
}

/// Parse one native leaf name into zero, one, or two lifecycle projections.
pub fn parse_cortex_lifecycle_name(name: &OsStr) -> CortexLifecycleProjections {
    let Some(text) = name.to_str() else {
        return CortexLifecycleProjections::default();
    };
    CortexLifecycleProjections {
        active: text
            .strip_suffix("_active.jsonl")
            .filter(|stem| !stem.is_empty())
            .map(str::to_owned),
        completed: text
            .strip_suffix(".jsonl")
            .filter(|stem| !stem.is_empty())
            .map(str::to_owned),
    }
}

#[cfg(any(test, feature = "test-hooks"))]
struct CortexCensusTraceState {
    attempted: Vec<CortexCensusPrimitive>,
    fault: Option<(CortexCensusPrimitive, usize)>,
    fault_consumed: bool,
    barriers: Vec<CortexCensusBarrier>,
    barriers_fired: usize,
}

#[cfg(any(test, feature = "test-hooks"))]
struct CortexCensusBarrier {
    primitive: CortexCensusPrimitive,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static CORTEX_CENSUS_TRACE: std::cell::RefCell<Option<CortexCensusTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Run an operation with one injected census fault.
#[cfg(feature = "test-hooks")]
pub fn run_with_cortex_census_fault<T>(
    primitive: CortexCensusPrimitive,
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, consumed, _) = run_with_trace(Some((primitive, ordinal)), Vec::new(), operation);
    (result, consumed)
}

/// Run an operation with one deterministic census barrier.
#[cfg(feature = "test-hooks")]
pub fn run_with_cortex_census_barrier<T>(
    primitive: CortexCensusPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, _, fired) = run_with_trace(
        None,
        vec![CortexCensusBarrier {
            primitive,
            ordinal,
            callback: Box::new(callback),
        }],
        operation,
    );
    (result, fired == 1)
}

#[cfg(any(test, feature = "test-hooks"))]
fn run_with_trace<T>(
    fault: Option<(CortexCensusPrimitive, usize)>,
    barriers: Vec<CortexCensusBarrier>,
    operation: impl FnOnce() -> T,
) -> (T, bool, usize) {
    CORTEX_CENSUS_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Cortex census trace is already active"
        );
        *trace.borrow_mut() = Some(CortexCensusTraceState {
            attempted: Vec::new(),
            fault,
            fault_consumed: false,
            barriers,
            barriers_fired: 0,
        });
    });
    let result = operation();
    let state = CORTEX_CENSUS_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Cortex census trace remains active")
    });
    (result, state.fault_consumed, state.barriers_fired)
}

pub(crate) fn checkpoint(primitive: CortexCensusPrimitive) -> Result<(), CortexCensusError> {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        checkpoint_traced(primitive)
    }
    #[cfg(not(any(test, feature = "test-hooks")))]
    {
        let _ = primitive;
        Ok(())
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint_traced(primitive: CortexCensusPrimitive) -> Result<(), CortexCensusError> {
    let (fault, barrier) = CORTEX_CENSUS_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return (false, None);
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        if state.fault.is_some_and(|(fault_primitive, fault_ordinal)| {
            fault_primitive == primitive && fault_ordinal == ordinal
        }) {
            state.fault = None;
            state.fault_consumed = true;
            (true, None)
        } else if let Some(index) = state
            .barriers
            .iter()
            .position(|barrier| barrier.primitive == primitive && barrier.ordinal == ordinal)
        {
            let barrier = state.barriers.remove(index);
            state.barriers_fired += 1;
            (false, Some(barrier.callback))
        } else {
            (false, None)
        }
    });
    if fault {
        return Err(match primitive {
            CortexCensusPrimitive::PostRootList => CortexCensusError::stage(
                CortexCensusStage::Root,
                CortexCensusClass::IdentityChanged,
            ),
            CortexCensusPrimitive::PreTalentOpen => CortexCensusError::stage(
                CortexCensusStage::TalentOpen,
                CortexCensusClass::IdentityChanged,
            ),
            CortexCensusPrimitive::PostLeafEnumeration => CortexCensusError::stage(
                CortexCensusStage::TalentList,
                CortexCensusClass::IdentityChanged,
            ),
            CortexCensusPrimitive::PostTalentList => CortexCensusError::stage(
                CortexCensusStage::TalentBinding,
                CortexCensusClass::IdentityChanged,
            ),
            CortexCensusPrimitive::PreFinalAuthorityCheck => CortexCensusError::stage(
                CortexCensusStage::Authority,
                CortexCensusClass::IdentityChanged,
            ),
        });
    }
    if let Some(barrier) = barrier {
        barrier();
    }
    Ok(())
}

/// Acquire the namespace lock and list every `talents/` entry under `maximum_entries`.
pub fn census_cortex_namespace(
    authority: CortexNamespaceAuthority,
    maximum_entries: usize,
) -> Result<CortexCensus, CortexCensusError> {
    let lock = acquire_cortex_namespace_lock(&authority).map_err(CortexCensusError::from_lock)?;
    census_after_lock(authority, lock, maximum_entries)
}

/// Acquire the namespace lock with caller-supplied lock timing.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn census_cortex_namespace_with_test_timing(
    authority: CortexNamespaceAuthority,
    maximum_entries: usize,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<CortexCensus, CortexCensusError> {
    let lock = acquire_cortex_namespace_lock_with_test_timing(&authority, timeout, poll_interval)
        .map_err(CortexCensusError::from_lock)?;
    census_after_lock(authority, lock, maximum_entries)
}

fn census_after_lock(
    authority: CortexNamespaceAuthority,
    lock: CortexNamespaceLock,
    maximum_entries: usize,
) -> Result<CortexCensus, CortexCensusError> {
    let listed = list_root(&authority, maximum_entries)?;
    let Some(listed) = listed else {
        return Err(CortexCensusError::limit_exceeded());
    };
    let root_entries = listed.iter().map(|item| leaf_from(&item.entry)).collect();
    let mut counted = listed.len();
    let mut talents = Vec::new();
    let mut refused_talents = 0;
    for item in listed {
        let _ = item.directory;
        if item.entry.kind != JournalEntryKind::Directory {
            continue;
        }
        checkpoint(CortexCensusPrimitive::PreTalentOpen)?;
        let directory = match open_census_talent(&authority, &item.entry.name) {
            Ok(Some(directory)) => directory,
            Ok(None) => {
                return Err(CortexCensusError::stage(
                    CortexCensusStage::TalentOpen,
                    CortexCensusClass::IdentityChanged,
                ));
            }
            Err(error) => {
                let mapped = map_listing(CortexCensusStage::TalentOpen, error);
                if mapped.is_io_at(CortexCensusStage::TalentOpen) {
                    refused_talents += 1;
                    continue;
                }
                return Err(mapped);
            }
        };
        if !opened_matches_listing(&directory, &item.entry) {
            return Err(CortexCensusError::stage(
                CortexCensusStage::TalentOpen,
                CortexCensusClass::IdentityChanged,
            ));
        }
        let remaining = maximum_entries.saturating_sub(counted);
        let children = match list_talent(&directory, remaining) {
            Ok(children) => children,
            Err(error) if error.is_io_at(CortexCensusStage::TalentList) => {
                refused_talents += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        let Some(children) = children else {
            return Err(CortexCensusError::limit_exceeded());
        };
        let entries = children
            .iter()
            .map(|child| leaf_from(&child.entry))
            .collect::<Vec<_>>();
        counted = counted.saturating_add(entries.len());
        checkpoint(CortexCensusPrimitive::PostTalentList)?;
        bind_talent(&authority, &item.entry.name, &directory)?;
        talents.push(CortexTalentCensus {
            name: item.entry.name,
            directory,
            entries,
        });
    }
    checkpoint(CortexCensusPrimitive::PreFinalAuthorityCheck)?;
    for talent in &talents {
        bind_talent(&authority, &talent.name, &talent.directory)?;
    }
    bind_authority(&authority)?;
    Ok(CortexCensus {
        authority,
        lock,
        root_entries,
        talents,
        refused_talents,
    })
}

fn leaf_from(entry: &FlatDirectoryEntry) -> CortexCensusLeaf {
    CortexCensusLeaf {
        name: entry.name.clone(),
        kind: entry.kind,
        size: entry.size,
        mtime: entry.mtime,
        projections: parse_cortex_lifecycle_name(&entry.name),
    }
}

#[cfg(unix)]
fn list_root(
    authority: &CortexNamespaceAuthority,
    maximum_entries: usize,
) -> Result<Option<Vec<NativeListed>>, CortexCensusError> {
    list_native_entries(
        authority.talents(),
        maximum_entries,
        false,
        CortexCensusPrimitive::PostRootList,
        None,
        CortexCensusStage::Root,
    )
}

#[cfg(unix)]
fn open_census_talent(
    authority: &CortexNamespaceAuthority,
    name: &OsStr,
) -> Result<Option<FlatDirectory>, FlatDirectoryError> {
    open_flat_directory_bound(
        authority.talents(),
        name,
        authority.talents().diagnostic_path(),
    )
}

#[cfg(unix)]
fn opened_matches_listing(directory: &FlatDirectory, entry: &FlatDirectoryEntry) -> bool {
    directory.identity() == ObjectIdentity::from_device_inode(entry.device, entry.inode)
}

#[cfg(unix)]
fn list_talent(
    directory: &FlatDirectory,
    remaining: usize,
) -> Result<Option<Vec<NativeListed>>, CortexCensusError> {
    list_native_entries(
        directory,
        remaining,
        false,
        CortexCensusPrimitive::PostLeafEnumeration,
        None,
        CortexCensusStage::TalentList,
    )
}

#[cfg(unix)]
fn bind_talent(
    authority: &CortexNamespaceAuthority,
    name: &OsStr,
    retained: &FlatDirectory,
) -> Result<(), CortexCensusError> {
    match crate::flat_directory::stat_entry(authority.talents(), name) {
        Ok(Some(entry))
            if entry.kind == JournalEntryKind::Directory
                && ObjectIdentity::from_device_inode(entry.device, entry.inode)
                    == retained.identity() =>
        {
            Ok(())
        }
        Ok(_) => Err(CortexCensusError::stage(
            CortexCensusStage::TalentBinding,
            CortexCensusClass::IdentityChanged,
        )),
        Err(error) => Err(map_listing(CortexCensusStage::TalentBinding, error)),
    }
}

#[cfg(unix)]
fn bind_authority(authority: &CortexNamespaceAuthority) -> Result<(), CortexCensusError> {
    map_root(authority.root().revalidate())?;
    bind_fixed(authority, "health", authority.health().identity())?;
    bind_fixed(authority, "talents", authority.talents().identity())?;
    Ok(())
}

#[cfg(unix)]
fn bind_fixed(
    authority: &CortexNamespaceAuthority,
    name: &str,
    expected: ObjectIdentity,
) -> Result<(), CortexCensusError> {
    match open_flat_directory_bound(
        authority.root(),
        OsStr::new(name),
        authority.root().canonical_path(),
    ) {
        Ok(Some(opened)) if opened.identity() == expected => Ok(()),
        Ok(_) => Err(CortexCensusError::stage(
            CortexCensusStage::Authority,
            CortexCensusClass::IdentityChanged,
        )),
        Err(error) => Err(map_listing(CortexCensusStage::Authority, error)),
    }
}

#[cfg(windows)]
fn list_root(
    authority: &CortexNamespaceAuthority,
    maximum_entries: usize,
) -> Result<Option<Vec<WindowsNativeListed>>, CortexCensusError> {
    list_windows_native_entries(
        authority.talents(),
        maximum_entries,
        false,
        CortexCensusPrimitive::PostRootList,
        None,
        CortexCensusStage::Root,
    )
}

#[cfg(windows)]
fn open_census_talent(
    authority: &CortexNamespaceAuthority,
    name: &OsStr,
) -> Result<Option<WindowsFlatDirectory>, crate::errors::FlatDirectoryError> {
    open_windows_flat_directory_bound(
        authority.talents(),
        name,
        authority.talents().diagnostic_path(),
    )
}

#[cfg(windows)]
fn opened_matches_listing(directory: &WindowsFlatDirectory, entry: &FlatDirectoryEntry) -> bool {
    directory.identity().volume_serial() == entry.device
        && directory.identity().folded_file_id() == entry.inode
}

#[cfg(windows)]
fn list_talent(
    directory: &WindowsFlatDirectory,
    remaining: usize,
) -> Result<Option<Vec<WindowsNativeListed>>, CortexCensusError> {
    list_windows_native_entries(
        directory,
        remaining,
        false,
        CortexCensusPrimitive::PostLeafEnumeration,
        None,
        CortexCensusStage::TalentList,
    )
}

#[cfg(windows)]
fn bind_talent(
    authority: &CortexNamespaceAuthority,
    name: &OsStr,
    retained: &WindowsFlatDirectory,
) -> Result<(), CortexCensusError> {
    match native_child_directory_identity(authority.talents(), name) {
        Ok(Some(identity)) if identity == retained.identity() => Ok(()),
        Ok(_) => Err(CortexCensusError::stage(
            CortexCensusStage::TalentBinding,
            CortexCensusClass::IdentityChanged,
        )),
        Err(error) => Err(map_listing(CortexCensusStage::TalentBinding, error)),
    }
}

#[cfg(windows)]
fn bind_authority(authority: &CortexNamespaceAuthority) -> Result<(), CortexCensusError> {
    map_root(authority.root().revalidate())?;
    bind_fixed(authority, "health", authority.health().identity())?;
    bind_fixed(authority, "talents", authority.talents().identity())?;
    Ok(())
}

#[cfg(windows)]
fn bind_fixed(
    authority: &CortexNamespaceAuthority,
    name: &str,
    expected: WindowsFileIdentity,
) -> Result<(), CortexCensusError> {
    match open_windows_flat_directory_bound(
        authority.root(),
        OsStr::new(name),
        authority.root().canonical_path(),
    ) {
        Ok(Some(opened)) if opened.identity() == expected => Ok(()),
        Ok(_) => Err(CortexCensusError::stage(
            CortexCensusStage::Authority,
            CortexCensusClass::IdentityChanged,
        )),
        Err(error) => Err(map_listing(CortexCensusStage::Authority, error)),
    }
}

fn map_root(result: Result<(), JournalRootError>) -> Result<(), CortexCensusError> {
    result.map_err(|error| match error {
        JournalRootError::Changed => CortexCensusError::stage(
            CortexCensusStage::Authority,
            CortexCensusClass::IdentityChanged,
        ),
        _ => CortexCensusError::stage(CortexCensusStage::Authority, CortexCensusClass::Io),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::*;
    use crate::cortex_use::create_or_admit_cortex_namespace;
    use crate::journal_root::JournalRoot;
    use crate::name_admission::NameAdmissionReason;

    const ZERO: Duration = Duration::ZERO;
    const MAX: usize = 64;

    fn temp() -> tempfile::TempDir {
        #[cfg(unix)]
        {
            tempfile::tempdir_in("/var/tmp").unwrap()
        }
        #[cfg(windows)]
        {
            tempfile::TempDir::new().unwrap()
        }
    }

    fn admit(root: &Path) -> CortexNamespaceAuthority {
        create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap()).unwrap()
    }

    fn fill_named(root: &Path, names: &[&str]) {
        let _ = admit(root);
        for name in names {
            let dir = root.join("talents").join(name);
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("sentinel"), name.as_bytes()).unwrap();
        }
    }

    fn census_at(
        authority: CortexNamespaceAuthority,
        maximum: usize,
    ) -> Result<CortexCensus, CortexCensusError> {
        census_cortex_namespace_with_test_timing(authority, maximum, ZERO, ZERO)
    }

    fn assert_token(error: CortexCensusError, token: &str) {
        assert_eq!(error.to_string(), token);
        assert_eq!(format!("{error:?}"), token);
        assert!(error.source().is_none());
    }

    fn census_err(result: Result<CortexCensus, CortexCensusError>) -> CortexCensusError {
        match result {
            Ok(_) => panic!("expected census error"),
            Err(error) => error,
        }
    }

    fn lock_err(
        result: Result<super::super::lock::CortexNamespaceLock, CortexNamespaceLockError>,
    ) -> CortexNamespaceLockError {
        match result {
            Ok(_) => panic!("expected lock error"),
            Err(error) => error,
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Snap {
        kind: String,
        id: (u64, u64),
        payload: Vec<u8>,
    }

    fn snap_one(path: &Path) -> Snap {
        let meta = fs::symlink_metadata(path).unwrap();
        let ft = meta.file_type();
        let kind = if ft.is_symlink() {
            "link"
        } else if ft.is_dir() {
            "dir"
        } else if ft.is_file() {
            "file"
        } else {
            "other"
        };
        #[cfg(unix)]
        let id = {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        };
        #[cfg(windows)]
        let id = {
            use std::os::windows::fs::MetadataExt;
            (meta.creation_time(), meta.file_size())
        };
        let payload = if ft.is_symlink() {
            fs::read_link(path)
                .unwrap()
                .to_string_lossy()
                .into_owned()
                .into_bytes()
        } else if ft.is_file() {
            if path.file_name() == Some(OsStr::new("cortex-use.lock")) {
                Vec::new()
            } else {
                fs::read(path).unwrap()
            }
        } else {
            Vec::new()
        };
        Snap {
            kind: kind.into(),
            id,
            payload,
        }
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Snap> {
        let mut out = BTreeMap::new();
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Snap>) {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                let snap = snap_one(&path);
                let walk_dir = snap.kind == "dir";
                out.insert(rel, snap);
                if walk_dir {
                    walk(&path, root, out);
                }
            }
        }
        walk(root, root, &mut out);
        out
    }

    fn rename_prefix(map: &mut BTreeMap<PathBuf, Snap>, from: &Path, to: &Path) {
        let keys: Vec<_> = map
            .keys()
            .filter(|key| *key == from || key.starts_with(from))
            .cloned()
            .collect();
        for key in keys {
            let value = map.remove(&key).unwrap();
            map.insert(to.join(key.strip_prefix(from).unwrap()), value);
        }
    }

    fn assert_accounted(
        before: BTreeMap<PathBuf, Snap>,
        after: &BTreeMap<PathBuf, Snap>,
        lock: bool,
        renames: &[(&str, &str)],
        added: &[&str],
    ) {
        let mut expected = before;
        if lock && let Some(value) = after.get(Path::new("cortex-use.lock")) {
            expected.insert(PathBuf::from("cortex-use.lock"), value.clone());
        }
        for (from, to) in renames {
            rename_prefix(&mut expected, Path::new(from), Path::new(to));
        }
        for path in added {
            let path = PathBuf::from(path);
            expected.insert(path.clone(), after.get(&path).cloned().unwrap());
        }
        assert_eq!(after, &expected);
    }

    fn replace_talent(root: &Path, name: &str) {
        let talents = root.join("talents");
        fs::rename(
            talents.join(name),
            talents.join(format!("{name}-displaced")),
        )
        .unwrap();
        fs::create_dir(talents.join(name)).unwrap();
        fs::write(talents.join(name).join("replacement"), b"replacement").unwrap();
    }

    fn count_expected(talents: &Path) -> usize {
        let mut count = 0;
        for entry in fs::read_dir(talents).unwrap() {
            let entry = entry.unwrap();
            count += 1;
            let meta = fs::symlink_metadata(entry.path()).unwrap();
            if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
                count += fs::read_dir(entry.path()).unwrap().count();
            }
        }
        count
    }

    fn proj(active: Option<&str>, completed: Option<&str>) -> CortexLifecycleProjections {
        CortexLifecycleProjections {
            active: active.map(str::to_owned),
            completed: completed.map(str::to_owned),
        }
    }

    fn round_trip(name: &str, parsed: &CortexLifecycleProjections) {
        if let Some(id) = parsed.active() {
            assert_eq!(format!("{id}_active.jsonl"), name);
        }
        if let Some(id) = parsed.completed() {
            assert_eq!(format!("{id}.jsonl"), name);
        }
    }

    #[test]
    fn parser_matrix() {
        let cases = [
            ("", None, None),
            (".jsonl", None, None),
            ("_active.jsonl", None, Some("_active")),
            ("alpha.jsonl", None, Some("alpha")),
            ("alpha_active.jsonl", Some("alpha"), Some("alpha_active")),
            (
                "alpha_active_active.jsonl",
                Some("alpha_active"),
                Some("alpha_active_active"),
            ),
            ("alpha.JSONL", None, None),
            ("alpha_ACTIVE.jsonl", None, Some("alpha_ACTIVE")),
            ("alpha_Active.jsonl", None, Some("alpha_Active")),
            ("Alpha_active.jsonl", Some("Alpha"), Some("Alpha_active")),
            ("alpha.jsonl.bak", None, None),
            ("alpha_active.jsonl.extra", None, None),
            ("alpha.jsonl.jsonl", None, Some("alpha.jsonl")),
            ("a/b.jsonl", None, Some("a/b")),
            ("a\\b.jsonl", None, Some("a\\b")),
            ("alpha\n.jsonl", None, Some("alpha\n")),
            ("α.jsonl", None, Some("α")),
            ("alpha_актив.jsonl", None, Some("alpha_актив")),
        ];
        for (name, active, completed) in cases {
            let parsed = parse_cortex_lifecycle_name(OsStr::new(name));
            assert_eq!(parsed, proj(active, completed), "{name}");
            round_trip(name, &parsed);
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let parsed = parse_cortex_lifecycle_name(&OsString::from_vec(b"x-\xff.jsonl".to_vec()));
            assert_eq!(parsed, CortexLifecycleProjections::default());
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let parsed = parse_cortex_lifecycle_name(&OsString::from_wide(&[0xD800, 0x0061]));
            assert_eq!(parsed, CortexLifecycleProjections::default());
        }
    }

    #[test]
    fn bounded_diagnostics() {
        let secret = PathBuf::from("controlled-secret");
        let staged = [
            CortexCensusStage::Root,
            CortexCensusStage::TalentOpen,
            CortexCensusStage::TalentList,
            CortexCensusStage::TalentBinding,
            CortexCensusStage::Authority,
        ];
        let mapped = [
            FlatDirectoryError::InvalidRelativePath {
                path: secret.clone(),
                reason: "controlled-secret",
            },
            FlatDirectoryError::InvalidName {
                name: OsString::from("controlled-secret"),
                reason: NameAdmissionReason::Empty,
            },
            FlatDirectoryError::NotDirectory {
                path: secret.clone(),
            },
            FlatDirectoryError::SymlinkRefused {
                path: secret.clone(),
            },
            FlatDirectoryError::NotRegular {
                path: secret.clone(),
            },
            FlatDirectoryError::SizeLimitExceeded {
                path: secret.clone(),
                kind: JournalEntryKind::RegularFile,
                size: 1,
                limit: 0,
            },
            FlatDirectoryError::IdentityChanged {
                path: secret.clone(),
            },
            FlatDirectoryError::EnumerationChanged {
                path: secret.clone(),
            },
            FlatDirectoryError::Io {
                operation: "controlled-secret",
                path: secret,
                source: io::Error::other("controlled-secret"),
            },
        ];
        let mut rows = vec![CortexCensusError::limit_exceeded()];
        for stage in staged {
            rows.push(CortexCensusError::stage(
                stage,
                CortexCensusClass::IdentityChanged,
            ));
            rows.push(CortexCensusError::stage(stage, CortexCensusClass::Io));
        }
        rows.extend([
            CortexCensusError {
                kind: CortexCensusErrorKind::LockUnsafe,
            },
            CortexCensusError {
                kind: CortexCensusErrorKind::LockIdentityChanged,
            },
            CortexCensusError {
                kind: CortexCensusErrorKind::LockBusy,
            },
            CortexCensusError {
                kind: CortexCensusErrorKind::LockIo,
            },
        ]);
        assert_eq!(rows.len(), 15);
        for error in rows {
            assert_eq!(error.to_string(), format!("{error:?}"));
            assert!(error.source().is_none());
            assert!(!error.to_string().contains("controlled-secret"));
            assert!(!format!("{error:?}").contains("controlled-secret"));
        }
        for stage in staged {
            for error in &mapped {
                let rendered = map_listing(stage, clone_flat(error));
                assert_eq!(rendered.to_string(), format!("{rendered:?}"));
                assert!(rendered.source().is_none());
                assert!(!rendered.to_string().contains("controlled-secret"));
            }
        }
    }

    fn clone_flat(error: &FlatDirectoryError) -> FlatDirectoryError {
        match error {
            FlatDirectoryError::InvalidRelativePath { path, reason } => {
                FlatDirectoryError::InvalidRelativePath {
                    path: path.clone(),
                    reason,
                }
            }
            FlatDirectoryError::InvalidName { name, reason } => FlatDirectoryError::InvalidName {
                name: name.clone(),
                reason: *reason,
            },
            FlatDirectoryError::NotDirectory { path } => {
                FlatDirectoryError::NotDirectory { path: path.clone() }
            }
            FlatDirectoryError::SymlinkRefused { path } => {
                FlatDirectoryError::SymlinkRefused { path: path.clone() }
            }
            FlatDirectoryError::NotRegular { path } => {
                FlatDirectoryError::NotRegular { path: path.clone() }
            }
            FlatDirectoryError::SizeLimitExceeded {
                path,
                kind,
                size,
                limit,
            } => FlatDirectoryError::SizeLimitExceeded {
                path: path.clone(),
                kind: *kind,
                size: *size,
                limit: *limit,
            },
            FlatDirectoryError::IdentityChanged { path } => {
                FlatDirectoryError::IdentityChanged { path: path.clone() }
            }
            FlatDirectoryError::EnumerationChanged { path } => {
                FlatDirectoryError::EnumerationChanged { path: path.clone() }
            }
            FlatDirectoryError::Io {
                operation,
                path,
                source,
            } => FlatDirectoryError::Io {
                operation,
                path: path.clone(),
                source: io::Error::new(source.kind(), source.to_string()),
            },
        }
    }

    #[test]
    fn empty_namespace_shapes() {
        let empty = temp();
        let _ = admit(empty.path());
        let before = snapshot_tree(empty.path());
        let census = census_at(admit(empty.path()), MAX).unwrap();
        assert_eq!(census.observed_entry_count(), 0);
        assert!(census.root_entries().is_empty());
        assert!(census.talents().is_empty());
        drop(census);
        let after = snapshot_tree(empty.path());
        assert_accounted(before, &after, true, &[], &[]);

        let vacant = temp();
        fill_named(vacant.path(), &["keep"]);
        fs::remove_file(vacant.path().join("talents/keep/sentinel")).unwrap();
        let before = snapshot_tree(vacant.path());
        let census = census_at(admit(vacant.path()), MAX).unwrap();
        assert_eq!(census.talents().len(), 1);
        assert_eq!(census.talents()[0].name(), "keep");
        assert!(census.talents()[0].entries().is_empty());
        assert_eq!(census.root_entries().len(), 1);
        assert_eq!(census.observed_entry_count(), 1);
        drop(census);
        let after = snapshot_tree(vacant.path());
        assert_accounted(before, &after, true, &[], &[]);
    }

    fn seed_rich(root: &Path) {
        let _ = admit(root);
        let talents = root.join("talents");
        let keep = talents.join("keep");
        fs::create_dir(&keep).unwrap();
        fs::write(keep.join("alpha.jsonl"), b"completed").unwrap();
        fs::write(keep.join("alpha_active.jsonl"), b"dual").unwrap();
        fs::write(keep.join("notes.txt"), b"notes").unwrap();
        fs::create_dir(keep.join("nested")).unwrap();
        fs::write(keep.join("not-a-run.jsonl"), b"wrong").unwrap();
        fs::write(talents.join("day"), b"day").unwrap();
        fs::write(talents.join("index"), b"index").unwrap();
        fs::write(talents.join("regular"), b"regular").unwrap();
        fs::write(root.join("outside"), b"outside").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("outside", talents.join("link")).unwrap();
            symlink("keep", talents.join("dirlink")).unwrap();
            nix::unistd::mkfifo(
                &keep.join("fifo"),
                nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
            )
            .unwrap();
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                create_socket(&keep.join("sock"));
                create_socket(&talents.join("root.sock"));
            }
        }
        fs::create_dir(talents.join("ambiguous")).unwrap();
        fs::write(
            talents.join("ambiguous").join("foo_active_active.jsonl"),
            b"hist",
        )
        .unwrap();
        #[cfg(all(unix, target_os = "linux"))]
        {
            use std::os::unix::ffi::OsStringExt;
            fs::write(
                keep.join(OsString::from_vec(b"bad-\xff.jsonl".to_vec())),
                b"bin",
            )
            .unwrap();
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn create_socket(path: &Path) {
        nix::sys::stat::mknod(
            path,
            nix::sys::stat::SFlag::S_IFSOCK,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
            0,
        )
        .unwrap();
    }

    #[test]
    fn all_entry_denominator_and_idempotency() {
        let temporary = temp();
        seed_rich(temporary.path());
        let expected = count_expected(&temporary.path().join("talents"));
        let before = snapshot_tree(temporary.path());
        let census = census_at(admit(temporary.path()), MAX).unwrap();
        assert_eq!(census.observed_entry_count(), expected);
        let root_names: Vec<_> = census
            .root_entries()
            .iter()
            .map(|entry| entry.name().to_os_string())
            .collect();
        let mut disk = fs::read_dir(temporary.path().join("talents"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        disk.sort();
        let mut listed = root_names.clone();
        listed.sort();
        assert_eq!(listed, disk);
        assert!(
            census
                .talents()
                .iter()
                .any(|talent| talent.name() == "keep")
        );
        assert!(
            census
                .talents()
                .iter()
                .any(|talent| talent.name() == "ambiguous")
        );
        assert!(
            !census
                .talents()
                .iter()
                .any(|talent| talent.name() == "dirlink")
        );
        assert!(
            census.root_entries().iter().any(|entry| {
                entry.name() == "dirlink" && matches!(entry.kind(), JournalEntryKind::Symlink)
            }) || !cfg!(unix)
        );
        let keep = census
            .talents()
            .iter()
            .find(|talent| talent.name() == "keep")
            .unwrap();
        assert_eq!(
            keep.entries()
                .iter()
                .find(|entry| entry.name() == "alpha.jsonl")
                .unwrap()
                .projections(),
            &proj(None, Some("alpha"))
        );
        assert_eq!(
            keep.entries()
                .iter()
                .find(|entry| entry.name() == "alpha_active.jsonl")
                .unwrap()
                .projections(),
            &proj(Some("alpha"), Some("alpha_active"))
        );
        assert!(
            keep.entries().iter().any(
                |entry| entry.name() == "nested" && entry.kind() == JournalEntryKind::Directory
            )
        );
        let view = (
            census.root_entries().to_vec(),
            census
                .talents()
                .iter()
                .map(|talent| (talent.name().to_os_string(), talent.entries().to_vec()))
                .collect::<Vec<_>>(),
        );
        drop(census);
        let after = snapshot_tree(temporary.path());
        assert_accounted(before, &after, true, &[], &[]);
        let second = census_at(admit(temporary.path()), MAX).unwrap();
        assert_eq!(second.root_entries(), view.0);
        assert_eq!(
            second
                .talents()
                .iter()
                .map(|talent| (talent.name().to_os_string(), talent.entries().to_vec()))
                .collect::<Vec<_>>(),
            view.1
        );
    }

    #[test]
    fn complete_or_nothing() {
        let temporary = temp();
        seed_rich(temporary.path());
        let expected = count_expected(&temporary.path().join("talents"));
        let before = snapshot_tree(temporary.path());
        assert_eq!(
            census_at(admit(temporary.path()), expected)
                .unwrap()
                .observed_entry_count(),
            expected
        );
        let after = snapshot_tree(temporary.path());
        assert_accounted(before.clone(), &after, true, &[], &[]);
        let error = census_err(census_at(admit(temporary.path()), expected - 1));
        assert_token(error, "cortex_census_limit_exceeded");
        let faults = [
            (
                CortexCensusPrimitive::PostRootList,
                1,
                "cortex_census_root_identity_changed",
            ),
            (
                CortexCensusPrimitive::PostLeafEnumeration,
                2,
                "cortex_census_talent_list_identity_changed",
            ),
            (
                CortexCensusPrimitive::PreFinalAuthorityCheck,
                1,
                "cortex_census_authority_identity_changed",
            ),
        ];
        for (primitive, ordinal, token) in faults {
            let root = temporary.path().to_path_buf();
            let (result, consumed, _) =
                run_with_trace(Some((primitive, ordinal)), Vec::new(), || {
                    census_at(admit(&root), MAX)
                });
            assert!(consumed, "{token}");
            assert_token(census_err(result), token);
            assert!(census_at(admit(&root), MAX).is_ok());
        }
        let after = snapshot_tree(temporary.path());
        assert_accounted(before, &after, true, &[], &[]);
    }

    #[cfg(unix)]
    fn lock_id(path: &Path) -> (u64, u64) {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(path).unwrap();
        (meta.dev(), meta.ino())
    }

    #[cfg(windows)]
    fn lock_id(path: &Path) -> (u64, u64) {
        use std::os::windows::fs::MetadataExt;
        let meta = fs::metadata(path).unwrap();
        (meta.creation_time(), meta.file_size())
    }

    #[test]
    fn lock_lifetime_and_exact_authority() {
        let first = temp();
        fill_named(first.path(), &["keep"]);
        let first_path = first.path().to_path_buf();
        let second = temp();
        fill_named(second.path(), &["other"]);
        let second_path = second.path().to_path_buf();
        let before = snapshot_tree(&first_path);
        let lock_path = first_path.join("cortex-use.lock");
        let (result, _, fired) = run_with_trace(
            None,
            vec![CortexCensusBarrier {
                primitive: CortexCensusPrimitive::PostRootList,
                ordinal: 1,
                callback: Box::new({
                    let first_path = first_path.clone();
                    let second_path = second_path.clone();
                    move || {
                        let contender = admit(&first_path);
                        assert_eq!(
                            lock_err(acquire_cortex_namespace_lock_with_test_timing(
                                &contender, ZERO, ZERO
                            ))
                            .to_string(),
                            "cortex_namespace_lock_busy"
                        );
                        drop(
                            acquire_cortex_namespace_lock_with_test_timing(
                                &admit(&second_path),
                                ZERO,
                                ZERO,
                            )
                            .unwrap(),
                        );
                    }
                }),
            }],
            || census_at(admit(&first_path), MAX),
        );
        assert_eq!(fired, 1);
        let census = result.unwrap();
        assert!(lock_path.exists());
        let held = lock_id(&lock_path);
        let contender = admit(&first_path);
        assert_eq!(
            lock_err(acquire_cortex_namespace_lock_with_test_timing(
                &contender, ZERO, ZERO,
            ))
            .to_string(),
            "cortex_namespace_lock_busy"
        );
        drop(census);
        let reacquired =
            acquire_cortex_namespace_lock_with_test_timing(&contender, ZERO, ZERO).unwrap();
        assert_eq!(lock_id(&lock_path), held);
        drop(reacquired);
        let after = snapshot_tree(&first_path);
        assert_accounted(before, &after, true, &[], &[]);
    }

    #[test]
    fn owned_capability_survives_path_replacement() {
        let temporary = temp();
        fill_named(temporary.path(), &["keep", "other"]);
        let census = census_at(admit(temporary.path()), MAX).unwrap();
        assert_eq!(census.talents().len(), 2);
        assert!(
            census
                .root_entries()
                .iter()
                .any(|entry| entry.name() == "keep")
        );
        let contender = admit(temporary.path());
        assert_eq!(
            lock_err(acquire_cortex_namespace_lock_with_test_timing(
                &contender, ZERO, ZERO,
            ))
            .to_string(),
            "cortex_namespace_lock_busy"
        );
        let keep = census
            .talents()
            .iter()
            .find(|talent| talent.name() == "keep")
            .unwrap();
        let before = snapshot_tree(temporary.path());
        replace_talent(temporary.path(), "keep");
        #[cfg(unix)]
        {
            let listed = crate::flat_directory::list_flat_directory(keep.directory(), 16)
                .unwrap()
                .unwrap();
            assert!(listed.iter().any(|entry| entry.name == "sentinel"));
            assert!(!listed.iter().any(|entry| entry.name == "replacement"));
            let observed =
                crate::flat_directory::read_observed_file(keep.directory(), OsStr::new("sentinel"))
                    .unwrap()
                    .unwrap();
            assert_eq!(observed.bytes, b"keep");
        }
        #[cfg(windows)]
        {
            let listed = crate::windows_sync_dir::list_windows_flat_directory(keep.directory(), 16)
                .unwrap()
                .unwrap();
            assert!(listed.iter().any(|entry| entry.name == "sentinel"));
            assert!(!listed.iter().any(|entry| entry.name == "replacement"));
        }
        assert_token(
            census.revalidate_bindings().unwrap_err(),
            "cortex_census_talent_binding_identity_changed",
        );
        drop(census);
        let after = snapshot_tree(temporary.path());
        assert_accounted(
            before,
            &after,
            true,
            &[("talents/keep", "talents/keep-displaced")],
            &["talents/keep", "talents/keep/replacement"],
        );
    }

    #[cfg(all(test, windows))]
    #[test]
    fn windows_identity_rejects_folded_collision() {
        let left = crate::windows_identity::WindowsFileIdentity::from_parts(
            1,
            [1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0],
        );
        let right = crate::windows_identity::WindowsFileIdentity::from_parts(
            1,
            [3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert_ne!(left, right);
        assert_eq!(left.folded_file_id(), right.folded_file_id());
        assert_eq!(
            left,
            crate::windows_identity::WindowsFileIdentity::from_parts(
                1,
                [1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0],
            )
        );
    }

    fn barrier_replace(
        root: &Path,
        primitive: CortexCensusPrimitive,
        ordinal: usize,
        target: &str,
        leaf: Option<&str>,
        token: &str,
        names: &[&str],
    ) {
        fill_named(root, names);
        let before = snapshot_tree(root);
        let root = root.to_path_buf();
        let target_name = target.to_owned();
        let leaf_name = leaf.map(str::to_owned);
        let (result, _, fired) = run_with_trace(
            None,
            vec![CortexCensusBarrier {
                primitive,
                ordinal,
                callback: Box::new({
                    let root = root.clone();
                    move || {
                        if let Some(leaf) = leaf_name {
                            fs::remove_file(root.join("talents").join(&target_name).join(leaf))
                                .unwrap();
                        } else {
                            replace_talent(&root, &target_name);
                        }
                    }
                }),
            }],
            || census_at(admit(&root), MAX),
        );
        assert_eq!(fired, 1);
        assert_token(census_err(result), token);
        let after = snapshot_tree(&root);
        if let Some(leaf) = leaf {
            let mut expected = before;
            expected.remove(&PathBuf::from(format!("talents/{target}/{leaf}")));
            if let Some(value) = after.get(Path::new("cortex-use.lock")) {
                expected.insert(PathBuf::from("cortex-use.lock"), value.clone());
            }
            assert_eq!(after, expected);
        } else {
            let from = format!("talents/{target}");
            let displaced = format!("talents/{target}-displaced");
            let replacement = format!("talents/{target}/replacement");
            assert_accounted(
                before,
                &after,
                true,
                &[(from.as_str(), displaced.as_str())],
                &[from.as_str(), replacement.as_str()],
            );
        }
    }

    fn run_barrier_stage_rows() {
        let rows = [
            (
                CortexCensusPrimitive::PreTalentOpen,
                1,
                "keep",
                None,
                "cortex_census_talent_open_identity_changed",
                &["keep", "stable"][..],
            ),
            (
                CortexCensusPrimitive::PostTalentList,
                1,
                "keep",
                None,
                "cortex_census_talent_binding_identity_changed",
                &["keep", "stable"][..],
            ),
            (
                CortexCensusPrimitive::PostLeafEnumeration,
                1,
                "keep",
                Some("sentinel"),
                "cortex_census_talent_list_identity_changed",
                &["keep"][..],
            ),
            (
                CortexCensusPrimitive::PostTalentList,
                2,
                "alpha",
                None,
                "cortex_census_talent_binding_identity_changed",
                &["alpha", "beta", "gamma"][..],
            ),
        ];
        for (primitive, ordinal, target, leaf, token, names) in rows {
            let temporary = temp();
            barrier_replace(
                temporary.path(),
                primitive,
                ordinal,
                target,
                leaf,
                token,
                names,
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn barrier_stage_errors() {
        run_barrier_stage_rows();
    }

    #[cfg(windows)]
    #[test]
    fn windows_barrier_stage_errors() {
        run_barrier_stage_rows();
    }

    #[test]
    fn closed_barrier_surface() {
        let rows = [
            (
                CortexCensusPrimitive::PostRootList,
                "cortex_census_root_identity_changed",
            ),
            (
                CortexCensusPrimitive::PreTalentOpen,
                "cortex_census_talent_open_identity_changed",
            ),
            (
                CortexCensusPrimitive::PostLeafEnumeration,
                "cortex_census_talent_list_identity_changed",
            ),
            (
                CortexCensusPrimitive::PostTalentList,
                "cortex_census_talent_binding_identity_changed",
            ),
            (
                CortexCensusPrimitive::PreFinalAuthorityCheck,
                "cortex_census_authority_identity_changed",
            ),
        ];
        for (primitive, token) in rows {
            let temporary = temp();
            fill_named(temporary.path(), &["keep"]);
            let root = temporary.path().to_path_buf();
            let (result, consumed, _) = run_with_trace(Some((primitive, 1)), Vec::new(), || {
                census_at(admit(&root), MAX)
            });
            assert!(consumed, "{token}");
            assert_token(census_err(result), token);
        }
    }
}
