// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable, journal-local MCP bearer-token verification.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ring::rand::{SecureRandom as _, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, PathError,
    create_directory_with_mode, hold_lock, write_json,
};
use subtle::ConstantTimeEq;

const ENDPOINT_DIRECTORY: &str = "mcp-endpoint";
const TOKENS_FILE: &str = "tokens.json";
const SCHEMA: u32 = 1;
const TOKEN_BYTES: usize = 32;

/// A journal-root-bound bearer-token verifier store.
///
/// The store never caches file contents. Every query reloads the durable
/// ledger, so a successful revoke is honored by the next verification call.
pub struct TokenStore {
    root: PathBuf,
}

/// A newly generated bearer token. The plaintext token is returned exactly
/// once from [`TokenStore::create`] and is never persisted.
pub struct CreatedToken {
    pub label: String,
    pub token: String,
}

/// Non-secret metadata suitable for listing managed bearer tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSummary {
    pub label: String,
    pub created_at: DateTime<Utc>,
}

/// The current identity resolved for a verified bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedToken {
    pub id: String,
    pub agent_identity: String,
}

/// Failure while operating the MCP bearer-token store.
#[derive(Debug)]
pub enum TokenStoreError {
    InvalidLabel(TokenLabelError),
    DuplicateLabel { label: String },
    NotFound { label: String },
    Randomness,
    InvalidToken,
    Directory(PathError),
    Lock(LockError),
    Read { path: PathBuf, source: io::Error },
    Malformed { path: PathBuf },
    UnsupportedSchema { path: PathBuf, found: u32 },
    Write(AtomicWriteError),
}

impl fmt::Display for TokenStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLabel(error) => {
                format_message(formatter, format_args!("invalid token label: {error}"))
            }
            Self::DuplicateLabel { label } => format_message(
                formatter,
                format_args!("a bearer token already exists for label {label:?}"),
            ),
            Self::NotFound { label } => format_message(
                formatter,
                format_args!("no bearer token exists for label {label:?}"),
            ),
            Self::Randomness => {
                formatter.write_str("could not obtain complete bearer-token randomness")
            }
            Self::InvalidToken => formatter.write_str("bearer token is invalid"),
            Self::Directory(error) => format_message(
                formatter,
                format_args!("could not prepare MCP token directory: {error}"),
            ),
            Self::Lock(error) => format_message(
                formatter,
                format_args!("could not lock MCP token store: {error}"),
            ),
            Self::Read { path, source } => format_message(
                formatter,
                format_args!("could not read {}: {source}", path.display()),
            ),
            Self::Malformed { path } => format_message(
                formatter,
                format_args!("MCP token store is malformed: {}", path.display()),
            ),
            Self::UnsupportedSchema { path, found } => format_message(
                formatter,
                format_args!(
                    "MCP token store has unsupported schema {found}: {}",
                    path.display()
                ),
            ),
            Self::Write(error) => format_message(
                formatter,
                format_args!("could not write MCP token store: {error}"),
            ),
        }
    }
}

fn format_message(formatter: &mut fmt::Formatter<'_>, message: fmt::Arguments<'_>) -> fmt::Result {
    fmt::Display::fmt(&message, formatter)
}

impl Error for TokenStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidLabel(error) => Some(error),
            Self::Directory(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Read { source, .. } => Some(source),
            Self::Write(error) => Some(error),
            Self::DuplicateLabel { .. }
            | Self::NotFound { .. }
            | Self::Randomness
            | Self::InvalidToken
            | Self::Malformed { .. }
            | Self::UnsupportedSchema { .. } => None,
        }
    }
}

/// Why a supplied token label cannot name an MCP agent identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenLabelError {
    AsciiControl,
    DisallowedByte,
    Empty,
    TooLong,
    Boundary,
}

impl fmt::Display for TokenLabelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AsciiControl => formatter.write_str("ASCII control bytes are not allowed"),
            Self::DisallowedByte => formatter
                .write_str("only ASCII letters, digits, spaces, '.', '_', and '-' are allowed"),
            Self::Empty => formatter.write_str("label must not be empty"),
            Self::TooLong => formatter.write_str("label must be at most 64 bytes"),
            Self::Boundary => {
                formatter.write_str("label must start and end with an alphanumeric character")
            }
        }
    }
}

impl Error for TokenLabelError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenStoreFile {
    schema: u32,
    tokens: Vec<StoredToken>,
}

