// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable sync-state inventory and private atomic state publication.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};

/// One backend recognised by the sync library.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendName {
    Plaud,
    Obsidian,
    Audio,
    Oura,
}

impl BackendName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plaud => "plaud",
            Self::Obsidian => "obsidian",
            Self::Audio => "audio",
            Self::Oura => "oura",
        }
    }
}

/// The fixture-defined backend presentation order.
pub const SYNC_BACKEND_INVENTORY: [BackendName; 4] = [
    BackendName::Plaud,
    BackendName::Obsidian,
    BackendName::Audio,
    BackendName::Oura,
];

/// A loss-preserving sync-state JSON document.
///
/// Keeping the root as an ordered JSON object preserves every known and unknown
/// root and per-file member while backend orchestration changes only its own keys.
#[derive(Clone, PartialEq)]
pub struct SyncState {
    backend: BackendName,
    root: Map<String, Value>,
}

impl SyncState {
    #[must_use]
    pub fn empty(backend: BackendName) -> Self {
        let mut root = Map::new();
        root.insert(
            "backend".to_owned(),
            Value::String(backend.as_str().to_owned()),
        );
        root.insert("files".to_owned(), Value::Object(Map::new()));
        Self { backend, root }
    }

    #[must_use]
    pub const fn backend(&self) -> BackendName {
        self.backend
    }

    #[must_use]
    pub fn root(&self) -> &Map<String, Value> {
        &self.root
    }

    #[must_use]
    pub fn root_mut(&mut self) -> &mut Map<String, Value> {
        &mut self.root
    }

    #[must_use]
    pub fn files_mut(&mut self) -> &mut Map<String, Value> {
        let files = self
            .root
            .entry("files".to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !files.is_object() {
            *files = Value::Object(Map::new());
        }
        files
            .as_object_mut()
            .expect("files was normalized to object")
    }

    fn from_value(backend: BackendName, value: Value) -> Result<Self, SyncStateReadClass> {
        let root = value
            .as_object()
            .cloned()
            .ok_or(SyncStateReadClass::WrongShape)?;
        match root.get("files") {
            None | Some(Value::Object(_)) => {
                validate_known_numbers(backend, &root)?;
                Ok(Self { backend, root })
            }
            Some(_) => Err(SyncStateReadClass::WrongShape),
        }
    }
}

impl fmt::Debug for SyncState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncState")
            .field("backend", &self.backend)
            .field("root", &redacted_root(&self.root))
            .finish()
    }
}

/// Why a sync-state document starts a benign re-catalogue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncStateReadClass {
    Io,
    MalformedJson,
    WrongShape,
    Overflow,
}

/// A sync-state read is never a blocking error.
#[derive(Clone, Debug, PartialEq)]
pub enum SyncStateRead {
    Absent,
    Unreadable { class: SyncStateReadClass },
    Loaded(SyncState),
}

/// A named failure to publish sync state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncStateWriteError {
    message: String,
}

impl fmt::Display for SyncStateWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SyncStateWriteError {}

/// Load one backend state document.
///
/// Unlike consent artifacts, unreadable *sync* state is benign and re-catalogs.
#[must_use]
pub fn read_sync_state(root: &Path, backend: BackendName) -> SyncStateRead {
    let path = state_path(root, backend);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return SyncStateRead::Absent,
        Err(_) => {
            return SyncStateRead::Unreadable {
                class: SyncStateReadClass::Io,
            };
        }
    };
    let value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return SyncStateRead::Unreadable {
                class: SyncStateReadClass::MalformedJson,
            };
        }
    };
    match SyncState::from_value(backend, value) {
        Ok(state) => SyncStateRead::Loaded(state),
        Err(class) => SyncStateRead::Unreadable { class },
    }
}

