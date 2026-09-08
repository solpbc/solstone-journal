// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact-child launch admission and noninherited capability transfer.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{DuplicateHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_READ, SYNCHRONIZE};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, SetEvent, WaitForSingleObject,
};

use super::identity::current_windows_process_instance;
use super::job_process::WindowsJobProcess;
use super::{launch_io, launch_pipe};
use crate::lifecycle::HostedServiceKind;
use crate::process::launch_context::require_handle_access;
use crate::process::{
    HostedLaunchProvenance, LaunchError, ProcessInstance, ReadFileGrant, ReadFileGrantKind,
};

const LAUNCH_ENV: &str = "SOL_WINDOWS_LAUNCH";
const OBSOLETE: [&str; 8] = [
    "SOL_SUPERVISOR_SPAWNED",
    "SOL_HOSTED_LAUNCH_ID",
    "SOL_HOSTED_PARENT_INSTANCE",
    "SOL_HOSTED_ACK_HANDLE",
    "SOL_HOSTED_STOP_HANDLE",
    "SOL_PARENT_LOSS_GENERATION",
    "SOL_PARENT_LOSS_LAUNCH_ID",
    "SOL_PARENT_LOSS_PARENT_LAUNCH_ID",
];
const GUARDS: [&str; 4] = [
    "SOLSTONE_INSTALLATION_NAMESPACE",
    "SOLSTONE_INSTALLATION_ID",
    "SOLSTONE_INSTALLATION_GENERATION",
    "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedTuple {
    parent: ProcessInstance,
    journal: PathBuf,
    generation: u64,
    launch_id: String,
    parent_launch_id: Option<String>,
    service: Option<HostedServiceKind>,
    guards: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledTaskTuple {
    parent: ProcessInstance,
    journal: PathBuf,
    launch_id: String,
    guards: BTreeMap<String, String>,
    arguments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "launch", deny_unknown_fields)]
enum LaunchTuple {
    Hosted(HostedTuple),
    InstalledTask(InstalledTaskTuple),
}

impl LaunchTuple {
    fn parent(&self) -> ProcessInstance {
        match self {
            Self::Hosted(value) => value.parent,
            Self::InstalledTask(value) => value.parent,
        }
    }
    fn journal(&self) -> &std::path::Path {
        match self {
            Self::Hosted(value) => &value.journal,
            Self::InstalledTask(value) => &value.journal,
        }
    }
    fn launch_id(&self) -> &str {
        match self {
            Self::Hosted(value) => &value.launch_id,
            Self::InstalledTask(value) => &value.launch_id,
        }
    }
    fn guards(&self) -> &BTreeMap<String, String> {
        match self {
            Self::Hosted(value) => &value.guards,
            Self::InstalledTask(value) => &value.guards,
        }
    }
}

/// Exact installed action to compare with the loaded binding and actual argv.
/// This metadata grants no speakers generation or hosted-service authority.
#[derive(Clone, Debug)]
pub struct InstalledTaskLaunchRequest {
    pub journal: PathBuf,
    pub guard: solstone_core_installation_identity::GuardFields,
    pub arguments: Vec<String>,
    pub acknowledgement_timeout: Duration,
}

/// Installed-root admission received from the retained exact forwarder.
#[derive(Debug)]
pub struct AdmittedInstalledTaskLaunch {
    tuple: InstalledTaskTuple,
    _stop: Arc<OwnedHandle>,
}

