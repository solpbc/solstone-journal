// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only loading of the committed link identity.
//!
//! Python-written journals keep state at `link/state.json`, while the native
//! provisioning bundle uses `link/ca/state.json`. This module accepts either
//! layout without entering the provisioning flow or writing journal state.

use std::error::Error;
#[cfg(unix)]
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use solstone_core_journal_io::{
    FlatDirectory, JournalRoot, ReadError, open_flat_directory_bound, read_bytes_bound,
};
use x509_parser::pem::parse_x509_pem;

use crate::ca::{CaError, LocalCa, jid_from_spki, load_ca};

/// A read-only view of the existing link identity.
pub struct CommittedIdentity {
    certificate_pem: Vec<u8>,
    certificate_der: Vec<u8>,
    ca: LocalCa,
    home_label: String,
    instance_id: String,
}

impl CommittedIdentity {
    /// Original PEM bytes from `link/ca/cert.pem`.
    pub fn certificate_pem(&self) -> &[u8] {
        &self.certificate_pem
    }

    /// Original DER decoded from `link/ca/cert.pem`.
    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    /// Human-readable link label from the committed state.
    pub fn home_label(&self) -> &str {
        &self.home_label
    }

    /// Link instance identifier from the committed state.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Validated CA signing material for host-side certificate issuance.
    pub fn ca(&self) -> &LocalCa {
        &self.ca
    }
}

/// Failure while reading a committed link identity.
#[derive(Debug)]
pub enum CommittedIdentityError {
    CertificateRead {
        path: PathBuf,
        source: io::Error,
    },
    PrivateKeyRead {
        path: PathBuf,
        source: io::Error,
    },
    CertificatePem {
        path: PathBuf,
    },
    Ca {
        certificate_path: PathBuf,
        source: CaError,
    },
    StateRead {
        path: PathBuf,
        source: io::Error,
    },
    StateMalformed {
        path: PathBuf,
        detail: &'static str,
    },
    StateInstanceMismatch {
        path: PathBuf,
    },
}

impl fmt::Display for CommittedIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CertificateRead { path, source } => {
                write!(
                    formatter,
                    "could not read committed CA certificate {}: {source}",
                    path.display()
                )
            }
            Self::PrivateKeyRead { path, source } => {
                write!(
                    formatter,
                    "could not read committed CA private key {}: {source}",
                    path.display()
                )
            }
            Self::CertificatePem { path } => {
                write!(
                    formatter,
                    "committed CA certificate {} is not PEM",
                    path.display()
                )
            }
            Self::Ca {
                certificate_path,
                source,
            } => write!(
                formatter,
                "committed CA material at {} is invalid: {source}",
                certificate_path.display()
            ),
            Self::StateRead { path, source } => {
                write!(
                    formatter,
                    "could not read committed link state {}: {source}",
                    path.display()
                )
            }
            Self::StateMalformed { path, detail } => {
                write!(
                    formatter,
                    "committed link state {} {detail}",
                    path.display()
                )
            }
            Self::StateInstanceMismatch { path } => write!(
                formatter,
                "committed link state {} instance_id does not match the CA public key",
                path.display()
            ),
        }
    }
}

impl Error for CommittedIdentityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CertificateRead { source, .. }
            | Self::PrivateKeyRead { source, .. }
            | Self::StateRead { source, .. } => Some(source),
            Self::Ca { source, .. } => Some(source),
            Self::CertificatePem { .. }
            | Self::StateMalformed { .. }
            | Self::StateInstanceMismatch { .. } => None,
        }
    }
}

