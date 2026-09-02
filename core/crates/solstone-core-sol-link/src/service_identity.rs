// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Python-compatible local-link identity persistence.
//!
//! Unlike [`crate::establish`], this module keeps `state.json` at
//! `link/state.json`, matching journals provisioned by the Python service.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, JsonWriteOptions, LockError, LockOptions, hold_lock,
    write_json, write_text,
};

use crate::ca::{CaError, LocalCa, generate_ca, jid_from_spki, load_ca};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceIdentity {
    pub instance_id: String,
    pub home_label: String,
    pub locked_at: Option<i64>,
}

#[derive(Debug)]
pub enum ServiceIdentityError {
    Io(io::Error),
    Lock(LockError),
    Write(AtomicWriteError),
    Ca(CaError),
    State(&'static str),
}

impl fmt::Display for ServiceIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Ca(error) => error.fmt(formatter),
            Self::State(message) => write!(formatter, "invalid local link state: {message}"),
        }
    }
}

impl std::error::Error for ServiceIdentityError {}

/// Load the Python-layout identity, repairing it against a committed CA, or
/// create it from a newly generated (or already committed) CA.
pub fn load_or_create_service_identity(
    journal_root: &Path,
    default_label: &str,
) -> Result<ServiceIdentity, ServiceIdentityError> {
    let link = journal_root.join("link");
    fs::create_dir_all(&link).map_err(ServiceIdentityError::Io)?;
    let _lock = hold_lock(link.join("identity"), LockOptions::default())
        .map_err(ServiceIdentityError::Lock)?;
    let state_path = link.join("state.json");

    if let Some(state) = load_state(&state_path, default_label)? {
        return normalize_against_committed_ca(&state_path, state);
    }

    let ca = load_or_generate_ca(journal_root)?;
    let state = ServiceIdentity {
        instance_id: jid_from_spki(ca.spki_der()).map_err(ServiceIdentityError::Ca)?,
        home_label: default_label.to_owned(),
        locked_at: Some(now_ms()),
    };
    write_state(&state_path, &state)?;
    Ok(state)
}

/// Reads the committed CA used by the Python-layout identity.
pub fn load_service_identity_ca(journal_root: &Path) -> Result<LocalCa, ServiceIdentityError> {
    let directory = ca_path(journal_root);
    let certificate =
        fs::read_to_string(directory.join("cert.pem")).map_err(ServiceIdentityError::Io)?;
    let private_key =
        fs::read_to_string(directory.join("private.pem")).map_err(ServiceIdentityError::Io)?;
    load_ca(&certificate, &private_key).map_err(ServiceIdentityError::Ca)
}

fn normalize_against_committed_ca(
    state_path: &Path,
    state: ServiceIdentity,
) -> Result<ServiceIdentity, ServiceIdentityError> {
    let journal_root =
        state_path
            .parent()
            .and_then(Path::parent)
            .ok_or(ServiceIdentityError::State(
                "state path has no journal root",
            ))?;
    let ca_directory = ca_path(journal_root);
    if !ca_directory.join("cert.pem").is_file() || !ca_directory.join("private.pem").is_file() {
        return Ok(state);
    }
    let ca = load_service_identity_ca(journal_root)?;
    let instance_id = jid_from_spki(ca.spki_der()).map_err(ServiceIdentityError::Ca)?;
    if state.instance_id == instance_id {
        return Ok(state);
    }
    let repaired = ServiceIdentity {
        instance_id,
        ..state
    };
    write_state(state_path, &repaired)?;
    Ok(repaired)
}

fn load_or_generate_ca(journal_root: &Path) -> Result<LocalCa, ServiceIdentityError> {
    let directory = ca_path(journal_root);
    let certificate = directory.join("cert.pem");
    let private_key = directory.join("private.pem");
    if certificate.is_file() && private_key.is_file() {
        return load_service_identity_ca(journal_root);
    }
    if certificate.exists() || private_key.exists() {
        return Err(ServiceIdentityError::State("committed CA is incomplete"));
    }
    fs::create_dir_all(&directory).map_err(ServiceIdentityError::Io)?;
    let ca = generate_ca().map_err(ServiceIdentityError::Ca)?;
    write_text(
        &certificate,
        ca.certificate_pem(),
        AtomicWriteOptions { mode: Some(0o644) },
    )
    .map_err(ServiceIdentityError::Write)?;
    write_text(
        &private_key,
        &ca.private_key_pem(),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(ServiceIdentityError::Write)?;
    Ok(ca)
}

fn load_state(
    path: &Path,
    default_label: &str,
) -> Result<Option<ServiceIdentity>, ServiceIdentityError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ServiceIdentityError::Io(error)),
    };
    let value: Value = serde_json::from_str(&contents)
        .map_err(|_| ServiceIdentityError::State("JSON is malformed"))?;
    let object = value
        .as_object()
        .ok_or(ServiceIdentityError::State("not an object"))?;
    let instance_id = object
        .get("instance_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(ServiceIdentityError::State("instance_id is missing"))?;
    Ok(Some(ServiceIdentity {
        instance_id: instance_id.to_owned(),
        home_label: object
            .get("home_label")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(default_label)
            .to_owned(),
        locked_at: object.get("locked_at").and_then(Value::as_i64),
    }))
}

fn write_state(path: &Path, state: &ServiceIdentity) -> Result<(), ServiceIdentityError> {
    let mut value = json!({
        "instance_id": state.instance_id,
        "home_label": state.home_label,
    });
    if let Some(locked_at) = state.locked_at {
        value["locked_at"] = json!(locked_at);
    }
    write_json(
        path,
        &value,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(ServiceIdentityError::Write)
}

fn ca_path(journal_root: &Path) -> PathBuf {
    journal_root.join("link").join("ca")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempJournal(PathBuf);
    impl TempJournal {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-service-identity-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn existing_state_is_returned_unchanged() {
        let journal = TempJournal::new();
        let original = load_or_create_service_identity(&journal.0, "Study").unwrap();
        let path = journal.0.join("link/state.json");
        let before = fs::read(&path).unwrap();
        let state = load_or_create_service_identity(&journal.0, "solstone").unwrap();
        assert_eq!(state, original);
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn absent_state_derives_a_fresh_id_from_the_ca() {
        let journal = TempJournal::new();
        let state = load_or_create_service_identity(&journal.0, "Home").unwrap();
        let ca = load_service_identity_ca(&journal.0).unwrap();
        assert_eq!(state.instance_id, jid_from_spki(ca.spki_der()).unwrap());
        assert_eq!(state.home_label, "Home");
    }

    #[test]
    fn inconsistent_id_is_repaired_without_regenerating_the_ca() {
        let journal = TempJournal::new();
        let initial = load_or_create_service_identity(&journal.0, "Home").unwrap();
        let certificate = fs::read(journal.0.join("link/ca/cert.pem")).unwrap();
        write_state(
            &journal.0.join("link/state.json"),
            &ServiceIdentity {
                instance_id: "wrong".to_owned(),
                ..initial.clone()
            },
        )
        .unwrap();
        let repaired = load_or_create_service_identity(&journal.0, "Other").unwrap();
        assert_eq!(repaired.instance_id, initial.instance_id);
        assert_eq!(repaired.home_label, "Home");
        assert_eq!(
            fs::read(journal.0.join("link/ca/cert.pem")).unwrap(),
            certificate
        );
    }
}
