// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-scoped, Unix-only installation identity records.
//!
//! This crate deliberately owns the small persistence protocol rather than
//! depending on a journal crate: the protocol's no-follow traversal, ownership,
//! exact-mode, and durability requirements apply to its owner-wide storage
//! directory, not to a journal tree.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use getrandom::fill;
use nix::dir::Dir;
use nix::errno::Errno;
use nix::fcntl::{AtFlags, Flock, FlockArg, OFlag, open, openat, renameat};
use nix::sys::stat::{FchmodatFlags, Mode, SFlag, fchmod, fchmodat, fstat, mkdirat};
use nix::unistd::{Uid, UnlinkatFlags, linkat, unlinkat};
use sha2::{Digest, Sha256};

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
}

impl PlatformTag {
    /// The fixed lowercase protocol tag.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
        }
    }

    /// The platform supported by this Unix build.
    pub const fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(target_os = "macos")]
        {
            Self::Macos
        }
    }

    fn parse(value: &str) -> Result<Self, RecordError> {
        match value {
            "linux" => Ok(Self::Linux),
            "macos" => Ok(Self::Macos),
            _ => Err(RecordError::InvalidField("platform")),
        }
    }
}

impl fmt::Display for PlatformTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Canonical raw absolute Unix path bytes for an installation root.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RootToken(Vec<u8>);

impl RootToken {
    /// Validates canonical raw absolute Unix pathname bytes.
    pub fn from_raw_absolute(bytes: Vec<u8>) -> Result<Self, IdentityError> {
        validate_absolute_token(&bytes, "root token")?;
        Ok(Self(bytes))
    }

    /// Returns the exact raw pathname bytes used by the protocol.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Canonical raw absolute Unix path bytes for a journal.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct JournalToken(Vec<u8>);

impl JournalToken {
    /// Validates canonical raw absolute Unix pathname bytes.
    pub fn from_raw_absolute(bytes: Vec<u8>) -> Result<Self, IdentityError> {
        validate_absolute_token(&bytes, "journal token")?;
        Ok(Self(bytes))
    }

