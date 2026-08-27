// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-scoped installation identity records.
//!
//! This crate deliberately owns the small persistence protocol rather than
//! depending on a journal crate: the protocol's no-follow traversal, ownership,
//! exact-mode, and durability requirements apply to its owner-wide storage
//! directory, not to a journal tree.
//! Unix directory synchronization is durable through `fsync`. Windows has no
//! reliably documented directory-handle flush guarantee, so its backend flushes
//! files before publication and uses write-through replacement, while directory
//! synchronization is an explicitly documented no-op.

use std::collections::{BTreeMap, HashMap};
#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};

use getrandom::fill;
#[cfg(unix)]
use nix::dir::Dir;
#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{AtFlags, Flock, FlockArg, OFlag, open, openat, renameat};
#[cfg(unix)]
use nix::sys::stat::{FchmodatFlags, Mode, SFlag, fchmod, fchmodat, fstat, mkdirat};
#[cfg(unix)]
use nix::unistd::{Uid, UnlinkatFlags, linkat, unlinkat};
use sha2::{Digest, Sha256};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    ERROR_FILE_EXISTS, ERROR_LOCK_VIOLATION, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, CreateHardLinkW, DeleteFileW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TRAVERSE, FileAttributeTagInfo, FileIdInfo,
    GetFileInformationByHandleEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_ALWAYS, OPEN_EXISTING,
    UnlockFileEx,
};
#[cfg(windows)]
use windows_sys::Win32::System::Com::CoTaskMemFree;
#[cfg(windows)]
use windows_sys::Win32::System::IO::OVERLAPPED;
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};

const MAX_RECORD_BYTES: usize = 24 * 1024;
const MAX_TOKEN_BYTES: usize = 4096;
const MAX_NAMESPACES: usize = 1024;
const MAX_REGISTRY_BYTES: u64 = 16 * 1024 * 1024;
const MARKER_BYTES: &[u8] = b"solstone-installation-identity-marker-v1\n";
const NAMESPACE_DOMAIN: &[u8] = b"solstone-installation-identity-namespace-v1\0";
const RECORD_DOMAIN: &[u8] = b"solstone-installation-identity-record-v1\0";

/// OS family used in the namespace hash and record.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PlatformTag {
    Linux,
    Macos,
    Windows,
}

impl PlatformTag {
    /// The fixed lowercase protocol tag.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }

    /// The platform supported by this build.
    pub const fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            Self::Macos
        }
        #[cfg(windows)]
        {
            Self::Windows
        }
    }

    fn parse(value: &str) -> Result<Self, RecordError> {
        match value {
            "linux" => Ok(Self::Linux),
            "macos" => Ok(Self::Macos),
            "windows" => Ok(Self::Windows),
            _ => Err(RecordError::InvalidField("platform")),
        }
    }
}

impl fmt::Display for PlatformTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Canonical raw absolute path bytes for an installation root.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RootToken(Vec<u8>);

impl RootToken {
    /// Validates canonical raw absolute pathname bytes for this build target.
    pub fn from_raw_absolute(bytes: Vec<u8>) -> Result<Self, IdentityError> {
        validate_absolute_token(&bytes, "root token")?;
        Ok(Self(bytes))
    }

    /// Returns the exact raw pathname bytes used by the protocol.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Canonical raw absolute path bytes for a journal.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct JournalToken(Vec<u8>);

impl JournalToken {
    /// Validates canonical raw absolute pathname bytes for this build target.
    pub fn from_raw_absolute(bytes: Vec<u8>) -> Result<Self, IdentityError> {
        validate_absolute_token(&bytes, "journal token")?;
        Ok(Self(bytes))
    }

    /// Returns the exact raw pathname bytes used by the protocol.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Reconstructs the raw absolute path represented by this token.
    pub fn to_path_buf(&self) -> PathBuf {
        #[cfg(unix)]
        {
            PathBuf::from(OsString::from_vec(self.0.clone()))
        }
        #[cfg(windows)]
        {
            let units = windows_units_from_bytes(&self.0).expect("validated Windows journal token");
            PathBuf::from(OsString::from_wide(&units))
        }
    }
}

/// The SHA-256 namespace key for a platform/root pair.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NamespaceName([u8; 32]);

impl NamespaceName {
    /// Parses the fixed 64-character lowercase-hex namespace spelling.
    pub fn parse(value: &str) -> Result<Self, IdentityError> {
        let bytes = parse_lower_hex(value, 32, "namespace")?;
        let mut output = [0_u8; 32];
        output.copy_from_slice(&bytes);
        Ok(Self(output))
    }

    /// Returns the fixed lowercase-hex namespace spelling.
    pub fn as_hex(&self) -> String {
        lower_hex(&self.0)
    }
}

impl fmt::Display for NamespaceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

/// A 128-bit installation ID generated only by the OS CSPRNG.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InstallationId([u8; 16]);

impl InstallationId {
    /// Parses the fixed 32-character lowercase-hex installation ID.
    pub fn parse(value: &str) -> Result<Self, IdentityError> {
        let bytes = parse_lower_hex(value, 16, "installation id")?;
        let mut output = [0_u8; 16];
        output.copy_from_slice(&bytes);
        Ok(Self(output))
    }

    /// Returns the fixed lowercase-hex installation ID.
    pub fn as_hex(&self) -> String {
        lower_hex(&self.0)
    }
}

impl fmt::Display for InstallationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

/// Monotonic installation generation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Generation(u64);

impl Generation {
    /// Creates a generation. Generation zero is not a valid persisted value.
    pub fn new(value: u64) -> Result<Self, IdentityError> {
        if value == 0 {
            return Err(IdentityError::InvalidInput("generation must be nonzero"));
        }
        Ok(Self(value))
    }

    /// Returns the decimal generation value.
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, IdentityError> {
        Self::new(
            self.0
                .checked_add(1)
                .ok_or(IdentityError::InvalidInput("generation overflow"))?,
        )
    }
}

/// Durable lifecycle state of a record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleState {
    Prepared,
    Adopted,
    Tombstoned,
}

impl LifecycleState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Adopted => "adopted",
            Self::Tombstoned => "tombstoned",
        }
    }

    fn parse(value: &str) -> Result<Self, RecordError> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "adopted" => Ok(Self::Adopted),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(RecordError::InvalidField("state")),
        }
    }
}

/// The stored identity record, excluding its derived checksum line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityRecord {
    pub state: LifecycleState,
    pub generation: Generation,
    pub id: InstallationId,
    pub platform: PlatformTag,
    pub root_token: RootToken,
    pub journal_token: JournalToken,
}

/// Identity fields used to bind wrappers and service units to a root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallationBinding {
    pub namespace: NamespaceName,
    pub id: InstallationId,
    pub generation: Generation,
    pub platform: PlatformTag,
    pub root_token: RootToken,
    pub journal_token: JournalToken,
}

impl InstallationBinding {
    fn from_record(namespace: NamespaceName, record: &IdentityRecord) -> Self {
        Self {
            namespace,
            id: record.id.clone(),
            generation: record.generation,
            platform: record.platform,
            root_token: record.root_token.clone(),
            journal_token: record.journal_token.clone(),
        }
    }
}

/// Owner-specific location of the provider's storage base.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerBase {
    home: PathBuf,
    platform: PlatformTag,
}

impl OwnerBase {
    /// Constructs an owner base from a caller-supplied absolute home directory on Unix.
    ///
    /// This is useful for adapters and isolated callers; production callers normally
    /// use [`owner_base`]. On Windows, the argument is deliberately ignored and
    /// the current user's LocalAppData known folder is used instead. The base
    /// itself is never created by the provider.
    pub fn at_home(home: PathBuf, platform: PlatformTag) -> Result<Self, IdentityError> {
        #[cfg(windows)]
        {
            let _ = home;
            if platform != PlatformTag::Windows {
                return Err(IdentityError::InvalidInput(
                    "owner platform does not match this Windows build",
                ));
            }
            return Ok(Self {
                home: known_folder_local_app_data()?,
                platform,
            });
        }
        #[cfg(unix)]
        if !home.is_absolute() {
            return Err(IdentityError::InvalidInput("home must be absolute"));
        }
        #[cfg(unix)]
        Ok(Self { home, platform })
    }

    /// Returns the resolved path of the provider base.
    pub fn path(&self) -> PathBuf {
        let mut path = self.home.clone();
        for segment in base_segments(self.platform) {
            path.push(segment);
        }
        path
    }

    /// Returns the protocol platform for this owner base.
    pub const fn platform(&self) -> PlatformTag {
        self.platform
    }
}

/// Evidence from a legacy Python-written setup manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyManifestEvidence {
    Absent,
    ValidProviderlessSchemaV1,
    Malformed,
    Unreadable,
}

/// The caller's read-only classification of current wrapper/service artifacts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArtifactBindingEvidence {
    /// No managed artifact exists for this setup.
    Fresh,
    /// Previously managed artifacts exist but predate identity guard fields.
    LegacyUnguarded,
    /// Every present guard agrees on the installation identity; journal selection may lag.
    Guarded(GuardFields),
    /// A managed-looking artifact is bound to another installation.
    Foreign,
    /// Artifact guard syntax or fields are malformed.
    Malformed,
    /// Present artifacts cannot be assigned one unambiguous binding.
    Ambiguous,
}

/// Request for setup admission. Artifact and legacy evidence are read-only facts
/// collected by the CLI adapter before it invokes this provider.
#[derive(Clone, Debug)]
pub struct SetupAdmissionRequest {
    pub owner: OwnerBase,
    pub root_token: RootToken,
    pub journal_token: JournalToken,
    /// Only an explicit CLI/environment journal selection may update an existing record.
    pub journal_is_explicit: bool,
    pub legacy_manifest: LegacyManifestEvidence,
    pub artifacts: ArtifactBindingEvidence,
}

/// Successful setup admission. Holding this value holds owner then namespace locks.
#[derive(Debug)]
pub struct SetupAdmission {
    binding: InstallationBinding,
    effective_journal: JournalToken,
    _lease: NamespaceLease,
}

impl SetupAdmission {
    /// Binding that wrappers and service units must serialize.
    pub fn binding(&self) -> &InstallationBinding {
        &self.binding
    }

    /// Journal selected for this invocation after implicit-selection preservation.
    pub fn effective_journal(&self) -> &JournalToken {
        &self.effective_journal
    }
}

/// Request for a clean-uninstall admission.
#[derive(Clone, Debug)]
pub struct CleanUninstallRequest {
    pub owner: OwnerBase,
    pub root_token: RootToken,
    pub artifacts: ArtifactBindingEvidence,
}

/// The root-specific work that a caller may do after uninstall admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanUninstallPlan {
    pub binding: InstallationBinding,
    pub remove_owner_config: bool,
    pub remove_journal_manifest: bool,
    pub already_tombstoned: bool,
}

/// Successful clean-uninstall admission. It owns the locks until it is dropped
/// or [`CleanUninstallSession::commit_tombstone`] succeeds.
#[derive(Debug)]
pub struct CleanUninstallSession {
    plan: CleanUninstallPlan,
    namespace: SecureDir,
    _lease: NamespaceLease,
}

impl CleanUninstallSession {
    /// Returns the precomputed all-or-nothing cleanup plan.
    pub fn plan(&self) -> &CleanUninstallPlan {
        &self.plan
    }

    /// Commits the provider tombstone after all caller-owned destructive work succeeds.
    pub fn commit_tombstone(self) -> Result<(), IdentityError> {
        if self.plan.already_tombstoned {
            return Ok(());
        }
        let record = IdentityRecord {
            state: LifecycleState::Tombstoned,
            generation: self.plan.binding.generation,
            id: self.plan.binding.id.clone(),
            platform: self.plan.binding.platform,
            root_token: self.plan.binding.root_token.clone(),
            journal_token: self.plan.binding.journal_token.clone(),
        };
        replace_record(&self.namespace, &record, StageKind::Update)?;
        Ok(())
    }
}

/// The four root-binding fields serialized into wrappers and service environments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardFields {
    pub namespace: NamespaceName,
    pub id: InstallationId,
    pub generation: Generation,
    pub journal_token: JournalToken,
}

impl GuardFields {
    /// Creates guard fields from an admitted binding.
    pub fn from_binding(binding: &InstallationBinding) -> Self {
        Self {
            namespace: binding.namespace.clone(),
            id: binding.id.clone(),
            generation: binding.generation,
            journal_token: binding.journal_token.clone(),
        }
    }

    /// Returns whether two guards bind the same installation, apart from journal selection.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.namespace == other.namespace
            && self.id == other.id
            && self.generation == other.generation
    }

    fn matches_identity(&self, binding: &InstallationBinding) -> bool {
        self.same_identity(&Self::from_binding(binding))
    }

    fn matches(&self, binding: &InstallationBinding) -> bool {
        self.namespace == binding.namespace
            && self.id == binding.id
            && self.generation == binding.generation
            && self.journal_token == binding.journal_token
    }
}

#[cfg(unix)]
type IdentityLock = Flock<File>;

#[cfg(windows)]
type IdentityLock = WindowsLockGuard;

#[derive(Debug)]
struct NamespaceLease {
    _owner_lock: IdentityLock,
    _marker_lock: IdentityLock,
    _namespace: SecureDir,
    // Struct fields drop in declaration order. Keep the process-local outer
    // guard last so it remains held until every file and namespace lock drops.
    _owner_coordinator: OwnerCoordinatorGuard,
}

/// Provider failures, including unsafe storage states that require repair.
#[derive(Debug)]
pub enum IdentityError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    InvalidInput(&'static str),
    UnsafeState(&'static str),
    AdmissionRefused(&'static str),
    /// An existing identity record is not in the adopted lifecycle state.
    NotAdopted(&'static str),
    Record(RecordError),
    Guard(GuardError),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::InvalidInput(message)
            | Self::UnsafeState(message)
            | Self::AdmissionRefused(message)
            | Self::NotAdopted(message) => formatter.write_str(message),
            Self::Record(error) => error.fmt(formatter),
            Self::Guard(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for IdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Record(error) => Some(error),
            Self::Guard(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RecordError> for IdentityError {
    fn from(error: RecordError) -> Self {
        Self::Record(error)
    }
}

impl From<GuardError> for IdentityError {
    fn from(error: GuardError) -> Self {
        Self::Guard(error)
    }
}

/// Canonical-record decoding failures.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum RecordError {
    NotAscii,
    TooLarge,
    WrongLineCount,
    InvalidField(&'static str),
    ChecksumMismatch,
}

impl fmt::Display for RecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAscii => formatter.write_str("identity record is not ASCII"),
            Self::TooLarge => formatter.write_str("identity record exceeds 24 KiB"),
            Self::WrongLineCount => {
                formatter.write_str("identity record is not exactly eight LF lines")
            }
            Self::InvalidField(field) => write!(formatter, "invalid identity record {field}"),
            Self::ChecksumMismatch => formatter.write_str("identity record checksum mismatch"),
        }
    }
}

impl std::error::Error for RecordError {}

/// Guard serialization or parsing failures.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum GuardError {
    Missing,
    Duplicate(&'static str),
    Invalid(&'static str),
}

impl fmt::Display for GuardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("identity guard fields are incomplete"),
            Self::Duplicate(field) => write!(formatter, "duplicate identity guard {field}"),
            Self::Invalid(field) => write!(formatter, "invalid identity guard {field}"),
        }
    }
}

impl std::error::Error for GuardError {}

/// Computes the exact domain-separated namespace hash.
pub fn namespace_name(platform: PlatformTag, root_token: &RootToken) -> NamespaceName {
    let mut hasher = Sha256::new();
    hasher.update(NAMESPACE_DOMAIN);
    let platform = platform.as_str().as_bytes();
    hasher.update((platform.len() as u32).to_be_bytes());
    hasher.update(platform);
    hasher.update((root_token.0.len() as u32).to_be_bytes());
    hasher.update(&root_token.0);
    let mut output = [0_u8; 32];
    output.copy_from_slice(&hasher.finalize());
    NamespaceName(output)
}

/// Encodes a record into the exact canonical eight-line ASCII wire format.
pub fn encode_record(record: &IdentityRecord) -> Result<Vec<u8>, RecordError> {
    validate_absolute_token(&record.root_token.0, "root token")
        .map_err(|_| RecordError::InvalidField("root_token"))?;
    validate_absolute_token(&record.journal_token.0, "journal token")
        .map_err(|_| RecordError::InvalidField("journal_token"))?;
    let mut data =
        Vec::with_capacity(512 + 4 * (record.root_token.0.len() + record.journal_token.0.len()));
    data.extend_from_slice(b"schema_version=1\nstate=");
    data.extend_from_slice(record.state.as_str().as_bytes());
    data.extend_from_slice(b"\ngeneration=");
    data.extend_from_slice(record.generation.0.to_string().as_bytes());
    data.extend_from_slice(b"\nid=");
    data.extend_from_slice(record.id.as_hex().as_bytes());
    data.extend_from_slice(b"\nplatform=");
    data.extend_from_slice(record.platform.as_str().as_bytes());
    data.extend_from_slice(b"\nroot_token=");
    data.extend_from_slice(lower_hex(&record.root_token.0).as_bytes());
    data.extend_from_slice(b"\njournal_token=");
    data.extend_from_slice(lower_hex(&record.journal_token.0).as_bytes());
    data.push(b'\n');
    let mut hasher = Sha256::new();
    hasher.update(RECORD_DOMAIN);
    hasher.update(&data);
    data.extend_from_slice(b"checksum=");
    data.extend_from_slice(lower_hex(&hasher.finalize()).as_bytes());
    data.push(b'\n');
    if data.len() > MAX_RECORD_BYTES {
        return Err(RecordError::TooLarge);
    }
    Ok(data)
}

/// Decodes and verifies a canonical identity record.
pub fn decode_record(bytes: &[u8]) -> Result<IdentityRecord, RecordError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(RecordError::TooLarge);
    }
    if !bytes.is_ascii() {
        return Err(RecordError::NotAscii);
    }
    let lines: Vec<&[u8]> = bytes.split_inclusive(|byte| *byte == b'\n').collect();
    if lines.len() != 8 || lines.iter().any(|line| !line.ends_with(b"\n")) {
        return Err(RecordError::WrongLineCount);
    }
    let values = [
        field(lines[0], b"schema_version=", "schema_version")?,
        field(lines[1], b"state=", "state")?,
        field(lines[2], b"generation=", "generation")?,
        field(lines[3], b"id=", "id")?,
        field(lines[4], b"platform=", "platform")?,
        field(lines[5], b"root_token=", "root_token")?,
        field(lines[6], b"journal_token=", "journal_token")?,
        field(lines[7], b"checksum=", "checksum")?,
    ];
    if values[0] != b"1" {
        return Err(RecordError::InvalidField("schema_version"));
    }
    let state = LifecycleState::parse(as_ascii(values[1], "state")?)?;
    let generation_text = as_ascii(values[2], "generation")?;
    if generation_text.is_empty()
        || (generation_text.len() > 1 && generation_text.starts_with('0'))
        || !generation_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(RecordError::InvalidField("generation"));
    }
    let generation = Generation::new(
        generation_text
            .parse()
            .map_err(|_| RecordError::InvalidField("generation"))?,
    )
    .map_err(|_| RecordError::InvalidField("generation"))?;
    let id = InstallationId::parse(as_ascii(values[3], "id")?)
        .map_err(|_| RecordError::InvalidField("id"))?;
    let platform = PlatformTag::parse(as_ascii(values[4], "platform")?)?;
    let root_token = RootToken::from_raw_absolute(
        parse_lower_hex(
            as_ascii(values[5], "root_token")?,
            values[5].len() / 2,
            "root_token",
        )
        .map_err(|_| RecordError::InvalidField("root_token"))?,
    )
    .map_err(|_| RecordError::InvalidField("root_token"))?;
    let journal_token = JournalToken::from_raw_absolute(
        parse_lower_hex(
            as_ascii(values[6], "journal_token")?,
            values[6].len() / 2,
            "journal_token",
        )
        .map_err(|_| RecordError::InvalidField("journal_token"))?,
    )
    .map_err(|_| RecordError::InvalidField("journal_token"))?;
    let checksum = parse_lower_hex(as_ascii(values[7], "checksum")?, 32, "checksum")
        .map_err(|_| RecordError::InvalidField("checksum"))?;
    let mut hasher = Sha256::new();
    hasher.update(RECORD_DOMAIN);
    hasher.update(&bytes[..lines[..7].iter().map(|line| line.len()).sum::<usize>()]);
    if checksum.as_slice() != hasher.finalize().as_slice() {
        return Err(RecordError::ChecksumMismatch);
    }
    Ok(IdentityRecord {
        state,
        generation,
        id,
        platform,
        root_token,
        journal_token,
    })
}