impl AdmittedInstalledTaskLaunch {
    pub fn forwarder(&self) -> ProcessInstance {
        self.tuple.parent
    }
    pub fn launch_id(&self) -> &str {
        &self.tuple.launch_id
    }
    pub fn journal(&self) -> &std::path::Path {
        &self.tuple.journal
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    pipe: String,
    expires_at_tick: u64,
    tuple: LaunchTuple,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireGrant {
    kind: ReadFileGrantKind,
    handle: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer {
    descriptor: Descriptor,
    child: ProcessInstance,
    stop: usize,
    grants: Vec<WireGrant>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Acknowledgement {
    descriptor: Descriptor,
    child: ProcessInstance,
}

/// Immutable, authenticated launch metadata and retained child capabilities.
/// The only constructor receives an exact-parent offer over the launch pipe.
#[derive(Clone, Debug)]
pub struct AdmittedWindowsLaunch {
    tuple: HostedTuple,
    stop: Arc<OwnedHandle>,
    grants: Vec<ReadFileGrant>,
}

impl AdmittedWindowsLaunch {
    pub fn parent(&self) -> ProcessInstance {
        self.tuple.parent
    }
    pub fn journal(&self) -> &std::path::Path {
        &self.tuple.journal
    }
    pub fn generation(&self) -> u64 {
        self.tuple.generation
    }
    pub fn service(&self) -> Option<HostedServiceKind> {
        self.tuple.service
    }
    pub fn launch_id(&self) -> &str {
        &self.tuple.launch_id
    }
    pub fn read_file_grants(&self) -> &[ReadFileGrant] {
        &self.grants
    }

    pub fn stop_requested(&self) -> io::Result<bool> {
        wait_stop(&self.stop)
    }

    pub fn child_launch_provenance(&self, launch_id: String) -> HostedLaunchProvenance {
        HostedLaunchProvenance {
            journal: self.tuple.journal.clone(),
            generation: self.tuple.generation,
            launch_id,
            service: None,
            parent_launch_id: Some(self.tuple.launch_id.clone()),
            acknowledgement_timeout: Duration::from_secs(3),
        }
    }
}

pub(super) fn wait_stop(stop: &OwnedHandle) -> io::Result<bool> {
    // SAFETY: retained event is valid; this observes without consuming its latch.
    #[allow(unsafe_code)]
    match unsafe { WaitForSingleObject(stop.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

fn tick() -> u64 {
    // SAFETY: GetTickCount64 takes no pointer arguments and is machine-wide.
    #[allow(unsafe_code)]
    unsafe {
        GetTickCount64()
    }
}

fn refusal(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn guard_environment(
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> io::Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for name in GUARDS {
        if let Some(value) = lookup(name) {
            values.insert(
                name.to_owned(),
                value
                    .into_string()
                    .map_err(|_| refusal("non-Unicode installation guard"))?,
            );
        }
    }
    solstone_core_installation_identity::parse_service_guard_environment(&values)
        .map_err(|error| refusal(&format!("installation guard: {error}")))?;
    Ok(values)
}

fn validate_tuple(tuple: &LaunchTuple) -> io::Result<()> {
    if tuple.launch_id().is_empty()
        || tuple.launch_id().len() > 256
        || !tuple.journal().is_absolute()
        || tuple.parent().birth.windows_filetime().is_none()
    {
        return Err(refusal("invalid launch provenance"));
    }
    match tuple {
        LaunchTuple::Hosted(hosted)
            if hosted
                .parent_launch_id
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 256) =>
        {
            return Err(refusal("invalid parent launch provenance"));
        }
        LaunchTuple::InstalledTask(root)
            if root.guards.len() != GUARDS.len()
                || root.arguments.is_empty()
                || root.arguments.iter().any(|arg| arg.contains('\0')) =>
        {
            return Err(refusal("invalid installed task provenance"));
        }
        _ => {}
    }
    let guard =
        solstone_core_installation_identity::parse_service_guard_environment(tuple.guards())
            .map_err(|error| refusal(&format!("installation guard: {error}")))?;
    if let Some(guard) = guard {
        let owner = solstone_core_installation_identity::owner_base()
            .map_err(|e| refusal(&e.to_string()))?;
        let exe = std::env::current_exe()?;
        let root = exe
            .parent()
            .and_then(solstone_core_journal::resolve_identity_root_from_executable_dir)
            .ok_or_else(|| refusal("launch executable has no installation root"))?;
        let root = solstone_core_installation_identity::root_token_from_path(&root)
            .map_err(|e| refusal(&e.to_string()))?;
        let binding = solstone_core_installation_identity::load_installation_binding(&owner, &root)
            .map_err(|e| refusal(&e.to_string()))?;
        let journal = solstone_core_installation_identity::journal_token_from_path(tuple.journal())
            .map_err(|e| refusal(&e.to_string()))?;
        if guard != solstone_core_installation_identity::GuardFields::from_binding(&binding)
            || journal != binding.journal_token
        {
            return Err(refusal("launch installation binding mismatch"));
        }
    }
    Ok(())
}

fn launch_deadline(timeout: Duration) -> io::Result<Instant> {
    launch_io::require_completed_cleanup()?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| refusal("launch timeout overflows"))?;
    if timeout.is_zero() || timeout > Duration::from_secs(60) {
        return Err(refusal(
            "launch timeout must be greater than zero and at most 60 seconds",
        ));
    }

    Ok(deadline)
}

pub(super) struct LaunchControl {
    descriptor: Descriptor,
    pipe: Arc<OwnedHandle>,
    stop: OwnedHandle,
    deadline: Instant,
}

impl LaunchControl {
    pub(super) fn prepare(
        provenance: &HostedLaunchProvenance,
        environment: &mut BTreeMap<OsString, OsString>,
    ) -> io::Result<Self> {
        let deadline = launch_deadline(provenance.acknowledgement_timeout)?;
        for name in OBSOLETE {
            if std::env::var_os(name).is_some()
                || environment.contains_key(std::ffi::OsStr::new(name))
            {
                return Err(refusal("obsolete Windows launch marker"));
            }
        }
        let guards = guard_environment(|name| {
            environment
                .get(std::ffi::OsStr::new(name))
                .cloned()
                .or_else(|| std::env::var_os(name))
        })?;
        let tuple = LaunchTuple::Hosted(HostedTuple {
            parent: current_windows_process_instance()?,
            journal: provenance.journal.clone(),
            generation: provenance.generation,
            launch_id: provenance.launch_id.clone(),
            parent_launch_id: provenance.parent_launch_id.clone(),
            service: provenance.service,
            guards,
        });
        Self::prepare_tuple(tuple, deadline, environment)
    }

    pub(super) fn prepare_installed(
        request: &InstalledTaskLaunchRequest,
        environment: &mut BTreeMap<OsString, OsString>,
    ) -> io::Result<Self> {
        let deadline = launch_deadline(request.acknowledgement_timeout)?;
        if std::env::var_os(LAUNCH_ENV).is_some()
            || OBSOLETE.iter().any(|name| std::env::var_os(name).is_some())
        {
            return Err(refusal(
                "installed task forwarder cannot inherit launch admission",
            ));
        }
        let guards = solstone_core_installation_identity::service_guard_environment(&request.guard);
        let inherited = guard_environment(|name| std::env::var_os(name))?;
        if !inherited.is_empty() && inherited != guards {
            return Err(refusal("installed task inherited guard mismatch"));
        }
        let mut nonce = [0_u8; 24];
        getrandom::fill(&mut nonce).map_err(|error| io::Error::other(error.to_string()))?;
        let tuple = LaunchTuple::InstalledTask(InstalledTaskTuple {
            parent: current_windows_process_instance()?,
            journal: request.journal.clone(),
            launch_id: nonce.iter().map(|byte| format!("{byte:02x}")).collect(),
            guards: guards.clone(),
            arguments: request.arguments.clone(),
        });
        // Complete binding validation precedes the secret pipe or environment grant.
        validate_tuple(&tuple)?;
        environment.extend(
            guards
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        Self::prepare_tuple(tuple, deadline, environment)
    }

    fn prepare_tuple(
        tuple: LaunchTuple,
        deadline: Instant,
        environment: &mut BTreeMap<OsString, OsString>,
    ) -> io::Result<Self> {
        validate_tuple(&tuple)?;
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).map_err(|error| io::Error::other(error.to_string()))?;
        let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let descriptor = Descriptor {
            pipe: format!("{}{nonce}", launch_pipe::PIPE_PREFIX),
            expires_at_tick: tick()
                .checked_add(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis() as u64,
                )
                .ok_or_else(|| refusal("launch deadline overflows"))?,
            tuple,
        };
        let pipe = launch_pipe::create(&descriptor.pipe)?;
        // SAFETY: unnamed noninheritable manual-reset stop event, initially unset.
        #[allow(unsafe_code)]
        let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful creation returned unique ownership.
        #[allow(unsafe_code)]
        let stop = unsafe { OwnedHandle::from_raw_handle(raw) };
        environment.insert(
            LAUNCH_ENV.into(),
            serde_json::to_string(&descriptor)?.into(),
        );
        Ok(Self {
            descriptor,
            pipe,
            stop,
            deadline,
        })
    }

    pub(super) fn admit(
        self,
        child: &WindowsJobProcess,
        grants: &[ReadFileGrant],
        inherited_stop: Option<&AdmittedWindowsLaunch>,
    ) -> io::Result<OwnedHandle> {
        if matches!(self.descriptor.tuple, LaunchTuple::InstalledTask(_))
            && (!grants.is_empty() || inherited_stop.is_some())
        {
            return Err(refusal("installed task cannot receive hosted grants"));
        }
        let mut kinds = BTreeSet::new();
        for grant in grants {
            if !kinds.insert(grant.kind()) {
                return Err(refusal("duplicate launch grant kind"));
            }
        }
        loop {
            launch_io::connect(&self.pipe, self.deadline)?;
            if launch_pipe::peer(&self.pipe, false).is_ok_and(|peer| peer == child.identity()) {
                break;
            }
            launch_pipe::disconnect(&self.pipe)?;
        }
        // The retained root handle is the transfer target. No OpenProcess by PID,
        // parent process handle, Job handle, or inheritable duplicate is offered.
        let stop = duplicate_into(child, self.stop.as_handle(), SYNCHRONIZE)?;
        let mut transferred = Vec::new();
        for grant in grants {
            transferred.push(WireGrant {
                kind: grant.kind(),
                handle: duplicate_into(child, grant.file().as_handle(), FILE_GENERIC_READ)?,
            });
        }
        let offer = Offer {
            descriptor: self.descriptor.clone(),
            child: child.identity(),
            stop,
            grants: transferred,
        };
        launch_io::write_frame(&self.pipe, &serde_json::to_vec(&offer)?, self.deadline)?;
        let ack: Acknowledgement =
            serde_json::from_slice(&launch_io::read_frame(&self.pipe, self.deadline)?)?;
        if ack
            != (Acknowledgement {
                descriptor: self.descriptor,
                child: child.identity(),
            })
        {
            return Err(refusal("launch acknowledgement does not match the offer"));
        }
        // Forward an already-latched upstream stop before admitting this hop.
        if inherited_stop
            .map(AdmittedWindowsLaunch::stop_requested)
            .transpose()?
            .unwrap_or(false)
        {
            signal_stop(&self.stop)?;
        }
        // ACK is receipt of authority, not application readiness. A final commit
        // byte prevents a child beginning work before the parent accepts its ACK.
        launch_io::write_frame(&self.pipe, b"admit", self.deadline)?;
        Ok(self.stop)
    }
}

pub(super) fn signal_stop(stop: &OwnedHandle) -> io::Result<()> {
    // SAFETY: caller retains the parent event with EVENT_MODIFY_STATE rights.
    #[allow(unsafe_code)]
    if unsafe { SetEvent(stop.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn duplicate_into(
    child: &WindowsJobProcess,
    source: BorrowedHandle<'_>,
    rights: u32,
) -> io::Result<usize> {
    let mut target = std::ptr::null_mut();
    // SAFETY: both source and exact CreateProcessW root remain retained. The
    // numeric result belongs to that child, never to this parent's handle table.
    #[allow(unsafe_code)]
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            source.as_raw_handle(),
            child.root_handle().as_raw_handle(),
            &mut target,
            rights,
            0,
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(target as usize)
}

pub fn receive_windows_launch() -> Result<Option<AdmittedWindowsLaunch>, LaunchError> {
    let received = receive(None).map_err(|error| LaunchError::Admission(error.to_string()))?;
    received
        .map(|received| match received.tuple {
            LaunchTuple::Hosted(tuple) => Ok(AdmittedWindowsLaunch {
                tuple,
                stop: received.stop,
                grants: received.grants,
            }),
            LaunchTuple::InstalledTask(_) => Err(LaunchError::Admission(
                "installed task at hosted entry".into(),
            )),
        })
        .transpose()
}

pub fn receive_windows_installed_task_launch(
    expected: &InstalledTaskLaunchRequest,
) -> Result<AdmittedInstalledTaskLaunch, LaunchError> {
    let received = receive(Some(expected))
        .map_err(|error| LaunchError::Admission(error.to_string()))?
        .ok_or_else(|| LaunchError::Admission("missing installed task launch admission".into()))?;
    match received.tuple {
        LaunchTuple::InstalledTask(tuple) => Ok(AdmittedInstalledTaskLaunch {
            tuple,
            _stop: received.stop,
        }),
        LaunchTuple::Hosted(_) => Err(LaunchError::Admission(
            "hosted launch at installed task entry".into(),
        )),
    }
}

struct ReceivedLaunch {
    tuple: LaunchTuple,
    stop: Arc<OwnedHandle>,
    grants: Vec<ReadFileGrant>,
}

// These checks run after complete loaded-binding validation and before opening
// the pipe, or after authenticated offer comparison and before handle adoption.
fn validate_entry(
    tuple: &LaunchTuple,
    expected: Option<&InstalledTaskLaunchRequest>,
) -> io::Result<()> {
    match (tuple, expected) {
        (LaunchTuple::Hosted(_), None) => Ok(()),
        (LaunchTuple::InstalledTask(root), Some(expected))
            if root.journal == expected.journal
                && root.arguments == expected.arguments
                && root.guards
                    == solstone_core_installation_identity::service_guard_environment(
                        &expected.guard,
                    ) =>
        {
            Ok(())
        }
        _ => Err(refusal("launch variant or installed action mismatch")),
    }
}

fn validate_offered_grants(tuple: &LaunchTuple, grants: &[WireGrant]) -> io::Result<()> {
    if matches!(tuple, LaunchTuple::InstalledTask(_)) && !grants.is_empty() {
        return Err(refusal("installed task offer contains file grants"));
    }
    Ok(())
}

fn receive(expected: Option<&InstalledTaskLaunchRequest>) -> io::Result<Option<ReceivedLaunch>> {
    for name in OBSOLETE {
        if std::env::var_os(name).is_some() {
            return Err(refusal("obsolete Windows launch marker"));
        }
    }
    let guards = guard_environment(|name| std::env::var_os(name))?;
    let Some(value) = std::env::var_os(LAUNCH_ENV) else {
        return Ok(None);
    };
    launch_io::require_completed_cleanup()?;
    let value = value
        .into_string()
        .map_err(|_| refusal("non-Unicode Windows launch descriptor"))?;
    if value.len() > 16384 {
        return Err(refusal("oversized Windows launch descriptor"));
    }
    let descriptor: Descriptor = serde_json::from_str(&value)?;
    let suffix = descriptor
        .pipe
        .strip_prefix(launch_pipe::PIPE_PREFIX)
        .ok_or_else(|| refusal("invalid launch endpoint"))?;
    if suffix.len() != 48 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(refusal("invalid launch endpoint nonce"));
    }
    let remaining = descriptor
        .expires_at_tick
        .checked_sub(tick())
        .filter(|remaining| *remaining > 0 && *remaining <= 60_000)
        .ok_or_else(|| refusal("expired or invalid Windows launch deadline"))?;
    let deadline = Instant::now() + Duration::from_millis(remaining);
    if descriptor.tuple.guards() != &guards {
        return Err(refusal("launch guard tuple differs from inherited guard"));
    }
    validate_tuple(&descriptor.tuple)?;
    validate_entry(&descriptor.tuple, expected)?;
    launch_pipe::require_direct_parent(descriptor.tuple.parent())?;
    let pipe = launch_pipe::open(&descriptor.pipe, deadline)?;
    if launch_pipe::peer(&pipe, true)? != descriptor.tuple.parent() {
        return Err(refusal("launch pipe server identity mismatch"));
    }
    let offer: Offer = serde_json::from_slice(&launch_io::read_frame(&pipe, deadline)?)?;
    let child = current_windows_process_instance()?;
    if offer.descriptor != descriptor || offer.child != child {
        return Err(refusal("launch offer provenance mismatch"));
    }
    validate_offered_grants(&descriptor.tuple, &offer.grants)?;
    let mut kinds = BTreeSet::new();
    let mut handles = BTreeSet::new();
    handles.insert(offer.stop);
    for grant in &offer.grants {
        if !kinds.insert(grant.kind) || !handles.insert(grant.handle) {
            return Err(refusal("duplicate launch handle or grant kind"));
        }
    }
    // Handle values are adopted only after authenticating the server and tuple.
    // Partial adoption failures cause parent admission failure and exact Job reap.
    let stop = adopt(offer.stop, SYNCHRONIZE, "Event")?;
    let mut grants = Vec::new();
    for grant in offer.grants {
        let handle = adopt(grant.handle, FILE_GENERIC_READ, "File")?;
        grants.push(ReadFileGrant::from_received_file(
            grant.kind,
            std::fs::File::from(handle),
        )?);
    }
    let acknowledgement = Acknowledgement {
        descriptor: descriptor.clone(),
        child,
    };
    launch_io::write_frame(&pipe, &serde_json::to_vec(&acknowledgement)?, deadline)?;
    if launch_io::read_frame(&pipe, deadline)? != b"admit" {
        return Err(refusal("launch admission commit mismatch"));
    }
    Ok(Some(ReceivedLaunch {
        tuple: descriptor.tuple,
        stop: Arc::new(stop),
        grants,
    }))
}

fn adopt(value: usize, rights: u32, object_type: &str) -> io::Result<OwnedHandle> {
    use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE};
    if value == 0 || value >= usize::MAX - 16 {
        return Err(refusal("invalid transferred handle"));
    }
    let mut flags = 0;
    // SAFETY: GetHandleInformation accepts an untrusted scalar and refuses an
    // invalid handle; no pointer derived from the value is dereferenced by Rust.
    #[allow(unsafe_code)]
    if unsafe { GetHandleInformation(value as HANDLE, &mut flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: this handle was transferred exclusively to us by the authenticated
    // parent; duplicate values were refused and validity was queried above.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(value as HANDLE) };
    require_handle_access(handle.as_handle(), rights, object_type)?;
    Ok(handle)
}

#[cfg(test)]
#[path = "launch_control_tests.rs"]
mod installed_task_controls;