/// Load existing committed link identity material without creating or changing it.
pub fn load_committed_identity(
    journal_root: &Path,
) -> Result<CommittedIdentity, CommittedIdentityError> {
    let link = journal_root.join("link");
    let certificate_path = link.join("ca/cert.pem");
    let private_key_path = link.join("ca/private.pem");
    let certificate_pem =
        fs::read(&certificate_path).map_err(|source| CommittedIdentityError::CertificateRead {
            path: certificate_path.clone(),
            source,
        })?;
    let private_key_pem = fs::read_to_string(&private_key_path).map_err(|source| {
        CommittedIdentityError::PrivateKeyRead {
            path: private_key_path.clone(),
            source,
        }
    })?;
    finish_committed_identity(certificate_path, certificate_pem, private_key_pem, || {
        read_state(&link)
    })
}

/// Load existing committed link identity material through an admitted journal root.
///
/// The retained root descriptor is the source authority. Its canonical path is
/// used only for diagnostics in [`CommittedIdentityError`].
#[cfg(unix)]
pub fn load_committed_identity_bound(
    root: &JournalRoot,
) -> Result<CommittedIdentity, CommittedIdentityError> {
    let link_path = root.canonical_path().join("link");
    let certificate_path = link_path.join("ca/cert.pem");
    let private_key_path = link_path.join("ca/private.pem");
    root.revalidate()
        .map_err(|source| CommittedIdentityError::CertificateRead {
            path: certificate_path.clone(),
            source: io::Error::other(source),
        })?;
    let link = match open_flat_directory_bound(root, OsStr::new("link"), root.canonical_path()) {
        Ok(Some(directory)) => directory,
        Ok(None) => {
            return Err(CommittedIdentityError::CertificateRead {
                path: certificate_path,
                source: missing_file_error(),
            });
        }
        Err(source) => {
            return Err(CommittedIdentityError::CertificateRead {
                path: certificate_path,
                source: io::Error::other(source),
            });
        }
    };
    let ca_path = link_path.join("ca");
    let ca = match open_flat_directory_bound(&link, OsStr::new("ca"), &link_path) {
        Ok(Some(directory)) => directory,
        Ok(None) => {
            return Err(CommittedIdentityError::CertificateRead {
                path: certificate_path,
                source: missing_file_error(),
            });
        }
        Err(source) => {
            return Err(CommittedIdentityError::CertificateRead {
                path: certificate_path,
                source: io::Error::other(source),
            });
        }
    };
    let certificate_pem =
        read_required_bound_file(&ca, OsStr::new("cert.pem")).map_err(|source| {
            CommittedIdentityError::CertificateRead {
                path: certificate_path.clone(),
                source,
            }
        })?;
    let private_key_bytes =
        read_required_bound_file(&ca, OsStr::new("private.pem")).map_err(|source| {
            CommittedIdentityError::PrivateKeyRead {
                path: private_key_path.clone(),
                source,
            }
        })?;
    let private_key_pem = String::from_utf8(private_key_bytes).map_err(|source| {
        CommittedIdentityError::PrivateKeyRead {
            path: private_key_path.clone(),
            source: io::Error::new(io::ErrorKind::InvalidData, source),
        }
    })?;
    let primary_state_path = link_path.join("state.json");
    let fallback_state_path = ca_path.join("state.json");
    finish_committed_identity(certificate_path, certificate_pem, private_key_pem, || {
        read_state_bound(&link, &ca, primary_state_path, fallback_state_path)
    })
}