impl Default for TokenStoreFile {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            tokens: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredToken {
    id: String,
    label: String,
    verifier: String,
    created_at: DateTime<Utc>,
}

/// A crate-private source of complete random byte sequences.
///
/// Returning a count makes an incomplete deterministic test source observable
/// rather than allowing a partially initialized token namespace to proceed.
pub(crate) trait RandomSource {
    fn fill(&self, bytes: &mut [u8]) -> Result<usize, RandomSourceError>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RandomSourceError;

pub(crate) struct SystemRandomSource;

impl RandomSource for SystemRandomSource {
    fn fill(&self, bytes: &mut [u8]) -> Result<usize, RandomSourceError> {
        SystemRandom::new()
            .fill(bytes)
            .map_err(|_| RandomSourceError)?;
        Ok(bytes.len())
    }
}

impl TokenStore {
    /// Bind a token store to one journal root.
    #[must_use]
    pub fn open(journal_root: &Path) -> Self {
        Self {
            root: journal_root.to_path_buf(),
        }
    }

    /// Generate, durably record, and return one bearer token exactly once.
    pub fn create(&self, label: &str) -> Result<CreatedToken, TokenStoreError> {
        self.create_with_random(label, &SystemRandomSource)
    }

    pub(crate) fn create_with_random(
        &self,
        label: &str,
        random: &dyn RandomSource,
    ) -> Result<CreatedToken, TokenStoreError> {
        let label = normalize_label(label).map_err(TokenStoreError::InvalidLabel)?;
        self.ensure_directory()?;
        let path = self.tokens_path();
        let _lock = hold_lock(
            &path,
            LockOptions {
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(TokenStoreError::Lock)?;
        let mut store = self.read_store()?;
        if store
            .tokens
            .iter()
            .any(|entry| entry.label.eq_ignore_ascii_case(&label))
        {
            return Err(TokenStoreError::DuplicateLabel { label });
        }

        let token_bytes = complete_random_bytes(random)?;
        let id_bytes = complete_random_bytes(random)?;
        let token = URL_SAFE_NO_PAD.encode(token_bytes);
        let verifier = URL_SAFE_NO_PAD.encode(Sha256::digest(token_bytes));
        store.tokens.push(StoredToken {
            id: URL_SAFE_NO_PAD.encode(id_bytes),
            label: label.clone(),
            verifier,
            created_at: Utc::now(),
        });
        self.write_store(&path, &store)?;
        Ok(CreatedToken { label, token })
    }

    /// List only non-secret bearer-token metadata from the current store.
    pub fn list(&self) -> Result<Vec<TokenSummary>, TokenStoreError> {
        Ok(self
            .read_store()?
            .tokens
            .into_iter()
            .map(|entry| TokenSummary {
                label: entry.label,
                created_at: entry.created_at,
            })
            .collect())
    }

    /// Remove the named token after rereading the locked durable store.
    pub fn revoke(&self, label: &str) -> Result<(), TokenStoreError> {
        let label = normalize_label(label).map_err(TokenStoreError::InvalidLabel)?;
        self.ensure_directory()?;
        let path = self.tokens_path();
        let _lock = hold_lock(
            &path,
            LockOptions {
                mode: Some(0o600),
                ..LockOptions::default()
            },
        )
        .map_err(TokenStoreError::Lock)?;
        let mut store = self.read_store()?;
        let Some(index) = store
            .tokens
            .iter()
            .position(|entry| entry.label.eq_ignore_ascii_case(&label))
        else {
            return Err(TokenStoreError::NotFound { label });
        };
        store.tokens.remove(index);
        self.write_store(&path, &store)
    }

    /// Verify a presented bearer token against freshly loaded durable state.
    pub fn verify(&self, presented_token: &str) -> Result<VerifiedToken, TokenStoreError> {
        let store = self.read_store()?;
        let raw = decode_token(presented_token).ok_or(TokenStoreError::InvalidToken)?;
        let digest: [u8; TOKEN_BYTES] = Sha256::digest(raw).into();
        let mut verified = None;
        for entry in store.tokens {
            let verifier =
                decode_token(&entry.verifier).ok_or_else(|| TokenStoreError::Malformed {
                    path: self.tokens_path(),
                })?;
            if bool::from(digest.ct_eq(&verifier)) && verified.is_none() {
                verified = Some(VerifiedToken {
                    id: entry.id,
                    agent_identity: entry.label,
                });
            }
        }
        verified.ok_or(TokenStoreError::InvalidToken)
    }

    fn endpoint_directory(&self) -> PathBuf {
        self.root.join(ENDPOINT_DIRECTORY)
    }

    fn tokens_path(&self) -> PathBuf {
        self.endpoint_directory().join(TOKENS_FILE)
    }

    fn ensure_directory(&self) -> Result<(), TokenStoreError> {
        create_directory_with_mode(&self.endpoint_directory(), 0o700)
            .map_err(TokenStoreError::Directory)
    }

    fn read_store(&self) -> Result<TokenStoreFile, TokenStoreError> {
        let path = self.tokens_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(TokenStoreFile::default());
            }
            Err(source) => return Err(TokenStoreError::Read { path, source }),
        };
        let store = serde_json::from_slice::<TokenStoreFile>(&bytes)
            .map_err(|_| TokenStoreError::Malformed { path: path.clone() })?;
        if store.schema != SCHEMA {
            return Err(TokenStoreError::UnsupportedSchema {
                path,
                found: store.schema,
            });
        }
        validate_store(&store, &path)?;
        Ok(store)
    }

