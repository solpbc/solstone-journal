// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local-link authorization and device-activity ledgers.
//!
//! `authorized_clients.json` is authoritative. `devices.json` holds only
//! non-authoritative last-seen metadata and must never grant authorization.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{JsonWriteOptions, LockError, LockOptions, hold_lock, write_json};
#[cfg(windows)]
use solstone_core_journal_io::{WindowsFileIdentity, windows_file_identity};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::pairing_identity::{PairingIdentityFields, Platform};

const CERT_KIND: &str = "cert";

/// A durable role, preserving unknown wire values exactly.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ClientRole {
    #[default]
    Roleless,
    Peer,
    Unknown(String),
}

impl ClientRole {
    pub fn from_wire(value: Option<&str>) -> Self {
        match value.unwrap_or_default() {
            "" => Self::Roleless,
            "peer" => Self::Peer,
            value => Self::Unknown(value.to_owned()),
        }
    }

    pub fn as_wire(&self) -> &str {
        match self {
            Self::Roleless => "",
            Self::Peer => "peer",
            Self::Unknown(value) => value,
        }
    }

    pub fn is_peer(&self) -> bool {
        matches!(self, Self::Peer)
    }
}

/// One authoritative certificate authorization record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientEntry {
    pub fingerprint: String,
    pub device_label: String,
    pub paired_at: String,
    pub instance_id: String,
    pub role: ClientRole,
    pub network: Option<String>,
    pub client_label: String,
    pub label_ordinal: u32,
    pub kind: String,
    pub platform: Option<Platform>,
}

/// Non-authoritative activity retained for one authorized certificate client.
///
/// `last_seen_at` is written by the completed-handshake path. The remaining
/// fields are owned by ingest and deliberately do not participate in access
/// decisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientActivity {
    pub last_seen_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accepted_ingest_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accepted_segment: Option<AcceptedSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingest_rejection: Option<IngestRejection>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, SourceRecord>,
}

impl ClientActivity {
    fn new(last_seen_at: impl Into<String>) -> Self {
        Self {
            last_seen_at: last_seen_at.into(),
            last_accepted_ingest_at: None,
            last_accepted_segment: None,
            ingest_rejection: None,
            sources: BTreeMap::new(),
        }
    }
}

/// The day and directory name of a durably accepted ingest segment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedSegment {
    pub day: String,
    pub name: String,
}

/// The active streak of post-commit ingest failures for a client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IngestRejection {
    pub reason_code: String,
    pub first: String,
    pub latest: String,
    pub active_count: u64,
}

/// Per-source ingest history for one `(device, source)` key.
///
/// Connection liveness stays on [`ClientActivity::last_seen_at`]; this record
/// only tracks accepted delivery and an active rejection streak.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceActivity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accepted_ingest_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accepted_segment: Option<AcceptedSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingest_rejection: Option<IngestRejection>,
}

/// One `sources` map entry. A malformed value is retained so a later sibling
/// write cannot drop it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceRecord {
    Valid(SourceActivity),
    Malformed(Value),
}

impl<'de> Deserialize<'de> for SourceRecord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(parse_source_record(value))
    }
}

impl Serialize for SourceRecord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Valid(activity) => activity.serialize(serializer),
            Self::Malformed(value) => value.serialize(serializer),
        }
    }
}

fn parse_source_record(value: Value) -> SourceRecord {
    match serde_json::from_value::<SourceActivity>(value.clone()) {
        Ok(activity)
            if validate_ingest_timestamps(
                activity.last_accepted_ingest_at.as_deref(),
                activity.ingest_rejection.as_ref(),
            )
            .is_ok() =>
        {
            SourceRecord::Valid(activity)
        }
        _ => SourceRecord::Malformed(value),
    }
}

impl ClientEntry {
    pub fn new(
        fingerprint: impl Into<String>,
        device_label: impl Into<String>,
        paired_at: impl Into<String>,
        instance_id: impl Into<String>,
        role: ClientRole,
    ) -> Self {
        Self {
            fingerprint: fingerprint.into(),
            device_label: device_label.into(),
            paired_at: paired_at.into(),
            instance_id: instance_id.into(),
            role,
            network: None,
            client_label: String::new(),
            label_ordinal: 1,
            kind: CERT_KIND.to_owned(),
            platform: None,
        }
    }

    pub fn base_label(&self) -> &str {
        if self.device_label.is_empty() {
            &self.client_label
        } else {
            &self.device_label
        }
    }

    pub fn display_label(&self) -> String {
        let base = self.base_label();
        if base.is_empty() || self.label_ordinal == 1 {
            base.to_owned()
        } else {
            format!("{base} ({})", self.label_ordinal)
        }
    }
}

/// Read-only posture for the authoritative authorization file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorizedClientsRead {
    Present(Vec<ClientEntry>),
    Missing,
    Unreadable,
    Malformed,
    DuplicateCid,
}

/// Strict failure while loading an existing authorization ledger for mutation.
///
/// This intentionally contains no decoded clients. A failed mutation load
/// cannot yield a collection that a caller could subsequently publish.
///
/// ```compile_fail
/// use solstone_core_sol_link::ledger::AuthorizedClientsLoadError;
///
/// fn extract_writable_clients(error: AuthorizedClientsLoadError) {
///     let clients = error.clients();
///     drop(clients);
/// }
/// ```
#[derive(Debug)]
pub enum AuthorizedClientsLoadError {
    Unreadable {
        path: PathBuf,
        source: io::Error,
    },
    Malformed {
        path: PathBuf,
        source: Box<dyn Error + Send + Sync>,
    },
    DuplicateCid {
        path: PathBuf,
        fingerprint: String,
    },
}

impl fmt::Display for AuthorizedClientsLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = match self {
            Self::Unreadable { path, .. }
            | Self::Malformed { path, .. }
            | Self::DuplicateCid { path, .. } => path,
        };
        write!(
            formatter,
            "your paired devices file at {} couldn't be read. your paired devices were NOT changed. repair the file or restore link/authorized_clients.json from a backup, then try again.",
            path.display()
        )
    }
}

impl Error for AuthorizedClientsLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unreadable { source, .. } => Some(source),
            Self::Malformed { source, .. } => Some(source.as_ref()),
            Self::DuplicateCid { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum AuthorizedClientsMutationError {
    Lock(LockError),
    Load(AuthorizedClientsLoadError),
    Device(DevicesMutationError),
    InvalidLabel(&'static str),
    InvalidLastSeenAt,
    InvalidActivityTimestamp(&'static str),
    Write(solstone_core_journal_io::AtomicWriteError),
}

impl fmt::Display for AuthorizedClientsMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Load(error) => error.fmt(formatter),
            Self::Device(error) => error.fmt(formatter),
            Self::InvalidLabel(message) => formatter.write_str(message),
            Self::InvalidLastSeenAt => {
                formatter.write_str("last_seen_at must be an RFC3339 UTC timestamp")
            }
            Self::InvalidActivityTimestamp(field) => {
                write!(formatter, "{field} must be an RFC3339 UTC timestamp")
            }
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl Error for AuthorizedClientsMutationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Load(error) => Some(error),
            Self::Device(error) => Some(error),
            Self::InvalidLabel(_) | Self::InvalidLastSeenAt | Self::InvalidActivityTimestamp(_) => {
                None
            }
            Self::Write(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub enum DevicesMutationError {
    Lock(LockError),
    Unreadable {
        path: PathBuf,
        source: io::Error,
    },
    Malformed {
        path: PathBuf,
        source: Box<dyn Error + Send + Sync>,
    },
    Write(solstone_core_journal_io::AtomicWriteError),
}

/// Read-only view of the non-authoritative device activity ledger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceActivityRead {
    Present(BTreeMap<String, ClientActivity>),
    Missing,
    Unreadable,
    Malformed,
}

impl fmt::Display for DevicesMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Unreadable { path, .. } | Self::Malformed { path, .. } => write!(
                formatter,
                "your paired device activity file at {} couldn't be read. your device activity was NOT changed. repair the file or restore link/devices.json from a backup, then try again.",
                path.display()
            ),
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl Error for DevicesMutationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Unreadable { source, .. } => Some(source),
            Self::Malformed { source, .. } => Some(source.as_ref()),
            Self::Write(error) => Some(error),
        }
    }
}

/// The result of an unpair operation. Device metadata is best effort only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoveOutcome {
    pub authorized_removed: bool,
    pub device_metadata_removed: bool,
}

#[derive(Clone, Debug, Default)]
struct Clients {
    entries: Vec<ClientEntry>,
    pairing: BTreeMap<String, PairingIdentityFields>,
}

impl Clients {
    fn get(&self, fingerprint: &str) -> Option<&ClientEntry> {
        self.entries
            .iter()
            .find(|entry| entry.fingerprint == fingerprint)
    }

    fn pairing(&self, fingerprint: &str) -> Option<&PairingIdentityFields> {
        self.pairing.get(fingerprint)
    }

    fn upsert_parsed(&mut self, entry: ClientEntry, pairing: PairingIdentityFields) {
        self.pairing.insert(entry.fingerprint.clone(), pairing);
        self.upsert_entry(entry);
    }

