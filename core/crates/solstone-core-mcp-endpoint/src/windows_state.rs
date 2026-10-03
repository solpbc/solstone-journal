// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows counterpart of the endpoint's owner state layer.
//!
//! A Windows journal relies on the ordinary access controls inherited from the
//! location its owner chose, like the rest of the journal's state on Windows.
//! This layer supplies no custom security descriptor and makes no
//! confidentiality claim against another account on the same computer. It
//! keeps the platform-independent contract: every read is bounded and fails
//! closed, one bootstrap runs at a time under the endpoint's creation lock,
//! the proof-of-possession key is published create-only and read back before
//! use, and certificate state is replaced only by a publication that is
//! durable and certain.
//!
//! The owner-hostname door keeps its certificate account and per-generation
//! certificate state here too, under `<journal>/mcp-endpoint/byo/`, with the
//! same bounds and fail-closed reads. The loopback and LAN doors keep no state
//! behind this layer.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;
use solstone_core_journal_config::{
    JournalConfigRead, McpEndpointCapability, mcp_endpoint_capability,
    mcp_endpoint_certificate_environment, mcp_endpoint_force_staging_renewal, read_journal_config,
};
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_journal_io::{
    AtomicWriteOptions, DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, DetailedAtomicOutcome,
    LockOptions, atomic_replace_detailed, errors::AtomicWriteError, hold_lock,
    write_bytes_exclusive,
};
use solstone_core_sol_link::committed::load_committed_identity;

use crate::{McpEndpointBootstrapError, McpEndpointOwnerContext};

const ENDPOINT_DIRECTORY: &str = "mcp-endpoint";
const TLS_DIRECTORY: &str = "tls";
const TLS_STATE_FILE: &str = "state.json";
const TLS_STAGING_ACCOUNT_FILE: &str = "account-staging.pk8";
const TLS_PRODUCTION_ACCOUNT_FILE: &str = "account-production.pk8";
/// `hold_lock` appends `.lock`, giving the same `.create.lock` name as Unix.
const CREATE_LOCK_STEM: &str = ".create";
const POP_KEY: &str = "pop.ed25519.pk8";
const BYO_DIRECTORY: &str = "byo";
const BYO_ACCOUNTS_DIR: &str = "accounts";
const BYO_CERTS_DIR: &str = "certs";
const BYO_ACCOUNT_KEY_FILE: &str = "account.pk8";
const BYO_ACCOUNT_URI_FILE: &str = "account.uri";
const MAX_BYO_ACCOUNT_URI_BYTES: usize = 2048;
const MAX_POP_PKCS8_DER_BYTES: usize = 512;
pub(crate) const MAX_TLS_STATE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_TLS_ACME_ACCOUNT_BYTES: usize = 1024;
/// Inert on Windows beyond validation; kept equal to the Unix file mode.
const FILE_MODE: u32 = 0o600;

/// The endpoint's TLS state directory, `<journal>/mcp-endpoint/tls/`.
///
/// Crate-private: callers can neither supply another directory nor name a file
/// inside it.
pub(crate) struct TlsStateDirectory {
    path: PathBuf,
}

pub(super) fn bootstrap(
    journal_root: &Path,
) -> Result<Option<McpEndpointOwnerContext>, McpEndpointBootstrapError> {
    let config =
        read_journal_config(journal_root).map_err(|_| McpEndpointBootstrapError::ConfigRead)?;
    match mcp_endpoint_capability(&config).map_err(|_| McpEndpointBootstrapError::Capability)? {
        McpEndpointCapability::Disabled => Ok(None),
        McpEndpointCapability::Enabled => bootstrap_enabled(journal_root, &config).map(Some),
    }
}