/// Returns the current user's platform-specific owner base without creating it.
pub fn owner_base() -> Result<OwnerBase, IdentityError> {
    #[cfg(unix)]
    {
        let home = env::var_os("HOME").ok_or(IdentityError::InvalidInput("HOME is not set"))?;
        OwnerBase::at_home(PathBuf::from(home), PlatformTag::current())
    }
    #[cfg(windows)]
    {
        OwnerBase::at_home(PathBuf::new(), PlatformTag::current())
    }
}

/// Canonicalizes an existing root and converts it to protocol bytes.
pub fn root_token_from_path(path: &Path) -> Result<RootToken, IdentityError> {
    #[cfg(unix)]
    {
        let canonical =
            std::fs::canonicalize(path).map_err(|source| io_error("canonicalize root", source))?;
        RootToken::from_raw_absolute(normalize_absolute_bytes(canonical.as_os_str().as_bytes())?)
    }
    #[cfg(windows)]
    {
        let canonical =
            std::fs::canonicalize(path).map_err(|source| io_error("canonicalize root", source))?;
        RootToken::from_raw_absolute(normalize_windows_canonical_path(&canonical)?)
    }
}

/// Lexically normalizes an absolute journal path and converts it to protocol bytes.
pub fn journal_token_from_path(path: &Path) -> Result<JournalToken, IdentityError> {
    #[cfg(unix)]
    {
        JournalToken::from_raw_absolute(normalize_absolute_bytes(path.as_os_str().as_bytes())?)
    }
    #[cfg(windows)]
    {
        JournalToken::from_raw_absolute(normalize_absolute_bytes(&wide_bytes(path.as_os_str()))?)
    }
}

/// Serializes the four guard comment lines appended to each managed wrapper.
pub fn wrapper_guard_lines(fields: &GuardFields) -> String {
    format!(
        "# solstone-installation-namespace: {}\n# solstone-installation-id: {}\n# solstone-installation-generation: {}\n# solstone-installation-journal-token: {}\n",
        fields.namespace,
        fields.id,
        fields.generation.0,
        lower_hex(fields.journal_token.as_bytes()),
    )
}

/// Parses guard fields from a managed wrapper. No guard lines returns `Ok(None)`;
/// partial or duplicate guards fail closed.
pub fn parse_wrapper_guard(wrapper: &str) -> Result<Option<GuardFields>, GuardError> {
    let mut values = BTreeMap::new();
    for line in wrapper.lines() {
        let Some(line) = line.strip_prefix("# solstone-installation-") else {
            continue;
        };
        let (key, value) = line.split_once(": ").ok_or(GuardError::Invalid("syntax"))?;
        if !matches!(key, "namespace" | "id" | "generation" | "journal-token") {
            return Err(GuardError::Invalid("unknown"));
        }
        if values.insert(key, value).is_some() {
            return Err(GuardError::Duplicate(match key {
                "namespace" => "namespace",
                "id" => "id",
                "generation" => "generation",
                _ => "journal-token",
            }));
        }
    }
    if values.is_empty() {
        return Ok(None);
    }
    let namespace = NamespaceName::parse(values.get("namespace").ok_or(GuardError::Missing)?)
        .map_err(|_| GuardError::Invalid("namespace"))?;
    let id = InstallationId::parse(values.get("id").ok_or(GuardError::Missing)?)
        .map_err(|_| GuardError::Invalid("id"))?;
    let generation = parse_generation(values.get("generation").ok_or(GuardError::Missing)?)
        .map_err(|_| GuardError::Invalid("generation"))?;
    let journal_token = JournalToken::from_raw_absolute(
        parse_lower_hex(
            values.get("journal-token").ok_or(GuardError::Missing)?,
            values
                .get("journal-token")
                .ok_or(GuardError::Missing)?
                .len()
                / 2,
            "journal-token",
        )
        .map_err(|_| GuardError::Invalid("journal-token"))?,
    )
    .map_err(|_| GuardError::Invalid("journal-token"))?;
    Ok(Some(GuardFields {
        namespace,
        id,
        generation,
        journal_token,
    }))
}

/// Returns service-unit environment entries for an admitted binding.
pub fn service_guard_environment(fields: &GuardFields) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "SOLSTONE_INSTALLATION_NAMESPACE".to_owned(),
            fields.namespace.as_hex(),
        ),
        ("SOLSTONE_INSTALLATION_ID".to_owned(), fields.id.as_hex()),
        (
            "SOLSTONE_INSTALLATION_GENERATION".to_owned(),
            fields.generation.0.to_string(),
        ),
        (
            "SOLSTONE_INSTALLATION_JOURNAL_TOKEN".to_owned(),
            lower_hex(fields.journal_token.as_bytes()),
        ),
    ])
}

/// Parses the four service-unit guard environment values. Missing all entries is
/// `Ok(None)`; partial, duplicate, or malformed entries fail closed.
pub fn parse_service_guard_environment(
    environment: &BTreeMap<String, String>,
) -> Result<Option<GuardFields>, GuardError> {
    let names = [
        "SOLSTONE_INSTALLATION_NAMESPACE",
        "SOLSTONE_INSTALLATION_ID",
        "SOLSTONE_INSTALLATION_GENERATION",
        "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
    ];
    let present = names
        .iter()
        .filter(|name| environment.contains_key(**name))
        .count();
    if present == 0 {
        return Ok(None);
    }
    if present != names.len() {
        return Err(GuardError::Missing);
    }
    let namespace = NamespaceName::parse(&environment[names[0]])
        .map_err(|_| GuardError::Invalid("namespace"))?;
    let id =
        InstallationId::parse(&environment[names[1]]).map_err(|_| GuardError::Invalid("id"))?;
    let generation =
        parse_generation(&environment[names[2]]).map_err(|_| GuardError::Invalid("generation"))?;
    let journal_token = JournalToken::from_raw_absolute(
        parse_lower_hex(
            &environment[names[3]],
            environment[names[3]].len() / 2,
            "journal-token",
        )
        .map_err(|_| GuardError::Invalid("journal-token"))?,
    )
    .map_err(|_| GuardError::Invalid("journal-token"))?;
    Ok(Some(GuardFields {
        namespace,
        id,
        generation,
        journal_token,
    }))
}

/// Loads the adopted installation binding for an existing owner/root without
/// modifying identity storage.
///
/// The owner lock serializes record changes while the namespace is read. The
/// adoption-marker lock is then acquired as a secondary validation before the
/// binding is returned.
pub fn load_installation_binding(
    owner: &OwnerBase,
    root_token: &RootToken,
) -> Result<InstallationBinding, IdentityError> {
    let namespace_name = namespace_name(owner.platform(), root_token);
    let provider = open_provider(owner, false)?;
    let owner_coordinator = acquire_owner_coordinator(owner_coordinator_key(&provider.file)?)?;
    let owner_lock = lock_existing_owner(&provider)?;
    let namespace = open_namespace(&provider, &namespace_name, false)?;
    let snapshot = read_namespace(&namespace, namespace_name.clone())?;
    let record = snapshot
        .record
        .ok_or(IdentityError::UnsafeState("namespace record is missing"))?;
    match record.state {
        LifecycleState::Prepared => {
            return Err(IdentityError::NotAdopted(
                "installation identity record is prepared",
            ));
        }
        LifecycleState::Tombstoned => {
            return Err(IdentityError::NotAdopted(
                "installation identity record is tombstoned",
            ));
        }
        LifecycleState::Adopted => {}
    }
    if record.platform != owner.platform() || record.root_token != *root_token {
        return Err(IdentityError::AdmissionRefused(
            "record does not bind the requested root",
        ));
    }
    let marker_lock = lock_existing_marker(&namespace)?;
    hit(FaultPoint::LoadLocksHeld)?;
    let binding = InstallationBinding::from_record(namespace_name, &record);
    drop(marker_lock);
    drop(owner_lock);
    drop(owner_coordinator);
    Ok(binding)
}

/// Admits setup and returns a lease held across all following setup mutations.
///
/// The initial no-replace publication intentionally occurs after the contender
/// releases its discovery lock. That makes filesystem no-replace publication,
/// rather than advisory-lock timing, the convergence primitive for independent
/// first-admission contenders. The winning or retrying contender reacquires the
/// owner lock before it creates the marker, adopts, and returns this session.
pub fn admit_setup(request: SetupAdmissionRequest) -> Result<SetupAdmission, IdentityError> {
    validate_setup_evidence(&request.artifacts, request.legacy_manifest)?;
    let namespace_name = namespace_name(request.owner.platform, &request.root_token);
    loop {
        let provider = open_provider(&request.owner, true)?;
        let owner_coordinator = acquire_owner_coordinator(owner_coordinator_key(&provider.file)?)?;
        let owner_lock = lock_owner(&provider)?;
        let registry = enumerate_registry(&provider)?;
        if let Some(snapshot) = registry
            .get(&namespace_name)
            .filter(|snapshot| snapshot.record.is_some())
        {
            let namespace = open_namespace(&provider, &namespace_name, false)?;
            return admit_existing_setup(
                request.clone(),
                namespace_name,
                namespace,
                snapshot.clone(),
                owner_coordinator,
                owner_lock,
                &registry,
            );
        }

        validate_initial_evidence(&request)?;
        let namespace = open_namespace(&provider, &namespace_name, true)?;
        remove_stale_stages(&namespace)?;
        let prepared = IdentityRecord {
            state: LifecycleState::Prepared,
            generation: Generation::new(1)?,
            id: generate_id()?,
            platform: request.owner.platform,
            root_token: request.root_token.clone(),
            journal_token: request.journal_token.clone(),
        };
        let stage = write_stage(&namespace, StageKind::Prepared, &prepared)?;
        // See the function-level documentation: do not make the cross-process
        // owner-file lock the first-writer arbitration mechanism. Keep the
        // process-local coordinator through publication so another thread in
        // this process cannot mistake this live stage for stale state on
        // platforms whose file locks are process-scoped.
        drop(owner_lock);
        match publish_prepared(&namespace, stage) {
            Ok(()) => continue,
            Err(PublishPreparedError::AlreadyExists) => continue,
            Err(PublishPreparedError::Identity(error)) => return Err(error),
        }
    }
}

/// Admits a clean uninstall and calculates the owner-wide cleanup tail before
/// any caller-owned service, wrapper, config, or manifest deletion occurs.
pub fn admit_clean_uninstall(
    request: CleanUninstallRequest,
) -> Result<CleanUninstallSession, IdentityError> {
    if matches!(
        request.artifacts,
        ArtifactBindingEvidence::Malformed | ArtifactBindingEvidence::Ambiguous
    ) {
        return Err(IdentityError::AdmissionRefused(
            "artifact binding is malformed or ambiguous",
        ));
    }
    let namespace_name = namespace_name(request.owner.platform, &request.root_token);
    let provider = open_provider(&request.owner, true)?;
    let owner_coordinator = acquire_owner_coordinator(owner_coordinator_key(&provider.file)?)?;
    let owner_lock = lock_owner(&provider)?;
    let registry = enumerate_registry(&provider)?;
    let snapshot = registry
        .get(&namespace_name)
        .ok_or(IdentityError::AdmissionRefused(
            "installation identity does not exist",
        ))?
        .clone();
    let namespace = open_namespace(&provider, &namespace_name, false)?;
    let record = snapshot
        .record
        .ok_or(IdentityError::UnsafeState("namespace record is missing"))?;
    if record.platform != request.owner.platform || record.root_token != request.root_token {
        return Err(IdentityError::AdmissionRefused(
            "record does not bind the requested root",
        ));
    }
    if !snapshot.marker {
        return Err(IdentityError::UnsafeState(
            "committed record has no adoption marker",
        ));
    }
    if record.state == LifecycleState::Prepared {
        return Err(IdentityError::AdmissionRefused(
            "prepared identity cannot be uninstalled",
        ));
    }
    let binding = InstallationBinding::from_record(namespace_name.clone(), &record);
    match &request.artifacts {
        ArtifactBindingEvidence::Guarded(fields) if !fields.matches(&binding) => {
            return Err(IdentityError::AdmissionRefused(
                "artifact guard does not match identity",
            ));
        }
        ArtifactBindingEvidence::LegacyUnguarded => {
            return Err(IdentityError::AdmissionRefused(
                "artifact guard is not a complete matching identity",
            ));
        }
        _ => {}
    }
    let marker_lock = lock_existing_marker(&namespace)?;
    let others: Vec<&NamespaceSnapshot> = registry
        .values()
        .filter(|other| {
            other.namespace != namespace_name
                && other
                    .record
                    .as_ref()
                    .is_some_and(|candidate| candidate.state == LifecycleState::Adopted)
        })
        .collect();
    let remove_owner_config = others.is_empty();
    let remove_journal_manifest = !others.iter().any(|other| {
        other
            .record
            .as_ref()
            .is_some_and(|candidate| candidate.journal_token == record.journal_token)
    });
    let lease = NamespaceLease {
        _owner_lock: owner_lock,
        _marker_lock: marker_lock,
        _namespace: namespace.try_clone()?,
        _owner_coordinator: owner_coordinator,
    };
    Ok(CleanUninstallSession {
        plan: CleanUninstallPlan {
            binding,
            remove_owner_config,
            remove_journal_manifest,
            already_tombstoned: record.state == LifecycleState::Tombstoned,
        },
        namespace,
        _lease: lease,
    })
}

#[derive(Clone, Debug)]
struct NamespaceSnapshot {
    namespace: NamespaceName,
    record: Option<IdentityRecord>,
    marker: bool,
    bytes: u64,
}

#[derive(Debug)]
struct SecureDir {
    file: File,
    path: PathBuf,
}

impl SecureDir {
    fn try_clone(&self) -> Result<Self, IdentityError> {
        Ok(Self {
            file: self
                .file
                .try_clone()
                .map_err(|source| io_error("clone directory descriptor", source))?,
            path: self.path.clone(),
        })
    }

    fn sync(&self) -> Result<(), IdentityError> {
        #[cfg(unix)]
        {
            self.file
                .sync_all()
                .map_err(|source| io_error("sync directory", source))
        }
        #[cfg(windows)]
        {
            // Windows does not provide a reliably documented directory-handle
            // flush guarantee. File contents are flushed before publication and
            // replacement uses MOVEFILE_WRITE_THROUGH; this is intentionally not
            // presented as an equivalent to Unix directory fsync.
            Ok(())
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum OwnerCoordinatorKey {
    Unix {
        device: u64,
        inode: u64,
    },
    Windows {
        volume_serial: u32,
        file_id: [u8; 16],
    },
}

#[derive(Debug)]
struct OwnerCoordinatorState {
    occupied: bool,
    waiters: usize,
}

#[derive(Debug)]
struct OwnerCoordinator {
    state: Mutex<OwnerCoordinatorState>,
    condvar: Condvar,
}

#[derive(Debug)]
struct OwnerCoordinatorGuard {
    coordinator: Arc<OwnerCoordinator>,
}

static OWNER_COORDINATORS: OnceLock<Mutex<HashMap<OwnerCoordinatorKey, Weak<OwnerCoordinator>>>> =
    OnceLock::new();

fn owner_coordinator_poisoned() -> IdentityError {
    IdentityError::UnsafeState("owner coordinator is poisoned")
}

fn owner_coordinator_key(dir: &File) -> Result<OwnerCoordinatorKey, IdentityError> {
    #[cfg(unix)]
    {
        let stat =
            fstat(dir).map_err(|error| nix_error("stat owner coordinator directory", error))?;
        Ok(OwnerCoordinatorKey::Unix {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
        })
    }
    #[cfg(windows)]
    {
        let identity = file_id_windows(dir)?;
        Ok(OwnerCoordinatorKey::Windows {
            volume_serial: identity.volume_serial,
            file_id: identity.file_id,
        })
    }
}

fn owner_coordinator_for(key: OwnerCoordinatorKey) -> Result<Arc<OwnerCoordinator>, IdentityError> {
    let registry = OWNER_COORDINATORS.get_or_init(|| Mutex::new(HashMap::new()));
    // Deliberately fail loudly rather than recovering a poisoned coordinator:
    // letting a caller proceed would defeat the owner-wide exclusion boundary.
    let mut registry = registry.lock().map_err(|_| owner_coordinator_poisoned())?;
    registry.retain(|_, coordinator| coordinator.strong_count() != 0);
    if let Some(coordinator) = registry.get(&key).and_then(Weak::upgrade) {
        return Ok(coordinator);
    }
    let coordinator = Arc::new(OwnerCoordinator {
        state: Mutex::new(OwnerCoordinatorState {
            occupied: false,
            waiters: 0,
        }),
        condvar: Condvar::new(),
    });
    registry.insert(key, Arc::downgrade(&coordinator));
    Ok(coordinator)
}

fn acquire_owner_coordinator(
    key: OwnerCoordinatorKey,
) -> Result<OwnerCoordinatorGuard, IdentityError> {
    let coordinator = owner_coordinator_for(key)?;
    let mut state = coordinator
        .state
        .lock()
        .map_err(|_| owner_coordinator_poisoned())?;
    while state.occupied {
        state.waiters += 1;
        state = coordinator
            .condvar
            .wait(state)
            .map_err(|_| owner_coordinator_poisoned())?;
        state.waiters -= 1;
    }
    state.occupied = true;
    drop(state);
    Ok(OwnerCoordinatorGuard { coordinator })
}

impl Drop for OwnerCoordinatorGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.coordinator.state.lock() {
            state.occupied = false;
        }
        // Wake all waiters even when the state mutex is poisoned. Each will
        // reacquire it, observe the poison, and fail rather than hanging.
        self.coordinator.condvar.notify_all();
    }
}

#[cfg(test)]
fn owner_coordinator_held_for_test(key: OwnerCoordinatorKey) -> bool {
    let registry = OWNER_COORDINATORS.get_or_init(|| Mutex::new(HashMap::new()));
    let coordinator = registry
        .lock()
        .expect("test owner coordinator registry poisoned")
        .get(&key)
        .and_then(Weak::upgrade);
    let Some(coordinator) = coordinator else {
        return false;
    };
    match coordinator.state.try_lock() {
        Ok(state) => state.occupied,
        Err(std::sync::TryLockError::WouldBlock) => true,
        Err(std::sync::TryLockError::Poisoned(_)) => false,
    }
}

#[cfg(test)]
fn owner_coordinator_waiters_for_test(key: OwnerCoordinatorKey) -> usize {
    let registry = OWNER_COORDINATORS.get_or_init(|| Mutex::new(HashMap::new()));
    let coordinator = registry
        .lock()
        .expect("test owner coordinator registry poisoned")
        .get(&key)
        .and_then(Weak::upgrade)
        .expect("test owner coordinator must exist");
    coordinator
        .state
        .lock()
        .expect("test owner coordinator state poisoned")
        .waiters
}

#[cfg(all(test, unix))]
fn owner_coordinator_entry_exists_for_test(key: OwnerCoordinatorKey) -> bool {
    OWNER_COORDINATORS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("test owner coordinator registry poisoned")
        .contains_key(&key)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StageKind {
    Prepared,
    Adopted,
    Update,
}

impl StageKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Prepared => ".prepared-",
            Self::Adopted => ".adopted-",
            Self::Update => ".update-",
        }
    }
}