    fn write_store(&self, path: &Path, store: &TokenStoreFile) -> Result<(), TokenStoreError> {
        write_json(
            path,
            store,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(TokenStoreError::Write)
    }
}

fn complete_random_bytes(random: &dyn RandomSource) -> Result<[u8; TOKEN_BYTES], TokenStoreError> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    let written = random
        .fill(&mut bytes)
        .map_err(|_| TokenStoreError::Randomness)?;
    if written != bytes.len() {
        return Err(TokenStoreError::Randomness);
    }
    Ok(bytes)
}

fn decode_token(value: &str) -> Option<[u8; TOKEN_BYTES]> {
    let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
    bytes.try_into().ok()
}

fn validate_store(store: &TokenStoreFile, path: &Path) -> Result<(), TokenStoreError> {
    let mut labels = HashSet::new();
    let mut ids = HashSet::new();
    let mut verifiers = HashSet::new();
    for entry in &store.tokens {
        if normalize_label(&entry.label).ok().as_deref() != Some(entry.label.as_str())
            || !labels.insert(entry.label.to_ascii_lowercase())
            || decode_token(&entry.id).is_none()
            || !ids.insert(entry.id.as_str())
            || decode_token(&entry.verifier).is_none()
            || !verifiers.insert(entry.verifier.as_str())
        {
            return Err(TokenStoreError::Malformed {
                path: path.to_path_buf(),
            });
        }
    }
    Ok(())
}

fn normalize_label(raw: &str) -> Result<String, TokenLabelError> {
    if raw.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(TokenLabelError::AsciiControl);
    }
    if raw
        .bytes()
        .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b' ' | b'.' | b'_' | b'-'))
    {
        return Err(TokenLabelError::DisallowedByte);
    }

    let mut normalized = String::with_capacity(raw.len());
    let mut previous_space = false;
    for byte in raw.trim_matches(' ').bytes() {
        if byte == b' ' {
            if !previous_space {
                normalized.push(' ');
            }
            previous_space = true;
        } else {
            normalized.push(char::from(byte));
            previous_space = false;
        }
    }

    if normalized.is_empty() {
        return Err(TokenLabelError::Empty);
    }
    if normalized.len() > 64 {
        return Err(TokenLabelError::TooLong);
    }
    let first = normalized.as_bytes()[0];
    let last = normalized.as_bytes()[normalized.len() - 1];
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        return Err(TokenLabelError::Boundary);
    }
    if normalized.is_empty() {
        return Err(TokenLabelError::Empty);
    }
    Ok(normalized)
}

