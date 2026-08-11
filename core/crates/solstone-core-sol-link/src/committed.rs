// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only loading of the committed link identity.
//!
//! Python-written journals keep state at `link/state.json`, while the native
//! provisioning bundle uses `link/ca/state.json`. This module accepts either
//! layout without entering the provisioning flow or writing journal state.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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

    let (state_path, state) = read_state(&link)?;
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
    match fs::read_to_string(&primary) {
        Ok(contents) => parse_state(primary, &contents),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            let fallback = link.join("ca/state.json");
            let contents = fs::read_to_string(&fallback).map_err(|source| {
                CommittedIdentityError::StateRead {
                    path: fallback.clone(),
                    source,
                }
            })?;
            parse_state(fallback, &contents)
        }
        Err(source) => Err(CommittedIdentityError::StateRead {
            path: primary,
            source,
        }),
    }
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

    use x509_parser::pem::parse_x509_pem;

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