#[derive(Debug)]
struct Stage {
    name: OsString,
    _lock: IdentityLock,
}

#[derive(Debug)]
enum PublishPreparedError {
    AlreadyExists,
    Identity(IdentityError),
}

fn base_segments(platform: PlatformTag) -> &'static [&'static str] {
    match platform {
        PlatformTag::Linux => &[".local", "share", "solstone", "installation-identity", "v1"],
        PlatformTag::Macos => &[
            "Library",
            "Application Support",
            "solstone",
            "installation-identity",
            "v1",
        ],
        PlatformTag::Windows => &["solstone", "installation-identity", "v1"],
    }
}

fn first_exact_index(platform: PlatformTag) -> usize {
    match platform {
        PlatformTag::Linux | PlatformTag::Macos => 3,
        PlatformTag::Windows => 0,
    }
}

#[cfg(unix)]
fn open_provider(owner: &OwnerBase, create: bool) -> Result<SecureDir, IdentityError> {
    let mut current = open_absolute_dir(&owner.home)?;
    for (index, segment) in base_segments(owner.platform).iter().enumerate() {
        let exact_mode = index >= first_exact_index(owner.platform);
        current = if create {
            open_or_create_child_dir(&current, segment, exact_mode)?
        } else {
            open_child_dir(&current, segment, exact_mode)?
        };
    }
    let namespaces = if create {
        open_or_create_child_dir(&current, "namespaces", true)?
    } else {
        open_child_dir(&current, "namespaces", true)?
    };
    drop(namespaces);
    if create {
        ensure_owner_lock_file(&current)?;
    }
    Ok(current)
}

#[cfg(unix)]
fn open_absolute_dir(path: &Path) -> Result<SecureDir, IdentityError> {
    let fd = open(
        path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| nix_error("open home directory", error))?;
    let file = File::from(fd);
    verify_directory(&file, false)?;
    Ok(SecureDir {
        file,
        path: path.to_path_buf(),
    })
}

#[cfg(unix)]
fn open_child_dir(
    parent: &SecureDir,
    name: &str,
    exact_mode: bool,
) -> Result<SecureDir, IdentityError> {
    let fd = openat(
        &parent.file,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| nix_error("open storage directory", error))?;
    let file = File::from(fd);
    verify_directory(&file, exact_mode)?;
    Ok(SecureDir {
        file,
        path: parent.path.join(name),
    })
}