fn bootstrap_enabled(
    journal_root: &Path,
    config: &JournalConfigRead,
) -> Result<McpEndpointOwnerContext, McpEndpointBootstrapError> {
    let certificate_environment = mcp_endpoint_certificate_environment(config)
        .map_err(|_| McpEndpointBootstrapError::Capability)?;
    let force_staging_renewal = mcp_endpoint_force_staging_renewal(config)
        .map_err(|_| McpEndpointBootstrapError::Capability)?;
    let root = JournalRoot::open(journal_root).map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    let committed = load_committed_identity(root.canonical_path())
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    let keypair =
        load_or_create_proof_key(&root).map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    root.revalidate()
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    Ok(McpEndpointOwnerContext {
        _private: (),
        committed: Arc::new(committed),
        keypair: Arc::new(keypair),
        journal_root: Arc::new(root),
        certificate_environment,
        force_staging_renewal,
        acme_account_uri: Arc::new(Mutex::new(None)),
        acme_account_setup: Arc::new(tokio::sync::Mutex::new(())),
    })
}

/// Load the journal's proof-of-possession key, creating it on first use.
///
/// Runs under the endpoint's creation lock, so concurrent bootstraps agree on
/// one key. A key that is oversized, not a regular file or not a valid Ed25519
/// PKCS#8 document is refused, never replaced.
fn load_or_create_proof_key(root: &JournalRoot) -> io::Result<Ed25519KeyPair> {
    let endpoint = open_real_directory(&root.canonical_path().join(ENDPOINT_DIRECTORY))?;
    let _lock = hold_lock(
        endpoint.join(CREATE_LOCK_STEM),
        LockOptions {
            timeout: DEFAULT_LOCK_TIMEOUT,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: Some(FILE_MODE),
        },
    )
    .map_err(|_| io::Error::other("endpoint creation lock is unavailable"))?;
    let path = endpoint.join(POP_KEY);
    if let Some(bytes) = read_bounded(&path, MAX_POP_PKCS8_DER_BYTES)? {
        return decode_proof_key(&bytes);
    }
    let generated =
        Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| invalid_entry())?;
    match write_bytes_exclusive(
        &path,
        generated.as_ref(),
        AtomicWriteOptions {
            mode: Some(FILE_MODE),
        },
    ) {
        Ok(()) => {}
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists =>
        {
            // Another writer published under our lock: never adopt it.
            return Err(identity_changed());
        }
        Err(_) => return Err(invalid_entry()),
    }
    let published = read_bounded(&path, MAX_POP_PKCS8_DER_BYTES)?.ok_or_else(identity_changed)?;
    if published != generated.as_ref() {
        return Err(identity_changed());
    }
    decode_proof_key(&published)
}

fn decode_proof_key(bytes: &[u8]) -> io::Result<Ed25519KeyPair> {
    Ed25519KeyPair::from_pkcs8(bytes).map_err(|_| invalid_entry())
}

/// Open the fixed `<journal>/mcp-endpoint/tls/` directory, creating it only
/// after the endpoint capability has admitted the journal.
pub(crate) fn open_tls_state_directory(root: &JournalRoot) -> io::Result<TlsStateDirectory> {
    root.revalidate()
        .map_err(|_| io::Error::other("journal root changed"))?;
    let endpoint = open_real_directory(&root.canonical_path().join(ENDPOINT_DIRECTORY))?;
    let path = open_real_directory(&endpoint.join(TLS_DIRECTORY))?;
    Ok(TlsStateDirectory { path })
}

/// Read the TLS state file. Missing is `None`; anything other than a regular
/// file within the size limit is an error and yields no bytes.
pub(crate) fn read_tls_state_bytes(directory: &TlsStateDirectory) -> io::Result<Option<Vec<u8>>> {
    read_bounded(&directory.path.join(TLS_STATE_FILE), MAX_TLS_STATE_BYTES)
}

/// Durably replace the TLS state file. A publication with any durability or
/// final-name uncertainty is a failure.
#[allow(dead_code)] // Called by the same-crate certificate lifecycle owner.
pub(crate) fn persist_tls_state_bytes(
    directory: &TlsStateDirectory,
    bytes: &[u8],
) -> io::Result<()> {
    persist_bounded(directory, TLS_STATE_FILE, bytes, MAX_TLS_STATE_BYTES)
}