/// Read a connection from the original native owner's retained root.
#[cfg(unix)]
pub fn read_sync_state_bound(
    root: &solstone_core_journal_io::JournalRoot,
    backend: BackendName,
) -> SyncStateRead {
    use solstone_core_journal_io::{open_flat_directory_bound, read_observed_file};
    use std::ffi::OsStr;
    let read = || -> Result<SyncStateRead, ()> {
        root.revalidate_canonical_binding().map_err(|_| ())?;
        let Some(imports) =
            open_flat_directory_bound(root, OsStr::new("imports"), root.canonical_path())
                .map_err(|_| ())?
        else {
            return Ok(SyncStateRead::Absent);
        };
        let name = format!("{}.json", backend.as_str());
        let observed = read_observed_file(&imports, OsStr::new(&name)).map_err(|_| ())?;
        root.revalidate_canonical_binding().map_err(|_| ())?;
        let current = solstone_core_journal_io::FlatDirectory::open(root, Path::new("imports"))
            .map_err(|_| ())?;
        if current.identity() != imports.identity() {
            return Err(());
        }
        let Some(observed) = observed else {
            return Ok(SyncStateRead::Absent);
        };
        let value = match serde_json::from_slice(&observed.bytes) {
            Ok(value) => value,
            Err(_) => {
                return Ok(SyncStateRead::Unreadable {
                    class: SyncStateReadClass::MalformedJson,
                });
            }
        };
        Ok(match SyncState::from_value(backend, value) {
            Ok(state) => SyncStateRead::Loaded(state),
            Err(class) => SyncStateRead::Unreadable { class },
        })
    };
    read().unwrap_or(SyncStateRead::Unreadable {
        class: SyncStateReadClass::Io,
    })
}

/// Publish connection changes while the caller retains its source operation.
#[cfg(unix)]
pub fn write_sync_state_bound(
    root: &solstone_core_journal_io::JournalRoot,
    state: &SyncState,
    stop: &crate::WorkStop,
) -> Result<(), SyncStateWriteError> {
    use solstone_core_journal_io::{
        BoundAtomicOutcome, FlatDirectory, LockOptions, atomic_replace_bound_checked,
        create_or_open_flat_directory_bound, hold_lock_bound_checked,
    };
    use std::ffi::OsStr;
    let checkpoint = || -> Result<(), String> {
        if stop.is_requested() {
            return Err("import stopped".into());
        }
        root.revalidate_canonical_binding()
            .map_err(|_| "journal binding changed".to_owned())
    };
    checkpoint().map_err(path_error)?;
    let imports = create_or_open_flat_directory_bound(
        root,
        OsStr::new("imports"),
        0o700,
        root.canonical_path(),
    )
    .map_err(path_error)?;
    let verify = || -> Result<(), String> {
        checkpoint()?;
        let current =
            FlatDirectory::open(root, Path::new("imports")).map_err(|_| "source state changed")?;
        if current.identity() != imports.identity() {
            return Err("source state changed".into());
        }
        Ok(())
    };
    let name = format!("{}.json", state.backend.as_str());
    let _lock = hold_lock_bound_checked(
        &imports,
        OsStr::new(&name),
        &root.canonical_path().join("imports"),
        LockOptions::default(),
        false,
        &verify,
    )
    .map_err(path_error)?;
    let bytes = serialize_state(&state.root).map_err(|message| SyncStateWriteError { message })?;
    match atomic_replace_bound_checked(&imports, OsStr::new(&name), &bytes, 0o600, &verify)
        .map_err(path_error)?
    {
        BoundAtomicOutcome::Published { .. } => verify().map_err(path_error),
        _ => Err(path_error(
            "source state publication could not be confirmed",
        )),
    }
}

/// Atomically publish state with the reference private modes.
pub fn write_sync_state(root: &Path, state: &SyncState) -> Result<(), SyncStateWriteError> {
    let imports = root.join("imports");
    create_directory_with_mode(&imports, 0o700).map_err(path_error)?;
    let bytes = serialize_state(&state.root).map_err(|message| SyncStateWriteError { message })?;
    atomic_replace(
        state_path(root, state.backend),
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(path_error)
}

#[must_use]
pub fn state_path(root: &Path, backend: BackendName) -> PathBuf {
    root.join("imports")
        .join(format!("{}.json", backend.as_str()))
}

fn path_error(error: impl fmt::Display) -> SyncStateWriteError {
    SyncStateWriteError {
        message: error.to_string(),
    }
}

fn serialize_state(value: &Map<String, Value>) -> Result<Vec<u8>, String> {
    let rendered = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    Ok(escape_non_ascii(&rendered).into_bytes())
}

fn escape_non_ascii(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii() {
            rendered.push(character);
        } else if u32::from(character) <= 0xffff {
            use std::fmt::Write;
            let _ = write!(rendered, "\\u{:04x}", u32::from(character));
        } else {
            use std::fmt::Write;
            let scalar = u32::from(character) - 0x1_0000;
            let high = 0xd800 + (scalar >> 10);
            let low = 0xdc00 + (scalar & 0x3ff);
            let _ = write!(rendered, "\\u{high:04x}\\u{low:04x}");
        }
    }
    rendered
}