    fn upsert(&mut self, entry: ClientEntry) {
        // Mutations do not re-parse disk JSON. Keep a lossless pairing row in
        // lockstep: existing fingerprints keep the parse-time fields
        // (first-write-wins), new fingerprints derive from the written object.
        self.pairing
            .entry(entry.fingerprint.clone())
            .or_insert_with(|| pairing_from_entry(&entry));
        self.upsert_entry(entry);
    }

    fn upsert_entry(&mut self, entry: ClientEntry) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|existing| existing.fingerprint == entry.fingerprint)
        {
            self.entries[index] = entry;
        } else {
            self.entries.push(entry);
        }
    }

    fn remove(&mut self, fingerprint: &str) -> bool {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.fingerprint == fingerprint)
        else {
            return false;
        };
        self.entries.remove(index);
        self.pairing.remove(fingerprint);
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReloadKey {
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    identity: WindowsFileIdentity,
    mtime_ns: i128,
    size: u64,
}

/// Cached view of `link/authorized_clients.json` with inode-aware reloads.
pub struct AuthorizationLedger {
    authorized_clients_path: PathBuf,
    devices_path: PathBuf,
    cached: Clients,
    reload_key: Option<ReloadKey>,
    read_state: AuthorizedClientsRead,
}

impl AuthorizationLedger {
    pub fn new(journal_root: &Path) -> Self {
        Self::from_paths(
            journal_root.join("link").join("authorized_clients.json"),
            journal_root.join("link").join("devices.json"),
        )
    }

    pub fn from_paths(authorized_clients_path: PathBuf, devices_path: PathBuf) -> Self {
        Self {
            authorized_clients_path,
            devices_path,
            cached: Clients::default(),
            reload_key: None,
            read_state: AuthorizedClientsRead::Missing,
        }
    }

    pub fn authorized_clients_path(&self) -> &Path {
        &self.authorized_clients_path
    }

    pub fn devices_path(&self) -> &Path {
        &self.devices_path
    }

    pub fn reload_if_stale(&mut self) -> bool {
        let current = reload_key(&self.authorized_clients_path);
        if current == self.reload_key
            && matches!(self.read_state, AuthorizedClientsRead::Present(_))
        {
            return false;
        }
        // Parse through `read_authorized` so the lossless pairing cache is
        // rebuilt with the ClientEntry vec, not reconstructed from the lossy
        // `AuthorizedClientsRead::Present` payload.
        match read_authorized(&self.authorized_clients_path) {
            Ok(Some(clients)) => {
                self.read_state = AuthorizedClientsRead::Present(clients.entries.clone());
                self.cached = clients;
            }
            Ok(None) => {
                self.cached = Clients::default();
                self.read_state = AuthorizedClientsRead::Missing;
            }
            Err(AuthorizedClientsLoadError::Unreadable { .. }) => {
                self.cached = Clients::default();
                self.read_state = AuthorizedClientsRead::Unreadable;
            }
            Err(AuthorizedClientsLoadError::Malformed { .. }) => {
                self.cached = Clients::default();
                self.read_state = AuthorizedClientsRead::Malformed;
            }
            Err(AuthorizedClientsLoadError::DuplicateCid { .. }) => {
                self.cached = Clients::default();
                self.read_state = AuthorizedClientsRead::DuplicateCid;
            }
        }
        self.reload_key = reload_key(&self.authorized_clients_path);
        true
    }

    pub fn read_state(&mut self) -> AuthorizedClientsRead {
        self.reload_if_stale();
        self.read_state.clone()
    }

    pub fn is_authorized(&mut self, fingerprint: &str) -> bool {
        self.reload_if_stale();
        matches!(self.read_state, AuthorizedClientsRead::Present(_))
            && self.cached.get(fingerprint).is_some()
    }

    pub fn snapshot(&mut self) -> Vec<ClientEntry> {
        self.reload_if_stale();
        self.cached.entries.clone()
    }

    pub fn get(&mut self, fingerprint: &str) -> Option<ClientEntry> {
        self.reload_if_stale();
        self.cached.get(fingerprint).cloned()
    }

    /// Lossless pairing-identity read-back for one fingerprint.
    ///
    /// `Ok(None)` means the ledger is readable and has no row for `fingerprint`
    /// (a missing file is treated the same: there is no row). `Err` is the same
    /// unreadable / malformed / duplicate-cid state `read_state` reports after
    /// `get()` on a broken file — `get()` itself collapses those to `None`.
    pub fn get_pairing_identity_fields(
        &mut self,
        fingerprint: &str,
    ) -> Result<Option<PairingIdentityFields>, AuthorizedClientsRead> {
        self.reload_if_stale();
        match &self.read_state {
            AuthorizedClientsRead::Present(_) | AuthorizedClientsRead::Missing => {
                Ok(self.cached.pairing(fingerprint).cloned())
            }
            other => Err(other.clone()),
        }
    }

    pub fn add(
        &mut self,
        mut entry: ClientEntry,
    ) -> Result<ClientEntry, AuthorizedClientsMutationError> {
        let _authorization_lock = lock(&self.authorized_clients_path)?;
        let mut clients = load_authorized_for_mutation(&self.authorized_clients_path)
            .map_err(AuthorizedClientsMutationError::Load)?;
        if let Some(existing) = clients.get(&entry.fingerprint) {
            entry.client_label = existing.client_label.clone();
            entry.platform = existing.platform;
        }
        entry.kind = CERT_KIND.to_owned();
        entry.label_ordinal =
            allocate_label_ordinal(&clients, entry.base_label(), &entry.fingerprint);
        clients.upsert(entry.clone());
        write_authorized_clients(&self.authorized_clients_path, &clients)
            .map_err(AuthorizedClientsMutationError::Write)?;
        self.set_cached(clients);
        Ok(entry)
    }

    pub fn update_label(
        &mut self,
        fingerprint: &str,
        label: &str,
    ) -> Result<Option<ClientEntry>, AuthorizedClientsMutationError> {
        let normalized = label.trim();
        if normalized.is_empty() {
            return Err(AuthorizedClientsMutationError::InvalidLabel(
                "label must not be empty",
            ));
        }
        let _authorization_lock = lock(&self.authorized_clients_path)?;
        let mut clients = load_authorized_for_mutation(&self.authorized_clients_path)
            .map_err(AuthorizedClientsMutationError::Load)?;
        let Some(existing) = clients.get(fingerprint).cloned() else {
            return Ok(None);
        };
        let device_label = if normalized == existing.display_label() {
            existing.base_label().to_owned()
        } else {
            normalized.to_owned()
        };
        if device_label.len() > 80 {
            return Err(AuthorizedClientsMutationError::InvalidLabel(
                "label too long",
            ));
        }
        let mut updated = existing;
        updated.device_label = device_label;
        updated.label_ordinal = allocate_label_ordinal(&clients, updated.base_label(), fingerprint);
        clients.upsert(updated.clone());
        write_authorized_clients(&self.authorized_clients_path, &clients)
            .map_err(AuthorizedClientsMutationError::Write)?;
        self.set_cached(clients);
        Ok(Some(updated))
    }