/// Load the bounded ACME account key for exactly one certificate environment.
pub(crate) fn read_tls_acme_account_bytes(
    directory: &TlsStateDirectory,
    production: bool,
) -> io::Result<Option<Vec<u8>>> {
    read_bounded(
        &directory.path.join(acme_account_file_name(production)),
        MAX_TLS_ACME_ACCOUNT_BYTES,
    )
}

pub(crate) fn persist_tls_acme_account_bytes(
    directory: &TlsStateDirectory,
    production: bool,
    bytes: &[u8],
) -> io::Result<()> {
    persist_bounded(
        directory,
        acme_account_file_name(production),
        bytes,
        MAX_TLS_ACME_ACCOUNT_BYTES,
    )
}

const fn acme_account_file_name(production: bool) -> &'static str {
    if production {
        TLS_PRODUCTION_ACCOUNT_FILE
    } else {
        TLS_STAGING_ACCOUNT_FILE
    }
}

/// The owner-hostname door's state directory, `<journal>/mcp-endpoint/byo/`.
pub(crate) struct ByoDirectory {
    path: PathBuf,
}

pub(crate) fn open_byo_directory(root: &JournalRoot) -> io::Result<ByoDirectory> {
    root.revalidate()
        .map_err(|_| io::Error::other("journal root changed"))?;
    let endpoint = open_real_directory(&root.canonical_path().join(ENDPOINT_DIRECTORY))?;
    let path = open_real_directory(&endpoint.join(BYO_DIRECTORY))?;
    root.revalidate()
        .map_err(|_| io::Error::other("journal root changed"))?;
    Ok(ByoDirectory { path })
}

/// `byo/accounts/<hostname>/`: the certificate account for one hostname.
pub(crate) fn open_byo_account_directory(
    byo_dir: &ByoDirectory,
    hostname: &str,
) -> io::Result<TlsStateDirectory> {
    require_real_directory(&byo_dir.path)?;
    let accounts = open_real_directory(&byo_dir.path.join(BYO_ACCOUNTS_DIR))?;
    let path = open_real_directory(&accounts.join(hostname_component(hostname)?))?;
    Ok(TlsStateDirectory { path })
}

/// `byo/certs/<hostname>/<generation>/`: certificate state for one hostname
/// generation.
pub(crate) fn open_byo_cert_directory(
    byo_dir: &ByoDirectory,
    hostname: &str,
    generation: u64,
) -> io::Result<TlsStateDirectory> {
    require_real_directory(&byo_dir.path)?;
    let certs = open_real_directory(&byo_dir.path.join(BYO_CERTS_DIR))?;
    let host = open_real_directory(&certs.join(hostname_component(hostname)?))?;
    let path = open_real_directory(&host.join(generation.to_string()))?;
    Ok(TlsStateDirectory { path })
}

pub(crate) fn read_byo_account_key(dir: &TlsStateDirectory) -> io::Result<Option<Vec<u8>>> {
    read_bounded(
        &dir.path.join(BYO_ACCOUNT_KEY_FILE),
        MAX_TLS_ACME_ACCOUNT_BYTES,
    )
}

pub(crate) fn persist_byo_account_key(dir: &TlsStateDirectory, bytes: &[u8]) -> io::Result<()> {
    persist_bounded(dir, BYO_ACCOUNT_KEY_FILE, bytes, MAX_TLS_ACME_ACCOUNT_BYTES)
}

pub(crate) fn read_byo_account_uri(dir: &TlsStateDirectory) -> io::Result<Option<String>> {
    match read_bounded(
        &dir.path.join(BYO_ACCOUNT_URI_FILE),
        MAX_BYO_ACCOUNT_URI_BYTES,
    )? {
        Some(bytes) => {
            let uri = String::from_utf8(bytes).map_err(|_| invalid_entry())?;
            Ok(Some(uri.trim().to_string()))
        }
        None => Ok(None),
    }
}

pub(crate) fn persist_byo_account_uri(dir: &TlsStateDirectory, uri: &str) -> io::Result<()> {
    persist_bounded(
        dir,
        BYO_ACCOUNT_URI_FILE,
        uri.as_bytes(),
        MAX_BYO_ACCOUNT_URI_BYTES,
    )
}