fn validate_known_numbers(
    backend: BackendName,
    root: &Map<String, Value>,
) -> Result<(), SyncStateReadClass> {
    let fields: &[&str] = match backend {
        BackendName::Plaud => &["filesize"],
        BackendName::Obsidian => &["edit_count", "segments"],
        BackendName::Audio => &["filesize", "duration"],
        BackendName::Oura => &[],
    };
    let Some(files) = root.get("files").and_then(Value::as_object) else {
        return Ok(());
    };
    for entry in files.values().filter_map(Value::as_object) {
        for field in fields {
            if entry.get(*field).is_some_and(outside_i64) {
                return Err(SyncStateReadClass::Overflow);
            }
        }
    }
    Ok(())
}

fn outside_i64(value: &Value) -> bool {
    value
        .as_u64()
        .is_some_and(|number| number > i64::MAX as u64)
}

const SOURCE_IDENTITY: &str = "source_identity";
const FOLDER_NAME: &str = "folder_name";
const REMEMBERED: &str = "remembered";
const ACCESS_STATE: &str = "access_state";
const ACCESS_MATERIAL: &str = "access_material";
const RETAINED_CATALOGS: &str = "retained_catalogs";
const REDACTED: &str = "[redacted]";

/// Whether a remembered folder grant can be read again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessState {
    Absent,
    Ready,
    Lost,
}

impl AccessState {
    fn parse(value: &Value) -> Option<Self> {
        match value.as_str() {
            Some("absent") => Some(Self::Absent),
            Some("ready") => Some(Self::Ready),
            Some("lost") => Some(Self::Lost),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Ready => "ready",
            Self::Lost => "lost",
        }
    }
}

impl fmt::Display for AccessState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The folder connection an owner can see. It never carries access material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicConnection {
    pub source_identity: String,
    pub folder_name: String,
    pub remembered: bool,
    pub access_state: AccessState,
}

impl fmt::Display for PublicConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} remembered={} {}",
            self.source_identity, self.folder_name, self.remembered, self.access_state
        )
    }
}

/// A remembered grant that may refresh on the page that still holds it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshPermit {
    pub source_identity: String,
    pub backend: BackendName,
    pub folder_name: String,
}

/// Why a remembered refresh cannot proceed. The reason never includes material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshRefusal {
    StalePage,
    NotRemembered,
    AccessLost,
    MissingIdentity,
}

impl fmt::Display for RefreshRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StalePage => "The page that asked for this folder is no longer current.",
            Self::NotRemembered => "Saved access to this folder is not available.",
            Self::AccessLost => "Saved access to this folder was lost. Choose the folder again.",
            Self::MissingIdentity => "This folder has no source identity.",
        })
    }
}

impl std::error::Error for RefreshRefusal {}

/// A folder-identity change was refused before the document changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FolderIdentityError {
    BackendRefused,
    IdentityPresent,
    MaterialRequired,
    MissingIdentity,
    NotRemembered,
    BadFolderName,
    Unavailable,
}

impl fmt::Display for FolderIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BackendRefused => "This source does not keep a folder identity.",
            Self::IdentityPresent => "This source already has a folder identity.",
            Self::MaterialRequired => "Saved access needs the folder grant.",
            Self::MissingIdentity => "This folder has no source identity.",
            Self::NotRemembered => "Saved access to this folder is not available.",
            Self::BadFolderName => "The folder name is not one component.",
            Self::Unavailable => "A folder identity could not be allocated.",
        })
    }
}

impl std::error::Error for FolderIdentityError {}

/// Allocate a folder identity when the document does not have one.
///
/// The active `files` object stays, including imported rows. This does not
/// park a catalog and does not clear `files`. No directory is deleted.
/// Plaud and Oura are refused.
pub fn begin_folder(
    state: &mut SyncState,
    folder_name: &str,
    remembered: bool,
    material: Option<&str>,
) -> Result<String, FolderIdentityError> {
    folder_backend(state.backend)?;
    if state.root.contains_key(SOURCE_IDENTITY) {
        return Err(FolderIdentityError::IdentityPresent);
    }
    prepare_grant(folder_name, remembered, material)?;
    let identity = mint_source_identity()?;
    state
        .root
        .insert(SOURCE_IDENTITY.to_owned(), Value::String(identity.clone()));
    store_grant(state, folder_name, remembered, material);
    Ok(identity)
}