fn finish_committed_identity(
    certificate_path: PathBuf,
    certificate_pem: Vec<u8>,
    private_key_pem: String,
    read_state: impl FnOnce() -> Result<(PathBuf, CommittedState), CommittedIdentityError>,
) -> Result<CommittedIdentity, CommittedIdentityError> {
    let certificate_pem_text = std::str::from_utf8(&certificate_pem).map_err(|_| {
        CommittedIdentityError::CertificatePem {
            path: certificate_path.clone(),
        }
    })?;
    let (_, pem) =
        parse_x509_pem(&certificate_pem).map_err(|_| CommittedIdentityError::CertificatePem {
            path: certificate_path.clone(),
        })?;
    let ca = load_ca(certificate_pem_text, &private_key_pem).map_err(|source| {
        CommittedIdentityError::Ca {
            certificate_path: certificate_path.clone(),
            source,
        }
    })?;

    let (state_path, state) = read_state()?;
    let expected_instance_id =
        jid_from_spki(ca.spki_der()).map_err(|source| CommittedIdentityError::Ca {
            certificate_path: certificate_path.clone(),
            source,
        })?;
    if state.instance_id != expected_instance_id {
        return Err(CommittedIdentityError::StateInstanceMismatch { path: state_path });
    }

    // `load_ca` reconstructs a private rcgen certificate with
    // `from_ca_cert_pem(...).self_signed(...)` (ca.rs:120). ECDSA signatures
    // are randomized, so using that re-mint's DER would orphan DER-pinned
    // devices. Return only the original PEM and DER decoded from it.
    Ok(CommittedIdentity {
        certificate_pem,
        certificate_der: pem.contents,
        ca,
        home_label: state.home_label,
        instance_id: state.instance_id,
    })
}

struct CommittedState {
    instance_id: String,
    home_label: String,
}

fn read_state(link: &Path) -> Result<(PathBuf, CommittedState), CommittedIdentityError> {
    let primary = link.join("state.json");
    let fallback = link.join("ca/state.json");
    let primary_read_path = primary.clone();
    let fallback_read_path = fallback.clone();
    read_state_from(
        primary,
        fallback,
        move || read_optional_string_path(&primary_read_path),
        move || read_optional_string_path(&fallback_read_path),
    )
}

fn read_state_from(
    primary_path: PathBuf,
    fallback_path: PathBuf,
    read_primary: impl FnOnce() -> Result<Option<String>, io::Error>,
    read_fallback: impl FnOnce() -> Result<Option<String>, io::Error>,
) -> Result<(PathBuf, CommittedState), CommittedIdentityError> {
    match read_primary() {
        Ok(Some(contents)) => parse_state(primary_path, &contents),
        Ok(None) => match read_fallback() {
            Ok(Some(contents)) => parse_state(fallback_path, &contents),
            Ok(None) => Err(CommittedIdentityError::StateRead {
                path: fallback_path,
                source: missing_file_error(),
            }),
            Err(source) => Err(CommittedIdentityError::StateRead {
                path: fallback_path,
                source,
            }),
        },
        Err(source) => Err(CommittedIdentityError::StateRead {
            path: primary_path,
            source,
        }),
    }
}

fn read_optional_string_path(path: &Path) -> Result<Option<String>, io::Error> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(source),
    }
}

#[cfg(unix)]
fn read_state_bound(
    link: &FlatDirectory,
    ca: &FlatDirectory,
    primary_path: PathBuf,
    fallback_path: PathBuf,
) -> Result<(PathBuf, CommittedState), CommittedIdentityError> {
    read_state_from(
        primary_path,
        fallback_path,
        || read_optional_bound_string(link, OsStr::new("state.json")),
        || read_optional_bound_string(ca, OsStr::new("state.json")),
    )
}

#[cfg(unix)]
fn read_required_bound_file(directory: &FlatDirectory, name: &OsStr) -> Result<Vec<u8>, io::Error> {
    read_bytes_bound(directory, name)
        .map_err(bound_read_error)?
        .ok_or_else(missing_file_error)
}

#[cfg(unix)]
fn read_optional_bound_string(
    directory: &FlatDirectory,
    name: &OsStr,
) -> Result<Option<String>, io::Error> {
    read_bytes_bound(directory, name)
        .map_err(bound_read_error)?
        .map(|bytes| {
            String::from_utf8(bytes)
                .map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))
        })
        .transpose()
}

#[cfg(unix)]
fn bound_read_error(error: ReadError) -> io::Error {
    match error {
        ReadError::Io { source, .. } => source,
        other => io::Error::other(other),
    }
}