#[cfg(all(test, not(feature = "full-tests")))]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::{
        RandomSource, RandomSourceError, TokenLabelError, TokenStore, TokenStoreError,
        normalize_label,
    };

    fn journal_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-mcp-tokens-")
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    #[test]
    fn label_normalization_enforces_the_closed_identity_grammar() {
        assert!(matches!(
            normalize_label("A\tB"),
            Err(TokenLabelError::AsciiControl)
        ));
        assert!(matches!(
            normalize_label("A\nB"),
            Err(TokenLabelError::AsciiControl)
        ));
        assert!(matches!(
            normalize_label("é"),
            Err(TokenLabelError::DisallowedByte)
        ));
        assert!(matches!(
            normalize_label("hello!"),
            Err(TokenLabelError::DisallowedByte)
        ));
        assert!(matches!(normalize_label(""), Err(TokenLabelError::Empty)));
        assert!(matches!(
            normalize_label("   "),
            Err(TokenLabelError::Empty)
        ));
        assert!(matches!(
            normalize_label(&format!("a{}a", "b".repeat(63))),
            Err(TokenLabelError::TooLong)
        ));
        assert!(matches!(
            normalize_label(".hello"),
            Err(TokenLabelError::Boundary)
        ));
        assert!(matches!(
            normalize_label("hello-"),
            Err(TokenLabelError::Boundary)
        ));
        assert_eq!(normalize_label("  hello   world  ").unwrap(), "hello world");
    }

    #[test]
    fn create_verify_and_revoke_reload_the_durable_store() {
        let journal = journal_root();
        let store = TokenStore::open(journal.path());
        let created = store.create("Ops-Agent").unwrap();

        assert_eq!(
            store.verify(&created.token).unwrap().agent_identity,
            "Ops-Agent"
        );
        assert!(matches!(
            store.verify("not-a-bearer-token"),
            Err(TokenStoreError::InvalidToken)
        ));
        assert!(matches!(
            store.create("ops-agent"),
            Err(TokenStoreError::DuplicateLabel { .. })
        ));
        store.revoke("ops-agent").unwrap();
        assert!(matches!(
            store.verify(&created.token),
            Err(TokenStoreError::InvalidToken)
        ));
    }

    struct ShortRandom;

    impl RandomSource for ShortRandom {
        fn fill(&self, bytes: &mut [u8]) -> Result<usize, RandomSourceError> {
            bytes[..31].fill(0x5a);
            Ok(31)
        }
    }

    #[test]
    fn incomplete_randomness_cannot_create_a_token_or_verifier() {
        let journal = journal_root();
        let store = TokenStore::open(journal.path());

        assert!(matches!(
            store.create_with_random("operator", &ShortRandom),
            Err(TokenStoreError::Randomness)
        ));
        assert!(store.list().unwrap().is_empty());
        assert!(!journal.path().join("mcp-endpoint/tokens.json").exists());
    }

    #[test]
    fn corrupt_store_fails_closed_without_overwriting_its_bytes() {
        let journal = journal_root();
        let store = TokenStore::open(journal.path());
        let directory = journal.path().join("mcp-endpoint");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("tokens.json");
        fs::write(&path, b"not json").unwrap();

        assert!(store.create("operator").is_err());
        assert!(store.list().is_err());
        assert!(store.revoke("operator").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not json");
    }

    #[test]
    fn independent_handles_serialize_create_and_revoke_mutations() {
        let journal = journal_root();
        let root = journal.path().to_path_buf();
        let create_barrier = Arc::new(Barrier::new(2));
        let left_barrier = Arc::clone(&create_barrier);
        let left_root = root.clone();
        let left = thread::spawn(move || {
            let store = TokenStore::open(&left_root);
            left_barrier.wait();
            store.create("alpha")
        });
        let right_barrier = Arc::clone(&create_barrier);
        let right_root = root.clone();
        let right = thread::spawn(move || {
            let store = TokenStore::open(&right_root);
            right_barrier.wait();
            store.create("beta")
        });
        left.join().unwrap().unwrap();
        right.join().unwrap().unwrap();

        let store = TokenStore::open(&root);
        let mut labels = store
            .list()
            .unwrap()
            .into_iter()
            .map(|token| token.label)
            .collect::<Vec<_>>();
        labels.sort();
        assert_eq!(labels, ["alpha", "beta"]);

        let original = store.create("revoked").unwrap();
        let mutation_barrier = Arc::new(Barrier::new(2));
        let revoke_barrier = Arc::clone(&mutation_barrier);
        let revoke_root = root.clone();
        let revoke = thread::spawn(move || {
            let store = TokenStore::open(&revoke_root);
            revoke_barrier.wait();
            store.revoke("revoked")
        });
        let create_barrier = Arc::clone(&mutation_barrier);
        let create_root = root.clone();
        let create = thread::spawn(move || {
            let store = TokenStore::open(&create_root);
            create_barrier.wait();
            store.create("survivor")
        });
        revoke.join().unwrap().unwrap();
        create.join().unwrap().unwrap();

        assert!(matches!(
            store.verify(&original.token),
            Err(TokenStoreError::InvalidToken)
        ));
        let labels = store
            .list()
            .unwrap()
            .into_iter()
            .map(|token| token.label)
            .collect::<Vec<_>>();
        assert!(labels.contains(&"survivor".to_owned()));
    }

    #[cfg(unix)]
    #[test]
    fn token_store_directory_file_and_lock_are_owner_only() {
        let journal = journal_root();
        let store = TokenStore::open(journal.path());
        store.create("operator").unwrap();

        let directory = journal.path().join("mcp-endpoint");
        let file = directory.join("tokens.json");
        let lock = directory.join("tokens.json.lock");
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(lock).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