    pub fn backfill_label_ordinals(&mut self) -> Result<bool, AuthorizedClientsMutationError> {
        let _authorization_lock = lock(&self.authorized_clients_path)?;
        let mut clients = load_authorized_for_mutation(&self.authorized_clients_path)
            .map_err(AuthorizedClientsMutationError::Load)?;
        let mut groups: Vec<String> = Vec::new();
        for entry in &clients.entries {
            let group = entry.base_label().to_lowercase();
            if !group.is_empty() && !groups.contains(&group) {
                groups.push(group);
            }
        }
        let mut changed = false;
        for group in groups {
            let mut indices = clients
                .entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    (entry.base_label().to_lowercase() == group).then_some(index)
                })
                .collect::<Vec<_>>();
            let mut ordinals = indices
                .iter()
                .map(|index| clients.entries[*index].label_ordinal)
                .collect::<Vec<_>>();
            ordinals.sort_unstable();
            if ordinals.windows(2).all(|pair| pair[0] != pair[1]) {
                continue;
            }
            indices.sort_by_key(|index| {
                let entry = &clients.entries[*index];
                (entry.paired_at.clone(), entry.fingerprint.clone())
            });
            for (ordinal, index) in indices.into_iter().enumerate() {
                let ordinal = u32::try_from(ordinal + 1).unwrap_or(u32::MAX);
                if clients.entries[index].label_ordinal != ordinal {
                    clients.entries[index].label_ordinal = ordinal;
                    changed = true;
                }
            }
        }
        if changed {
            write_authorized_clients(&self.authorized_clients_path, &clients)
                .map_err(AuthorizedClientsMutationError::Write)?;
            self.set_cached(clients);
        }
        Ok(changed)
    }

    /// Record an accepted connection using the current RFC3339 UTC time.
    pub fn touch_last_seen(
        &mut self,
        fingerprint: &str,
    ) -> Result<bool, AuthorizedClientsMutationError> {
        self.touch_last_seen_at(fingerprint, &rfc3339_utc(OffsetDateTime::now_utc()))
    }

    /// Record an accepted connection at a caller-supplied RFC3339 UTC time.
    pub fn touch_last_seen_at(
        &mut self,
        fingerprint: &str,
        last_seen_at: &str,
    ) -> Result<bool, AuthorizedClientsMutationError> {
        if parse_rfc3339_utc(last_seen_at).is_none() {
            return Err(AuthorizedClientsMutationError::InvalidLastSeenAt);
        }
        let _authorization_lock = lock(&self.authorized_clients_path)?;
        let clients = load_authorized_for_mutation(&self.authorized_clients_path)
            .map_err(AuthorizedClientsMutationError::Load)?;
        if clients.get(fingerprint).is_none() {
            return Ok(false);
        }
        self.set_cached(clients);
        touch_device(&self.devices_path, fingerprint, last_seen_at)
            .map_err(AuthorizedClientsMutationError::Device)?;
        Ok(true)
    }

    /// Record an ingest whose content and durable event were accepted.
    ///
    /// The authorization lock deliberately remains held while the devices lock
    /// is acquired, matching connection activity and unpair mutations.
    pub fn record_accepted_ingest(
        &mut self,
        cid: &str,
        accepted_at: &str,
        segment: AcceptedSegment,
        source: Option<&str>,
    ) -> Result<bool, AuthorizedClientsMutationError> {
        if parse_rfc3339_utc(accepted_at).is_none() {
            return Err(AuthorizedClientsMutationError::InvalidActivityTimestamp(
                "accepted_at",
            ));
        }
        let _authorization_lock = lock(&self.authorized_clients_path)?;
        let clients = load_authorized_for_mutation(&self.authorized_clients_path)
            .map_err(AuthorizedClientsMutationError::Load)?;
        if clients.get(cid).is_none() {
            return Ok(false);
        }
        self.set_cached(clients);
        record_accepted_device(&self.devices_path, cid, accepted_at, segment, source)
            .map_err(AuthorizedClientsMutationError::Device)?;
        Ok(true)
    }

    /// Record a post-commit ingest failure without discarding the active streak.
    ///
    /// A later successful accepted ingest clears the streak. Until then every
    /// rejection, including one with a different reason code, keeps the first
    /// timestamp and advances the saturating count.
    pub fn record_ingest_rejection(
        &mut self,
        cid: &str,
        at: &str,
        reason_code: &str,
        source: Option<&str>,
    ) -> Result<bool, AuthorizedClientsMutationError> {
        if parse_rfc3339_utc(at).is_none() {
            return Err(AuthorizedClientsMutationError::InvalidActivityTimestamp(
                "at",
            ));
        }
        let _authorization_lock = lock(&self.authorized_clients_path)?;
        let clients = load_authorized_for_mutation(&self.authorized_clients_path)
            .map_err(AuthorizedClientsMutationError::Load)?;
        if clients.get(cid).is_none() {
            return Ok(false);
        }
        self.set_cached(clients);
        record_device_rejection(&self.devices_path, cid, at, reason_code, source)
            .map_err(AuthorizedClientsMutationError::Device)?;
        Ok(true)
    }

    pub fn remove(
        &mut self,
        fingerprint: &str,
    ) -> Result<RemoveOutcome, AuthorizedClientsMutationError> {
        let _authorization_lock = lock(&self.authorized_clients_path)?;
        let mut clients = load_authorized_for_mutation(&self.authorized_clients_path)
            .map_err(AuthorizedClientsMutationError::Load)?;
        if !clients.remove(fingerprint) {
            return Ok(RemoveOutcome {
                authorized_removed: false,
                device_metadata_removed: false,
            });
        }
        write_authorized_clients(&self.authorized_clients_path, &clients)
            .map_err(AuthorizedClientsMutationError::Write)?;
        self.set_cached(clients);

        // The authorization lock remains held while acquiring the devices lock.
        // A crash or devices.json failure after this point can retain stale
        // last-seen metadata, but that file is never authorization evidence.
        let device_metadata_removed =
            remove_device(&self.devices_path, fingerprint).unwrap_or(false);
        Ok(RemoveOutcome {
            authorized_removed: true,
            device_metadata_removed,
        })
    }

    fn set_cached(&mut self, clients: Clients) {
        self.read_state = AuthorizedClientsRead::Present(clients.entries.clone());
        self.cached = clients;
        self.reload_key = reload_key(&self.authorized_clients_path);
    }
}

pub fn read_authorized_clients(path: &Path) -> AuthorizedClientsRead {
    match read_authorized(path) {
        Ok(Some(clients)) => AuthorizedClientsRead::Present(clients.entries),
        Ok(None) => AuthorizedClientsRead::Missing,
        Err(AuthorizedClientsLoadError::Unreadable { .. }) => AuthorizedClientsRead::Unreadable,
        Err(AuthorizedClientsLoadError::Malformed { .. }) => AuthorizedClientsRead::Malformed,
        Err(AuthorizedClientsLoadError::DuplicateCid { .. }) => AuthorizedClientsRead::DuplicateCid,
    }
}

/// Read `link/devices.json` without acquiring a lock or changing the file.
///
/// Device activity is presentation metadata only; callers must never use this
/// result as authorization evidence.
pub fn read_device_activity(path: &Path) -> DeviceActivityRead {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return DeviceActivityRead::Missing;
        }
        Err(_) => return DeviceActivityRead::Unreadable,
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return DeviceActivityRead::Malformed;
    };
    match parse_devices(&value) {
        Ok(devices) => DeviceActivityRead::Present(devices),
        Err(_) => DeviceActivityRead::Malformed,
    }
}

fn lock(path: &Path) -> Result<solstone_core_journal_io::FileLock, AuthorizedClientsMutationError> {
    hold_lock(
        path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(AuthorizedClientsMutationError::Lock)
}

fn load_authorized_for_mutation(path: &Path) -> Result<Clients, AuthorizedClientsLoadError> {
    Ok(read_authorized(path)?.unwrap_or_default())
}

fn read_authorized(path: &Path) -> Result<Option<Clients>, AuthorizedClientsLoadError> {
    // The authorization ledger is a bounded local JSON document. Reject special
    // files before opening them: a FIFO, device, or directory can otherwise
    // leave a caller indefinitely blocked in `fs::read`, which is especially
    // unsafe on a per-request authorization path.
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(AuthorizedClientsLoadError::Unreadable {
                path: path.to_path_buf(),
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "authorized clients ledger is not a regular file",
                ),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(AuthorizedClientsLoadError::Unreadable {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(AuthorizedClientsLoadError::Unreadable {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let value = serde_json::from_slice::<Value>(&bytes).map_err(|source| {
        AuthorizedClientsLoadError::Malformed {
            path: path.to_path_buf(),
            source: Box::new(source),
        }
    })?;
    let raw = value
        .as_array()
        .ok_or_else(|| AuthorizedClientsLoadError::Malformed {
            path: path.to_path_buf(),
            source: Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                "authorized clients must be a JSON array",
            )),
        })?;
    let mut clients = Clients::default();
    let mut dropped_non_cert = false;
    for item in raw {
        let Some(item) = item.as_object() else {
            return Err(AuthorizedClientsLoadError::Malformed {
                path: path.to_path_buf(),
                source: Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "authorized client entry must be an object",
                )),
            });
        };
        if item.get("kind").is_some_and(|kind| kind != CERT_KIND) {
            dropped_non_cert = true;
            continue;
        }
        let Some(fingerprint) = item.get("fingerprint").and_then(Value::as_str) else {
            return Err(AuthorizedClientsLoadError::Malformed {
                path: path.to_path_buf(),
                source: Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "authorized client entry must contain a string fingerprint",
                )),
            });
        };
        if clients.get(fingerprint).is_some() {
            return Err(AuthorizedClientsLoadError::DuplicateCid {
                path: path.to_path_buf(),
                fingerprint: fingerprint.to_owned(),
            });
        }
        let label_ordinal = item
            .get("label_ordinal")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or(1);
        clients.upsert_parsed(
            ClientEntry {
                fingerprint: fingerprint.to_owned(),
                device_label: json_string(item.get("device_label")),
                paired_at: json_string(item.get("paired_at")),
                instance_id: json_string(item.get("instance_id")),
                role: ClientRole::from_wire(item.get("role").and_then(Value::as_str)),
                network: item
                    .get("network")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                client_label: item
                    .get("client_label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                label_ordinal,
                kind: CERT_KIND.to_owned(),
                platform: item
                    .get("platform")
                    .and_then(Value::as_str)
                    .and_then(Platform::from_wire),
            },
            PairingIdentityFields::from_object(item),
        );
    }
    if dropped_non_cert {
        log::warn!(
            "authorized_clients.json ignored one or more entries with an unsupported kind (expected cert)"
        );
    }
    Ok(Some(clients))
}

fn json_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn write_authorized_clients(
    path: &Path,
    clients: &Clients,
) -> Result<(), solstone_core_journal_io::AtomicWriteError> {
    let payload = clients
        .entries
        .iter()
        .map(client_to_json)
        .collect::<Vec<_>>();
    write_json(
        path,
        &payload,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
}

fn pairing_from_entry(entry: &ClientEntry) -> PairingIdentityFields {
    let value = client_to_json(entry);
    PairingIdentityFields::from_object(value.as_object().expect("client JSON is an object"))
}

fn client_to_json(entry: &ClientEntry) -> Value {
    let mut object = Map::from_iter([
        ("fingerprint".to_owned(), json!(entry.fingerprint)),
        ("device_label".to_owned(), json!(entry.device_label)),
        ("paired_at".to_owned(), json!(entry.paired_at)),
        ("instance_id".to_owned(), json!(entry.instance_id)),
        ("role".to_owned(), json!(entry.role.as_wire())),
        ("kind".to_owned(), json!(CERT_KIND)),
    ]);
    if entry
        .network
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        object.insert("network".to_owned(), json!(entry.network));
    }
    if !entry.client_label.is_empty() {
        object.insert("client_label".to_owned(), json!(entry.client_label));
    }
    if let Some(platform) = entry.platform {
        object.insert("platform".to_owned(), json!(platform.as_wire()));
    }
    if entry.label_ordinal != 1 {
        object.insert("label_ordinal".to_owned(), json!(entry.label_ordinal));
    }
    Value::Object(object)
}

fn allocate_label_ordinal(clients: &Clients, base_label: &str, fingerprint: &str) -> u32 {
    if base_label.is_empty() {
        return 1;
    }
    let held = clients
        .entries
        .iter()
        .filter(|entry| {
            entry.fingerprint != fingerprint && entry.base_label().eq_ignore_ascii_case(base_label)
        })
        .map(|entry| entry.label_ordinal)
        .collect::<Vec<_>>();
    let mut ordinal = 1;
    while held.contains(&ordinal) {
        ordinal += 1;
    }
    ordinal
}

fn touch_device(
    path: &Path,
    fingerprint: &str,
    last_seen_at: &str,
) -> Result<(), DevicesMutationError> {
    mutate_devices(path, |devices| {
        let activity = devices
            .entry(fingerprint.to_owned())
            .or_insert_with(|| ClientActivity::new(last_seen_at));
        activity.last_seen_at = last_seen_at.to_owned();
        ((), true)
    })
}

fn remove_device(path: &Path, fingerprint: &str) -> Result<bool, DevicesMutationError> {
    mutate_devices(path, |devices| {
        let removed = devices.remove(fingerprint).is_some();
        (removed, removed)
    })
}

fn apply_accepted(
    last_accepted_ingest_at: &mut Option<String>,
    last_accepted_segment: &mut Option<AcceptedSegment>,
    ingest_rejection: &mut Option<IngestRejection>,
    accepted_at: &str,
    segment: AcceptedSegment,
) {
    *last_accepted_ingest_at = Some(accepted_at.to_owned());
    *last_accepted_segment = Some(segment);
    *ingest_rejection = None;
}

fn apply_rejected(ingest_rejection: &mut Option<IngestRejection>, at: &str, reason_code: &str) {
    *ingest_rejection = Some(match ingest_rejection.take() {
        Some(mut rejection) => {
            rejection.reason_code = reason_code.to_owned();
            rejection.latest = at.to_owned();
            rejection.active_count = rejection.active_count.saturating_add(1);
            rejection
        }
        None => IngestRejection {
            reason_code: reason_code.to_owned(),
            first: at.to_owned(),
            latest: at.to_owned(),
            active_count: 1,
        },
    });
}

fn source_activity_for_mutation<'a>(
    sources: &'a mut BTreeMap<String, SourceRecord>,
    source: &str,
) -> &'a mut SourceActivity {
    let reset = !matches!(sources.get(source), Some(SourceRecord::Valid(_)));
    if reset {
        sources.insert(
            source.to_owned(),
            SourceRecord::Valid(SourceActivity::default()),
        );
    }
    match sources.get_mut(source) {
        Some(SourceRecord::Valid(activity)) => activity,
        _ => unreachable!("source activity just inserted as Valid"),
    }
}