/// Replace the grant for an existing identity. The identity and `files` stay.
pub fn reauthorize(
    state: &mut SyncState,
    folder_name: &str,
    remembered: bool,
    material: Option<&str>,
) -> Result<(), FolderIdentityError> {
    folder_backend(state.backend)?;
    if valid_identity(state).is_none() {
        return Err(FolderIdentityError::MissingIdentity);
    }
    prepare_grant(folder_name, remembered, material)?;
    store_grant(state, folder_name, remembered, material);
    Ok(())
}

/// Allocate a new identity and park the previous file catalogue.
///
/// No path is deleted. The remembered rule matches [`begin_folder`].
pub fn replace_folder(
    state: &mut SyncState,
    folder_name: &str,
    remembered: bool,
    material: Option<&str>,
) -> Result<String, FolderIdentityError> {
    folder_backend(state.backend)?;
    prepare_grant(folder_name, remembered, material)?;
    let identity = mint_source_identity()?;
    let previous = valid_identity(state).map(str::to_owned);
    if let Some(files) = files_object(state) {
        if let Some(previous) = previous {
            park_files(state, files, &previous, true)?;
        } else if !files.is_empty() {
            park_files(state, files, "unscoped", false)?;
        }
    }
    state
        .root
        .insert(SOURCE_IDENTITY.to_owned(), Value::String(identity.clone()));
    state
        .root
        .insert("files".to_owned(), Value::Object(Map::new()));
    store_grant(state, folder_name, remembered, material);
    // The previous selection belongs to the parked catalogue. The replacement
    // must be resolved from its new grant before a source-less sync can run.
    state.root.remove("source_path");
    Ok(identity)
}

/// Forget the stored grant after the owner-facing access was lost.
///
/// `remembered` stays true. The identity and `files` stay.
pub fn mark_access_lost(state: &mut SyncState) -> Result<(), FolderIdentityError> {
    folder_backend(state.backend)?;
    if valid_identity(state).is_none() {
        return Err(FolderIdentityError::MissingIdentity);
    }
    if state.root.get(REMEMBERED).and_then(Value::as_bool) != Some(true) {
        return Err(FolderIdentityError::NotRemembered);
    }
    state.root.remove(ACCESS_MATERIAL);
    state.root.insert(
        ACCESS_STATE.to_owned(),
        Value::String(AccessState::Lost.as_str().to_owned()),
    );
    Ok(())
}

/// The public fields of a document that has a valid folder identity.
#[must_use]
pub fn public_connection(state: &SyncState) -> Option<PublicConnection> {
    let source_identity = valid_identity(state)?.to_owned();
    let folder_name = state
        .root
        .get(FOLDER_NAME)
        .and_then(Value::as_str)
        .filter(|name| one_component(name))?
        .to_owned();
    let remembered = state.root.get(REMEMBERED).and_then(Value::as_bool)?;
    let access_state = state.root.get(ACCESS_STATE).and_then(AccessState::parse)?;
    Some(PublicConnection {
        source_identity,
        folder_name,
        remembered,
        access_state,
    })
}

/// Decide whether a remembered grant may refresh. This does not mutate state,
/// read a directory, or return access material.
pub fn permit_remembered_refresh(
    state: &SyncState,
    page_token: &str,
    bound_page_token: &str,
) -> Result<RefreshPermit, RefreshRefusal> {
    if page_token.is_empty() || bound_page_token.is_empty() || page_token != bound_page_token {
        return Err(RefreshRefusal::StalePage);
    }
    permit_ready_connection(state)
}

/// Decide whether a remembered connection may refresh after navigation.
///
/// There is no page token. This does not mutate state, read a directory, or
/// return access material.
pub fn permit_runtime_refresh(state: &SyncState) -> Result<RefreshPermit, RefreshRefusal> {
    permit_ready_connection(state)
}