fn missing_file_error() -> io::Error {
    io::Error::from(io::ErrorKind::NotFound)
}

fn parse_state(
    path: PathBuf,
    contents: &str,
) -> Result<(PathBuf, CommittedState), CommittedIdentityError> {
    let value: serde_json::Value =
        serde_json::from_str(contents).map_err(|_| CommittedIdentityError::StateMalformed {
            path: path.clone(),
            detail: "is not valid JSON",
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| CommittedIdentityError::StateMalformed {
            path: path.clone(),
            detail: "must be a JSON object",
        })?;
    let instance_id = object
        .get("instance_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CommittedIdentityError::StateMalformed {
            path: path.clone(),
            detail: "instance_id is missing",
        })?;
    let home_label = object
        .get("home_label")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CommittedIdentityError::StateMalformed {
            path: path.clone(),
            detail: "home_label is missing",
        })?;
    // Python omits locked_at before lock-in. It is intentionally not read here.
    Ok((
        path,
        CommittedState {
            instance_id: instance_id.to_owned(),
            home_label: home_label.to_owned(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    use solstone_core_journal_io::JournalRoot;
    #[cfg(all(unix, feature = "test-hooks"))]
    use solstone_core_journal_io::{BoundReadPrimitive, run_with_two_bound_read_barriers};
    use x509_parser::pem::parse_x509_pem;

    #[cfg(unix)]
    use super::load_committed_identity_bound;
    use super::{CommittedIdentityError, load_committed_identity};
    use crate::ca::{generate_ca, jid_from_spki};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock is after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-committed-identity-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary root creates");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_identity(
        root: &Path,
        ca: &crate::ca::LocalCa,
        state: &str,
        state_in_ca: bool,
    ) -> Vec<u8> {
        let ca_dir = root.join("link/ca");
        fs::create_dir_all(&ca_dir).expect("CA directory creates");
        let certificate = ca.certificate_pem().as_bytes().to_vec();
        fs::write(ca_dir.join("cert.pem"), &certificate).expect("certificate writes");
        fs::write(ca_dir.join("private.pem"), ca.private_key_pem()).expect("key writes");
        let state_path = if state_in_ca {
            ca_dir.join("state.json")
        } else {
            root.join("link/state.json")
        };
        fs::write(state_path, state).expect("state writes");
        certificate
    }

    fn python_state(instance_id: &str) -> String {
        format!(r#"{{"instance_id":"{instance_id}","home_label":"Python Home"}}"#)
    }

    fn python_locked_state(instance_id: &str) -> String {
        format!(
            r#"{{"instance_id":"{instance_id}","home_label":"Python Home","locked_at":1700000000000}}"#
        )
    }

    #[test]
    fn loads_python_state_without_locked_at() {
        let root = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        let state = python_state(&instance_id);
        let certificate = write_identity(root.path(), &ca, &state, false);

        let identity = load_committed_identity(root.path()).expect("identity loads");

        assert_eq!(identity.home_label(), "Python Home");
        assert_eq!(identity.instance_id(), instance_id);
        assert_eq!(identity.certificate_pem(), certificate);
    }

    #[test]
    fn loads_python_state_with_locked_at() {
        let root = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        let state = python_locked_state(&instance_id);
        write_identity(root.path(), &ca, &state, false);

        let identity = load_committed_identity(root.path()).expect("identity loads");

        assert_eq!(identity.home_label(), "Python Home");
        assert_eq!(identity.instance_id(), instance_id);
    }

    #[test]
    fn loads_native_state_fallback() {
        let root = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        let state = python_locked_state(&instance_id);
        write_identity(root.path(), &ca, &state, true);

        let identity = load_committed_identity(root.path()).expect("identity loads");

        assert_eq!(identity.instance_id(), instance_id);
    }

    #[test]
    fn absent_ca_is_specific_and_writes_nothing() {
        let root = TempDir::new();
        let before = directory_tree(root.path());

        let error = match load_committed_identity(root.path()) {
            Ok(_) => panic!("missing CA must refuse"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            CommittedIdentityError::CertificateRead { .. }
        ));
        assert_eq!(directory_tree(root.path()), before);
    }

    #[test]
    fn returns_original_ca_der_without_changing_files() {
        let root = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        let state = python_state(&instance_id);
        let certificate = write_identity(root.path(), &ca, &state, false);
        let (_, pem) = parse_x509_pem(&certificate).expect("certificate parses");
        let before = fs::read(root.path().join("link/ca/cert.pem")).expect("certificate reads");

        let identity = load_committed_identity(root.path()).expect("identity loads");

        assert_eq!(identity.certificate_der(), pem.contents.as_slice());
        assert!(!identity.ca().private_key_pem().is_empty());
        assert_eq!(
            fs::read(root.path().join("link/ca/cert.pem")).expect("certificate rereads"),
            before
        );
    }

    #[cfg(unix)]
    fn assert_bound_matches_path(root: &Path) {
        let path_identity = load_committed_identity(root).expect("path identity loads");
        let admitted = JournalRoot::open(root).expect("journal root admits");
        let bound_identity =
            load_committed_identity_bound(&admitted).expect("bound identity loads");

        assert_eq!(
            bound_identity.certificate_pem(),
            path_identity.certificate_pem()
        );
        assert_eq!(
            bound_identity.certificate_der(),
            path_identity.certificate_der()
        );
        assert_eq!(bound_identity.home_label(), path_identity.home_label());
        assert_eq!(bound_identity.instance_id(), path_identity.instance_id());
    }

    #[cfg(unix)]
    #[test]
    fn bound_reader_matches_path_reader_for_committed_state_layouts() {
        let primary = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        write_identity(primary.path(), &ca, &python_state(&instance_id), false);
        assert_bound_matches_path(primary.path());

        let fallback = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        write_identity(fallback.path(), &ca, &python_state(&instance_id), true);
        assert_bound_matches_path(fallback.path());

        let both = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        write_identity(both.path(), &ca, &python_state(&instance_id), false);
        fs::write(
            both.path().join("link/ca/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"Native Fallback"}}"#),
        )
        .expect("fallback state writes");
        assert_bound_matches_path(both.path());
        let admitted = JournalRoot::open(both.path()).expect("journal root admits");
        assert_eq!(
            load_committed_identity_bound(&admitted)
                .expect("bound identity loads")
                .home_label(),
            "Python Home"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bound_reader_matches_path_reader_failures() {
        let missing = TempDir::new();
        assert_failure_variants_match(missing.path());

        let incomplete = TempDir::new();
        fs::create_dir_all(incomplete.path().join("link/ca")).expect("CA directory creates");
        assert_failure_variants_match(incomplete.path());

        let malformed = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        write_identity(malformed.path(), &ca, &python_state(&instance_id), false);
        fs::write(malformed.path().join("link/state.json"), b"{").expect("state overwrites");
        assert_failure_variants_match(malformed.path());

        let mismatched = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        write_identity(
            mismatched.path(),
            &ca,
            r#"{"instance_id":"wrong","home_label":"Python Home"}"#,
            false,
        );
        assert_failure_variants_match(mismatched.path());
    }

    #[cfg(unix)]
    fn assert_failure_variants_match(root: &Path) {
        let path_error = match load_committed_identity(root) {
            Ok(_) => panic!("path identity must fail"),
            Err(error) => error,
        };
        let admitted = JournalRoot::open(root).expect("journal root admits");
        let bound_error = match load_committed_identity_bound(&admitted) {
            Ok(_) => panic!("bound identity must fail"),
            Err(error) => error,
        };
        assert_eq!(
            committed_error_kind(&bound_error),
            committed_error_kind(&path_error)
        );
    }

    #[cfg(unix)]
    fn committed_error_kind(error: &CommittedIdentityError) -> &'static str {
        match error {
            CommittedIdentityError::CertificateRead { .. } => "certificate-read",
            CommittedIdentityError::PrivateKeyRead { .. } => "private-key-read",
            CommittedIdentityError::CertificatePem { .. } => "certificate-pem",
            CommittedIdentityError::Ca { .. } => "ca",
            CommittedIdentityError::StateRead { .. } => "state-read",
            CommittedIdentityError::StateMalformed { .. } => "state-malformed",
            CommittedIdentityError::StateInstanceMismatch { .. } => "state-instance-mismatch",
        }
    }

    #[cfg(unix)]
    #[test]
    fn bound_reader_keeps_the_admitted_root_after_a_path_swap() {
        let temporary = TempDir::new();
        let original = temporary.path().join("original");
        let replacement = temporary.path().join("replacement");
        let moved = temporary.path().join("moved");
        fs::create_dir(&original).expect("original directory creates");
        fs::create_dir(&replacement).expect("replacement directory creates");

        let original_ca = generate_ca().expect("original CA generates");
        let original_id = jid_from_spki(original_ca.spki_der()).expect("original JID derives");
        write_identity(&original, &original_ca, &python_state(&original_id), false);
        let replacement_ca = generate_ca().expect("replacement CA generates");
        let replacement_id =
            jid_from_spki(replacement_ca.spki_der()).expect("replacement JID derives");
        write_identity(
            &replacement,
            &replacement_ca,
            &python_state(&replacement_id),
            false,
        );

        let admitted = JournalRoot::open(&original).expect("journal root admits");
        fs::rename(&original, &moved).expect("original moves");
        fs::rename(&replacement, &original).expect("replacement installs");

        let bound = load_committed_identity_bound(&admitted).expect("bound identity loads");
        let path = load_committed_identity(&original).expect("path identity loads");
        assert_eq!(bound.instance_id(), original_id);
        assert_eq!(path.instance_id(), replacement_id);
    }

    #[cfg(all(unix, feature = "test-hooks"))]
    #[test]
    fn bound_reader_rejects_regular_certificate_replacement_after_open() {
        let root = TempDir::new();
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("JID derives");
        write_identity(root.path(), &ca, &python_state(&instance_id), false);
        let certificate = root.path().join("link/ca/cert.pem");
        let aside = root.path().join("link/ca/cert.pem.aside");
        let replacement = root.path().join("link/ca/replacement.pem");
        fs::write(&replacement, b"replacement").expect("replacement writes");
        let admitted = JournalRoot::open(root.path()).expect("journal root admits");
        let (result, fired) = run_with_two_bound_read_barriers(
            BoundReadPrimitive::Read,
            1,
            move || {
                fs::rename(&certificate, &aside).expect("certificate moves aside");
                fs::rename(&replacement, &certificate).expect("replacement installs");
            },
            BoundReadPrimitive::FinalNameObserve,
            1,
            || {},
            || load_committed_identity_bound(&admitted),
        );
        assert_eq!(fired, 2);
        let error = match result {
            Ok(_) => panic!("certificate replacement must reject"),
            Err(error) => error,
        };
        assert_eq!(committed_error_kind(&error), "certificate-read");
        assert!(!root.path().join("mcp-endpoint").exists());
    }

    fn directory_tree(root: &Path) -> Vec<PathBuf> {
        let mut entries = Vec::new();
        collect_tree(root, root, &mut entries);
        entries.sort();
        entries
    }

    fn collect_tree(root: &Path, path: &Path, entries: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).expect("tree reads") {
            let entry = entry.expect("tree entry reads");
            let entry_path = entry.path();
            entries.push(
                entry_path
                    .strip_prefix(root)
                    .expect("relative path")
                    .to_path_buf(),
            );
            if entry.file_type().expect("file type reads").is_dir() {
                collect_tree(root, &entry_path, entries);
            }
        }
    }
}