fn record_accepted_device(
    path: &Path,
    cid: &str,
    accepted_at: &str,
    segment: AcceptedSegment,
    source: Option<&str>,
) -> Result<(), DevicesMutationError> {
    mutate_devices(path, |devices| {
        let activity = devices
            .entry(cid.to_owned())
            .or_insert_with(|| ClientActivity::new(accepted_at));
        if let Some(source) = source {
            apply_accepted(
                &mut activity.last_accepted_ingest_at,
                &mut activity.last_accepted_segment,
                &mut activity.ingest_rejection,
                accepted_at,
                segment.clone(),
            );
            let source_activity = source_activity_for_mutation(&mut activity.sources, source);
            apply_accepted(
                &mut source_activity.last_accepted_ingest_at,
                &mut source_activity.last_accepted_segment,
                &mut source_activity.ingest_rejection,
                accepted_at,
                segment,
            );
        } else {
            apply_accepted(
                &mut activity.last_accepted_ingest_at,
                &mut activity.last_accepted_segment,
                &mut activity.ingest_rejection,
                accepted_at,
                segment,
            );
        }
        ((), true)
    })
}

fn record_device_rejection(
    path: &Path,
    cid: &str,
    at: &str,
    reason_code: &str,
    source: Option<&str>,
) -> Result<(), DevicesMutationError> {
    mutate_devices(path, |devices| {
        let activity = devices
            .entry(cid.to_owned())
            .or_insert_with(|| ClientActivity::new(at));
        apply_rejected(&mut activity.ingest_rejection, at, reason_code);
        if let Some(source) = source {
            let source_activity = source_activity_for_mutation(&mut activity.sources, source);
            apply_rejected(&mut source_activity.ingest_rejection, at, reason_code);
        }
        ((), true)
    })
}

fn mutate_devices<T>(
    path: &Path,
    mutate: impl FnOnce(&mut BTreeMap<String, ClientActivity>) -> (T, bool),
) -> Result<T, DevicesMutationError> {
    let _devices_lock = hold_lock(
        path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(DevicesMutationError::Lock)?;
    let mut devices = load_devices_for_mutation(path)?;
    let (result, changed) = mutate(&mut devices);
    if changed {
        write_json(
            path,
            &devices,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(DevicesMutationError::Write)?;
    }
    Ok(result)
}

fn load_devices_for_mutation(
    path: &Path,
) -> Result<BTreeMap<String, ClientActivity>, DevicesMutationError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(source) => {
            return Err(DevicesMutationError::Unreadable {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let value = serde_json::from_slice::<Value>(&bytes).map_err(|source| {
        DevicesMutationError::Malformed {
            path: path.to_path_buf(),
            source: Box::new(source),
        }
    })?;
    parse_devices(&value).map_err(|source| DevicesMutationError::Malformed {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_devices(
    value: &Value,
) -> Result<BTreeMap<String, ClientActivity>, Box<dyn Error + Send + Sync>> {
    let object = value.as_object().ok_or_else(|| {
        Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            "devices must be a JSON object",
        )) as Box<dyn Error + Send + Sync>
    })?;
    let mut devices = BTreeMap::new();
    for (cid, value) in object {
        let device = value.as_object().ok_or_else(|| {
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                "device entry must be an object",
            )) as Box<dyn Error + Send + Sync>
        })?;
        for field in [
            "last_accepted_ingest_at",
            "last_accepted_segment",
            "ingest_rejection",
            "sources",
        ] {
            if device.get(field).is_some_and(Value::is_null) {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{field} must be omitted or have its declared shape"),
                )));
            }
        }
        if device
            .get("sources")
            .is_some_and(|sources| !sources.is_object())
        {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                "sources must be omitted or be a JSON object",
            )));
        }
        let activity = serde_json::from_value::<ClientActivity>(value.clone())
            .map_err(|source| Box::new(source) as Box<dyn Error + Send + Sync>)?;
        validate_activity(&activity)?;
        devices.insert(cid.clone(), activity);
    }
    Ok(devices)
}

fn validate_ingest_timestamps(
    last_accepted_ingest_at: Option<&str>,
    ingest_rejection: Option<&IngestRejection>,
) -> Result<(), &'static str> {
    if last_accepted_ingest_at.is_some_and(|value| parse_rfc3339_utc(value).is_none()) {
        return Err("last_accepted_ingest_at");
    }
    if ingest_rejection.is_some_and(|rejection| {
        parse_rfc3339_utc(&rejection.first).is_none()
            || parse_rfc3339_utc(&rejection.latest).is_none()
    }) {
        return Err("ingest_rejection");
    }
    Ok(())
}

fn validate_activity(activity: &ClientActivity) -> Result<(), Box<dyn Error + Send + Sync>> {
    for (field, value) in [("last_seen_at", &activity.last_seen_at)] {
        if parse_rfc3339_utc(value).is_none() {
            return Err(invalid_activity_field(field));
        }
    }
    if let Err(field) = validate_ingest_timestamps(
        activity.last_accepted_ingest_at.as_deref(),
        activity.ingest_rejection.as_ref(),
    ) {
        return Err(invalid_activity_field(field));
    }
    Ok(())
}

fn invalid_activity_field(field: &str) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{field} must be an RFC3339 UTC timestamp"),
    ))
}

#[cfg(unix)]
fn reload_key(path: &Path) -> Option<ReloadKey> {
    let metadata = fs::metadata(path).ok()?;
    Some(ReloadKey {
        inode: metadata.ino(),
        mtime_ns: i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
        size: metadata.len(),
    })
}

#[cfg(windows)]
fn reload_key(path: &Path) -> Option<ReloadKey> {
    let file = fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    Some(ReloadKey {
        identity: windows_file_identity(&file).ok()?,
        mtime_ns: i128::from(metadata.last_write_time()),
        size: metadata.file_size(),
    })
}