fn permit_ready_connection(state: &SyncState) -> Result<RefreshPermit, RefreshRefusal> {
    if state.root.get(REMEMBERED).and_then(Value::as_bool) != Some(true) {
        return Err(RefreshRefusal::NotRemembered);
    }
    match state.root.get(ACCESS_STATE).and_then(AccessState::parse) {
        Some(AccessState::Lost) => return Err(RefreshRefusal::AccessLost),
        Some(AccessState::Absent) | None => return Err(RefreshRefusal::NotRemembered),
        Some(AccessState::Ready) => {}
    }
    let Some(source_identity) = valid_identity(state) else {
        return Err(RefreshRefusal::MissingIdentity);
    };
    let Some(folder_name) = state
        .root
        .get(FOLDER_NAME)
        .and_then(Value::as_str)
        .filter(|name| one_component(name))
    else {
        return Err(RefreshRefusal::MissingIdentity);
    };
    Ok(RefreshPermit {
        source_identity: source_identity.to_owned(),
        backend: state.backend,
        folder_name: folder_name.to_owned(),
    })
}

fn folder_backend(backend: BackendName) -> Result<(), FolderIdentityError> {
    match backend {
        BackendName::Obsidian | BackendName::Audio => Ok(()),
        BackendName::Plaud | BackendName::Oura => Err(FolderIdentityError::BackendRefused),
    }
}

fn prepare_grant(
    folder_name: &str,
    remembered: bool,
    material: Option<&str>,
) -> Result<(), FolderIdentityError> {
    if !one_component(folder_name) {
        return Err(FolderIdentityError::BadFolderName);
    }
    if remembered && material_text(material).is_none() {
        return Err(FolderIdentityError::MaterialRequired);
    }
    Ok(())
}

fn store_grant(state: &mut SyncState, folder_name: &str, remembered: bool, material: Option<&str>) {
    state.root.insert(
        FOLDER_NAME.to_owned(),
        Value::String(folder_name.to_owned()),
    );
    state
        .root
        .insert(REMEMBERED.to_owned(), Value::Bool(remembered));
    if remembered {
        state.root.insert(
            ACCESS_MATERIAL.to_owned(),
            Value::String(material_text(material).unwrap_or("").to_owned()),
        );
        state.root.insert(
            ACCESS_STATE.to_owned(),
            Value::String(AccessState::Ready.as_str().to_owned()),
        );
    } else {
        state.root.remove(ACCESS_MATERIAL);
        state.root.insert(
            ACCESS_STATE.to_owned(),
            Value::String(AccessState::Absent.as_str().to_owned()),
        );
    }
}

fn material_text(material: Option<&str>) -> Option<&str> {
    material.filter(|value| !value.is_empty())
}

fn files_object(state: &SyncState) -> Option<Map<String, Value>> {
    state.root.get("files").and_then(Value::as_object).cloned()
}

fn retained_is_extendable(state: &SyncState) -> bool {
    match state.root.get(RETAINED_CATALOGS) {
        None => true,
        Some(Value::Object(_)) => true,
        Some(_) => false,
    }
}

/// Park `files` under `retained_catalogs`. An existing key is left in place.
fn park_files(
    state: &mut SyncState,
    files: Map<String, Value>,
    base: &str,
    identity_key: bool,
) -> Result<(), FolderIdentityError> {
    if !retained_is_extendable(state) {
        return Err(FolderIdentityError::Unavailable);
    }
    let catalogs = state.root.get(RETAINED_CATALOGS).and_then(Value::as_object);
    let key =
        next_catalog_key(catalogs, base, identity_key).ok_or(FolderIdentityError::Unavailable)?;
    let catalogs = match state.root.get_mut(RETAINED_CATALOGS) {
        Some(Value::Object(catalogs)) => catalogs,
        _ => {
            state
                .root
                .insert(RETAINED_CATALOGS.to_owned(), Value::Object(Map::new()));
            state
                .root
                .get_mut(RETAINED_CATALOGS)
                .and_then(Value::as_object_mut)
                .expect("retained catalogs were just inserted")
        }
    };
    catalogs.insert(key, Value::Object(files));
    Ok(())
}

fn next_catalog_key(
    catalogs: Option<&Map<String, Value>>,
    base: &str,
    identity_key: bool,
) -> Option<String> {
    let occupied = |key: &str| catalogs.is_some_and(|map| map.contains_key(key));
    if !occupied(base) {
        return Some(base.to_owned());
    }
    for number in 2..10_000 {
        let key = if identity_key {
            format!("{base}~{number}")
        } else {
            format!("{base}-{number}")
        };
        if !occupied(&key) {
            return Some(key);
        }
    }
    None
}