/// Remove the account key and its URI. Like Unix, a name that is already
/// gone is not an error.
pub(crate) fn delete_byo_account_pair(dir: &TlsStateDirectory) -> io::Result<()> {
    require_real_directory(&dir.path)?;
    for name in [BYO_ACCOUNT_KEY_FILE, BYO_ACCOUNT_URI_FILE] {
        let _ = fs::remove_file(dir.path.join(name));
    }
    Ok(())
}

/// A canonical hostname as one directory name. Hostnames reach here already
/// canonicalized; this refuses anything Windows would not keep as an ordinary
/// name in that directory: a character outside letters, digits, `-` and `.`,
/// a trailing dot, or a first label that names a reserved device.
fn hostname_component(hostname: &str) -> io::Result<&str> {
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let first = hostname.split('.').next().unwrap_or_default();
    if hostname.is_empty()
        || hostname.len() > 253
        || hostname.ends_with('.')
        || !hostname
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        || RESERVED.contains(&first)
    {
        return Err(invalid_entry());
    }
    Ok(hostname)
}

fn persist_bounded(
    directory: &TlsStateDirectory,
    name: &str,
    bytes: &[u8],
    max_bytes: usize,
) -> io::Result<()> {
    if bytes.len() > max_bytes {
        return Err(invalid_entry());
    }
    require_real_directory(&directory.path)?;
    match atomic_replace_detailed(&directory.path.join(name), bytes, FILE_MODE) {
        Ok(DetailedAtomicOutcome::Published) => Ok(()),
        Ok(_) => Err(io::Error::other(
            "TLS state publication was not durable and certain",
        )),
        Err(_) => Err(io::Error::other("TLS state publication failed")),
    }
}

/// Create `path` if missing, then require it to be a real directory rather
/// than a link or other reparse point.
fn open_real_directory(path: &Path) -> io::Result<PathBuf> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    require_real_directory(path)?;
    Ok(path.to_path_buf())
}

fn require_real_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_entry());
    }
    Ok(())
}

/// Read a regular file of at most `max_bytes`. A missing file is `None`.
fn read_bounded(path: &Path, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > max_bytes as u64
    {
        return Err(invalid_entry());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(invalid_entry());
    }
    Ok(Some(bytes))
}

fn invalid_entry() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "endpoint owner state entry is invalid",
    )
}