    /// Returns the exact raw pathname bytes used by the protocol.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Reconstructs the raw absolute Unix path represented by this token.
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(OsString::from_vec(self.0.clone()))
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
    /// Constructs an owner base from a caller-supplied absolute home directory.
    ///
    /// This is useful for adapters and isolated callers; production callers normally
    /// use [`owner_base`]. The home itself is never created by the provider.
    pub fn at_home(home: PathBuf, platform: PlatformTag) -> Result<Self, IdentityError> {
        if !home.is_absolute() {
            return Err(IdentityError::InvalidInput("home must be absolute"));
        }
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

#[derive(Debug)]
struct NamespaceLease {
    _owner_lock: Flock<File>,
    _marker_lock: Flock<File>,
    _namespace: SecureDir,
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
    let home = env::var_os("HOME").ok_or(IdentityError::InvalidInput("HOME is not set"))?;
    OwnerBase::at_home(PathBuf::from(home), PlatformTag::current())
}

/// Canonicalizes an existing root and converts it to protocol bytes.
pub fn root_token_from_path(path: &Path) -> Result<RootToken, IdentityError> {
    let canonical =
        std::fs::canonicalize(path).map_err(|source| io_error("canonicalize root", source))?;
    RootToken::from_raw_absolute(normalize_absolute_bytes(canonical.as_os_str().as_bytes())?)
}

/// Lexically normalizes an absolute journal path and converts it to protocol bytes.
pub fn journal_token_from_path(path: &Path) -> Result<JournalToken, IdentityError> {
    JournalToken::from_raw_absolute(normalize_absolute_bytes(path.as_os_str().as_bytes())?)
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
    let binding = InstallationBinding::from_record(namespace_name, &record);
    drop(marker_lock);
    drop(owner_lock);
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
        // See the function-level documentation: do not make owner-lock timing
        // the first-writer arbitration mechanism.
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
        self.file
            .sync_all()
            .map_err(|source| io_error("sync directory", source))
    }
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
    _lock: Flock<File>,
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
    }
}

fn open_provider(owner: &OwnerBase, create: bool) -> Result<SecureDir, IdentityError> {
    let mut current = open_absolute_dir(&owner.home)?;
    for (index, segment) in base_segments(owner.platform).iter().enumerate() {
        let exact_mode = index >= 3;
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

fn ensure_owner_lock_file(provider: &SecureDir) -> Result<(), IdentityError> {
    let file = open_or_create_file(provider, "owner.lock")?;
    verify_regular(&file, true)?;
    Ok(())
}

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

fn lock_owner(provider: &SecureDir) -> Result<Flock<File>, IdentityError> {
    let file = open_or_create_file(provider, "owner.lock")?;
    Flock::lock(file, FlockArg::LockExclusive)
        .map_err(|(_, error)| nix_error("lock owner registry", error))
}

fn lock_existing_owner(provider: &SecureDir) -> Result<Flock<File>, IdentityError> {
    let file = open_existing_file(provider, "owner.lock", true)?;
    Flock::lock(file, FlockArg::LockExclusive)
        .map_err(|(_, error)| nix_error("lock owner registry", error))
}

fn lock_existing_marker(namespace: &SecureDir) -> Result<Flock<File>, IdentityError> {
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

fn create_or_lock_marker(namespace: &SecureDir) -> Result<Flock<File>, IdentityError> {
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
    owner_lock: Flock<File>,
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

fn set_mode(file: &File, mode: Mode) -> Result<(), IdentityError> {
    fchmod(file, mode).map_err(|error| nix_error("set storage mode", error))
}

fn validate_absolute_token(bytes: &[u8], label: &'static str) -> Result<(), IdentityError> {
    if bytes.is_empty() || bytes.len() > MAX_TOKEN_BYTES || bytes[0] != b'/' || bytes.contains(&0) {
        return Err(IdentityError::InvalidInput(label));
    }
    if normalize_absolute_bytes(bytes)? != bytes {
        return Err(IdentityError::InvalidInput(label));
    }
    Ok(())
}

fn normalize_absolute_bytes(bytes: &[u8]) -> Result<Vec<u8>, IdentityError> {
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

fn nix_error(operation: &'static str, error: Errno) -> IdentityError {
    io_error(operation, io::Error::from_raw_os_error(error as i32))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FaultPoint {
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
    rendezvous: Option<(FaultPoint, std::sync::Arc<std::sync::Barrier>)>,
    park: Option<(FaultPoint, std::sync::Arc<ParkGate>)>,
}

#[cfg(test)]
static TEST_CONTROL: std::sync::OnceLock<std::sync::Mutex<TestControl>> =
    std::sync::OnceLock::new();

#[cfg(test)]
fn hit(point: FaultPoint) -> Result<(), IdentityError> {
    let control = TEST_CONTROL.get_or_init(|| {
        std::sync::Mutex::new(TestControl {
            fail: None,
            rendezvous: None,
            park: None,
        })
    });
    let (fail, rendezvous, park) = {
        let guard = control.lock().expect("test fault control poisoned");
        (guard.fail, guard.rendezvous.clone(), guard.park.clone())
    };
    if let Some((target, gate)) = park.filter(|(target, _)| *target == point) {
        let _ = target;
        gate.arrived.wait();
        gate.release.wait();
    }
    if let Some((target, barrier)) = rendezvous.filter(|(target, _)| *target == point) {
        let _ = target;
        barrier.wait();
    }
    if fail == Some(point) {
        return Err(IdentityError::Io {
            operation: "injected identity fault",
            source: io::Error::other("fault injection"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::{Arc, Barrier, Mutex, OnceLock};
    use std::thread;
    use std::time::Duration;

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
                    rendezvous: None,
                    park: None,
                })
            })
            .lock()
            .expect("test control");
        control.fail = None;
        control.rendezvous = None;
        control.park = None;
    }

    fn fail_at(point: FaultPoint) {
        let mut control = TEST_CONTROL
            .get_or_init(|| {
                Mutex::new(TestControl {
                    fail: None,
                    rendezvous: None,
                    park: None,
                })
            })
            .lock()
            .expect("test control");
        control.fail = Some(point);
        control.rendezvous = None;
        control.park = None;
    }

    fn rendezvous_at(point: FaultPoint, barrier: Arc<Barrier>) {
        let mut control = TEST_CONTROL
            .get_or_init(|| {
                Mutex::new(TestControl {
                    fail: None,
                    rendezvous: None,
                    park: None,
                })
            })
            .lock()
            .expect("test control");
        control.fail = None;
        control.rendezvous = Some((point, barrier));
        control.park = None;
    }

    fn park_at(point: FaultPoint) -> Arc<ParkGate> {
        let gate = ParkGate::new();
        let mut control = TEST_CONTROL
            .get_or_init(|| {
                Mutex::new(TestControl {
                    fail: None,
                    rendezvous: None,
                    park: None,
                })
            })
            .lock()
            .expect("test control");
        control.fail = None;
        control.rendezvous = None;
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
        let barrier = Arc::new(Barrier::new(2));
        rendezvous_at(FaultPoint::PreparedNoReplace, barrier);
        let first = fixture.request(b"/install/race", b"/journal/race");
        let second = first.clone();
        let one = thread::spawn(move || {
            admit_setup(first)
                .expect("first admission")
                .binding()
                .clone()
        });
        let two = thread::spawn(move || {
            admit_setup(second)
                .expect("second admission")
                .binding()
                .clone()
        });
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