fn valid_identity(state: &SyncState) -> Option<&str> {
    state
        .root
        .get(SOURCE_IDENTITY)
        .and_then(Value::as_str)
        .filter(|identity| {
            identity.len() == 32
                && identity
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
}

fn one_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !std::path::Path::new(name).is_absolute()
}

fn mint_source_identity() -> Result<String, FolderIdentityError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| FolderIdentityError::Unavailable)?;
    Ok(hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn redacted_root(root: &Map<String, Value>) -> Map<String, Value> {
    let mut redacted = root.clone();
    if redacted.contains_key(ACCESS_MATERIAL) {
        redacted.insert(
            ACCESS_MATERIAL.to_owned(),
            Value::String(REDACTED.to_owned()),
        );
    }
    redacted
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;

    use super::{valid_identity, *};

    const SECRET: &str = "SECRET-BOOKMARK-VALUE";

    fn obsidian_with_files() -> SyncState {
        let mut state = SyncState::empty(BackendName::Obsidian);
        state.files_mut().insert(
            "notes/a.md".to_owned(),
            serde_json::json!({"status": "imported", "content_hash": "abc"}),
        );
        state
            .root_mut()
            .insert("legacy".to_owned(), Value::from(1_u64));
        state
    }

    fn retained<'a>(state: &'a SyncState, key: &str) -> Option<&'a Value> {
        state
            .root()
            .get(RETAINED_CATALOGS)
            .and_then(Value::as_object)
            .and_then(|catalogs| catalogs.get(key))
    }

    #[cfg(unix)]
    #[test]
    fn bound_state_refuses_a_replacement_root_without_reading_or_mutating_its_twin() {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("journal");
        let displaced = parent.path().join("original");
        fs::create_dir(&path).unwrap();
        let root = solstone_core_journal_io::JournalRoot::open(&path).unwrap();
        let state = obsidian_with_files();
        write_sync_state_bound(&root, &state, &crate::WorkStop::new()).unwrap();
        let original = fs::read(state_path(&path, BackendName::Obsidian)).unwrap();
        fs::rename(&path, &displaced).unwrap();
        fs::create_dir_all(path.join("imports")).unwrap();
        let twin = state_path(&path, BackendName::Obsidian);
        fs::write(&twin, b"replacement state sentinel").unwrap();
        assert!(matches!(
            read_sync_state_bound(&root, BackendName::Obsidian),
            SyncStateRead::Unreadable {
                class: SyncStateReadClass::Io
            }
        ));
        assert!(write_sync_state_bound(&root, &state, &crate::WorkStop::new()).is_err());
        assert_eq!(fs::read(&twin).unwrap(), b"replacement state sentinel");
        assert_eq!(
            fs::read(state_path(&displaced, BackendName::Obsidian)).unwrap(),
            original
        );
    }

    #[cfg(unix)]
    #[test]
    fn bound_state_refuses_a_connection_leaf_alias() {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("journal");
        fs::create_dir_all(path.join("imports")).unwrap();
        let outside = parent.path().join("outside.json");
        fs::write(
            &outside,
            br#"{"files":{},"access_material":"outside-grant"}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, state_path(&path, BackendName::Obsidian)).unwrap();
        let root = solstone_core_journal_io::JournalRoot::open(&path).unwrap();
        assert!(matches!(
            read_sync_state_bound(&root, BackendName::Obsidian),
            SyncStateRead::Unreadable {
                class: SyncStateReadClass::Io
            }
        ));
        assert_eq!(
            fs::read(&outside).unwrap(),
            br#"{"files":{},"access_material":"outside-grant"}"#
        );
    }

    #[test]
    fn begin_parks_unscoped_files_and_keeps_a_sibling_file() {
        let root = tempfile::tempdir().unwrap();
        let sibling = root.path().join("sibling.txt");
        fs::write(&sibling, b"keep").unwrap();
        let mut state = obsidian_with_files();
        let original = state.files_mut().get("notes/a.md").cloned().unwrap();
        let identity = begin_folder(&mut state, "Notes", false, Some(SECRET)).unwrap();
        assert_eq!(identity.len(), 32);
        assert_eq!(state.files_mut().get("notes/a.md"), Some(&original));
        assert!(state.root().get(RETAINED_CATALOGS).is_none());
        assert!(state.root().get(ACCESS_MATERIAL).is_none());
        assert_eq!(
            state.root().get(ACCESS_STATE).and_then(Value::as_str),
            Some("absent")
        );
        assert_eq!(state.root().get("legacy"), Some(&Value::from(1_u64)));
        assert_eq!(fs::read(&sibling).unwrap(), b"keep");
        let again = begin_folder(&mut state, "Notes", false, None).unwrap_err();
        assert_eq!(again, FolderIdentityError::IdentityPresent);
        assert_eq!(state.files_mut().get("notes/a.md"), Some(&original));
        assert!(state.root().get(RETAINED_CATALOGS).is_none());
    }

    #[test]
    fn reauthorize_keeps_identity_and_replace_parks_the_catalogue() {
        let root = tempfile::tempdir().unwrap();
        let admitted = root.path().join("imports").join("admitted-note.txt");
        fs::create_dir_all(admitted.parent().unwrap()).unwrap();
        fs::write(&admitted, b"admitted").unwrap();
        let mut state = obsidian_with_files();
        let identity = begin_folder(&mut state, "Notes", true, Some(SECRET)).unwrap();
        let active = serde_json::json!({"status": "imported"});
        state
            .files_mut()
            .insert("notes/a.md".to_owned(), active.clone());
        reauthorize(&mut state, "Notes", false, Some(SECRET)).unwrap();
        assert_eq!(valid_identity(&state), Some(identity.as_str()));
        assert!(state.root().get(ACCESS_MATERIAL).is_none());
        assert_eq!(state.files_mut().len(), 1);
        reauthorize(&mut state, "Renamed", true, Some("next-grant")).unwrap();
        assert_eq!(valid_identity(&state), Some(identity.as_str()));
        assert_eq!(state.files_mut().len(), 1);
        assert_eq!(
            state.root().get(ACCESS_MATERIAL).and_then(Value::as_str),
            Some("next-grant")
        );
        let replaced = replace_folder(&mut state, "Other", true, Some(SECRET)).unwrap();
        assert_ne!(replaced, identity);
        assert!(state.files_mut().is_empty());
        assert_eq!(
            retained(&state, &identity)
                .and_then(Value::as_object)
                .and_then(|files| files.get("notes/a.md")),
            Some(&active)
        );
        assert_eq!(fs::read(&admitted).unwrap(), b"admitted");
    }

    #[test]
    fn second_park_does_not_overwrite_the_first_retained_object() {
        let mut state = obsidian_with_files();
        let original = state.files_mut().get("notes/a.md").cloned().unwrap();
        begin_folder(&mut state, "Notes", false, None).unwrap();
        assert!(state.root().get(RETAINED_CATALOGS).is_none());
        assert_eq!(state.files_mut().get("notes/a.md"), Some(&original));
        state.root_mut().remove(SOURCE_IDENTITY);
        state.files_mut().insert(
            "other.md".to_owned(),
            serde_json::json!({"status": "imported", "content_hash": "zzz"}),
        );
        begin_folder(&mut state, "Notes", false, None).unwrap();
        assert!(state.root().get(RETAINED_CATALOGS).is_none());
        assert_eq!(state.files_mut().get("notes/a.md"), Some(&original));
        assert_eq!(
            state
                .files_mut()
                .get("other.md")
                .and_then(|row| row.get("content_hash"))
                .and_then(Value::as_str),
            Some("zzz")
        );

        let identity = valid_identity(&state).unwrap().to_owned();
        let active = serde_json::json!({"status": "imported", "content_hash": "live"});
        state
            .files_mut()
            .insert("notes/a.md".to_owned(), active.clone());
        let sentinel = serde_json::json!({"sentinel": true});
        state
            .root_mut()
            .insert(RETAINED_CATALOGS.to_owned(), serde_json::json!({}));
        state
            .root_mut()
            .get_mut(RETAINED_CATALOGS)
            .and_then(Value::as_object_mut)
            .unwrap()
            .insert(identity.clone(), sentinel.clone());
        let replaced = replace_folder(&mut state, "Other", false, None).unwrap();
        assert_ne!(replaced, identity);
        assert_eq!(retained(&state, &identity), Some(&sentinel));
        assert_eq!(
            retained(&state, &format!("{identity}~2"))
                .and_then(Value::as_object)
                .and_then(|files| files.get("notes/a.md")),
            Some(&active)
        );
        assert!(state.files_mut().is_empty());
        assert!(retained(&state, "unscoped").is_none());
    }

    #[test]
    fn plaud_and_oura_refuse_without_mutation_and_bad_grants_do_not_stick() {
        for backend in [BackendName::Plaud, BackendName::Oura] {
            let mut state = SyncState::empty(backend);
            state
                .root_mut()
                .insert("custom".to_owned(), Value::String("stay".to_owned()));
            let before = state.clone();
            assert_eq!(
                begin_folder(&mut state, "Notes", true, Some(SECRET)).unwrap_err(),
                FolderIdentityError::BackendRefused
            );
            assert_eq!(state, before);
        }
        let mut state = SyncState::empty(BackendName::Audio);
        let before = state.clone();
        let bad_name = begin_folder(&mut state, "a/b", true, Some(SECRET)).unwrap_err();
        assert_eq!(bad_name, FolderIdentityError::BadFolderName);
        assert!(!bad_name.to_string().contains(SECRET));
        assert_eq!(
            begin_folder(&mut state, "Notes", true, Some("")).unwrap_err(),
            FolderIdentityError::MaterialRequired
        );
        assert_eq!(state, before);
        state.root_mut().insert(
            SOURCE_IDENTITY.to_owned(),
            Value::String("not-hex".to_owned()),
        );
        let present = state.clone();
        assert_eq!(
            begin_folder(&mut state, "Notes", false, None).unwrap_err(),
            FolderIdentityError::IdentityPresent
        );
        assert_eq!(state, present);
    }

    #[test]
    fn public_connection_hides_material_and_refresh_keeps_files() {
        let mut state = obsidian_with_files();
        let identity = begin_folder(&mut state, "Notes", true, Some(SECRET)).unwrap();
        state.files_mut().insert(
            "notes/a.md".to_owned(),
            serde_json::json!({"status": "imported"}),
        );
        let public = public_connection(&state).unwrap();
        let rendered = format!("{public:?} {public} {state:?}");
        assert!(!rendered.contains(SECRET));
        assert!(rendered.contains("[redacted]"));
        assert_eq!(public.source_identity, identity);
        assert!(public.remembered);
        assert_eq!(public.access_state, AccessState::Ready);
        let before = state.clone();
        assert_eq!(
            permit_remembered_refresh(&state, "", "page").unwrap_err(),
            RefreshRefusal::StalePage
        );
        assert_eq!(
            permit_remembered_refresh(&state, "page", "other").unwrap_err(),
            RefreshRefusal::StalePage
        );
        let permit = permit_remembered_refresh(&state, "page", "page").unwrap();
        assert_eq!(permit.source_identity, identity);
        assert_eq!(permit.folder_name, "Notes");
        assert!(!format!("{permit:?}").contains(SECRET));
        let runtime = permit_runtime_refresh(&state).unwrap();
        assert_eq!(runtime.source_identity, identity);
        assert_eq!(runtime.folder_name, "Notes");
        assert!(!format!("{runtime:?}").contains(SECRET));
        assert_eq!(state, before);

        reauthorize(&mut state, "Notes", false, None).unwrap();
        assert_eq!(
            permit_remembered_refresh(&state, "page", "page").unwrap_err(),
            RefreshRefusal::NotRemembered
        );
        assert_eq!(
            permit_runtime_refresh(&state).unwrap_err(),
            RefreshRefusal::NotRemembered
        );
        reauthorize(&mut state, "Notes", true, Some(SECRET)).unwrap();
        mark_access_lost(&mut state).unwrap();
        assert_eq!(state.files_mut().len(), 1);
        assert_eq!(valid_identity(&state), Some(identity.as_str()));
        assert!(state.root().get(ACCESS_MATERIAL).is_none());
        assert_eq!(
            state.root().get(REMEMBERED).and_then(Value::as_bool),
            Some(true)
        );
        let lost = permit_remembered_refresh(&state, "page", "page").unwrap_err();
        assert_eq!(lost, RefreshRefusal::AccessLost);
        assert_eq!(permit_runtime_refresh(&state).unwrap_err(), lost);
        assert!(!lost.to_string().contains(SECRET));
        assert_eq!(state.files_mut().len(), 1);
    }
}