fn identity_changed() -> io::Error {
    io::Error::other("endpoint owner state changed during use")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal() -> (tempfile::TempDir, JournalRoot) {
        let directory = tempfile::Builder::new()
            .prefix("sme-owner-state")
            .tempdir_in(crate::test_scratch())
            .expect("journal tempdir");
        let root = JournalRoot::open(directory.path()).expect("journal root");
        (directory, root)
    }

    fn write_config(root: &Path, config: &[u8]) {
        fs::create_dir_all(root.join("config")).expect("config dir");
        fs::write(root.join("config/journal.json"), config).expect("config");
    }

    fn write_identity(root: &Path) {
        let ca = solstone_core_sol_link::ca::generate_ca().expect("test CA");
        let instance_id =
            solstone_core_sol_link::ca::jid_from_spki(ca.spki_der()).expect("test JID");
        let ca_directory = root.join("link/ca");
        fs::create_dir_all(&ca_directory).expect("CA directory");
        fs::write(ca_directory.join("cert.pem"), ca.certificate_pem()).expect("certificate");
        fs::write(ca_directory.join("private.pem"), ca.private_key_pem()).expect("private key");
        fs::write(
            root.join("link/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"Primary"}}"#),
        )
        .expect("state");
    }

    #[test]
    fn a_disabled_operated_endpoint_is_off_and_creates_nothing() {
        let (directory, _root) = journal();
        write_config(directory.path(), br#"{"mcp_endpoint":{"enabled":false}}"#);
        assert!(matches!(bootstrap(directory.path()), Ok(None)));
        assert!(!directory.path().join(ENDPOINT_DIRECTORY).exists());
    }

    #[test]
    fn an_enabled_operated_endpoint_needs_a_committed_identity() {
        let (directory, _root) = journal();
        write_config(directory.path(), br#"{"mcp_endpoint":{"enabled":true}}"#);
        assert!(matches!(
            bootstrap(directory.path()),
            Err(McpEndpointBootstrapError::Endpoint)
        ));
        assert!(!directory.path().join(ENDPOINT_DIRECTORY).exists());
    }

    #[test]
    fn an_enabled_operated_endpoint_keeps_one_proof_key_across_starts() {
        let (directory, _root) = journal();
        write_config(directory.path(), br#"{"mcp_endpoint":{"enabled":true}}"#);
        write_identity(directory.path());
        let first = bootstrap(directory.path())
            .expect("first start")
            .expect("enabled context");
        let second = bootstrap(directory.path())
            .expect("second start")
            .expect("enabled context");
        assert_eq!(
            ring::signature::KeyPair::public_key(first.keypair.as_ref()).as_ref(),
            ring::signature::KeyPair::public_key(second.keypair.as_ref()).as_ref()
        );
        assert!(
            directory
                .path()
                .join("mcp-endpoint/pop.ed25519.pk8")
                .is_file()
        );
    }

    #[test]
    fn the_proof_key_is_created_once_and_read_back_unchanged() {
        let (directory, root) = journal();
        let first = load_or_create_proof_key(&root).expect("first key");
        let stored = fs::read(directory.path().join("mcp-endpoint/pop.ed25519.pk8")).expect("key");
        let second = load_or_create_proof_key(&root).expect("second key");
        assert_eq!(
            ring::signature::KeyPair::public_key(&first).as_ref(),
            ring::signature::KeyPair::public_key(&second).as_ref()
        );
        assert_eq!(
            fs::read(directory.path().join("mcp-endpoint/pop.ed25519.pk8")).expect("key"),
            stored
        );
        assert!(directory.path().join("mcp-endpoint/.create.lock").is_file());
    }

    #[test]
    fn an_invalid_or_oversized_proof_key_is_refused_and_left_in_place() {
        let (directory, root) = journal();
        let path = directory.path().join("mcp-endpoint/pop.ed25519.pk8");
        fs::create_dir_all(path.parent().unwrap()).expect("endpoint dir");

        fs::write(&path, b"not a key").expect("invalid key");
        assert!(load_or_create_proof_key(&root).is_err());
        assert_eq!(fs::read(&path).expect("kept"), b"not a key");

        fs::write(&path, vec![0_u8; MAX_POP_PKCS8_DER_BYTES + 1]).expect("oversized key");
        assert!(load_or_create_proof_key(&root).is_err());
        assert_eq!(
            fs::read(&path).expect("kept").len(),
            MAX_POP_PKCS8_DER_BYTES + 1
        );

        fs::remove_file(&path).expect("remove");
        fs::create_dir(&path).expect("directory in its place");
        assert!(load_or_create_proof_key(&root).is_err());
    }

    #[test]
    fn tls_state_round_trips_and_is_bounded() {
        let (directory, root) = journal();
        let state = open_tls_state_directory(&root).expect("tls dir");
        assert!(directory.path().join("mcp-endpoint/tls").is_dir());
        assert_eq!(read_tls_state_bytes(&state).expect("missing"), None);

        persist_tls_state_bytes(&state, b"{\"v\":1}").expect("first");
        persist_tls_state_bytes(&state, b"{\"v\":2}").expect("replace");
        assert_eq!(
            read_tls_state_bytes(&state).expect("read").as_deref(),
            Some(&b"{\"v\":2}"[..])
        );

        let too_large = vec![b'x'; MAX_TLS_STATE_BYTES + 1];
        assert!(persist_tls_state_bytes(&state, &too_large).is_err());
        assert_eq!(
            read_tls_state_bytes(&state).expect("unchanged").as_deref(),
            Some(&b"{\"v\":2}"[..])
        );

        fs::write(
            directory.path().join("mcp-endpoint/tls/state.json"),
            &too_large,
        )
        .expect("oversized on disk");
        assert!(read_tls_state_bytes(&state).is_err());
    }

    #[test]
    fn byo_account_and_certificate_state_round_trip_and_are_bounded() {
        let (directory, root) = journal();
        let byo = open_byo_directory(&root).expect("byo dir");
        let account = open_byo_account_directory(&byo, "journal.example.com").expect("account");
        assert_eq!(read_byo_account_uri(&account).expect("missing"), None);
        assert_eq!(read_byo_account_key(&account).expect("missing"), None);

        persist_byo_account_key(&account, b"key").expect("key");
        persist_byo_account_uri(&account, "https://acme.example/acct/1\n").expect("uri");
        assert_eq!(
            read_byo_account_key(&account).expect("key").as_deref(),
            Some(&b"key"[..])
        );
        assert_eq!(
            read_byo_account_uri(&account).expect("uri").as_deref(),
            Some("https://acme.example/acct/1")
        );
        assert!(
            persist_byo_account_key(&account, &[0_u8; MAX_TLS_ACME_ACCOUNT_BYTES + 1]).is_err()
        );
        assert_eq!(
            read_byo_account_key(&account)
                .expect("unchanged")
                .as_deref(),
            Some(&b"key"[..])
        );
        delete_byo_account_pair(&account).expect("delete");
        delete_byo_account_pair(&account).expect("delete again");
        assert_eq!(read_byo_account_uri(&account).expect("gone"), None);
        assert_eq!(read_byo_account_key(&account).expect("gone"), None);

        let first = open_byo_cert_directory(&byo, "journal.example.com", 1).expect("gen 1");
        let second = open_byo_cert_directory(&byo, "journal.example.com", 2).expect("gen 2");
        persist_tls_state_bytes(&first, b"{\"g\":1}").expect("gen 1 state");
        assert_eq!(read_tls_state_bytes(&second).expect("gen 2 empty"), None);
        assert!(
            directory
                .path()
                .join("mcp-endpoint/byo/certs/journal.example.com/1/state.json")
                .is_file()
        );
        assert!(
            directory
                .path()
                .join("mcp-endpoint/byo/accounts/journal.example.com")
                .is_dir()
        );
    }

    #[test]
    fn a_hostname_that_is_not_an_ordinary_directory_name_is_refused() {
        let (_directory, root) = journal();
        let byo = open_byo_directory(&root).expect("byo dir");
        for hostname in [
            "",
            "con.example.com",
            "nul",
            "lpt1.example.com",
            "journal.example.com.",
            "Journal.example.com",
            "a<b.example.com",
            "..",
        ] {
            assert!(
                open_byo_account_directory(&byo, hostname).is_err(),
                "{hostname:?}"
            );
            assert!(
                open_byo_cert_directory(&byo, hostname, 1).is_err(),
                "{hostname:?}"
            );
        }
        assert!(open_byo_account_directory(&byo, "console.example.com").is_ok());
    }

    #[test]
    fn acme_accounts_are_kept_per_environment() {
        let (_directory, root) = journal();
        let state = open_tls_state_directory(&root).expect("tls dir");
        persist_tls_acme_account_bytes(&state, false, b"staging").expect("staging");
        assert_eq!(
            read_tls_acme_account_bytes(&state, true).expect("prod"),
            None
        );
        persist_tls_acme_account_bytes(&state, true, b"production").expect("production");
        assert_eq!(
            read_tls_acme_account_bytes(&state, false)
                .expect("staging")
                .as_deref(),
            Some(&b"staging"[..])
        );
        assert_eq!(
            read_tls_acme_account_bytes(&state, true)
                .expect("production")
                .as_deref(),
            Some(&b"production"[..])
        );
        assert!(
            persist_tls_acme_account_bytes(&state, true, &[0_u8; MAX_TLS_ACME_ACCOUNT_BYTES + 1])
                .is_err()
        );
    }
}