#[cfg(unix)]
fn open_or_create_child_dir(
    parent: &SecureDir,
    name: &str,
    exact_mode: bool,
) -> Result<SecureDir, IdentityError> {
    match open_child_dir(parent, name, exact_mode) {
        Ok(directory) => Ok(directory),
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            mkdirat(&parent.file, name, Mode::from_bits_truncate(0o700))
                .or_else(|error| {
                    if error == Errno::EEXIST {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(|error| nix_error("create storage directory", error))?;
            // mkdir(2) is subject to umask. Correct the entry through the
            // already-verified parent descriptor before opening it, so even a
            // umask of 0777 cannot make the just-created directory inaccessible.
            fchmodat(
                &parent.file,
                name,
                Mode::from_bits_truncate(0o700),
                FchmodatFlags::NoFollowSymlink,
            )
            .map_err(|error| nix_error("set storage directory mode", error))?;
            let directory = open_child_dir(parent, name, exact_mode)?;
            set_mode(&directory.file, Mode::from_bits_truncate(0o700))?;
            verify_directory(&directory.file, true)?;
            parent.sync()?;
            Ok(directory)
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn open_namespace(
    provider: &SecureDir,
    namespace: &NamespaceName,
    create: bool,
) -> Result<SecureDir, IdentityError> {
    let namespaces = open_child_dir(provider, "namespaces", true)?;
    if create {
        open_or_create_child_dir(&namespaces, &namespace.as_hex(), true)
    } else {
        open_child_dir(&namespaces, &namespace.as_hex(), true)
    }
}

#[cfg(unix)]
fn ensure_owner_lock_file(provider: &SecureDir) -> Result<(), IdentityError> {
    let file = open_or_create_file(provider, "owner.lock")?;
    verify_regular(&file, true)?;
    Ok(())
}

#[cfg(unix)]
fn open_or_create_file(parent: &SecureDir, name: &str) -> Result<File, IdentityError> {
    let fd = openat(
        &parent.file,
        name,
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|error| nix_error("open storage file", error))?;
    let file = File::from(fd);
    set_mode(&file, Mode::from_bits_truncate(0o600))?;
    verify_regular(&file, true)?;
    Ok(file)
}

#[cfg(unix)]
fn lock_owner(provider: &SecureDir) -> Result<IdentityLock, IdentityError> {
    #[cfg(test)]
    note_owner_file_lock_attempt();
    let file = open_or_create_file(provider, "owner.lock")?;
    Flock::lock(file, FlockArg::LockExclusive)
        .map_err(|(_, error)| nix_error("lock owner registry", error))
}

#[cfg(unix)]
fn lock_existing_owner(provider: &SecureDir) -> Result<IdentityLock, IdentityError> {
    #[cfg(test)]
    note_owner_file_lock_attempt();
    let file = open_existing_file(provider, "owner.lock", true)?;
    Flock::lock(file, FlockArg::LockExclusive)
        .map_err(|(_, error)| nix_error("lock owner registry", error))
}

#[cfg(unix)]
fn lock_existing_marker(namespace: &SecureDir) -> Result<IdentityLock, IdentityError> {
    let file = open_existing_file(namespace, "adoption.marker", true)?;
    let mut bytes = Vec::new();
    file.try_clone()
        .map_err(|source| io_error("clone adoption marker", source))?
        .read_to_end(&mut bytes)
        .map_err(|source| io_error("read adoption marker", source))?;
    if bytes != MARKER_BYTES {
        return Err(IdentityError::UnsafeState(
            "adoption marker has invalid content",
        ));
    }
    Flock::lock(file, FlockArg::LockExclusive)
        .map_err(|(_, error)| nix_error("lock adoption marker", error))
}

#[cfg(unix)]
fn create_or_lock_marker(namespace: &SecureDir) -> Result<IdentityLock, IdentityError> {
    match open_existing_file(namespace, "adoption.marker", true) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.try_clone()
                .map_err(|source| io_error("clone adoption marker", source))?
                .read_to_end(&mut bytes)
                .map_err(|source| io_error("read adoption marker", source))?;
            if bytes != MARKER_BYTES {
                return Err(IdentityError::UnsafeState(
                    "adoption marker has invalid content",
                ));
            }
            hit(FaultPoint::MarkerLock)?;
            return Flock::lock(file, FlockArg::LockExclusive)
                .map_err(|(_, error)| nix_error("lock adoption marker", error));
        }
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    hit(FaultPoint::MarkerCreation)?;
    let fd = openat(
        &namespace.file,
        "adoption.marker",
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|error| nix_error("create adoption marker", error))?;
    let mut file = File::from(fd);
    set_mode(&file, Mode::from_bits_truncate(0o600))?;
    verify_regular(&file, true)?;
    file.write_all(MARKER_BYTES)
        .map_err(|source| io_error("write adoption marker", source))?;
    file.sync_all()
        .map_err(|source| io_error("sync adoption marker", source))?;
    namespace.sync()?;
    hit(FaultPoint::MarkerSync)?;
    hit(FaultPoint::MarkerLock)?;
    Flock::lock(file, FlockArg::LockExclusive)
        .map_err(|(_, error)| nix_error("lock adoption marker", error))
}

#[cfg(unix)]
fn open_existing_file(
    parent: &SecureDir,
    name: &str,
    exact_mode: bool,
) -> Result<File, IdentityError> {
    let fd = openat(
        &parent.file,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| nix_error("open storage file", error))?;
    let file = File::from(fd);
    verify_regular(&file, exact_mode)?;
    Ok(file)
}

#[cfg(unix)]
fn read_namespace(
    namespace: &SecureDir,
    name: NamespaceName,
) -> Result<NamespaceSnapshot, IdentityError> {
    validate_namespace_children(namespace)?;
    let record_file = match open_existing_file(namespace, "record", true) {
        Ok(file) => Some(file),
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let marker_file = match open_existing_file(namespace, "adoption.marker", true) {
        Ok(file) => Some(file),
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut bytes = 0_u64;
    let record = if let Some(mut file) = record_file {
        let size = fstat(&file)
            .map_err(|error| nix_error("stat identity record", error))?
            .st_size;
        if size < 0 || size as usize > MAX_RECORD_BYTES {
            return Err(IdentityError::UnsafeState("identity record exceeds bounds"));
        }
        bytes += size as u64;
        let mut encoded = Vec::with_capacity(size as usize);
        file.read_to_end(&mut encoded)
            .map_err(|source| io_error("read identity record", source))?;
        Some(decode_record(&encoded)?)
    } else {
        None
    };
    let marker = if let Some(mut file) = marker_file {
        let size = fstat(&file)
            .map_err(|error| nix_error("stat adoption marker", error))?
            .st_size;
        if size < 0 || size as usize != MARKER_BYTES.len() {
            return Err(IdentityError::UnsafeState(
                "adoption marker has invalid size",
            ));
        }
        bytes += size as u64;
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .map_err(|source| io_error("read adoption marker", source))?;
        if content != MARKER_BYTES {
            return Err(IdentityError::UnsafeState(
                "adoption marker has invalid content",
            ));
        }
        true
    } else {
        false
    };
    match (&record, marker) {
        (None, true) => return Err(IdentityError::UnsafeState("orphan adoption marker")),
        (Some(record), false) if record.state != LifecycleState::Prepared => {
            return Err(IdentityError::UnsafeState(
                "committed record has no adoption marker",
            ));
        }
        _ => {}
    }
    Ok(NamespaceSnapshot {
        namespace: name,
        record,
        marker,
        bytes,
    })
}

#[cfg(unix)]
fn enumerate_registry(
    provider: &SecureDir,
) -> Result<BTreeMap<NamespaceName, NamespaceSnapshot>, IdentityError> {
    let namespaces = open_child_dir(provider, "namespaces", true)?;
    let fd: OwnedFd = namespaces
        .file
        .try_clone()
        .map_err(|source| io_error("clone namespace registry", source))?
        .into();
    let mut directory =
        Dir::from_fd(fd).map_err(|error| nix_error("open namespace registry", error))?;
    let mut registry = BTreeMap::new();
    let mut total_bytes = 0_u64;
    for entry in directory.iter() {
        let entry = entry.map_err(|error| nix_error("read namespace registry", error))?;
        let name = entry.file_name();
        let raw = name.to_bytes();
        if raw == b"." || raw == b".." {
            continue;
        }
        let text = std::str::from_utf8(raw)
            .map_err(|_| IdentityError::UnsafeState("namespace entry is not lowercase hex"))?;
        let namespace_name = NamespaceName::parse(text)
            .map_err(|_| IdentityError::UnsafeState("namespace entry is not lowercase hex"))?;
        if registry.len() == MAX_NAMESPACES {
            return Err(IdentityError::UnsafeState(
                "namespace registry exceeds 1024 entries",
            ));
        }
        let namespace = open_child_dir(&namespaces, text, true)?;
        let snapshot = read_namespace(&namespace, namespace_name.clone())?;
        total_bytes = total_bytes
            .checked_add(snapshot.bytes)
            .ok_or(IdentityError::UnsafeState(
                "namespace registry exceeds 16 MiB",
            ))?;
        if total_bytes > MAX_REGISTRY_BYTES {
            return Err(IdentityError::UnsafeState(
                "namespace registry exceeds 16 MiB",
            ));
        }
        registry.insert(namespace_name, snapshot);
    }
    Ok(registry)
}

fn admit_existing_setup(
    request: SetupAdmissionRequest,
    namespace_name: NamespaceName,
    namespace: SecureDir,
    snapshot: NamespaceSnapshot,
    owner_coordinator: OwnerCoordinatorGuard,
    owner_lock: IdentityLock,
    _registry: &BTreeMap<NamespaceName, NamespaceSnapshot>,
) -> Result<SetupAdmission, IdentityError> {
    let mut record = snapshot
        .record
        .ok_or(IdentityError::UnsafeState("namespace record is missing"))?;
    if record.platform != request.owner.platform || record.root_token != request.root_token {
        return Err(IdentityError::UnsafeState(
            "namespace record does not match its root namespace",
        ));
    }
    let mut binding = InstallationBinding::from_record(namespace_name, &record);
    validate_existing_evidence(&request.artifacts, &binding)?;
    match record.state {
        LifecycleState::Prepared => {
            let marker_lock = create_or_lock_marker(&namespace)?;
            record.state = LifecycleState::Adopted;
            replace_record(&namespace, &record, StageKind::Adopted)?;
            binding = InstallationBinding::from_record(binding.namespace.clone(), &record);
            Ok(SetupAdmission {
                effective_journal: record.journal_token.clone(),
                binding,
                _lease: NamespaceLease {
                    _owner_lock: owner_lock,
                    _marker_lock: marker_lock,
                    _namespace: namespace,
                    _owner_coordinator: owner_coordinator,
                },
            })
        }
        LifecycleState::Adopted => {
            if !snapshot.marker {
                return Err(IdentityError::UnsafeState(
                    "adopted record has no adoption marker",
                ));
            }
            let marker_lock = lock_existing_marker(&namespace)?;
            if request.journal_is_explicit && request.journal_token != record.journal_token {
                record.journal_token = request.journal_token.clone();
                replace_record(&namespace, &record, StageKind::Update)?;
                binding = InstallationBinding::from_record(binding.namespace.clone(), &record);
            }
            Ok(SetupAdmission {
                effective_journal: record.journal_token.clone(),
                binding,
                _lease: NamespaceLease {
                    _owner_lock: owner_lock,
                    _marker_lock: marker_lock,
                    _namespace: namespace,
                    _owner_coordinator: owner_coordinator,
                },
            })
        }
        LifecycleState::Tombstoned => {
            if !snapshot.marker {
                return Err(IdentityError::UnsafeState(
                    "tombstoned record has no adoption marker",
                ));
            }
            let marker_lock = lock_existing_marker(&namespace)?;
            let prepared = IdentityRecord {
                state: LifecycleState::Prepared,
                generation: record.generation.next()?,
                id: generate_id()?,
                platform: request.owner.platform,
                root_token: request.root_token,
                journal_token: request.journal_token,
            };
            replace_record(&namespace, &prepared, StageKind::Prepared)?;
            let mut adopted = prepared;
            adopted.state = LifecycleState::Adopted;
            replace_record(&namespace, &adopted, StageKind::Adopted)?;
            binding = InstallationBinding::from_record(binding.namespace.clone(), &adopted);
            Ok(SetupAdmission {
                effective_journal: adopted.journal_token.clone(),
                binding,
                _lease: NamespaceLease {
                    _owner_lock: owner_lock,
                    _marker_lock: marker_lock,
                    _namespace: namespace,
                    _owner_coordinator: owner_coordinator,
                },
            })
        }
    }
}

fn validate_setup_evidence(
    artifacts: &ArtifactBindingEvidence,
    legacy: LegacyManifestEvidence,
) -> Result<(), IdentityError> {
    if matches!(
        artifacts,
        ArtifactBindingEvidence::Malformed | ArtifactBindingEvidence::Ambiguous
    ) {
        return Err(IdentityError::AdmissionRefused(
            "artifact binding is malformed or ambiguous",
        ));
    }
    if matches!(
        legacy,
        LegacyManifestEvidence::Malformed | LegacyManifestEvidence::Unreadable
    ) {
        return Err(IdentityError::AdmissionRefused(
            "legacy manifest evidence is malformed or unreadable",
        ));
    }
    Ok(())
}

fn validate_initial_evidence(request: &SetupAdmissionRequest) -> Result<(), IdentityError> {
    match (&request.artifacts, request.legacy_manifest) {
        (ArtifactBindingEvidence::Fresh, _) => Ok(()),
        (
            ArtifactBindingEvidence::LegacyUnguarded,
            LegacyManifestEvidence::ValidProviderlessSchemaV1,
        ) => Ok(()),
        (ArtifactBindingEvidence::LegacyUnguarded, _) => Err(IdentityError::AdmissionRefused(
            "legacy artifacts need a valid provider-less schema-v1 manifest",
        )),
        _ => Err(IdentityError::AdmissionRefused(
            "existing artifacts have no valid bootstrap evidence",
        )),
    }
}

fn validate_existing_evidence(
    artifacts: &ArtifactBindingEvidence,
    binding: &InstallationBinding,
) -> Result<(), IdentityError> {
    match artifacts {
        ArtifactBindingEvidence::Fresh | ArtifactBindingEvidence::LegacyUnguarded => Ok(()),
        ArtifactBindingEvidence::Guarded(fields) if fields.matches_identity(binding) => Ok(()),
        ArtifactBindingEvidence::Guarded(_) | ArtifactBindingEvidence::Foreign => Err(
            IdentityError::AdmissionRefused("artifact guard does not match the admitted root"),
        ),
        ArtifactBindingEvidence::Malformed | ArtifactBindingEvidence::Ambiguous => Err(
            IdentityError::AdmissionRefused("artifact binding is malformed or ambiguous"),
        ),
    }
}

#[cfg(unix)]
fn write_stage(
    namespace: &SecureDir,
    kind: StageKind,
    record: &IdentityRecord,
) -> Result<Stage, IdentityError> {
    let bytes = encode_record(record)?;
    let name = OsString::from(format!("{}{}", kind.prefix(), record.id.as_hex()));
    let fd = match openat(
        &namespace.file,
        name.as_os_str(),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    ) {
        Ok(fd) => fd,
        Err(Errno::EEXIST) => {
            remove_stale_stages(namespace)?;
            openat(
                &namespace.file,
                name.as_os_str(),
                OFlag::O_WRONLY
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|error| nix_error("create record stage", error))?
        }
        Err(error) => return Err(nix_error("create record stage", error)),
    };
    let mut file = File::from(fd);
    set_mode(&file, Mode::from_bits_truncate(0o600))?;
    verify_regular(&file, true)?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|source| io_error("write record stage", source))?;
        if kind == StageKind::Prepared {
            hit(FaultPoint::PreparedWrite)?;
        }
        file.sync_all()
            .map_err(|source| io_error("sync record stage", source))?;
        hit(match kind {
            StageKind::Prepared => FaultPoint::PreparedFileSync,
            StageKind::Adopted | StageKind::Update => FaultPoint::AdoptedFileSync,
        })?;
        Flock::lock(file, FlockArg::LockExclusive)
            .map_err(|(_, error)| nix_error("lock record stage", error))
    })();
    match result {
        Ok(lock) => Ok(Stage { name, _lock: lock }),
        Err(error) => {
            // `file` is still owned only on errors before flock. The stage is
            // intentionally discarded; no pre-publish failure leaves a source
            // that a later invocation could rename into place.
            let _ = unlinkat(
                &namespace.file,
                name.as_os_str(),
                UnlinkatFlags::NoRemoveDir,
            );
            let _ = namespace.sync();
            Err(error)
        }
    }
}

#[cfg(unix)]
fn publish_prepared(namespace: &SecureDir, stage: Stage) -> Result<(), PublishPreparedError> {
    hit(FaultPoint::PreparedNoReplace).map_err(PublishPreparedError::Identity)?;
    let name = stage.name.clone();
    let linked = linkat(
        &namespace.file,
        name.as_os_str(),
        &namespace.file,
        "record",
        AtFlags::empty(),
    );
    drop(stage);
    match linked {
        Ok(()) => {
            unlinkat(
                &namespace.file,
                name.as_os_str(),
                UnlinkatFlags::NoRemoveDir,
            )
            .map_err(|error| {
                PublishPreparedError::Identity(nix_error("remove published record stage", error))
            })?;
            hit(FaultPoint::PreparedDirectorySync).map_err(PublishPreparedError::Identity)?;
            namespace.sync().map_err(PublishPreparedError::Identity)
        }
        Err(Errno::EEXIST) => {
            unlinkat(
                &namespace.file,
                name.as_os_str(),
                UnlinkatFlags::NoRemoveDir,
            )
            .map_err(|error| {
                PublishPreparedError::Identity(nix_error("remove losing record stage", error))
            })?;
            namespace.sync().map_err(PublishPreparedError::Identity)?;
            Err(PublishPreparedError::AlreadyExists)
        }
        Err(error) => {
            let _ = unlinkat(
                &namespace.file,
                name.as_os_str(),
                UnlinkatFlags::NoRemoveDir,
            );
            let _ = namespace.sync();
            Err(PublishPreparedError::Identity(nix_error(
                "publish prepared record",
                error,
            )))
        }
    }
}

#[cfg(unix)]
fn replace_record(
    namespace: &SecureDir,
    record: &IdentityRecord,
    kind: StageKind,
) -> Result<(), IdentityError> {
    let stage = write_stage(namespace, kind, record)?;
    let name = stage.name.clone();
    hit(FaultPoint::AdoptedReplace)?;
    renameat(&namespace.file, name.as_os_str(), &namespace.file, "record")
        .map_err(|error| nix_error("replace identity record", error))?;
    drop(stage);
    hit(FaultPoint::AdoptedDirectorySync)?;
    namespace.sync()
}

#[cfg(unix)]
fn remove_stale_stages(namespace: &SecureDir) -> Result<(), IdentityError> {
    let fd: OwnedFd = namespace
        .file
        .try_clone()
        .map_err(|source| io_error("clone namespace directory", source))?
        .into();
    let mut directory =
        Dir::from_fd(fd).map_err(|error| nix_error("open namespace directory", error))?;
    let mut stale = Vec::new();
    for entry in directory.iter() {
        let entry = entry.map_err(|error| nix_error("read namespace directory", error))?;
        let name = entry.file_name();
        let raw = name.to_bytes();
        let os_name = OsString::from_vec(raw.to_vec());
        if let Some(kind) = stage_kind(raw) {
            let file = open_existing_file_os(namespace, os_name.as_os_str(), true)?;
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(lock) => {
                    let mut reader = lock
                        .try_clone()
                        .map_err(|source| io_error("clone stale record stage", source))?;
                    let mut content = Vec::new();
                    reader
                        .read_to_end(&mut content)
                        .map_err(|source| io_error("read stale record stage", source))?;
                    let record = decode_record(&content)?;
                    if !matches!(
                        kind,
                        StageKind::Prepared | StageKind::Adopted | StageKind::Update
                    ) || record.id.as_hex().as_bytes() != &raw[kind.prefix().len()..]
                    {
                        return Err(IdentityError::UnsafeState(
                            "stale record stage does not match its name",
                        ));
                    }
                    drop(lock);
                    stale.push(os_name);
                }
                Err((_, Errno::EAGAIN)) => {}
                Err((_, error)) => return Err(nix_error("lock stale record stage", error)),
            }
        } else if raw.starts_with(b".prepared-")
            || raw.starts_with(b".adopted-")
            || raw.starts_with(b".update-")
        {
            return Err(IdentityError::UnsafeState("malformed record stage"));
        }
    }
    for name in stale {
        unlinkat(
            &namespace.file,
            name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        )
        .map_err(|error| nix_error("remove stale record stage", error))?;
        namespace.sync()?;
    }
    Ok(())
}

#[cfg(unix)]
fn validate_namespace_children(namespace: &SecureDir) -> Result<(), IdentityError> {
    let fd: OwnedFd = namespace
        .file
        .try_clone()
        .map_err(|source| io_error("clone namespace directory", source))?
        .into();
    let mut directory =
        Dir::from_fd(fd).map_err(|error| nix_error("open namespace directory", error))?;
    for entry in directory.iter() {
        let entry = entry.map_err(|error| nix_error("read namespace directory", error))?;
        let name = entry.file_name();
        let raw = name.to_bytes();
        if raw == b"."
            || raw == b".."
            || raw == b"record"
            || raw == b"adoption.marker"
            || stage_kind(raw).is_some()
        {
            continue;
        }
        return Err(IdentityError::UnsafeState("unknown namespace child"));
    }
    Ok(())
}

fn stage_kind(name: &[u8]) -> Option<StageKind> {
    for kind in [StageKind::Prepared, StageKind::Adopted, StageKind::Update] {
        let prefix = kind.prefix().as_bytes();
        if name.len() == prefix.len() + 32
            && name.starts_with(prefix)
            && name[prefix.len()..]
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && InstallationId::parse(std::str::from_utf8(&name[prefix.len()..]).ok()?).is_ok()
        {
            return Some(kind);
        }
    }
    None
}

#[cfg(unix)]
fn open_existing_file_os(
    parent: &SecureDir,
    name: &OsStr,
    exact_mode: bool,
) -> Result<File, IdentityError> {
    let fd = openat(
        &parent.file,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| nix_error("open storage file", error))?;
    let file = File::from(fd);
    verify_regular(&file, exact_mode)?;
    Ok(file)
}

fn generate_id() -> Result<InstallationId, IdentityError> {
    let mut bytes = [0_u8; 16];
    fill(&mut bytes).map_err(|error| {
        io_error(
            "generate installation identity",
            io::Error::other(format!("{error:?}")),
        )
    })?;
    Ok(InstallationId(bytes))
}

#[cfg(unix)]
fn verify_directory(file: &File, exact_mode: bool) -> Result<(), IdentityError> {
    let stat = fstat(file).map_err(|error| nix_error("stat storage directory", error))?;
    if SFlag::from_bits_truncate(stat.st_mode) != SFlag::S_IFDIR {
        return Err(IdentityError::UnsafeState(
            "storage path is not a directory",
        ));
    }
    if stat.st_uid != Uid::effective().as_raw() {
        return Err(IdentityError::UnsafeState(
            "storage directory is not owned by the current user",
        ));
    }
    if exact_mode && (stat.st_mode & 0o777) != 0o700 {
        return Err(IdentityError::UnsafeState(
            "storage directory does not have mode 0700",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_regular(file: &File, exact_mode: bool) -> Result<(), IdentityError> {
    let stat = fstat(file).map_err(|error| nix_error("stat storage file", error))?;
    if SFlag::from_bits_truncate(stat.st_mode) != SFlag::S_IFREG {
        return Err(IdentityError::UnsafeState(
            "storage path is not a regular file",
        ));
    }
    if stat.st_uid != Uid::effective().as_raw() {
        return Err(IdentityError::UnsafeState(
            "storage file is not owned by the current user",
        ));
    }
    if exact_mode && (stat.st_mode & 0o777) != 0o600 {
        return Err(IdentityError::UnsafeState(
            "storage file does not have mode 0600",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(file: &File, mode: Mode) -> Result<(), IdentityError> {
    fchmod(file, mode).map_err(|error| nix_error("set storage mode", error))
}

fn validate_absolute_token(bytes: &[u8], label: &'static str) -> Result<(), IdentityError> {
    #[cfg(unix)]
    {
        validate_unix_absolute_token(bytes, label)
    }
    #[cfg(windows)]
    {
        validate_windows_absolute_token(bytes, label)
    }
}

#[cfg(unix)]
fn validate_unix_absolute_token(bytes: &[u8], label: &'static str) -> Result<(), IdentityError> {
    if bytes.is_empty() || bytes.len() > MAX_TOKEN_BYTES || bytes[0] != b'/' || bytes.contains(&0) {
        return Err(IdentityError::InvalidInput(label));
    }
    if normalize_unix_absolute_bytes(bytes)? != bytes {
        return Err(IdentityError::InvalidInput(label));
    }
    Ok(())
}

fn normalize_absolute_bytes(bytes: &[u8]) -> Result<Vec<u8>, IdentityError> {
    #[cfg(unix)]
    {
        normalize_unix_absolute_bytes(bytes)
    }
    #[cfg(windows)]
    {
        normalize_windows_absolute_bytes(bytes)
    }
}

#[cfg(unix)]
fn normalize_unix_absolute_bytes(bytes: &[u8]) -> Result<Vec<u8>, IdentityError> {
    if bytes.is_empty() || bytes[0] != b'/' || bytes.contains(&0) {
        return Err(IdentityError::InvalidInput(
            "path must be absolute and NUL-free",
        ));
    }
    let mut segments = Vec::new();
    for segment in bytes.split(|byte| *byte == b'/') {
        match segment {
            b"" | b"." => {}
            b".." => {
                if segments.pop().is_none() {
                    return Err(IdentityError::InvalidInput(
                        "path escapes its absolute root",
                    ));
                }
            }
            value => segments.push(value),
        }
    }
    let mut output = Vec::with_capacity(bytes.len());
    output.push(b'/');
    for (index, segment) in segments.iter().enumerate() {
        if index != 0 {
            output.push(b'/');
        }
        output.extend_from_slice(segment);
    }
    if output.len() > MAX_TOKEN_BYTES {
        return Err(IdentityError::InvalidInput("path exceeds 4096 bytes"));
    }
    Ok(output)
}

fn field<'a>(line: &'a [u8], prefix: &[u8], name: &'static str) -> Result<&'a [u8], RecordError> {
    if !line.ends_with(b"\n") || !line.starts_with(prefix) {
        return Err(RecordError::InvalidField(name));
    }
    Ok(&line[prefix.len()..line.len() - 1])
}

fn as_ascii<'a>(bytes: &'a [u8], name: &'static str) -> Result<&'a str, RecordError> {
    std::str::from_utf8(bytes).map_err(|_| RecordError::InvalidField(name))
}

fn parse_generation(value: &str) -> Result<Generation, IdentityError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(IdentityError::InvalidInput(
            "generation is not canonical decimal",
        ));
    }
    Generation::new(
        value
            .parse()
            .map_err(|_| IdentityError::InvalidInput("generation is out of range"))?,
    )
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn parse_lower_hex(
    value: &str,
    bytes: usize,
    label: &'static str,
) -> Result<Vec<u8>, IdentityError> {
    if value.len() != bytes * 2
        || !value.bytes().all(|byte| {
            byte.is_ascii_digit()
                || (byte as char).is_ascii_lowercase()
                    && (byte as char) >= 'a'
                    && (byte as char) <= 'f'
        })
    {
        return Err(IdentityError::InvalidInput(label));
    }
    let mut output = Vec::with_capacity(bytes);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = hex_nibble(pair[0]).ok_or(IdentityError::InvalidInput(label))?;
        let low = hex_nibble(pair[1]).ok_or(IdentityError::InvalidInput(label))?;
        output.push(high << 4 | low);
    }
    Ok(output)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn io_error(operation: &'static str, source: io::Error) -> IdentityError {
    IdentityError::Io { operation, source }
}

#[cfg(unix)]
fn nix_error(operation: &'static str, error: Errno) -> IdentityError {
    io_error(operation, io::Error::from_raw_os_error(error as i32))
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsFileIdentity {
    volume_serial: u32,
    file_id: [u8; 16],
}

#[cfg(windows)]
struct WindowsLockGuard {
    file: File,
    locked_handle: Option<RawHandle>,
    overlapped: OVERLAPPED,
}

#[cfg(windows)]
impl fmt::Debug for WindowsLockGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsLockGuard")
            .field("file", &self.file)
            .field("locked_handle", &self.locked_handle)
            .finish_non_exhaustive()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
unsafe impl Send for WindowsLockGuard {}

#[cfg(windows)]
impl Drop for WindowsLockGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        let Some(handle) = self.locked_handle else {
            return;
        };
        // SAFETY: this guard owns the file and retains the exact whole-file
        // range and OVERLAPPED value used for LockFileEx.
        let _ = unsafe { UnlockFileEx(handle, 0, u32::MAX, u32::MAX, &mut self.overlapped) };
    }
}

#[cfg(windows)]
fn windows_units_from_bytes(bytes: &[u8]) -> Result<Vec<u16>, IdentityError> {
    if bytes.len() % 2 != 0 {
        return Err(IdentityError::InvalidInput(
            "Windows path has an odd byte count",
        ));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if units.contains(&0) {
        return Err(IdentityError::InvalidInput(
            "Windows path contains an interior NUL",
        ));
    }
    String::from_utf16(&units)
        .map_err(|_| IdentityError::InvalidInput("Windows path is not valid UTF-16"))?;
    Ok(units)
}

#[cfg(windows)]
fn wide_bytes(value: &OsStr) -> Vec<u8> {
    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(windows)]
fn windows_wide_nul(path: &Path) -> Result<Vec<u16>, IdentityError> {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.contains(&0) {
        return Err(IdentityError::InvalidInput(
            "Windows path contains an interior NUL",
        ));
    }
    let mut output = units;
    output.push(0);
    Ok(output)
}

#[cfg(windows)]
fn normalize_windows_segments<'a>(
    segments: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<&'a str>, IdentityError> {
    let mut output = Vec::new();
    for segment in segments {
        match segment {
            "" | "." => {}
            ".." => {
                if output.pop().is_none() {
                    return Err(IdentityError::InvalidInput(
                        "Windows path escapes its absolute root",
                    ));
                }
            }
            value => output.push(value),
        }
    }
    Ok(output)
}

#[cfg(windows)]
fn normalize_windows_absolute_bytes(bytes: &[u8]) -> Result<Vec<u8>, IdentityError> {
    if bytes.len() > MAX_TOKEN_BYTES {
        return Err(IdentityError::InvalidInput("path exceeds 4096 bytes"));
    }
    let units = windows_units_from_bytes(bytes)?;
    let input = String::from_utf16(&units)
        .map_err(|_| IdentityError::InvalidInput("Windows path is not valid UTF-16"))?;
    let input = input.replace('/', "\\");
    if input.starts_with(r"\\?\") || input.starts_with(r"\\.\") {
        return Err(IdentityError::InvalidInput(
            "Windows verbatim and device paths are not accepted",
        ));
    }
    let normalized = if let Some(rest) = input.strip_prefix(r"\\") {
        let mut parts = rest.split('\\');
        let server = parts
            .next()
            .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
            .ok_or(IdentityError::InvalidInput(
                "Windows UNC path has no server",
            ))?;
        let share = parts
            .next()
            .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
            .ok_or(IdentityError::InvalidInput("Windows UNC path has no share"))?;
        let tail = normalize_windows_segments(parts)?;
        let mut output = format!(r"\\{server}\{share}");
        for segment in tail {
            output.push('\\');
            output.push_str(segment);
        }
        output
    } else {
        let units: Vec<u16> = input.encode_utf16().collect();
        if units.len() < 3
            || !((b'A' as u16..=b'Z' as u16).contains(&units[0])
                || (b'a' as u16..=b'z' as u16).contains(&units[0]))
            || units[1] != b':' as u16
            || units[2] != b'\\' as u16
        {
            return Err(IdentityError::InvalidInput(
                "Windows path must be fully qualified",
            ));
        }
        let tail = normalize_windows_segments(input[3..].split('\\'))?;
        let drive = char::from_u32(units[0] as u32)
            .expect("validated ASCII drive")
            .to_ascii_uppercase();
        let mut output = format!("{drive}:\\");
        for (index, segment) in tail.iter().enumerate() {
            if index != 0 {
                output.push('\\');
            }
            output.push_str(segment);
        }
        output
    };
    let output = wide_bytes(OsStr::new(&normalized));
    if output.len() > MAX_TOKEN_BYTES {
        return Err(IdentityError::InvalidInput("path exceeds 4096 bytes"));
    }
    Ok(output)
}

#[cfg(windows)]
fn validate_windows_absolute_token(bytes: &[u8], label: &'static str) -> Result<(), IdentityError> {
    if bytes.is_empty() || bytes.len() > MAX_TOKEN_BYTES {
        return Err(IdentityError::InvalidInput(label));
    }
    if normalize_windows_absolute_bytes(bytes)? != bytes {
        return Err(IdentityError::InvalidInput(label));
    }
    Ok(())
}

#[cfg(windows)]
fn normalize_windows_canonical_path(path: &Path) -> Result<Vec<u8>, IdentityError> {
    let input = path.to_string_lossy();
    let ordinary = if let Some(rest) = input.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = input.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        input.into_owned()
    };
    normalize_windows_absolute_bytes(&wide_bytes(OsStr::new(&ordinary)))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn known_folder_local_app_data() -> Result<PathBuf, IdentityError> {
    #[cfg(test)]
    if let Some(path) = known_folder_override_for_test() {
        return Ok(path);
    }
    let mut raw = std::ptr::null_mut();
    // SAFETY: the folder GUID is valid and SHGetKnownFolderPath initializes the
    // returned CoTaskMem allocation on success.
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, std::ptr::null_mut(), &mut raw) };
    if result < 0 || raw.is_null() {
        return Err(io_error(
            "resolve LocalAppData known folder",
            io::Error::from_raw_os_error(result),
        ));
    }
    let mut length = 0;
    // SAFETY: the successful result is a NUL-terminated PWSTR allocation.
    unsafe {
        while *raw.add(length) != 0 {
            length += 1;
        }
    }
    // SAFETY: the pointer remains valid until CoTaskMemFree below.
    let units = unsafe { std::slice::from_raw_parts(raw, length) }.to_vec();
    // SAFETY: SHGetKnownFolderPath documents CoTaskMemFree as the matching free.
    unsafe { CoTaskMemFree(raw.cast()) };
    String::from_utf16(&units).map_err(|_| {
        IdentityError::InvalidInput("LocalAppData known folder is not valid UTF-16")
    })?;
    Ok(PathBuf::from(OsString::from_wide(&units)))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn open_windows_file(
    path: &Path,
    access: u32,
    disposition: u32,
    flags: u32,
    operation: &'static str,
) -> Result<File, IdentityError> {
    let wide = windows_wide_nul(path)?;
    // SAFETY: wide is NUL terminated and all remaining arguments are documented constants.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            disposition,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io_error(operation, io::Error::last_os_error()));
    }
    // SAFETY: CreateFileW returned an owned valid handle exactly once.
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn attribute_tag_windows(file: &File) -> Result<FILE_ATTRIBUTE_TAG_INFO, IdentityError> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: info is writable for the exact supplied size and file is a live handle.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io_error(
            "query storage attributes",
            io::Error::last_os_error(),
        ));
    }
    Ok(info)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn file_id_windows(file: &File) -> Result<WindowsFileIdentity, IdentityError> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: info is writable for the exact supplied size and file is a live handle.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io_error(
            "query storage identity",
            io::Error::last_os_error(),
        ));
    }
    Ok(WindowsFileIdentity {
        volume_serial: u32::try_from(info.VolumeSerialNumber)
            .map_err(|_| IdentityError::UnsafeState("Windows volume serial exceeds u32"))?,
        file_id: info.FileId.Identifier,
    })
}

#[cfg(windows)]
fn verify_directory(file: &File, _exact_mode: bool) -> Result<(), IdentityError> {
    // Windows access is governed by ordinary inherited LocalAppData controls.
    // This protocol intentionally does not inspect or create owner SIDs, DACLs,
    // integrity labels, or any custom ACL representation.
    let attributes = attribute_tag_windows(file)?;
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(IdentityError::UnsafeState(
            "storage path is a reparse point",
        ));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(IdentityError::UnsafeState(
            "storage path is not a directory",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn verify_regular(file: &File, _exact_mode: bool) -> Result<(), IdentityError> {
    let attributes = attribute_tag_windows(file)?;
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(IdentityError::UnsafeState(
            "storage path is a reparse point",
        ));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(IdentityError::UnsafeState(
            "storage path is not a regular file",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn open_absolute_dir(path: &Path) -> Result<SecureDir, IdentityError> {
    let file = open_windows_file(
        path,
        FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        "open owner directory",
    )?;
    verify_directory(&file, false)?;
    Ok(SecureDir {
        file,
        path: path.to_path_buf(),
    })
}

#[cfg(windows)]
fn open_child_dir(
    parent: &SecureDir,
    name: &str,
    exact_mode: bool,
) -> Result<SecureDir, IdentityError> {
    let path = parent.path.join(name);
    let file = open_windows_file(
        &path,
        FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        "open storage directory",
    )?;
    verify_directory(&file, exact_mode)?;
    Ok(SecureDir { file, path })
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn open_or_create_child_dir(
    parent: &SecureDir,
    name: &str,
    exact_mode: bool,
) -> Result<SecureDir, IdentityError> {
    match open_child_dir(parent, name, exact_mode) {
        Ok(directory) => Ok(directory),
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            let path = parent.path.join(name);
            let wide = windows_wide_nul(&path)?;
            // SAFETY: wide is NUL terminated and the security descriptor is intentionally inherited.
            let created = unsafe { CreateDirectoryW(wide.as_ptr(), std::ptr::null()) };
            if created == 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(io_error(
                    "create storage directory",
                    io::Error::last_os_error(),
                ));
            }
            let directory = open_child_dir(parent, name, exact_mode)?;
            parent.sync()?;
            Ok(directory)
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn open_provider(owner: &OwnerBase, create: bool) -> Result<SecureDir, IdentityError> {
    let mut current = open_absolute_dir(&owner.home)?;
    for (index, segment) in base_segments(owner.platform).iter().enumerate() {
        let exact_mode = index >= first_exact_index(owner.platform);
        current = if create {
            open_or_create_child_dir(&current, segment, exact_mode)?
        } else {
            open_child_dir(&current, segment, exact_mode)?
        };
    }
    let namespaces = if create {
        open_or_create_child_dir(&current, "namespaces", true)?
    } else {
        open_child_dir(&current, "namespaces", true)?
    };
    drop(namespaces);
    if create {
        ensure_owner_lock_file(&current)?;
    }
    Ok(current)
}

#[cfg(windows)]
fn open_namespace(
    provider: &SecureDir,
    namespace: &NamespaceName,
    create: bool,
) -> Result<SecureDir, IdentityError> {
    let namespaces = open_child_dir(provider, "namespaces", true)?;
    if create {
        open_or_create_child_dir(&namespaces, &namespace.as_hex(), true)
    } else {
        open_child_dir(&namespaces, &namespace.as_hex(), true)
    }
}

#[cfg(windows)]
fn open_or_create_file(parent: &SecureDir, name: &str) -> Result<File, IdentityError> {
    let file = open_windows_file(
        &parent.path.join(name),
        GENERIC_READ | GENERIC_WRITE,
        OPEN_ALWAYS,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
        "open storage file",
    )?;
    verify_regular(&file, true)?;
    Ok(file)
}

#[cfg(windows)]
fn open_existing_file(
    parent: &SecureDir,
    name: &str,
    exact_mode: bool,
) -> Result<File, IdentityError> {
    let file = open_windows_file(
        &parent.path.join(name),
        GENERIC_READ | GENERIC_WRITE,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
        "open storage file",
    )?;
    verify_regular(&file, exact_mode)?;
    Ok(file)
}

#[cfg(windows)]
fn open_existing_file_os(
    parent: &SecureDir,
    name: &OsStr,
    exact_mode: bool,
) -> Result<File, IdentityError> {
    let file = open_windows_file(
        &parent.path.join(name),
        GENERIC_READ | GENERIC_WRITE,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
        "open storage file",
    )?;
    verify_regular(&file, exact_mode)?;
    Ok(file)
}

#[cfg(windows)]
fn create_new_file(path: &Path, operation: &'static str) -> Result<File, IdentityError> {
    open_windows_file(
        path,
        GENERIC_READ | GENERIC_WRITE,
        CREATE_NEW,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
        operation,
    )
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn lock_file_windows(file: File) -> Result<WindowsLockGuard, (File, io::Error)> {
    let mut overlapped = OVERLAPPED::default();
    let handle = file.as_raw_handle();
    // SAFETY: the file owns the handle and the zeroed OVERLAPPED describes the retained whole-file range.
    let result = unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result == 0 {
        return Err((file, io::Error::last_os_error()));
    }
    Ok(WindowsLockGuard {
        file,
        locked_handle: Some(handle),
        overlapped,
    })
}

#[cfg(windows)]
fn lock_file_windows_blocking(mut file: File) -> Result<WindowsLockGuard, IdentityError> {
    loop {
        match lock_file_windows(file) {
            Ok(lock) => return Ok(lock),
            Err((returned, error)) if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) => {
                file = returned;
                std::thread::yield_now();
            }
            Err((_, error)) => return Err(io_error("lock identity storage", error)),
        }
    }
}

#[cfg(windows)]
fn ensure_owner_lock_file(provider: &SecureDir) -> Result<(), IdentityError> {
    let file = open_or_create_file(provider, "owner.lock")?;
    verify_regular(&file, true)
}

#[cfg(windows)]
fn lock_owner(provider: &SecureDir) -> Result<IdentityLock, IdentityError> {
    #[cfg(test)]
    note_owner_file_lock_attempt();
    lock_file_windows_blocking(open_or_create_file(provider, "owner.lock")?)
}

#[cfg(windows)]
fn lock_existing_owner(provider: &SecureDir) -> Result<IdentityLock, IdentityError> {
    #[cfg(test)]
    note_owner_file_lock_attempt();
    lock_file_windows_blocking(open_existing_file(provider, "owner.lock", true)?)
}

#[cfg(windows)]
fn lock_existing_marker(namespace: &SecureDir) -> Result<IdentityLock, IdentityError> {
    let file = open_existing_file(namespace, "adoption.marker", true)?;
    let mut bytes = Vec::new();
    file.try_clone()
        .map_err(|source| io_error("clone adoption marker", source))?
        .read_to_end(&mut bytes)
        .map_err(|source| io_error("read adoption marker", source))?;
    if bytes != MARKER_BYTES {
        return Err(IdentityError::UnsafeState(
            "adoption marker has invalid content",
        ));
    }
    lock_file_windows_blocking(file)
}

#[cfg(windows)]
fn create_or_lock_marker(namespace: &SecureDir) -> Result<IdentityLock, IdentityError> {
    match open_existing_file(namespace, "adoption.marker", true) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.try_clone()
                .map_err(|source| io_error("clone adoption marker", source))?
                .read_to_end(&mut bytes)
                .map_err(|source| io_error("read adoption marker", source))?;
            if bytes != MARKER_BYTES {
                return Err(IdentityError::UnsafeState(
                    "adoption marker has invalid content",
                ));
            }
            hit(FaultPoint::MarkerLock)?;
            return lock_file_windows_blocking(file);
        }
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    hit(FaultPoint::MarkerCreation)?;
    let mut file = create_new_file(
        &namespace.path.join("adoption.marker"),
        "create adoption marker",
    )?;
    verify_regular(&file, true)?;
    file.write_all(MARKER_BYTES)
        .map_err(|source| io_error("write adoption marker", source))?;
    file.sync_all()
        .map_err(|source| io_error("sync adoption marker", source))?;
    namespace.sync()?;
    hit(FaultPoint::MarkerSync)?;
    hit(FaultPoint::MarkerLock)?;
    lock_file_windows_blocking(file)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn delete_file_windows(path: &Path, operation: &'static str) -> Result<(), IdentityError> {
    let wide = windows_wide_nul(path)?;
    // SAFETY: wide is NUL terminated.
    if unsafe { DeleteFileW(wide.as_ptr()) } == 0 {
        return Err(io_error(operation, io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(windows)]
fn read_namespace(
    namespace: &SecureDir,
    name: NamespaceName,
) -> Result<NamespaceSnapshot, IdentityError> {
    validate_namespace_children(namespace)?;
    let record_file = match open_existing_file(namespace, "record", true) {
        Ok(file) => Some(file),
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let marker_file = match open_existing_file(namespace, "adoption.marker", true) {
        Ok(file) => Some(file),
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut bytes = 0_u64;
    let record = if let Some(mut file) = record_file {
        let size = file
            .metadata()
            .map_err(|source| io_error("stat identity record", source))?
            .len();
        if size > MAX_RECORD_BYTES as u64 {
            return Err(IdentityError::UnsafeState("identity record exceeds bounds"));
        }
        bytes += size;
        let mut encoded = Vec::with_capacity(size as usize);
        file.read_to_end(&mut encoded)
            .map_err(|source| io_error("read identity record", source))?;
        Some(decode_record(&encoded)?)
    } else {
        None
    };
    let marker = if let Some(mut file) = marker_file {
        let size = file
            .metadata()
            .map_err(|source| io_error("stat adoption marker", source))?
            .len();
        if size != MARKER_BYTES.len() as u64 {
            return Err(IdentityError::UnsafeState(
                "adoption marker has invalid size",
            ));
        }
        bytes += size;
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .map_err(|source| io_error("read adoption marker", source))?;
        if content != MARKER_BYTES {
            return Err(IdentityError::UnsafeState(
                "adoption marker has invalid content",
            ));
        }
        true
    } else {
        false
    };
    match (&record, marker) {
        (None, true) => return Err(IdentityError::UnsafeState("orphan adoption marker")),
        (Some(record), false) if record.state != LifecycleState::Prepared => {
            return Err(IdentityError::UnsafeState(
                "committed record has no adoption marker",
            ));
        }
        _ => {}
    }
    Ok(NamespaceSnapshot {
        namespace: name,
        record,
        marker,
        bytes,
    })
}

#[cfg(windows)]
fn validate_namespace_children(namespace: &SecureDir) -> Result<(), IdentityError> {
    let entries = std::fs::read_dir(&namespace.path)
        .map_err(|source| io_error("read namespace directory", source))?;
    for entry in entries {
        let entry = entry.map_err(|source| io_error("read namespace directory", source))?;
        let name = entry.file_name();
        let text = name
            .to_str()
            .ok_or(IdentityError::UnsafeState("unknown namespace child"))?;
        if matches!(text, "record" | "adoption.marker") || stage_kind(text.as_bytes()).is_some() {
            continue;
        }
        return Err(IdentityError::UnsafeState("unknown namespace child"));
    }
    Ok(())
}

#[cfg(windows)]
fn enumerate_registry(
    provider: &SecureDir,
) -> Result<BTreeMap<NamespaceName, NamespaceSnapshot>, IdentityError> {
    let namespaces = open_child_dir(provider, "namespaces", true)?;
    let entries = std::fs::read_dir(&namespaces.path)
        .map_err(|source| io_error("read namespace registry", source))?;
    let mut registry = BTreeMap::new();
    let mut total_bytes = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|source| io_error("read namespace registry", source))?;
        let name = entry.file_name();
        let text = name.to_str().ok_or(IdentityError::UnsafeState(
            "namespace entry is not lowercase hex",
        ))?;
        let namespace_name = NamespaceName::parse(text)
            .map_err(|_| IdentityError::UnsafeState("namespace entry is not lowercase hex"))?;
        if registry.len() == MAX_NAMESPACES {
            return Err(IdentityError::UnsafeState(
                "namespace registry exceeds 1024 entries",
            ));
        }
        let namespace = open_child_dir(&namespaces, text, true)?;
        let snapshot = read_namespace(&namespace, namespace_name.clone())?;
        total_bytes = total_bytes
            .checked_add(snapshot.bytes)
            .ok_or(IdentityError::UnsafeState(
                "namespace registry exceeds 16 MiB",
            ))?;
        if total_bytes > MAX_REGISTRY_BYTES {
            return Err(IdentityError::UnsafeState(
                "namespace registry exceeds 16 MiB",
            ));
        }
        registry.insert(namespace_name, snapshot);
    }
    Ok(registry)
}

#[cfg(windows)]
fn write_stage(
    namespace: &SecureDir,
    kind: StageKind,
    record: &IdentityRecord,
) -> Result<Stage, IdentityError> {
    let bytes = encode_record(record)?;
    let name = OsString::from(format!("{}{}", kind.prefix(), record.id.as_hex()));
    let path = namespace.path.join(&name);
    let mut file = match create_new_file(&path, "create record stage") {
        Ok(file) => file,
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists => {
            remove_stale_stages(namespace)?;
            create_new_file(&path, "create record stage")?
        }
        Err(error) => return Err(error),
    };
    verify_regular(&file, true)?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|source| io_error("write record stage", source))?;
        if kind == StageKind::Prepared {
            hit(FaultPoint::PreparedWrite)?;
        }
        file.sync_all()
            .map_err(|source| io_error("sync record stage", source))?;
        hit(match kind {
            StageKind::Prepared => FaultPoint::PreparedFileSync,
            StageKind::Adopted | StageKind::Update => FaultPoint::AdoptedFileSync,
        })?;
        lock_file_windows_blocking(file)
    })();
    match result {
        Ok(lock) => Ok(Stage { name, _lock: lock }),
        Err(error) => {
            let _ = delete_file_windows(&path, "remove failed record stage");
            let _ = namespace.sync();
            Err(error)
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn create_hard_link_windows(new_path: &Path, existing_path: &Path) -> Result<(), io::Error> {
    let new_wide =
        windows_wide_nul(new_path).map_err(|error| io::Error::other(error.to_string()))?;
    let existing_wide =
        windows_wide_nul(existing_path).map_err(|error| io::Error::other(error.to_string()))?;
    // SAFETY: both paths are NUL terminated and the security descriptor is intentionally inherited.
    if unsafe { CreateHardLinkW(new_wide.as_ptr(), existing_wide.as_ptr(), std::ptr::null()) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn publish_prepared(namespace: &SecureDir, stage: Stage) -> Result<(), PublishPreparedError> {
    hit(FaultPoint::PreparedNoReplace).map_err(PublishPreparedError::Identity)?;
    let name = stage.name.clone();
    let stage_path = namespace.path.join(&name);
    let record_path = namespace.path.join("record");
    let linked = create_hard_link_windows(&record_path, &stage_path);
    drop(stage);
    match linked {
        Ok(()) => {
            delete_file_windows(&stage_path, "remove published record stage")
                .map_err(PublishPreparedError::Identity)?;
            hit(FaultPoint::PreparedDirectorySync).map_err(PublishPreparedError::Identity)?;
            namespace.sync().map_err(PublishPreparedError::Identity)
        }
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                || error.raw_os_error() == Some(ERROR_FILE_EXISTS as i32) =>
        {
            delete_file_windows(&stage_path, "remove losing record stage")
                .map_err(PublishPreparedError::Identity)?;
            namespace.sync().map_err(PublishPreparedError::Identity)?;
            Err(PublishPreparedError::AlreadyExists)
        }
        Err(error) => {
            let _ = delete_file_windows(&stage_path, "remove failed record stage");
            let _ = namespace.sync();
            Err(PublishPreparedError::Identity(io_error(
                "publish prepared record",
                error,
            )))
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn replace_record(
    namespace: &SecureDir,
    record: &IdentityRecord,
    kind: StageKind,
) -> Result<(), IdentityError> {
    let stage = write_stage(namespace, kind, record)?;
    let name = stage.name.clone();
    hit(FaultPoint::AdoptedReplace)?;
    let source = windows_wide_nul(&namespace.path.join(name))?;
    let destination = windows_wide_nul(&namespace.path.join("record"))?;
    // SAFETY: both paths are NUL terminated and refer to the same storage directory.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io_error(
            "replace identity record",
            io::Error::last_os_error(),
        ));
    }
    drop(stage);
    hit(FaultPoint::AdoptedDirectorySync)?;
    namespace.sync()
}

#[cfg(windows)]
fn remove_stale_stages(namespace: &SecureDir) -> Result<(), IdentityError> {
    let entries = std::fs::read_dir(&namespace.path)
        .map_err(|source| io_error("read namespace directory", source))?;
    let mut stale = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| io_error("read namespace directory", source))?;
        let name = entry.file_name();
        let text = name
            .to_str()
            .ok_or(IdentityError::UnsafeState("malformed record stage"))?;
        if let Some(kind) = stage_kind(text.as_bytes()) {
            let file = open_existing_file_os(namespace, &name, true)?;
            match lock_file_windows(file) {
                Ok(lock) => {
                    let mut reader = lock
                        .file
                        .try_clone()
                        .map_err(|source| io_error("clone stale record stage", source))?;
                    let mut content = Vec::new();
                    reader
                        .read_to_end(&mut content)
                        .map_err(|source| io_error("read stale record stage", source))?;
                    let record = decode_record(&content)?;
                    if record.id.as_hex().as_bytes() != &text.as_bytes()[kind.prefix().len()..] {
                        return Err(IdentityError::UnsafeState(
                            "stale record stage does not match its name",
                        ));
                    }
                    drop(lock);
                    stale.push(name);
                }
                Err((_, error)) if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) => {}
                Err((_, error)) => return Err(io_error("lock stale record stage", error)),
            }
        } else if text.starts_with(".prepared-")
            || text.starts_with(".adopted-")
            || text.starts_with(".update-")
        {
            return Err(IdentityError::UnsafeState("malformed record stage"));
        }
    }
    for name in stale {
        delete_file_windows(&namespace.path.join(name), "remove stale record stage")?;
        namespace.sync()?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FaultPoint {
    LoadLocksHeld,
    PreparedWrite,
    PreparedFileSync,
    PreparedNoReplace,
    PreparedDirectorySync,
    MarkerCreation,
    MarkerSync,
    MarkerLock,
    AdoptedReplace,
    AdoptedFileSync,
    AdoptedDirectorySync,
}

#[cfg(test)]
thread_local! {
    static OWNER_FILE_LOCK_ATTEMPTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn note_owner_file_lock_attempt() {
    OWNER_FILE_LOCK_ATTEMPTS.with(|attempts| attempts.set(attempts.get() + 1));
}

#[cfg(all(test, unix))]
fn owner_file_lock_attempts_for_test() -> usize {
    OWNER_FILE_LOCK_ATTEMPTS.with(std::cell::Cell::get)
}

#[cfg(not(test))]
fn hit(_: FaultPoint) -> Result<(), IdentityError> {
    Ok(())
}

#[cfg(test)]
struct ParkGate {
    arrived: std::sync::Barrier,
    release: std::sync::Barrier,
}

#[cfg(test)]
impl ParkGate {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            arrived: std::sync::Barrier::new(2),
            release: std::sync::Barrier::new(2),
        })
    }
}

#[cfg(test)]
#[derive(Clone)]
struct TestControl {
    fail: Option<FaultPoint>,
    park: Option<(FaultPoint, std::sync::Arc<ParkGate>)>,
    #[cfg(windows)]
    known_folder: Option<PathBuf>,
}

#[cfg(test)]
static TEST_CONTROL: std::sync::OnceLock<std::sync::Mutex<TestControl>> =
    std::sync::OnceLock::new();

#[cfg(test)]
fn hit(point: FaultPoint) -> Result<(), IdentityError> {
    let control = TEST_CONTROL.get_or_init(|| {
        std::sync::Mutex::new(TestControl {
            fail: None,
            park: None,
            #[cfg(windows)]
            known_folder: None,
        })
    });
    let (fail, park) = {
        let guard = control.lock().expect("test fault control poisoned");
        (guard.fail, guard.park.clone())
    };
    if let Some((target, gate)) = park.filter(|(target, _)| *target == point) {
        let _ = target;
        gate.arrived.wait();
        gate.release.wait();
    }
    if fail == Some(point) {
        return Err(IdentityError::Io {
            operation: "injected identity fault",
            source: io::Error::other("fault injection"),
        });
    }
    Ok(())
}

#[cfg(all(test, windows))]
fn set_known_folder_override_for_test(path: Option<PathBuf>) {
    let mut control = TEST_CONTROL
        .get_or_init(|| {
            std::sync::Mutex::new(TestControl {
                fail: None,
                park: None,
                known_folder: None,
            })
        })
        .lock()
        .expect("test fault control poisoned");
    control.known_folder = path;
}

#[cfg(all(test, windows))]
fn known_folder_override_for_test() -> Option<PathBuf> {
    TEST_CONTROL
        .get_or_init(|| {
            std::sync::Mutex::new(TestControl {
                fail: None,
                park: None,
                known_folder: None,
            })
        })
        .lock()
        .expect("test fault control poisoned")
        .known_folder
        .clone()
}

#[cfg(all(test, windows))]
fn fail_at_for_test(point: FaultPoint) {
    let mut control = TEST_CONTROL
        .get_or_init(|| {
            std::sync::Mutex::new(TestControl {
                fail: None,
                park: None,
                #[cfg(windows)]
                known_folder: None,
            })
        })
        .lock()
        .expect("test fault control poisoned");
    control.fail = Some(point);
}

#[cfg(all(test, windows))]
fn clear_fault_control_for_test() {
    let mut control = TEST_CONTROL
        .get_or_init(|| {
            std::sync::Mutex::new(TestControl {
                fail: None,
                park: None,
                #[cfg(windows)]
                known_folder: None,
            })
        })
        .lock()
        .expect("test fault control poisoned");
    control.fail = None;
    control.park = None;
}

#[cfg(all(test, windows))]
fn park_at_for_test(point: FaultPoint) -> Arc<ParkGate> {
    let gate = ParkGate::new();
    let mut control = TEST_CONTROL
        .get_or_init(|| {
            std::sync::Mutex::new(TestControl {
                fail: None,
                park: None,
                #[cfg(windows)]
                known_folder: None,
            })
        })
        .lock()
        .expect("test fault control poisoned");
    control.fail = None;
    control.park = Some((point, gate.clone()));
    gate
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static TEST_SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

    #[derive(Debug, Eq, PartialEq)]
    enum TreeEntry {
        Directory { mode: u32 },
        File { mode: u32, bytes: Option<Vec<u8>> },
        Symlink { mode: u32, target: PathBuf },
        Other { mode: u32 },
    }

    struct UmaskGuard(Mode);

    impl UmaskGuard {
        fn set(mode: Mode) -> Self {
            Self(nix::sys::stat::umask(mode))
        }
    }

    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            nix::sys::stat::umask(self.0);
        }
    }

    struct TestRoot {
        root: PathBuf,
        owner: OwnerBase,
    }

    impl TestRoot {
        fn new() -> Self {
            let root = PathBuf::from(format!(
                "/var/tmp/solstone-installation-identity-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("create test root");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("set test root mode");
            fs::create_dir(root.join("home")).expect("create test home");
            fs::set_permissions(root.join("home"), fs::Permissions::from_mode(0o700))
                .expect("set test home mode");
            let owner =
                OwnerBase::at_home(root.join("home"), PlatformTag::Linux).expect("owner base");
            Self { root, owner }
        }

        fn request(&self, root: &[u8], journal: &[u8]) -> SetupAdmissionRequest {
            SetupAdmissionRequest {
                owner: self.owner.clone(),
                root_token: RootToken::from_raw_absolute(root.to_vec()).expect("root token"),
                journal_token: JournalToken::from_raw_absolute(journal.to_vec())
                    .expect("journal token"),
                journal_is_explicit: true,
                legacy_manifest: LegacyManifestEvidence::Absent,
                artifacts: ArtifactBindingEvidence::Fresh,
            }
        }

        fn namespace_path(&self, root: &[u8]) -> PathBuf {
            let token = RootToken::from_raw_absolute(root.to_vec()).expect("root token");
            self.owner
                .path()
                .join("namespaces")
                .join(namespace_name(PlatformTag::Linux, &token).as_hex())
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_SERIAL
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_control() {
        let mut control = TEST_CONTROL
            .get_or_init(|| {
                Mutex::new(TestControl {
                    fail: None,
                    park: None,
                })
            })
            .lock()
            .expect("test control");
        control.fail = None;
        control.park = None;
    }

    fn fail_at(point: FaultPoint) {
        let mut control = TEST_CONTROL
            .get_or_init(|| {
                Mutex::new(TestControl {
                    fail: None,
                    park: None,
                })
            })
            .lock()
            .expect("test control");
        control.fail = Some(point);
        control.park = None;
    }

    fn park_at(point: FaultPoint) -> Arc<ParkGate> {
        let gate = ParkGate::new();
        let mut control = TEST_CONTROL
            .get_or_init(|| {
                Mutex::new(TestControl {
                    fail: None,
                    park: None,
                })
            })
            .lock()
            .expect("test control");
        control.fail = None;
        control.park = Some((point, gate.clone()));
        gate
    }

    fn read_record(path: &Path) -> IdentityRecord {
        decode_record(&fs::read(path).expect("read record")).expect("decode record")
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("stat path").permissions().mode() & 0o777
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, TreeEntry> {
        fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<PathBuf, TreeEntry>) {
            let metadata = fs::symlink_metadata(path).expect("snapshot metadata");
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path is relative")
                .to_path_buf();
            let mode = metadata.permissions().mode() & 0o777;
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                snapshot.insert(relative, TreeEntry::Directory { mode });
                if let Ok(entries) = fs::read_dir(path) {
                    let mut entries = entries
                        .map(|entry| entry.expect("snapshot directory entry"))
                        .collect::<Vec<_>>();
                    entries.sort_by_key(|entry| entry.file_name());
                    for entry in entries {
                        visit(root, &entry.path(), snapshot);
                    }
                }
            } else if file_type.is_file() {
                snapshot.insert(
                    relative,
                    TreeEntry::File {
                        mode,
                        bytes: fs::read(path).ok(),
                    },
                );
            } else if file_type.is_symlink() {
                snapshot.insert(
                    relative,
                    TreeEntry::Symlink {
                        mode,
                        target: fs::read_link(path).expect("snapshot symlink target"),
                    },
                );
            } else {
                snapshot.insert(relative, TreeEntry::Other { mode });
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    fn assert_tree_unchanged(root: &Path, before: BTreeMap<PathBuf, TreeEntry>) {
        assert_eq!(snapshot_tree(root), before);
    }

    fn provider_key(owner: &OwnerBase) -> OwnerCoordinatorKey {
        let provider = open_provider(owner, true).expect("open provider");
        owner_coordinator_key(&provider.file).expect("owner coordinator key")
    }

    fn wait_for_owner_coordinator_waiters(key: OwnerCoordinatorKey, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner_coordinator_waiters_for_test(key) < expected {
            assert!(
                Instant::now() < deadline,
                "owner coordinator never reached {expected} waiters"
            );
            thread::yield_now();
        }
    }

    fn release_waiter(label: u8, first: &mpsc::Sender<()>, second: &mpsc::Sender<()>) {
        match label {
            1 => first.send(()).expect("release first waiter"),
            2 => second.send(()).expect("release second waiter"),
            _ => panic!("unknown waiter"),
        }
    }

    #[test]
    fn owner_coordinator_excludes_same_process_threads_without_flock() {
        let fixture = TestRoot::new();
        let key = provider_key(&fixture.owner);
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first = thread::spawn(move || {
            let guard = acquire_owner_coordinator(key).expect("first coordinator guard");
            held_tx
                .send(Arc::as_ptr(&guard.coordinator) as usize)
                .expect("first guard ready");
            release_rx.recv().expect("release first guard");
            drop(guard);
        });
        let first_identity = held_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first guard ready");
        let (second_tx, second_rx) = mpsc::channel();
        let second = thread::spawn(move || {
            // This deliberately invokes no Flock/file-lock backend: the
            // process-local guard alone must exclude same-process threads.
            let guard = acquire_owner_coordinator(key).expect("second coordinator guard");
            second_tx
                .send(Arc::as_ptr(&guard.coordinator) as usize)
                .expect("second guard ready");
            drop(guard);
        });

        wait_for_owner_coordinator_waiters(key, 1);
        assert!(matches!(
            second_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).expect("release first guard");
        let second_identity = second_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("second guard after release");
        first.join().expect("first thread");
        second.join().expect("second thread");

        assert_eq!(first_identity, second_identity);
        assert!(!owner_coordinator_held_for_test(key));
    }

    #[test]
    fn owner_coordinator_distinct_provider_bases_do_not_block_each_other() {
        let first = TestRoot::new();
        let second = TestRoot::new();
        let first_key = provider_key(&first.owner);
        let second_key = provider_key(&second.owner);
        assert_ne!(first_key, second_key);
        let first_guard = acquire_owner_coordinator(first_key).expect("first coordinator guard");
        let (second_tx, second_rx) = mpsc::channel();
        let second_thread = thread::spawn(move || {
            let guard = acquire_owner_coordinator(second_key).expect("second coordinator guard");
            second_tx.send(()).expect("second guard ready");
            drop(guard);
        });

        second_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("different owner must not wait");
        drop(first_guard);
        second_thread.join().expect("second thread");
    }

    #[test]
    fn owner_coordinator_alias_paths_share_the_physical_provider() {
        let fixture = TestRoot::new();
        let plain_key = provider_key(&fixture.owner);
        let alias = OwnerBase::at_home(
            fixture.root.join("home").join("..").join("home"),
            PlatformTag::Linux,
        )
        .expect("alias owner");
        let alias_key = provider_key(&alias);
        assert_eq!(plain_key, alias_key);
        let first_guard = acquire_owner_coordinator(plain_key).expect("plain coordinator guard");
        let (alias_tx, alias_rx) = mpsc::channel();
        let alias_thread = thread::spawn(move || {
            let guard = acquire_owner_coordinator(alias_key).expect("alias coordinator guard");
            alias_tx.send(()).expect("alias guard ready");
            drop(guard);
        });

        wait_for_owner_coordinator_waiters(plain_key, 1);
        assert!(matches!(
            alias_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(first_guard);
        alias_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("alias guard after release");
        alias_thread.join().expect("alias thread");
    }

    #[test]
    fn owner_coordinator_registry_reclaims_after_hold_wait_and_arrival() {
        let fixture = TestRoot::new();
        let key = provider_key(&fixture.owner);
        let sweep = TestRoot::new();
        let sweep_key = provider_key(&sweep.owner);
        drop(acquire_owner_coordinator(sweep_key).expect("sweep coordinator guard"));
        assert!(!owner_coordinator_entry_exists_for_test(key));
        let first_guard = acquire_owner_coordinator(key).expect("first coordinator guard");
        let first_identity = Arc::as_ptr(&first_guard.coordinator) as usize;
        let active = Arc::new(AtomicUsize::new(0));
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (first_release_tx, first_release_rx) = mpsc::channel();
        let (second_release_tx, second_release_rx) = mpsc::channel();

        let first_active = Arc::clone(&active);
        let first_acquired = acquired_tx.clone();
        let first_waiter = thread::spawn(move || {
            let guard = acquire_owner_coordinator(key).expect("first waiter coordinator guard");
            assert_eq!(first_active.fetch_add(1, Ordering::SeqCst), 0);
            first_acquired
                .send((1_u8, Arc::as_ptr(&guard.coordinator) as usize))
                .expect("first waiter acquired");
            first_release_rx.recv().expect("release first waiter");
            assert_eq!(first_active.fetch_sub(1, Ordering::SeqCst), 1);
            drop(guard);
        });
        wait_for_owner_coordinator_waiters(key, 1);

        let second_active = Arc::clone(&active);
        let second_waiter = thread::spawn(move || {
            let guard = acquire_owner_coordinator(key).expect("second waiter coordinator guard");
            assert_eq!(second_active.fetch_add(1, Ordering::SeqCst), 0);
            acquired_tx
                .send((2_u8, Arc::as_ptr(&guard.coordinator) as usize))
                .expect("second waiter acquired");
            second_release_rx.recv().expect("release second waiter");
            assert_eq!(second_active.fetch_sub(1, Ordering::SeqCst), 1);
            drop(guard);
        });
        wait_for_owner_coordinator_waiters(key, 2);

        drop(first_guard);
        let (first_label, first_waiter_identity) = acquired_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("one queued waiter acquired");
        assert_eq!(first_waiter_identity, first_identity);
        assert_eq!(active.load(Ordering::SeqCst), 1);
        assert!(matches!(
            acquired_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_waiter(first_label, &first_release_tx, &second_release_tx);

        let (second_label, second_waiter_identity) = acquired_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("remaining queued waiter acquired");
        assert_ne!(first_label, second_label);
        assert_eq!(second_waiter_identity, first_identity);
        assert_eq!(active.load(Ordering::SeqCst), 1);
        release_waiter(second_label, &first_release_tx, &second_release_tx);
        first_waiter.join().expect("first waiter thread");
        second_waiter.join().expect("second waiter thread");
        assert_eq!(active.load(Ordering::SeqCst), 0);

        let cleanup = TestRoot::new();
        let cleanup_key = provider_key(&cleanup.owner);
        drop(acquire_owner_coordinator(cleanup_key).expect("cleanup coordinator guard"));
        assert!(!owner_coordinator_entry_exists_for_test(key));
        let fresh_guard = acquire_owner_coordinator(key).expect("fresh coordinator guard");
        drop(fresh_guard);
        drop(acquire_owner_coordinator(cleanup_key).expect("second cleanup coordinator guard"));
        assert!(!owner_coordinator_entry_exists_for_test(key));

        for _ in 0..4 {
            let short_lived = TestRoot::new();
            let short_lived_key = provider_key(&short_lived.owner);
            assert!(!owner_coordinator_entry_exists_for_test(short_lived_key));
            drop(
                acquire_owner_coordinator(short_lived_key).expect("short-lived coordinator guard"),
            );
            drop(acquire_owner_coordinator(cleanup_key).expect("retain coordinator guard"));
            assert!(!owner_coordinator_entry_exists_for_test(short_lived_key));
        }
    }

    #[test]
    fn poisoned_owner_coordinator_refuses_before_owner_file_lock() {
        let fixture = TestRoot::new();
        let request = fixture.request(b"/install/poisoned", b"/journal/poisoned");
        let root = request.root_token.clone();
        let admission = admit_setup(request).expect("initial admission");
        drop(admission);
        let key = provider_key(&fixture.owner);
        let coordinator = owner_coordinator_for(key).expect("owner coordinator");
        let poisoned = Arc::clone(&coordinator);
        let poisoner = thread::spawn(move || {
            let _state = poisoned.state.lock().expect("poison coordinator state");
            panic!("poison owner coordinator");
        });
        assert!(poisoner.join().is_err());

        let before = owner_file_lock_attempts_for_test();
        let result = load_installation_binding(&fixture.owner, &root);
        let after = owner_file_lock_attempts_for_test();

        assert!(matches!(
            result,
            Err(IdentityError::UnsafeState("owner coordinator is poisoned"))
        ));
        assert_eq!(after, before);
        drop(coordinator);
    }

    #[test]
    fn load_holds_owner_coordinator_for_its_whole_call() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let request = fixture.request(b"/install/load-coordinator", b"/journal/load-coordinator");
        let root = request.root_token.clone();
        let admission = admit_setup(request).expect("initial admission");
        drop(admission);
        let key = provider_key(&fixture.owner);
        let gate = park_at(FaultPoint::LoadLocksHeld);
        let owner = fixture.owner.clone();
        let reader = thread::spawn(move || load_installation_binding(&owner, &root));

        gate.arrived.wait();
        assert!(owner_coordinator_held_for_test(key));
        gate.release.wait();
        reader
            .join()
            .expect("reader thread")
            .expect("loaded binding");
        clear_control();
        assert!(!owner_coordinator_held_for_test(key));
    }

    #[test]
    fn setup_and_clean_sessions_hold_owner_coordinator_until_drop() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let request = fixture.request(
            b"/install/session-coordinator",
            b"/journal/session-coordinator",
        );
        let root = request.root_token.clone();
        let initial = admit_setup(request.clone()).expect("initial admission");
        let key = provider_key(&fixture.owner);
        assert!(owner_coordinator_held_for_test(key));
        drop(initial);
        assert!(!owner_coordinator_held_for_test(key));

        let setup = admit_setup(request).expect("existing setup admission");
        assert!(owner_coordinator_held_for_test(key));
        drop(setup);
        assert!(!owner_coordinator_held_for_test(key));

        let clean = admit_clean_uninstall(CleanUninstallRequest {
            owner: fixture.owner.clone(),
            root_token: root,
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("clean uninstall admission");
        assert!(owner_coordinator_held_for_test(key));
        clean.commit_tombstone().expect("commit tombstone");
        assert!(!owner_coordinator_held_for_test(key));
    }

    #[test]
    fn first_admission_holds_owner_coordinator_through_no_replace_publication() {
        let _serial = serial();
        clear_control();
        let held_fixture = TestRoot::new();
        let held_gate = park_at(FaultPoint::PreparedWrite);
        let held_request =
            held_fixture.request(b"/install/prepublish-held", b"/journal/prepublish-held");
        let held_thread = thread::spawn(move || {
            admit_setup(held_request)
                .expect("held first admission")
                .binding()
                .clone()
        });

        held_gate.arrived.wait();
        let held_key = provider_key(&held_fixture.owner);
        assert!(owner_coordinator_held_for_test(held_key));
        held_gate.release.wait();
        held_thread.join().expect("held admission thread");
        clear_control();
        assert!(!owner_coordinator_held_for_test(held_key));

        let released_fixture = TestRoot::new();
        let released_gate = park_at(FaultPoint::PreparedNoReplace);
        let released_request = released_fixture.request(
            b"/install/prepublish-released",
            b"/journal/prepublish-released",
        );
        let released_thread = thread::spawn(move || {
            admit_setup(released_request)
                .expect("released first admission")
                .binding()
                .clone()
        });

        released_gate.arrived.wait();
        let released_key = provider_key(&released_fixture.owner);
        assert!(owner_coordinator_held_for_test(released_key));
        released_gate.release.wait();
        released_thread.join().expect("released admission thread");
        clear_control();
    }

    #[test]
    fn first_admission_is_canonical_durable_and_umask_independent() {
        let _serial = serial();
        clear_control();
        let _umask = UmaskGuard::set(Mode::from_bits_truncate(0o777));
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/a", b"/journal/a")).expect("admit setup");
        let namespace = fixture.namespace_path(b"/install/a");

        let record_bytes = fs::read(namespace.join("record")).expect("read canonical record");
        assert_eq!(
            record_bytes,
            encode_record(&read_record(&namespace.join("record"))).expect("encode record")
        );
        assert_eq!(admission.binding().id.as_hex().len(), 32);
        assert!(
            admission
                .binding()
                .id
                .as_hex()
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(mode(&fixture.owner.path()), 0o700);
        assert_eq!(mode(&fixture.owner.path().join("namespaces")), 0o700);
        assert_eq!(mode(&namespace), 0o700);
        assert_eq!(mode(&namespace.join("record")), 0o600);
        assert_eq!(mode(&namespace.join("adoption.marker")), 0o600);
        assert_eq!(mode(&fixture.owner.path().join("owner.lock")), 0o600);
        assert!(record_bytes.len() <= MAX_RECORD_BYTES);
    }

    #[test]
    fn load_returns_the_admitted_binding_without_mutating_storage() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/load", b"/journal/load")).expect("admit");
        let expected = admission.binding().clone();
        drop(admission);
        let root = RootToken::from_raw_absolute(b"/install/load".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let loaded = load_installation_binding(&fixture.owner, &root).expect("load binding");

        assert_eq!(loaded, expected);
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_an_absent_provider_without_creation() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let root =
            RootToken::from_raw_absolute(b"/install/absent-provider".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(
            result,
            Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(!fixture.owner.path().exists());
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_an_absent_namespace_without_creation() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/present", b"/journal/present")).expect("admit");
        drop(admission);
        let root = RootToken::from_raw_absolute(b"/install/absent-namespace".to_vec())
            .expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(
            result,
            Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(
            !fixture
                .namespace_path(b"/install/absent-namespace")
                .exists()
        );
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_missing_owner_lock_without_creation() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/missing-lock", b"/journal/missing-lock"))
                .expect("admit");
        drop(admission);
        let owner_lock = fixture.owner.path().join("owner.lock");
        fs::remove_file(&owner_lock).expect("remove owner lock");
        let root =
            RootToken::from_raw_absolute(b"/install/missing-lock".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(
            result,
            Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(!owner_lock.exists());
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_wrong_mode_owner_lock_without_repairing_it() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/wrong-lock-mode", b"/journal/wrong-lock-mode"))
                .expect("admit");
        drop(admission);
        let owner_lock = fixture.owner.path().join("owner.lock");
        fs::set_permissions(&owner_lock, fs::Permissions::from_mode(0o644))
            .expect("make owner lock mode wrong");
        let root =
            RootToken::from_raw_absolute(b"/install/wrong-lock-mode".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(result, Err(IdentityError::UnsafeState(_))));
        assert_eq!(mode(&owner_lock), 0o644);
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_namespace_without_a_record_without_mutating_it() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let root =
            RootToken::from_raw_absolute(b"/install/missing-record".to_vec()).expect("root token");
        let namespace_name = namespace_name(PlatformTag::Linux, &root);
        let provider = open_provider(&fixture.owner, true).expect("provider");
        let namespace = open_namespace(&provider, &namespace_name, true).expect("namespace");
        drop(namespace);
        drop(provider);
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(
            result,
            Err(IdentityError::UnsafeState("namespace record is missing"))
        ));
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_committed_record_without_a_marker_without_mutating_it() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/missing-marker", b"/journal/missing-marker"))
                .expect("admit");
        drop(admission);
        let namespace = fixture.namespace_path(b"/install/missing-marker");
        fs::remove_file(namespace.join("adoption.marker")).expect("remove marker");
        let root =
            RootToken::from_raw_absolute(b"/install/missing-marker".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(result, Err(IdentityError::UnsafeState(_))));
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_prepared_record_without_locking_its_missing_marker() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let request = fixture.request(b"/install/prepared", b"/journal/prepared");
        fail_at(FaultPoint::MarkerCreation);
        assert!(admit_setup(request).is_err());
        clear_control();
        let root = RootToken::from_raw_absolute(b"/install/prepared".to_vec()).expect("root token");
        let namespace = fixture.namespace_path(b"/install/prepared");
        assert!(!namespace.join("adoption.marker").exists());
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(result, Err(IdentityError::NotAdopted(_))));
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_tombstoned_record_without_mutating_it() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission = admit_setup(fixture.request(b"/install/tombstone", b"/journal/tombstone"))
            .expect("admit");
        drop(admission);
        let root =
            RootToken::from_raw_absolute(b"/install/tombstone".to_vec()).expect("root token");
        admit_clean_uninstall(CleanUninstallRequest {
            owner: fixture.owner.clone(),
            root_token: root.clone(),
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("clean uninstall admission")
        .commit_tombstone()
        .expect("tombstone");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(result, Err(IdentityError::NotAdopted(_))));
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_corrupt_record_without_mutating_it() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/corrupt", b"/journal/corrupt")).expect("admit");
        drop(admission);
        let namespace = fixture.namespace_path(b"/install/corrupt");
        fs::write(namespace.join("record"), b"not a canonical record\n").expect("corrupt record");
        let root = RootToken::from_raw_absolute(b"/install/corrupt".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(result, Err(IdentityError::Record(_))));
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_refuses_a_record_bound_to_another_root_or_platform() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission = admit_setup(fixture.request(b"/install/mismatch", b"/journal/mismatch"))
            .expect("admit");
        drop(admission);
        let namespace = fixture.namespace_path(b"/install/mismatch");
        let mut record = read_record(&namespace.join("record"));
        record.platform = PlatformTag::Macos;
        record.root_token =
            RootToken::from_raw_absolute(b"/install/other".to_vec()).expect("other root token");
        fs::write(
            namespace.join("record"),
            encode_record(&record).expect("encode mismatched record"),
        )
        .expect("write mismatched record");
        let root = RootToken::from_raw_absolute(b"/install/mismatch".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(result, Err(IdentityError::AdmissionRefused(_))));
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_symlinked_owner_lock_without_mutating_it() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/symlink", b"/journal/symlink")).expect("admit");
        drop(admission);
        let owner_lock = fixture.owner.path().join("owner.lock");
        fs::remove_file(&owner_lock).expect("remove owner lock");
        std::os::unix::fs::symlink("record", &owner_lock).expect("symlink owner lock");
        let root = RootToken::from_raw_absolute(b"/install/symlink".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(result, Err(IdentityError::Io { .. })));
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_a_wrong_mode_record_without_repairing_it() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission = admit_setup(
            fixture.request(b"/install/wrong-record-mode", b"/journal/wrong-record-mode"),
        )
        .expect("admit");
        drop(admission);
        let record = fixture
            .namespace_path(b"/install/wrong-record-mode")
            .join("record");
        fs::set_permissions(&record, fs::Permissions::from_mode(0o644))
            .expect("make record mode wrong");
        let root = RootToken::from_raw_absolute(b"/install/wrong-record-mode".to_vec())
            .expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(result, Err(IdentityError::UnsafeState(_))));
        assert_eq!(mode(&record), 0o644);
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_rejects_an_unreadable_record_without_mutating_it() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let admission =
            admit_setup(fixture.request(b"/install/unreadable", b"/journal/unreadable"))
                .expect("admit");
        drop(admission);
        let record = fixture
            .namespace_path(b"/install/unreadable")
            .join("record");
        fs::set_permissions(&record, fs::Permissions::from_mode(0o000))
            .expect("make record unreadable");
        let root =
            RootToken::from_raw_absolute(b"/install/unreadable".to_vec()).expect("root token");
        let before = snapshot_tree(&fixture.root);

        let result = load_installation_binding(&fixture.owner, &root);

        assert!(matches!(
            result,
            Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::PermissionDenied
        ));
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn load_waits_for_an_owner_locked_adopted_replacement() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let root =
            RootToken::from_raw_absolute(b"/install/locked-update".to_vec()).expect("root token");
        let admission = admit_setup(fixture.request(b"/install/locked-update", b"/journal/one"))
            .expect("initial admission");
        drop(admission);
        let gate = park_at(FaultPoint::AdoptedReplace);
        let writer_request = fixture.request(b"/install/locked-update", b"/journal/two");
        let (writer_tx, writer_rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            let admission = admit_setup(writer_request).expect("writer admission");
            let binding = admission.binding().clone();
            drop(admission);
            writer_tx.send(binding).expect("writer result");
        });
        gate.arrived.wait();

        let (started_tx, started_rx) = mpsc::channel();
        let (reader_tx, reader_rx) = mpsc::channel();
        let reader_owner = fixture.owner.clone();
        let reader_root = root.clone();
        let reader = thread::spawn(move || {
            started_tx.send(()).expect("reader started");
            reader_tx
                .send(load_installation_binding(&reader_owner, &reader_root))
                .expect("reader result");
        });
        started_rx.recv().expect("reader start signal");
        let early = reader_rx.recv_timeout(Duration::from_millis(100));
        let reader_was_blocked = matches!(&early, Err(RecvTimeoutError::Timeout));
        gate.release.wait();

        let writer_binding = writer_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("writer completion");
        let reader_binding = match early {
            Err(RecvTimeoutError::Timeout) => reader_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("reader completion")
                .expect("reader binding"),
            Ok(result) => result.expect("reader binding before writer release"),
            Err(RecvTimeoutError::Disconnected) => panic!("reader disconnected"),
        };
        writer.join().expect("writer thread");
        reader.join().expect("reader thread");
        clear_control();

        assert!(reader_was_blocked);
        assert_eq!(reader_binding, writer_binding);
        assert_eq!(
            reader_binding.journal_token.to_path_buf(),
            PathBuf::from("/journal/two")
        );
    }

    #[test]
    fn real_threads_converge_at_the_no_replace_publication() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let key = provider_key(&fixture.owner);
        let gate = park_at(FaultPoint::PreparedNoReplace);
        let first = fixture.request(b"/install/race", b"/journal/race");
        let second = first.clone();
        let one = thread::spawn(move || {
            admit_setup(first)
                .expect("first admission")
                .binding()
                .clone()
        });
        gate.arrived.wait();
        let two = thread::spawn(move || {
            admit_setup(second)
                .expect("second admission")
                .binding()
                .clone()
        });
        wait_for_owner_coordinator_waiters(key, 1);
        gate.release.wait();
        let one = one.join().expect("first thread");
        let two = two.join().expect("second thread");
        clear_control();
        assert_eq!(one.id, two.id);
        assert_eq!(one.generation, Generation::new(1).expect("generation"));
        assert_eq!(one.namespace, two.namespace);
    }

    #[test]
    fn every_commit_fault_resumes_or_retries_at_the_correct_boundary() {
        let _serial = serial();
        let prepublication = [
            FaultPoint::PreparedWrite,
            FaultPoint::PreparedFileSync,
            FaultPoint::PreparedNoReplace,
        ];
        let surviving_prepared = [
            FaultPoint::PreparedDirectorySync,
            FaultPoint::MarkerCreation,
            FaultPoint::MarkerSync,
            FaultPoint::MarkerLock,
            FaultPoint::AdoptedReplace,
            FaultPoint::AdoptedFileSync,
        ];
        for point in prepublication.into_iter().chain(surviving_prepared) {
            clear_control();
            let fixture = TestRoot::new();
            let request = fixture.request(b"/install/fault", b"/journal/fault");
            fail_at(point);
            assert!(admit_setup(request.clone()).is_err(), "{point:?} must fail");
            clear_control();
            let record_path = fixture.namespace_path(b"/install/fault").join("record");
            let prepared = if record_path.exists() {
                Some(read_record(&record_path))
            } else {
                None
            };
            let admitted = admit_setup(request).expect("retry admission");
            if let Some(prepared) = prepared {
                assert_eq!(
                    prepared.id,
                    admitted.binding().id,
                    "{point:?} must resume the surviving prepared ID"
                );
                assert_eq!(prepared.generation, admitted.binding().generation);
            }
        }
        let point = FaultPoint::AdoptedDirectorySync;
        clear_control();
        let fixture = TestRoot::new();
        let request = fixture.request(b"/install/committed", b"/journal/committed");
        fail_at(point);
        assert!(admit_setup(request.clone()).is_err());
        clear_control();
        let before = read_record(&fixture.namespace_path(b"/install/committed").join("record"));
        assert_eq!(before.state, LifecycleState::Adopted);
        let retry = admit_setup(request).expect("trusted retry");
        assert_eq!(before.id, retry.binding().id);
        clear_control();
    }

    #[test]
    fn stale_stages_are_removed_never_reused() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let namespace_name = namespace_name(
            PlatformTag::Linux,
            &RootToken::from_raw_absolute(b"/install/stale".to_vec()).expect("root"),
        );
        let provider = open_provider(&fixture.owner, true).expect("provider");
        let namespace = open_namespace(&provider, &namespace_name, true).expect("namespace");
        let stale = IdentityRecord {
            state: LifecycleState::Prepared,
            generation: Generation::new(1).expect("generation"),
            id: InstallationId::parse("00112233445566778899aabbccddeeff").expect("id"),
            platform: PlatformTag::Linux,
            root_token: RootToken::from_raw_absolute(b"/install/stale".to_vec()).expect("root"),
            journal_token: JournalToken::from_raw_absolute(b"/journal/stale".to_vec())
                .expect("journal"),
        };
        let stale_path = namespace
            .path
            .join(".prepared-00112233445566778899aabbccddeeff");
        fs::write(&stale_path, encode_record(&stale).expect("stale record"))
            .expect("write stale stage");
        fs::set_permissions(&stale_path, fs::Permissions::from_mode(0o600))
            .expect("set stage mode");
        let admission = admit_setup(fixture.request(b"/install/stale", b"/journal/stale"))
            .expect("admit after stale stage");
        assert!(!stale_path.exists());
        assert_ne!(admission.binding().id, stale.id);
    }

    #[test]
    fn fixed_vectors_cover_non_utf8_tokens_checksum_and_limits() {
        let _serial = serial();
        let root = RootToken::from_raw_absolute(vec![b'/', b'r', b'o', b'o', b't', b'/', 0xff])
            .expect("non-UTF8 root");
        let journal = JournalToken::from_raw_absolute(vec![
            b'/', b'j', b'o', b'u', b'r', b'n', b'a', b'l', b'/', 0xfe,
        ])
        .expect("non-UTF8 journal");
        let namespace = namespace_name(PlatformTag::Linux, &root);
        assert_eq!(
            namespace.as_hex(),
            "7e256e6ba0a49340d0c9b3271b6b02a70a476f745d212858e905c18fd8ba84ec"
        );
        for state in [
            LifecycleState::Prepared,
            LifecycleState::Adopted,
            LifecycleState::Tombstoned,
        ] {
            let record = IdentityRecord {
                state,
                generation: Generation::new(17).expect("generation"),
                id: InstallationId::parse("00112233445566778899aabbccddeeff").expect("id"),
                platform: PlatformTag::Linux,
                root_token: root.clone(),
                journal_token: journal.clone(),
            };
            let encoded = encode_record(&record).expect("encode vector");
            assert_eq!(decode_record(&encoded).expect("decode vector"), record);
            let mut corrupt = encoded;
            corrupt[0] ^= 1;
            assert!(matches!(
                decode_record(&corrupt),
                Err(RecordError::InvalidField("schema_version"))
            ));
        }
        let maximal = vec![b'a'; 4095];
        let root =
            RootToken::from_raw_absolute([vec![b'/'], maximal.clone()].concat()).expect("max root");
        let journal =
            JournalToken::from_raw_absolute([vec![b'/'], maximal].concat()).expect("max journal");
        let record = IdentityRecord {
            state: LifecycleState::Adopted,
            generation: Generation::new(u64::MAX).expect("max generation"),
            id: InstallationId::parse("ffffffffffffffffffffffffffffffff").expect("id"),
            platform: PlatformTag::Macos,
            root_token: root,
            journal_token: journal,
        };
        assert!(encode_record(&record).expect("max record").len() <= MAX_RECORD_BYTES);
        assert!(RootToken::from_raw_absolute(vec![b'a'; 4097]).is_err());
        assert!(InstallationId::parse("00112233445566778899AABBCCDDEEFF").is_err());
    }

    #[test]
    fn enumeration_is_bounded_and_ignores_outside_siblings() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        fs::create_dir(fixture.root.join("home").join("unrelated")).expect("outside sibling");
        fs::set_permissions(
            fixture.root.join("home").join("unrelated"),
            fs::Permissions::from_mode(0o000),
        )
        .expect("unreadable sibling");
        admit_setup(fixture.request(b"/install/clean", b"/journal/clean"))
            .expect("outside sibling ignored");
        let provider = open_provider(&fixture.owner, true).expect("provider");
        let registry = provider.path.join("namespaces");
        fs::create_dir(registry.join("not-a-namespace")).expect("malformed entry");
        assert!(admit_setup(fixture.request(b"/install/another", b"/journal/another")).is_err());
    }

    #[test]
    fn registry_entry_and_byte_limits_fail_before_admission() {
        let _serial = serial();
        clear_control();
        let entries = TestRoot::new();
        let provider = open_provider(&entries.owner, true).expect("provider");
        let registry = provider.path.join("namespaces");
        for index in 0..=MAX_NAMESPACES {
            let directory = registry.join(format!("{index:064x}"));
            fs::create_dir(&directory).expect("create namespace entry");
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("set namespace mode");
        }
        assert!(matches!(
            admit_setup(entries.request(b"/install/over-entries", b"/journal/over-entries")),
            Err(IdentityError::UnsafeState(
                "namespace registry exceeds 1024 entries"
            ))
        ));

        let bytes = TestRoot::new();
        let provider = open_provider(&bytes.owner, true).expect("provider");
        let registry = provider.path.join("namespaces");
        let maximal = vec![b'x'; 4095];
        let record = IdentityRecord {
            state: LifecycleState::Prepared,
            generation: Generation::new(1).expect("generation"),
            id: InstallationId::parse("00112233445566778899aabbccddeeff").expect("id"),
            platform: PlatformTag::Linux,
            root_token: RootToken::from_raw_absolute([vec![b'/'], maximal.clone()].concat())
                .expect("root"),
            journal_token: JournalToken::from_raw_absolute([vec![b'/'], maximal].concat())
                .expect("journal"),
        };
        let encoded = encode_record(&record).expect("maximal record");
        assert!(encoded.len() * MAX_NAMESPACES > MAX_REGISTRY_BYTES as usize);
        for index in 0..MAX_NAMESPACES {
            let directory = registry.join(format!("{index:064x}"));
            fs::create_dir(&directory).expect("create namespace entry");
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("set namespace mode");
            let file = directory.join("record");
            fs::write(&file, &encoded).expect("write namespace record");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("set record mode");
        }
        assert!(matches!(
            admit_setup(bytes.request(b"/install/over-bytes", b"/journal/over-bytes")),
            Err(IdentityError::UnsafeState(
                "namespace registry exceeds 16 MiB"
            ))
        ));
    }

    #[test]
    fn clean_uninstall_uses_each_record_journal_and_last_root_policy() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        admit_setup(fixture.request(b"/install/a", b"/journal/a")).expect("setup A");
        admit_setup(fixture.request(b"/install/b", b"/journal/b")).expect("setup B");
        let root_a = RootToken::from_raw_absolute(b"/install/a".to_vec()).expect("root A");
        let first = admit_clean_uninstall(CleanUninstallRequest {
            owner: fixture.owner.clone(),
            root_token: root_a,
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("uninstall A admission");
        assert!(!first.plan().remove_owner_config);
        assert!(first.plan().remove_journal_manifest);
        first.commit_tombstone().expect("tombstone A");
        let root_b = RootToken::from_raw_absolute(b"/install/b".to_vec()).expect("root B");
        let last = admit_clean_uninstall(CleanUninstallRequest {
            owner: fixture.owner.clone(),
            root_token: root_b,
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("uninstall B admission");
        assert!(last.plan().remove_owner_config);
        assert!(last.plan().remove_journal_manifest);
    }

    #[test]
    fn clean_uninstall_preserves_a_shared_journal_manifest_until_the_last_root() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        admit_setup(fixture.request(b"/install/a", b"/journal/shared")).expect("setup A");
        admit_setup(fixture.request(b"/install/b", b"/journal/shared")).expect("setup B");
        let first = admit_clean_uninstall(CleanUninstallRequest {
            owner: fixture.owner.clone(),
            root_token: RootToken::from_raw_absolute(b"/install/a".to_vec()).expect("root A"),
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("uninstall A admission");
        assert!(!first.plan().remove_owner_config);
        assert!(!first.plan().remove_journal_manifest);
        first.commit_tombstone().expect("tombstone A");
        let last = admit_clean_uninstall(CleanUninstallRequest {
            owner: fixture.owner.clone(),
            root_token: RootToken::from_raw_absolute(b"/install/b".to_vec()).expect("root B"),
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("uninstall B admission");
        assert!(last.plan().remove_owner_config);
        assert!(last.plan().remove_journal_manifest);
    }

    #[test]
    fn setup_from_a_tombstone_generates_one_new_generation() {
        let _serial = serial();
        clear_control();
        let fixture = TestRoot::new();
        let request = fixture.request(b"/install/revive", b"/journal/revive");
        let initial = admit_setup(request.clone()).expect("initial setup");
        let initial_id = initial.binding().id.clone();
        drop(initial);
        admit_clean_uninstall(CleanUninstallRequest {
            owner: fixture.owner.clone(),
            root_token: RootToken::from_raw_absolute(b"/install/revive".to_vec()).expect("root"),
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("uninstall admission")
        .commit_tombstone()
        .expect("tombstone");
        let revived = admit_setup(request).expect("revived setup");
        assert_ne!(revived.binding().id, initial_id);
        assert_eq!(
            revived.binding().generation,
            Generation::new(2).expect("generation")
        );
    }

    #[test]
    fn wrapper_and_service_guards_round_trip_without_parallel_formats() {
        let _serial = serial();
        let binding = InstallationBinding {
            namespace: NamespaceName::parse(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("namespace"),
            id: InstallationId::parse("00112233445566778899aabbccddeeff").expect("id"),
            generation: Generation::new(9).expect("generation"),
            platform: PlatformTag::Linux,
            root_token: RootToken::from_raw_absolute(b"/install/guard".to_vec()).expect("root"),
            journal_token: JournalToken::from_raw_absolute(b"/journal/guard".to_vec())
                .expect("journal"),
        };
        let fields = GuardFields::from_binding(&binding);
        let changed_journal = GuardFields {
            journal_token: JournalToken::from_raw_absolute(b"/journal/changed".to_vec())
                .expect("changed journal"),
            ..fields.clone()
        };
        assert_ne!(fields, changed_journal);
        assert!(fields.same_identity(&changed_journal));
        let wrapper = wrapper_guard_lines(&fields);
        assert_eq!(
            parse_wrapper_guard(&wrapper).expect("parse wrapper"),
            Some(GuardFields::from_binding(&binding))
        );
        let environment = service_guard_environment(&fields);
        assert_eq!(
            parse_service_guard_environment(&environment).expect("parse environment"),
            Some(GuardFields::from_binding(&binding))
        );
        assert!(
            parse_wrapper_guard("# solstone-installation-id: 00112233445566778899aabbccddeeff\n")
                .is_err()
        );
        assert!(parse_wrapper_guard("# solstone-installation-unexpected: value\n").is_err());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::fs;
    use std::os::windows::fs::MetadataExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static TEST_SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

    #[derive(Debug, Eq, PartialEq)]
    enum TreeEntry {
        Directory {
            attributes: u32,
        },
        File {
            attributes: u32,
            bytes: Option<Vec<u8>>,
        },
        Reparse {
            attributes: u32,
        },
        Other {
            attributes: u32,
        },
    }

    struct TestRoot {
        root: PathBuf,
        local_app_data: PathBuf,
        owner: OwnerBase,
    }

    impl TestRoot {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "solstone-installation-identity-windows-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&root).expect("create Windows test root");
            let local_app_data = root.join("local-app-data");
            fs::create_dir(&local_app_data).expect("create LocalAppData fixture");
            set_known_folder_override_for_test(Some(local_app_data.clone()));
            let owner = OwnerBase::at_home(root.join("ignored-home"), PlatformTag::Windows)
                .expect("Windows owner base");
            Self {
                root,
                local_app_data,
                owner,
            }
        }

        fn request(&self, root: &str, journal: &str) -> SetupAdmissionRequest {
            SetupAdmissionRequest {
                owner: self.owner.clone(),
                root_token: RootToken::from_raw_absolute(token_bytes(root)).expect("root token"),
                journal_token: JournalToken::from_raw_absolute(token_bytes(journal))
                    .expect("journal token"),
                journal_is_explicit: true,
                legacy_manifest: LegacyManifestEvidence::Absent,
                artifacts: ArtifactBindingEvidence::Fresh,
            }
        }

        fn root_token(&self, root: &str) -> RootToken {
            RootToken::from_raw_absolute(token_bytes(root)).expect("root token")
        }

        fn namespace_path(&self, root: &str) -> PathBuf {
            let root = self.root_token(root);
            self.owner
                .path()
                .join("namespaces")
                .join(namespace_name(PlatformTag::Windows, &root).as_hex())
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            set_known_folder_override_for_test(None);
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_SERIAL
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn token_bytes(value: &str) -> Vec<u8> {
        value.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, TreeEntry> {
        fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<PathBuf, TreeEntry>) {
            let metadata = fs::symlink_metadata(path).expect("snapshot metadata");
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path is relative")
                .to_path_buf();
            let attributes = metadata.file_attributes();
            let file_type = metadata.file_type();
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                snapshot.insert(relative, TreeEntry::Reparse { attributes });
            } else if file_type.is_dir() {
                snapshot.insert(relative, TreeEntry::Directory { attributes });
                let mut entries = fs::read_dir(path)
                    .expect("snapshot directory")
                    .map(|entry| entry.expect("snapshot directory entry"))
                    .collect::<Vec<_>>();
                entries.sort_by_key(|entry| entry.file_name());
                for entry in entries {
                    visit(root, &entry.path(), snapshot);
                }
            } else if file_type.is_file() {
                snapshot.insert(
                    relative,
                    TreeEntry::File {
                        attributes,
                        bytes: fs::read(path).ok(),
                    },
                );
            } else {
                snapshot.insert(relative, TreeEntry::Other { attributes });
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    fn assert_tree_unchanged(root: &Path, before: BTreeMap<PathBuf, TreeEntry>) {
        assert_eq!(snapshot_tree(root), before);
    }

    fn provider_key(owner: &OwnerBase) -> OwnerCoordinatorKey {
        let provider = open_provider(owner, true).expect("open provider");
        owner_coordinator_key(&provider.file).expect("owner coordinator key")
    }

    fn wait_for_owner_coordinator_waiters(key: OwnerCoordinatorKey, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while owner_coordinator_waiters_for_test(key) < expected {
            assert!(
                Instant::now() < deadline,
                "owner coordinator never reached {expected} waiters"
            );
            thread::yield_now();
        }
    }

    #[test]
    fn known_folder_injection_ignores_home_argument() {
        let _serial = serial();
        clear_fault_control_for_test();
        let fixture = TestRoot::new();
        let expected = fixture
            .local_app_data
            .join("solstone")
            .join("installation-identity")
            .join("v1");
        assert_eq!(fixture.owner.path(), expected);
        let alternate = OwnerBase::at_home(
            fixture.root.join("a-completely-different-home"),
            PlatformTag::Windows,
        )
        .expect("alternate owner");
        assert_eq!(alternate.path(), fixture.owner.path());
    }

    #[test]
    fn windows_lifecycle_admission_and_read_only_load_are_canonical() {
        let _serial = serial();
        clear_fault_control_for_test();
        let fixture = TestRoot::new();
        let root = r"C:\solstone\install";
        let admission =
            admit_setup(fixture.request(root, r"C:\solstone\journal")).expect("admit setup");
        assert_eq!(admission.binding().platform, PlatformTag::Windows);
        let expected = admission.binding().clone();
        drop(admission);
        let before = snapshot_tree(&fixture.root);

        let loaded = load_installation_binding(&fixture.owner, &fixture.root_token(root))
            .expect("load adopted binding");

        assert_eq!(loaded, expected);
        assert_tree_unchanged(&fixture.root, before);
    }

    #[test]
    fn windows_load_rejects_non_regular_provider_namespace_record_marker_and_lock() {
        let _serial = serial();
        clear_fault_control_for_test();
        enum StoragePoint {
            Provider,
            Namespace,
            Record,
            Marker,
            OwnerLock,
        }
        for point in [
            StoragePoint::Provider,
            StoragePoint::Namespace,
            StoragePoint::Record,
            StoragePoint::Marker,
            StoragePoint::OwnerLock,
        ] {
            let fixture = TestRoot::new();
            let root = r"C:\solstone\non-regular";
            let admission =
                admit_setup(fixture.request(root, r"C:\solstone\journal")).expect("admit setup");
            drop(admission);
            let target = match point {
                StoragePoint::Provider => fixture.owner.path(),
                StoragePoint::Namespace => fixture.namespace_path(root),
                StoragePoint::Record => fixture.namespace_path(root).join("record"),
                StoragePoint::Marker => fixture.namespace_path(root).join("adoption.marker"),
                StoragePoint::OwnerLock => fixture.owner.path().join("owner.lock"),
            };
            let was_directory = target.is_dir();
            if was_directory {
                fs::remove_dir_all(&target).expect("remove directory storage point");
                fs::write(&target, b"not a directory").expect("replace directory with file");
            } else {
                fs::remove_file(&target).expect("remove file storage point");
                fs::create_dir(&target).expect("replace file with directory");
            }
            let before = snapshot_tree(&fixture.root);

            assert!(load_installation_binding(&fixture.owner, &fixture.root_token(root)).is_err());
            assert_tree_unchanged(&fixture.root, before);
        }
    }

    #[test]
    fn windows_load_rejects_reparse_points_at_all_storage_points() {
        let _serial = serial();
        clear_fault_control_for_test();
        enum StoragePoint {
            Provider,
            Namespace,
            Record,
            Marker,
            OwnerLock,
        }
        for point in [
            StoragePoint::Provider,
            StoragePoint::Namespace,
            StoragePoint::Record,
            StoragePoint::Marker,
            StoragePoint::OwnerLock,
        ] {
            let fixture = TestRoot::new();
            let root = r"C:\solstone\reparse";
            let admission =
                admit_setup(fixture.request(root, r"C:\solstone\journal")).expect("admit setup");
            drop(admission);
            let (target, directory) = match point {
                StoragePoint::Provider => (fixture.owner.path(), true),
                StoragePoint::Namespace => (fixture.namespace_path(root), true),
                StoragePoint::Record => (fixture.namespace_path(root).join("record"), false),
                StoragePoint::Marker => {
                    (fixture.namespace_path(root).join("adoption.marker"), false)
                }
                StoragePoint::OwnerLock => (fixture.owner.path().join("owner.lock"), false),
            };
            let outside = fixture.root.join(format!(
                "reparse-target-{}",
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            if directory {
                fs::remove_dir_all(&target).expect("remove directory storage point");
                fs::create_dir(&outside).expect("create reparse directory target");
                if std::os::windows::fs::symlink_dir(&outside, &target).is_err() {
                    eprintln!(
                        "skipping reparse fixture: symlink creation unavailable (no Developer Mode / elevated privilege)"
                    );
                    return;
                }
            } else {
                fs::remove_file(&target).expect("remove file storage point");
                fs::write(&outside, b"outside").expect("create reparse file target");
                if std::os::windows::fs::symlink_file(&outside, &target).is_err() {
                    eprintln!(
                        "skipping reparse fixture: symlink creation unavailable (no Developer Mode / elevated privilege)"
                    );
                    return;
                }
            }
            let before = snapshot_tree(&fixture.root);

            assert!(load_installation_binding(&fixture.owner, &fixture.root_token(root)).is_err());
            assert_tree_unchanged(&fixture.root, before);
        }
    }

    #[test]
    fn windows_fault_retry_covers_every_publication_boundary() {
        let _serial = serial();
        for point in [
            FaultPoint::PreparedWrite,
            FaultPoint::PreparedFileSync,
            FaultPoint::PreparedNoReplace,
            FaultPoint::PreparedDirectorySync,
            FaultPoint::MarkerCreation,
            FaultPoint::MarkerSync,
            FaultPoint::MarkerLock,
            FaultPoint::AdoptedReplace,
            FaultPoint::AdoptedFileSync,
            FaultPoint::AdoptedDirectorySync,
        ] {
            clear_fault_control_for_test();
            let fixture = TestRoot::new();
            let request = fixture.request(r"C:\solstone\fault", r"C:\solstone\journal");
            fail_at_for_test(point);
            assert!(admit_setup(request.clone()).is_err(), "{point:?}");
            clear_fault_control_for_test();
            assert!(admit_setup(request).is_ok(), "retry after {point:?}");
        }

        let fixture = TestRoot::new();
        let root = r"C:\solstone\load-fault";
        let admission =
            admit_setup(fixture.request(root, r"C:\solstone\journal")).expect("admit setup");
        drop(admission);
        fail_at_for_test(FaultPoint::LoadLocksHeld);
        assert!(load_installation_binding(&fixture.owner, &fixture.root_token(root)).is_err());
        clear_fault_control_for_test();
        assert!(load_installation_binding(&fixture.owner, &fixture.root_token(root)).is_ok());
    }

    #[test]
    fn windows_owner_coordinator_excludes_aliases_and_holds_load() {
        let _serial = serial();
        clear_fault_control_for_test();
        let fixture = TestRoot::new();
        let root = r"C:\solstone\coordinator";
        let admission =
            admit_setup(fixture.request(root, r"C:\solstone\journal")).expect("admit setup");
        drop(admission);
        let key = provider_key(&fixture.owner);
        let alias = OwnerBase::at_home(fixture.root.join("lexical-alias"), PlatformTag::Windows)
            .expect("owner ignores home argument");
        assert_eq!(key, provider_key(&alias));

        let gate = park_at_for_test(FaultPoint::LoadLocksHeld);
        let owner = fixture.owner.clone();
        let root_token = fixture.root_token(root);
        let reader = thread::spawn(move || load_installation_binding(&owner, &root_token));
        gate.arrived.wait();
        assert!(owner_coordinator_held_for_test(key));

        let (acquired_tx, acquired_rx) = mpsc::channel();
        let contender = thread::spawn(move || {
            let _guard = acquire_owner_coordinator(key).expect("coordinator guard");
            acquired_tx.send(()).expect("coordinator acquired");
        });
        wait_for_owner_coordinator_waiters(key, 1);
        assert!(matches!(
            acquired_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        gate.release.wait();
        reader.join().expect("load thread").expect("loaded binding");
        acquired_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("contender acquired after load");
        contender.join().expect("contender thread");
        clear_fault_control_for_test();
    }

    #[test]
    fn windows_stable_file_identity_detects_lexical_provider_aliases() {
        let _serial = serial();
        clear_fault_control_for_test();
        let fixture = TestRoot::new();
        let provider = open_provider(&fixture.owner, true).expect("open provider");
        let lexical_alias = open_absolute_dir(&fixture.owner.path().join("."))
            .expect("open lexical provider alias");
        assert_eq!(
            owner_coordinator_key(&provider.file).expect("provider identity"),
            owner_coordinator_key(&lexical_alias.file).expect("alias identity"),
        );
    }

    #[test]
    fn windows_codec_round_trips_ascii_non_bmp_and_distinct_unit_sequences() {
        let canonical = [
            r"C:\solstone\ascii",
            r"C:\solstone\emoji-😀",
            r"\\server\share\non-bmp-𐐷",
        ];
        for path in canonical {
            let bytes = token_bytes(path);
            let token = JournalToken::from_raw_absolute(bytes.clone()).expect("canonical token");
            assert_eq!(token.as_bytes(), bytes);
            assert_eq!(
                token
                    .to_path_buf()
                    .as_os_str()
                    .encode_wide()
                    .collect::<Vec<_>>(),
                path.encode_utf16().collect::<Vec<_>>()
            );
        }
        assert_ne!(
            token_bytes(r"C:\solstone\😀"),
            token_bytes(r"C:\solstone\𐐷")
        );
        assert_eq!(
            normalize_windows_absolute_bytes(&token_bytes(r"c:/solstone//.\journal"))
                .expect("normalize Windows spelling"),
            token_bytes(r"C:\solstone\journal")
        );
    }

    #[test]
    fn windows_codec_rejects_malformed_utf16_and_noncanonical_path_classes() {
        let malformed = [0x00_u8, 0xd8];
        assert!(RootToken::from_raw_absolute(malformed.to_vec()).is_err());
        for path in [
            r"\current-drive",
            r"C:relative",
            r"relative\path",
            r"/unix-only",
            r"\\?\C:\verbatim",
            r"\\.\C:\device",
        ] {
            assert!(
                normalize_windows_absolute_bytes(&token_bytes(path)).is_err(),
                "{path}"
            );
        }
        assert!(RootToken::from_raw_absolute(vec![b'C', 0]).is_err());
        assert!(RootToken::from_raw_absolute(vec![0; MAX_TOKEN_BYTES + 2]).is_err());
        assert!(normalize_windows_absolute_bytes(&token_bytes(r"C:\..\escape")).is_err());
    }

    #[test]
    fn windows_clean_uninstall_tombstones_the_admitted_binding() {
        let _serial = serial();
        clear_fault_control_for_test();
        let fixture = TestRoot::new();
        let root = r"C:\solstone\uninstall";
        let admission =
            admit_setup(fixture.request(root, r"C:\solstone\journal")).expect("admit setup");
        drop(admission);
        let session = admit_clean_uninstall(CleanUninstallRequest {
            owner: fixture.owner.clone(),
            root_token: fixture.root_token(root),
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("admit clean uninstall");
        session.commit_tombstone().expect("commit tombstone");
        assert!(matches!(
            load_installation_binding(&fixture.owner, &fixture.root_token(root)),
            Err(IdentityError::NotAdopted(_))
        ));
    }
}