pub(crate) fn parse_rfc3339_utc(value: &str) -> Option<OffsetDateTime> {
    let timestamp = OffsetDateTime::parse(value, &Rfc3339).ok()?;
    (timestamp.offset() == UtcOffset::UTC).then_some(timestamp)
}

fn rfc3339_utc(timestamp: OffsetDateTime) -> String {
    let (year, month, day) = timestamp.to_calendar_date();
    format!(
        "{year:04}-{:02}-{day:02}T{:02}:{:02}:{:02}Z",
        month as u8,
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second()
    )
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

    use super::*;

    const NOW: &str = "2026-04-19T18:03:12Z";

    #[test]
    fn roles_round_trip_without_normalizing_unknown_values() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        for (fingerprint, role) in [
            ("a", ClientRole::Roleless),
            ("b", ClientRole::Peer),
            ("c", ClientRole::Unknown("Observer".to_owned())),
        ] {
            ledger.add(entry(fingerprint, "phone", role)).unwrap();
        }
        let values = ledger.snapshot();
        assert!(values[1].role.is_peer());
        assert_eq!(values[2].role, ClientRole::Unknown("Observer".to_owned()));
        let bytes = fs::read(ledger.authorized_clients_path()).unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("\"Observer\""));
    }

    #[test]
    fn read_path_distinguishes_missing_unreadable_and_malformed_but_fails_closed() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        assert!(matches!(
            ledger.read_state(),
            AuthorizedClientsRead::Missing
        ));
        fs::create_dir_all(ledger.authorized_clients_path()).unwrap();
        assert!(matches!(
            ledger.read_state(),
            AuthorizedClientsRead::Unreadable
        ));
        fs::remove_dir(ledger.authorized_clients_path()).unwrap();
        fs::create_dir_all(ledger.authorized_clients_path().parent().unwrap()).unwrap();
        fs::write(ledger.authorized_clients_path(), b"{bad").unwrap();
        assert!(matches!(
            ledger.read_state(),
            AuthorizedClientsRead::Malformed
        ));
        assert!(!ledger.is_authorized("a"));
    }

    #[test]
    fn malformed_individual_entry_makes_the_whole_read_malformed() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        fs::create_dir_all(ledger.authorized_clients_path().parent().unwrap()).unwrap();
        fs::write(
            ledger.authorized_clients_path(),
            br#"[{"fingerprint":"a"},{"device_label":"missing fingerprint"}]"#,
        )
        .unwrap();

        assert_eq!(ledger.read_state(), AuthorizedClientsRead::Malformed);
    }

    #[test]
    fn non_cert_entry_without_a_fingerprint_is_tolerated() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        fs::create_dir_all(ledger.authorized_clients_path().parent().unwrap()).unwrap();
        fs::write(
            ledger.authorized_clients_path(),
            br#"[{"fingerprint":"a"},{"kind":"token"}]"#,
        )
        .unwrap();

        assert!(matches!(
            ledger.read_state(),
            AuthorizedClientsRead::Present(entries)
                if entries.len() == 1 && entries[0].fingerprint == "a"
        ));
    }

    #[test]
    fn corrupt_authorization_ledger_blocks_every_mutator_without_rewrite() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        fs::create_dir_all(ledger.authorized_clients_path().parent().unwrap()).unwrap();
        let original = b"{corrupt authorization ledger\n";
        fs::write(ledger.authorized_clients_path(), original).unwrap();
        for result in [
            ledger
                .add(entry("a", "phone", ClientRole::Roleless))
                .map(|_| ()),
            ledger.remove("a").map(|_| ()),
            ledger.update_label("a", "renamed").map(|_| ()),
            ledger.touch_last_seen_at("a", NOW).map(|_| ()),
        ] {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("were NOT changed"));
            assert_eq!(
                fs::read(ledger.authorized_clients_path()).unwrap(),
                original
            );
        }
    }

    #[test]
    fn touch_last_seen_only_changes_devices_file() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        let before = fs::read(ledger.authorized_clients_path()).unwrap();
        assert!(ledger.touch_last_seen_at("a", NOW).unwrap());
        assert_eq!(fs::read(ledger.authorized_clients_path()).unwrap(), before);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(ledger.devices_path()).unwrap()).unwrap(),
            json!({"a": {"last_seen_at": NOW}})
        );
    }

    #[test]
    fn last_seen_only_activity_remains_backward_readable() {
        let temporary = TempDir::new();
        let path = temporary.path().join("link/devices.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, json!({"a": {"last_seen_at": NOW}}).to_string()).unwrap();

        assert_eq!(
            read_device_activity(&path),
            DeviceActivityRead::Present(BTreeMap::from_iter([(
                "a".to_owned(),
                ClientActivity::new(NOW),
            )]))
        );
    }

    #[test]
    fn activity_reader_rejects_unknown_and_wrongly_typed_fields() {
        let temporary = TempDir::new();
        let path = temporary.path().join("link/devices.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for payload in [
            json!({"a": {"last_seen_at": NOW, "unknown": true}}),
            json!({"a": {"last_seen_at": 1}}),
            json!({"a": {"last_seen_at": NOW, "last_accepted_ingest_at": null}}),
            json!({"a": {"last_seen_at": NOW, "last_accepted_segment": {"day": "20260419"}}}),
            json!({"a": {"last_seen_at": NOW, "ingest_rejection": {"reason_code": "x", "first": NOW, "latest": NOW, "active_count": -1}}}),
        ] {
            fs::write(&path, payload.to_string()).unwrap();
            assert_eq!(read_device_activity(&path), DeviceActivityRead::Malformed);
        }
    }

    #[test]
    fn accepted_ingest_clears_rejection_and_preserves_connection_activity() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        ledger.touch_last_seen_at("a", NOW).unwrap();
        ledger
            .record_ingest_rejection("a", "2026-04-19T18:04:12Z", "event_append_failed", None)
            .unwrap();

        assert!(
            ledger
                .record_accepted_ingest(
                    "a",
                    "2026-04-19T18:05:12Z",
                    AcceptedSegment {
                        day: "20260419".to_owned(),
                        name: "180512_1".to_owned(),
                    },
                    None,
                )
                .unwrap()
        );

        let DeviceActivityRead::Present(activity) = read_device_activity(ledger.devices_path())
        else {
            panic!("activity present");
        };
        assert_eq!(activity["a"].last_seen_at, NOW);
        assert_eq!(
            activity["a"].last_accepted_ingest_at.as_deref(),
            Some("2026-04-19T18:05:12Z")
        );
        assert_eq!(
            activity["a"].last_accepted_segment,
            Some(AcceptedSegment {
                day: "20260419".to_owned(),
                name: "180512_1".to_owned(),
            })
        );
        assert_eq!(activity["a"].ingest_rejection, None);
    }

    #[test]
    fn rejection_streak_crosses_reason_codes_and_saturates() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        assert!(
            ledger
                .record_ingest_rejection("a", NOW, "event_append_failed", None)
                .unwrap()
        );
        assert!(
            ledger
                .record_ingest_rejection("a", "2026-04-19T18:04:12Z", "stream_advance_failed", None)
                .unwrap()
        );

        let DeviceActivityRead::Present(activity) = read_device_activity(ledger.devices_path())
        else {
            panic!("activity present");
        };
        assert_eq!(
            activity["a"].ingest_rejection,
            Some(IngestRejection {
                reason_code: "stream_advance_failed".to_owned(),
                first: NOW.to_owned(),
                latest: "2026-04-19T18:04:12Z".to_owned(),
                active_count: 2,
            })
        );

        fs::write(
            ledger.devices_path(),
            json!({"a": {
                "last_seen_at": NOW,
                "ingest_rejection": {
                    "reason_code": "previous",
                    "first": NOW,
                    "latest": NOW,
                    "active_count": u64::MAX,
                }
            }})
            .to_string(),
        )
        .unwrap();
        assert!(
            ledger
                .record_ingest_rejection("a", "2026-04-19T18:05:12Z", "next", None)
                .unwrap()
        );
        let DeviceActivityRead::Present(activity) = read_device_activity(ledger.devices_path())
        else {
            panic!("activity present");
        };
        assert_eq!(
            activity["a"]
                .ingest_rejection
                .as_ref()
                .unwrap()
                .active_count,
            u64::MAX
        );
        assert_eq!(activity["a"].ingest_rejection.as_ref().unwrap().first, NOW);
        assert_eq!(
            activity["a"].ingest_rejection.as_ref().unwrap().latest,
            "2026-04-19T18:05:12Z"
        );
        assert_eq!(
            activity["a"].ingest_rejection.as_ref().unwrap().reason_code,
            "next"
        );
    }

    #[test]
    fn activity_mutators_do_not_create_metadata_for_an_absent_client() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        assert!(
            !ledger
                .record_ingest_rejection("missing", NOW, "event_append_failed", None)
                .unwrap()
        );
        assert!(
            !ledger
                .record_accepted_ingest(
                    "missing",
                    NOW,
                    AcceptedSegment {
                        day: "20260419".to_owned(),
                        name: "180312_1".to_owned(),
                    },
                    None,
                )
                .unwrap()
        );
        assert!(!ledger.devices_path().exists());
    }

    #[test]
    fn activity_mutator_io_failure_does_not_partially_apply() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        fs::create_dir_all(ledger.devices_path()).unwrap();

        assert!(
            ledger
                .record_ingest_rejection("a", NOW, "event_append_failed", None)
                .is_err()
        );
        assert!(ledger.devices_path().is_dir());
        assert!(
            fs::read_dir(ledger.devices_path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn touch_last_seen_records_a_rfc3339_utc_timestamp() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        assert!(ledger.touch_last_seen("a").unwrap());
        let timestamp = serde_json::from_slice::<Value>(&fs::read(ledger.devices_path()).unwrap())
            .unwrap()["a"]["last_seen_at"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(timestamp.ends_with('Z'));
        assert_eq!(timestamp.len(), 20);
    }

    #[test]
    fn touch_last_seen_at_rejects_invalid_or_non_utc_timestamps_without_writing() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();

        for timestamp in ["not-a-timestamp", "2026-04-19T18:03:12+01:00"] {
            assert!(matches!(
                ledger.touch_last_seen_at("a", timestamp),
                Err(AuthorizedClientsMutationError::InvalidLastSeenAt)
            ));
            assert!(!ledger.devices_path().exists());
        }
    }

    #[test]
    fn update_label_reports_invalid_input_without_claiming_ledger_corruption() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        let before = fs::read(ledger.authorized_clients_path()).unwrap();

        for (label, expected) in [
            ("", "label must not be empty"),
            (&"x".repeat(81), "label too long"),
        ] {
            let error = ledger.update_label("a", label).unwrap_err();
            assert!(matches!(
                error,
                AuthorizedClientsMutationError::InvalidLabel(message) if message == expected
            ));
            assert_eq!(error.to_string(), expected);
            assert_eq!(fs::read(ledger.authorized_clients_path()).unwrap(), before);
        }
    }

    #[test]
    fn corrupt_devices_file_blocks_touch_without_overwrite() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        fs::write(ledger.devices_path(), b"{broken").unwrap();
        let original = fs::read(ledger.devices_path()).unwrap();
        assert!(ledger.touch_last_seen_at("a", NOW).is_err());
        assert_eq!(fs::read(ledger.devices_path()).unwrap(), original);
    }

    #[test]
    fn devices_loader_rejects_records_outside_the_last_seen_schema() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        let original = br#"{"a":{"last_seen_at":"2026-04-19T18:03:12Z","role":"peer"}}"#;
        fs::write(ledger.devices_path(), original).unwrap();
        assert!(ledger.touch_last_seen_at("a", NOW).is_err());
        assert_eq!(fs::read(ledger.devices_path()).unwrap(), original);
    }

    #[test]
    fn remove_deletes_authorization_and_device_metadata() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        ledger.touch_last_seen_at("a", NOW).unwrap();
        assert_eq!(
            ledger.remove("a").unwrap(),
            RemoveOutcome {
                authorized_removed: true,
                device_metadata_removed: true
            }
        );
        assert!(!ledger.is_authorized("a"));
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(ledger.devices_path()).unwrap()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn ordinals_are_case_insensitive_sticky_and_repaired_by_pairing_order() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        let mut first = entry("a", "iPhone", ClientRole::Roleless);
        first.paired_at = "2026-04-19T00:00:02Z".to_owned();
        let mut second = entry("b", "IPHONE", ClientRole::Roleless);
        second.paired_at = "2026-04-19T00:00:01Z".to_owned();
        assert_eq!(ledger.add(first).unwrap().label_ordinal, 1);
        assert_eq!(ledger.add(second).unwrap().label_ordinal, 2);
        assert!(ledger.remove("a").unwrap().authorized_removed);
        assert_eq!(ledger.get("b").unwrap().label_ordinal, 2);

        let payload = json!([
            {"fingerprint":"c","device_label":"Phone","paired_at":"2026-04-19T00:00:03Z","instance_id":"i","label_ordinal":1},
            {"fingerprint":"d","device_label":"phone","paired_at":"2026-04-19T00:00:01Z","instance_id":"i","label_ordinal":1}
        ]);
        write_json(
            ledger.authorized_clients_path(),
            &payload,
            JsonWriteOptions::default(),
        )
        .unwrap();
        assert!(ledger.backfill_label_ordinals().unwrap());
        assert_eq!(ledger.get("d").unwrap().label_ordinal, 1);
        assert_eq!(ledger.get("c").unwrap().label_ordinal, 2);
    }

    #[test]
    fn backfill_leaves_unique_nonsequential_ordinals_untouched() {
        let temporary = TempDir::new();
        let path = temporary
            .path()
            .join("link")
            .join("authorized_clients.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = json!([
            {"fingerprint":"a","device_label":"iPhone","paired_at":"2026-04-19T00:00:02Z","instance_id":"i","label_ordinal":3},
            {"fingerprint":"b","device_label":"iphone","paired_at":"2026-04-19T00:00:01Z","instance_id":"i","label_ordinal":1}
        ])
        .to_string();
        fs::write(&path, &original).unwrap();
        let mut ledger = AuthorizationLedger::new(temporary.path());

        assert!(!ledger.backfill_label_ordinals().unwrap());
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn reload_detects_atomic_replace_when_mtime_and_size_match() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        assert!(ledger.is_authorized("a"));
        let path = ledger.authorized_clients_path().to_path_buf();
        let metadata = fs::metadata(&path).unwrap();
        let before_inode = metadata.ino();
        let reference = temporary.path().join("mtime-reference");
        fs::hard_link(&path, &reference).unwrap();
        let mut replacement = fs::read(&path).unwrap();
        let fingerprint = replacement
            .windows(3)
            .position(|window| window == b"\"a\"")
            .unwrap();
        replacement[fingerprint + 1] = b'b';
        atomic_replace(
            &path,
            &replacement,
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .unwrap();
        assert!(
            std::process::Command::new("touch")
                .arg("-r")
                .arg(&reference)
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(fs::metadata(&path).unwrap().mtime(), metadata.mtime());
        assert_eq!(
            fs::metadata(&path).unwrap().mtime_nsec(),
            metadata.mtime_nsec()
        );
        assert_ne!(fs::metadata(&path).unwrap().ino(), before_inode);
        assert!(ledger.reload_if_stale());
        assert!(!ledger.is_authorized("a"));
        assert!(ledger.is_authorized("b"));
    }

    #[test]
    fn load_preserves_array_order_of_unique_fingerprints() {
        let temporary = TempDir::new();
        let path = temporary
            .path()
            .join("link")
            .join("authorized_clients.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!([
                {"fingerprint":"a","device_label":"first","paired_at":"1","instance_id":"i"},
                {"fingerprint":"b","device_label":"middle","paired_at":"2","instance_id":"i"}
            ])
            .to_string(),
        )
        .unwrap();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        let entries = ledger.snapshot();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.fingerprint.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(entries[0].device_label, "first");
    }

    #[test]
    fn duplicate_fingerprint_makes_the_read_unavailable() {
        let temporary = TempDir::new();
        let path = temporary
            .path()
            .join("link")
            .join("authorized_clients.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = json!([
            {"fingerprint":"a","device_label":"old","paired_at":"1","instance_id":"i"},
            {"fingerprint":"b","device_label":"middle","paired_at":"2","instance_id":"i"},
            {"fingerprint":"a","device_label":"new","paired_at":"3","instance_id":"i"}
        ])
        .to_string();
        fs::write(&path, &original).unwrap();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        assert_eq!(ledger.read_state(), AuthorizedClientsRead::DuplicateCid);
        assert!(!ledger.is_authorized("a"));
        assert!(!ledger.is_authorized("b"));
        for result in [
            ledger
                .add(entry("c", "phone", ClientRole::Roleless))
                .map(|_| ()),
            ledger.remove("a").map(|_| ()),
            ledger.update_label("a", "renamed").map(|_| ()),
            ledger.touch_last_seen_at("a", NOW).map(|_| ()),
        ] {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("were NOT changed"));
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        }
    }

    #[test]
    fn add_preserves_existing_pairing_identity_on_the_same_fingerprint() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        let mut first = entry("a", "phone", ClientRole::Roleless);
        first.client_label = "host-a".to_owned();
        first.platform = Some(Platform::Linux);
        ledger.add(first).unwrap();
        let mut second = entry("a", "tablet", ClientRole::Roleless);
        second.client_label = "replaced".to_owned();
        second.platform = Some(Platform::Ios);
        let stored = ledger.add(second).unwrap();
        assert_eq!(stored.device_label, "tablet");
        assert_eq!(stored.client_label, "host-a");
        assert_eq!(stored.platform, Some(Platform::Linux));
        let payload = serde_json::from_slice::<Vec<Value>>(
            &fs::read(ledger.authorized_clients_path()).unwrap(),
        )
        .unwrap();
        assert_eq!(payload[0]["client_label"], "host-a");
        assert_eq!(payload[0]["platform"], "linux");
    }

    #[test]
    fn update_label_and_ordinal_repair_preserve_pairing_identity() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        let mut first = entry("a", "iPhone", ClientRole::Roleless);
        first.client_label = "host-a".to_owned();
        first.platform = Some(Platform::Macos);
        ledger.add(first).unwrap();
        let mut second = entry("b", "IPHONE", ClientRole::Roleless);
        second.client_label = "host-b".to_owned();
        second.platform = Some(Platform::Ios);
        ledger.add(second).unwrap();

        let updated = ledger.update_label("a", "Studio").unwrap().unwrap();
        assert_eq!(updated.device_label, "Studio");
        assert_eq!(updated.client_label, "host-a");
        assert_eq!(updated.platform, Some(Platform::Macos));
        assert_eq!(ledger.get("b").unwrap().client_label, "host-b");
        assert_eq!(ledger.get("b").unwrap().platform, Some(Platform::Ios));

        let payload = json!([
            {"fingerprint":"c","device_label":"Phone","paired_at":"2026-04-19T00:00:03Z","instance_id":"i","label_ordinal":1,"client_label":"host-c","platform":"android"},
            {"fingerprint":"d","device_label":"phone","paired_at":"2026-04-19T00:00:01Z","instance_id":"i","label_ordinal":1,"client_label":"host-d","platform":"windows"}
        ]);
        write_json(
            ledger.authorized_clients_path(),
            &payload,
            JsonWriteOptions::default(),
        )
        .unwrap();
        assert!(ledger.backfill_label_ordinals().unwrap());
        let repaired_c = ledger.get("c").unwrap();
        let repaired_d = ledger.get("d").unwrap();
        assert_eq!(repaired_c.client_label, "host-c");
        assert_eq!(repaired_c.platform, Some(Platform::Android));
        assert_eq!(repaired_d.client_label, "host-d");
        assert_eq!(repaired_d.platform, Some(Platform::Windows));
    }

    #[test]
    fn legacy_kind_defaults_to_cert_and_optional_fields_only_emit_when_meaningful() {
        let temporary = TempDir::new();
        let path = temporary
            .path()
            .join("link")
            .join("authorized_clients.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!([
                {"fingerprint":"legacy","device_label":"phone","paired_at":"1","instance_id":"i"},
                {"fingerprint":"ignored","device_label":"legacy","paired_at":"2","instance_id":"i","kind":"token"}
            ])
            .to_string(),
        )
        .unwrap();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        assert_eq!(ledger.snapshot().len(), 1);
        ledger
            .add(entry("sibling", "tablet", ClientRole::Roleless))
            .unwrap();
        let mut meaningful = entry("meaningful", "tablet", ClientRole::Roleless);
        meaningful.network = Some("local".to_owned());
        meaningful.client_label = "tablet-host".to_owned();
        meaningful.label_ordinal = 2;
        meaningful.platform = Some(Platform::Linux);
        ledger.add(meaningful).unwrap();
        let payload = serde_json::from_slice::<Vec<Value>>(
            &fs::read(ledger.authorized_clients_path()).unwrap(),
        )
        .unwrap();
        let legacy = payload
            .iter()
            .find(|item| item["fingerprint"] == "legacy")
            .unwrap();
        assert_eq!(legacy["kind"], CERT_KIND);
        assert!(legacy.get("network").is_none());
        assert!(legacy.get("client_label").is_none());
        assert!(legacy.get("platform").is_none());
        assert!(legacy.get("label_ordinal").is_none());
        let meaningful = payload
            .iter()
            .find(|item| item["fingerprint"] == "meaningful")
            .unwrap();
        assert_eq!(meaningful["network"], "local");
        assert_eq!(meaningful["client_label"], "tablet-host");
        assert_eq!(meaningful["label_ordinal"], 2);
        assert_eq!(meaningful["platform"], "linux");
    }

    #[test]
    fn sourced_deliveries_on_one_device_read_back_independently() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        assert!(
            ledger
                .record_accepted_ingest(
                    "a",
                    "2026-04-19T18:05:12Z",
                    AcceptedSegment {
                        day: "20260419".to_owned(),
                        name: "180512_1".to_owned(),
                    },
                    Some("audio"),
                )
                .unwrap()
        );
        assert!(
            ledger
                .record_accepted_ingest(
                    "a",
                    "2026-04-19T18:06:12Z",
                    AcceptedSegment {
                        day: "20260419".to_owned(),
                        name: "180612_1".to_owned(),
                    },
                    Some(""),
                )
                .unwrap()
        );

        let activity = present_activity(&ledger, "a");
        assert_eq!(
            activity.last_accepted_ingest_at.as_deref(),
            Some("2026-04-19T18:06:12Z")
        );
        match activity.sources.get("audio") {
            Some(SourceRecord::Valid(source)) => {
                assert_eq!(
                    source.last_accepted_ingest_at.as_deref(),
                    Some("2026-04-19T18:05:12Z")
                );
                assert_eq!(
                    source.last_accepted_segment,
                    Some(AcceptedSegment {
                        day: "20260419".to_owned(),
                        name: "180512_1".to_owned(),
                    })
                );
            }
            other => panic!("audio source valid, got {other:?}"),
        }
        match activity.sources.get("") {
            Some(SourceRecord::Valid(source)) => {
                assert_eq!(
                    source.last_accepted_ingest_at.as_deref(),
                    Some("2026-04-19T18:06:12Z")
                );
            }
            other => panic!("empty source valid, got {other:?}"),
        }
    }

    #[test]
    fn device_level_only_activity_without_sources_key_still_loads() {
        let temporary = TempDir::new();
        let path = temporary.path().join("link/devices.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!({"a": {
                "last_seen_at": NOW,
                "last_accepted_ingest_at": NOW,
                "last_accepted_segment": {"day": "20260419", "name": "180312_1"},
            }})
            .to_string(),
        )
        .unwrap();

        let DeviceActivityRead::Present(activity) = read_device_activity(&path) else {
            panic!("activity present");
        };
        assert!(activity["a"].sources.is_empty());
        assert_eq!(activity["a"].last_accepted_ingest_at.as_deref(), Some(NOW));
        assert_eq!(
            activity["a"].last_accepted_segment,
            Some(AcceptedSegment {
                day: "20260419".to_owned(),
                name: "180312_1".to_owned(),
            })
        );
    }

    #[test]
    fn malformed_source_entry_does_not_fail_the_device_or_sibling() {
        let temporary = TempDir::new();
        let path = temporary.path().join("link/devices.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!({"a": {
                "last_seen_at": NOW,
                "sources": {
                    "audio": {
                        "last_accepted_ingest_at": NOW,
                        "unknown": true
                    },
                    "location": {
                        "last_accepted_ingest_at": "not-a-timestamp"
                    },
                    "screen": {
                        "last_accepted_ingest_at": NOW,
                        "last_accepted_segment": {"day": "20260419", "name": "180312_1"}
                    }
                }
            }})
            .to_string(),
        )
        .unwrap();

        let DeviceActivityRead::Present(activity) = read_device_activity(&path) else {
            panic!("activity present, not whole-file malformed");
        };
        assert!(matches!(
            activity["a"].sources.get("audio"),
            Some(SourceRecord::Malformed(_))
        ));
        assert!(matches!(
            activity["a"].sources.get("location"),
            Some(SourceRecord::Malformed(_))
        ));
        match activity["a"].sources.get("screen") {
            Some(SourceRecord::Valid(source)) => {
                assert_eq!(source.last_accepted_ingest_at.as_deref(), Some(NOW));
            }
            other => panic!("screen source valid, got {other:?}"),
        }
    }

    #[test]
    fn accept_and_reject_on_different_sources_do_not_interfere() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        assert!(
            ledger
                .record_accepted_ingest(
                    "a",
                    "2026-04-19T18:05:12Z",
                    AcceptedSegment {
                        day: "20260419".to_owned(),
                        name: "180512_1".to_owned(),
                    },
                    Some("audio"),
                )
                .unwrap()
        );
        assert!(
            ledger
                .record_ingest_rejection(
                    "a",
                    "2026-04-19T18:06:12Z",
                    "event_append_failed",
                    Some("location"),
                )
                .unwrap()
        );

        let activity = present_activity(&ledger, "a");
        match activity.sources.get("audio") {
            Some(SourceRecord::Valid(source)) => {
                assert_eq!(
                    source.last_accepted_ingest_at.as_deref(),
                    Some("2026-04-19T18:05:12Z")
                );
                assert_eq!(source.ingest_rejection, None);
            }
            other => panic!("audio source valid, got {other:?}"),
        }
        match activity.sources.get("location") {
            Some(SourceRecord::Valid(source)) => {
                assert_eq!(source.last_accepted_ingest_at, None);
                assert_eq!(
                    source.ingest_rejection,
                    Some(IngestRejection {
                        reason_code: "event_append_failed".to_owned(),
                        first: "2026-04-19T18:06:12Z".to_owned(),
                        latest: "2026-04-19T18:06:12Z".to_owned(),
                        active_count: 1,
                    })
                );
            }
            other => panic!("location source valid, got {other:?}"),
        }
        assert_eq!(
            activity
                .ingest_rejection
                .as_ref()
                .map(|row| row.reason_code.as_str()),
            Some("event_append_failed")
        );
    }

    #[test]
    fn source_rejection_streak_accumulates_and_clears_independently() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        assert!(
            ledger
                .record_ingest_rejection("a", NOW, "event_append_failed", Some("audio"))
                .unwrap()
        );
        assert!(
            ledger
                .record_ingest_rejection(
                    "a",
                    "2026-04-19T18:04:12Z",
                    "stream_advance_failed",
                    Some("audio"),
                )
                .unwrap()
        );
        assert!(
            ledger
                .record_ingest_rejection(
                    "a",
                    "2026-04-19T18:05:12Z",
                    "notify_failed",
                    Some("location"),
                )
                .unwrap()
        );
        assert!(
            ledger
                .record_accepted_ingest(
                    "a",
                    "2026-04-19T18:06:12Z",
                    AcceptedSegment {
                        day: "20260419".to_owned(),
                        name: "180612_1".to_owned(),
                    },
                    Some("audio"),
                )
                .unwrap()
        );

        let activity = present_activity(&ledger, "a");
        match activity.sources.get("audio") {
            Some(SourceRecord::Valid(source)) => {
                assert_eq!(source.ingest_rejection, None);
                assert_eq!(
                    source.last_accepted_ingest_at.as_deref(),
                    Some("2026-04-19T18:06:12Z")
                );
            }
            other => panic!("audio source valid, got {other:?}"),
        }
        match activity.sources.get("location") {
            Some(SourceRecord::Valid(source)) => {
                assert_eq!(
                    source.ingest_rejection,
                    Some(IngestRejection {
                        reason_code: "notify_failed".to_owned(),
                        first: "2026-04-19T18:05:12Z".to_owned(),
                        latest: "2026-04-19T18:05:12Z".to_owned(),
                        active_count: 1,
                    })
                );
            }
            other => panic!("location source valid, got {other:?}"),
        }
        assert_eq!(activity.ingest_rejection, None);
    }

    #[test]
    fn sibling_source_mutation_preserves_malformed_source_json() {
        let temporary = TempDir::new();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        ledger
            .add(entry("a", "phone", ClientRole::Roleless))
            .unwrap();
        let malformed_audio = json!({
            "last_accepted_ingest_at": NOW,
            "unknown": true
        });
        fs::write(
            ledger.devices_path(),
            json!({"a": {
                "last_seen_at": NOW,
                "sources": {
                    "audio": malformed_audio,
                }
            }})
            .to_string(),
        )
        .unwrap();

        assert!(
            ledger
                .record_accepted_ingest(
                    "a",
                    "2026-04-19T18:06:12Z",
                    AcceptedSegment {
                        day: "20260419".to_owned(),
                        name: "180612_1".to_owned(),
                    },
                    Some("location"),
                )
                .unwrap()
        );

        let stored =
            serde_json::from_slice::<Value>(&fs::read(ledger.devices_path()).unwrap()).unwrap();
        assert_eq!(stored["a"]["sources"]["audio"], malformed_audio);
        assert_eq!(
            stored["a"]["sources"]["location"]["last_accepted_ingest_at"],
            "2026-04-19T18:06:12Z"
        );
        let activity = present_activity(&ledger, "a");
        assert!(matches!(
            activity.sources.get("audio"),
            Some(SourceRecord::Malformed(_))
        ));
    }

    fn present_activity(ledger: &AuthorizationLedger, cid: &str) -> ClientActivity {
        let DeviceActivityRead::Present(activity) = read_device_activity(ledger.devices_path())
        else {
            panic!("activity present");
        };
        activity.get(cid).cloned().expect("activity for client")
    }

    fn entry(fingerprint: &str, label: &str, role: ClientRole) -> ClientEntry {
        ClientEntry::new(fingerprint, label, "2026-04-19T00:00:00Z", "instance", role)
    }

    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "sol-link-ledger-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod pairing_identity_read_tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::*;
    use crate::pairing_identity::{ClientLabelState, Platform, PlatformState};

    #[test]
    fn get_pairing_identity_fields_preserves_distinctions_that_get_collapses() {
        let temporary = TempDir::new();
        let path = authorized_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let long = "a".repeat(254);
        fs::write(
            &path,
            json!([
                {"fingerprint":"absent-label","device_label":"p","paired_at":"1","instance_id":"i"},
                {"fingerprint":"empty-label","device_label":"p","paired_at":"1","instance_id":"i","client_label":""},
                {"fingerprint":"valid-label","device_label":"p","paired_at":"1","instance_id":"i","client_label":"phone"},
                {"fingerprint":"long-label","device_label":"p","paired_at":"1","instance_id":"i","client_label": long},
                {"fingerprint":"malformed-label","device_label":"p","paired_at":"1","instance_id":"i","client_label":1},
                {"fingerprint":"absent-platform","device_label":"p","paired_at":"1","instance_id":"i"},
                {"fingerprint":"linux","device_label":"p","paired_at":"1","instance_id":"i","platform":"linux"},
                {"fingerprint":"macos","device_label":"p","paired_at":"1","instance_id":"i","platform":"macos"},
                {"fingerprint":"windows","device_label":"p","paired_at":"1","instance_id":"i","platform":"windows"},
                {"fingerprint":"ios","device_label":"p","paired_at":"1","instance_id":"i","platform":"ios"},
                {"fingerprint":"android","device_label":"p","paired_at":"1","instance_id":"i","platform":"android"},
                {"fingerprint":"empty-platform","device_label":"p","paired_at":"1","instance_id":"i","platform":""},
                {"fingerprint":"unknown-platform","device_label":"p","paired_at":"1","instance_id":"i","platform":"plan9"},
                {"fingerprint":"malformed-platform","device_label":"p","paired_at":"1","instance_id":"i","platform":true}
            ])
            .to_string(),
        )
        .unwrap();

        let mut ledger = AuthorizationLedger::new(temporary.path());

        assert_eq!(
            ledger
                .get_pairing_identity_fields("absent-label")
                .unwrap()
                .unwrap()
                .client_label,
            ClientLabelState::Absent
        );
        assert_eq!(ledger.get("absent-label").unwrap().client_label, "");

        assert_eq!(
            ledger
                .get_pairing_identity_fields("empty-label")
                .unwrap()
                .unwrap()
                .client_label,
            ClientLabelState::Empty
        );
        assert_eq!(ledger.get("empty-label").unwrap().client_label, "");

        assert_eq!(
            ledger
                .get_pairing_identity_fields("valid-label")
                .unwrap()
                .unwrap()
                .client_label,
            ClientLabelState::Valid("phone".to_owned())
        );
        assert_eq!(ledger.get("valid-label").unwrap().client_label, "phone");

        assert_eq!(
            ledger
                .get_pairing_identity_fields("long-label")
                .unwrap()
                .unwrap()
                .client_label,
            ClientLabelState::Unprojectable("a".repeat(254))
        );
        assert_eq!(
            ledger.get("long-label").unwrap().client_label,
            "a".repeat(254)
        );

        assert_eq!(
            ledger
                .get_pairing_identity_fields("malformed-label")
                .unwrap()
                .unwrap()
                .client_label,
            ClientLabelState::Malformed
        );
        assert_eq!(ledger.get("malformed-label").unwrap().client_label, "");

        assert_eq!(
            ledger
                .get_pairing_identity_fields("absent-platform")
                .unwrap()
                .unwrap()
                .platform,
            PlatformState::Absent
        );
        assert_eq!(ledger.get("absent-platform").unwrap().platform, None);

        for (fingerprint, platform) in [
            ("linux", Platform::Linux),
            ("macos", Platform::Macos),
            ("windows", Platform::Windows),
            ("ios", Platform::Ios),
            ("android", Platform::Android),
        ] {
            assert_eq!(
                ledger
                    .get_pairing_identity_fields(fingerprint)
                    .unwrap()
                    .unwrap()
                    .platform,
                PlatformState::Valid(platform)
            );
            assert_eq!(ledger.get(fingerprint).unwrap().platform, Some(platform));
        }

        for fingerprint in ["empty-platform", "unknown-platform", "malformed-platform"] {
            assert_eq!(
                ledger
                    .get_pairing_identity_fields(fingerprint)
                    .unwrap()
                    .unwrap()
                    .platform,
                PlatformState::Malformed
            );
            assert_eq!(ledger.get(fingerprint).unwrap().platform, None);
        }

        assert_eq!(
            ledger.get_pairing_identity_fields("missing-row").unwrap(),
            None
        );
        assert_eq!(ledger.get("missing-row"), None);
    }

    #[test]
    fn get_pairing_identity_fields_and_get_agree_on_a_broken_ledger() {
        let temporary = TempDir::new();
        let path = authorized_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{bad").unwrap();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        assert_eq!(ledger.get("a"), None);
        assert_eq!(
            ledger.get_pairing_identity_fields("a"),
            Err(AuthorizedClientsRead::Malformed)
        );
        assert_eq!(ledger.read_state(), AuthorizedClientsRead::Malformed);

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let mut ledger = AuthorizationLedger::new(temporary.path());
        assert_eq!(ledger.get("a"), None);
        assert_eq!(
            ledger.get_pairing_identity_fields("a"),
            Err(AuthorizedClientsRead::Unreadable)
        );
        assert_eq!(ledger.read_state(), AuthorizedClientsRead::Unreadable);
    }

    fn authorized_path(root: &Path) -> PathBuf {
        root.join("link").join("authorized_clients.json")
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "sol-link-pairing-read-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
