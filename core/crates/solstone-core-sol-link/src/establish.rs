// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Atomic local-link identity establishment.
//!
//! The Rust-only identity bundle deliberately nests `state.json` in `link/ca/`
//! beside `cert.pem` and `private.pem`, so `publish_staged_dir` can publish the
//! complete identity with one create-only rename. Python-written journals keep
//! `state.json` at `link/state.json`; future production wiring must reconcile
//! that layout difference before this module becomes a runtime authority.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, JsonWriteOptions, LockError, LockOptions, StagedDirOptions,
    StagedWriteError, hold_lock, publish_staged_dir, write_bytes_exclusive, write_json,
};

use crate::ca::{CaError, generate_ca, jid_from_spki, load_ca};

const DEFAULT_HOME_LABEL: &str = "solstone";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkState {
    pub instance_id: String,
    pub home_label: String,
    pub locked_at: i64,
}

#[derive(Debug)]
pub enum EstablishError {
    Lock(LockError),
    Publish(StagedWriteError),
    Ca(CaError),
    State(String),
}

impl fmt::Display for EstablishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Publish(error) => error.fmt(formatter),
            Self::Ca(error) => error.fmt(formatter),
            Self::State(message) => write!(formatter, "invalid local link state: {message}"),
        }
    }
}

impl std::error::Error for EstablishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::Ca(error) => Some(error),
            Self::State(_) => None,
        }
    }
}

impl From<LockError> for EstablishError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

impl From<StagedWriteError> for EstablishError {
    fn from(error: StagedWriteError) -> Self {
        Self::Publish(error)
    }
}

impl From<CaError> for EstablishError {
    fn from(error: CaError) -> Self {
        Self::Ca(error)
    }
}

pub fn lock_in(journal_root: &Path, home_label: Option<&str>) -> Result<LinkState, EstablishError> {
    let bundle = bundle_path(journal_root);
    let _lock = hold_lock(identity_lock_path(journal_root), LockOptions::default())?;
    if bundle.exists() {
        return load_bundle(&bundle);
    }

    let ca = generate_ca()?;
    let state = LinkState {
        instance_id: jid_from_spki(ca.spki_der())?,
        home_label: home_label.unwrap_or(DEFAULT_HOME_LABEL).to_owned(),
        locked_at: now_ms(),
    };
    publish_bundle(&bundle, &ca, &state)?;
    Ok(state)
}

pub fn load_committed(journal_root: &Path) -> Result<Option<LinkState>, EstablishError> {
    let bundle = bundle_path(journal_root);
    if !bundle.exists() {
        return Ok(None);
    }
    load_bundle(&bundle).map(Some)
}

pub fn bundle_path(journal_root: &Path) -> PathBuf {
    journal_root.join("link").join("ca")
}

fn identity_lock_path(journal_root: &Path) -> PathBuf {
    journal_root.join("link").join("identity")
}

fn publish_bundle(
    bundle: &Path,
    ca: &crate::ca::LocalCa,
    state: &LinkState,
) -> Result<(), EstablishError> {
    publish_staged_dir(
        bundle,
        StagedDirOptions {
            directory_mode: Some(0o700),
        },
        |staging| {
            write_bytes_exclusive(
                staging.join("cert.pem"),
                ca.certificate_pem().as_bytes(),
                AtomicWriteOptions { mode: Some(0o644) },
            )
            .map_err(io::Error::other)?;
            pause_at("mid-populate-cert");
            write_bytes_exclusive(
                staging.join("private.pem"),
                ca.private_key_pem().as_bytes(),
                AtomicWriteOptions { mode: Some(0o600) },
            )
            .map_err(io::Error::other)?;
            pause_at("mid-populate-key");
            write_json(
                staging.join("state.json"),
                &json!({
                    "instance_id": state.instance_id,
                    "home_label": state.home_label,
                    "locked_at": state.locked_at,
                }),
                JsonWriteOptions {
                    mode: Some(0o600),
                    ..JsonWriteOptions::default()
                },
            )
            .map_err(io::Error::other)?;
            Ok::<_, io::Error>(())
        },
    )?;
    Ok(())
}

fn load_bundle(bundle: &Path) -> Result<LinkState, EstablishError> {
    if !bundle.is_dir() {
        return Err(EstablishError::State(
            "CA bundle is not a directory".to_owned(),
        ));
    }
    let certificate_pem = fs::read_to_string(bundle.join("cert.pem"))
        .map_err(|error| EstablishError::State(format!("cert.pem: {error}")))?;
    let private_key_pem = fs::read_to_string(bundle.join("private.pem"))
        .map_err(|error| EstablishError::State(format!("private.pem: {error}")))?;
    let ca = load_ca(&certificate_pem, &private_key_pem)?;
    let state = read_state(&bundle.join("state.json"))?;
    let expected = jid_from_spki(ca.spki_der())?;
    if state.instance_id != expected {
        return Err(EstablishError::State(
            "state instance_id does not match the CA public key".to_owned(),
        ));
    }
    Ok(state)
}

fn read_state(path: &Path) -> Result<LinkState, EstablishError> {
    let contents = fs::read_to_string(path)
        .map_err(|error| EstablishError::State(format!("state.json: {error}")))?;
    let value: Value = serde_json::from_str(&contents)
        .map_err(|error| EstablishError::State(format!("state.json: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| EstablishError::State("state.json must be an object".to_owned()))?;
    let instance_id = object
        .get("instance_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| EstablishError::State("state.json instance_id is missing".to_owned()))?;
    let home_label = object
        .get("home_label")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_HOME_LABEL);
    let locked_at = object
        .get("locked_at")
        .and_then(Value::as_i64)
        .ok_or_else(|| EstablishError::State("state.json locked_at is missing".to_owned()))?;
    Ok(LinkState {
        instance_id: instance_id.to_owned(),
        home_label: home_label.to_owned(),
        locked_at,
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
fn pause_at(step: &str) {
    if std::env::var("JOURNAL_IO_TEST_PAUSE_AT").ok().as_deref() != Some(step) {
        return;
    }
    if let Ok(marker) = std::env::var("JOURNAL_IO_TEST_MARKER") {
        let _ = fs::write(marker, step);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(not(test))]
fn pause_at(_step: &str) {}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::process::Command;
    #[cfg(unix)]
    use std::thread;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn lock_in_publishes_one_complete_bundle_and_is_idempotent() {
        let temporary = TempDir::new();
        let first = lock_in(temporary.path(), Some("laptop")).unwrap();
        let bundle = bundle_path(temporary.path());
        let before = fs::read(bundle.join("state.json")).unwrap();

        let second = lock_in(temporary.path(), Some("ignored")).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(bundle.join("state.json")).unwrap(), before);
        assert!(bundle.join("cert.pem").is_file());
        assert_eq!(
            fs::metadata(bundle.join("private.pem"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn inconsistent_existing_bundle_fails_loudly() {
        let temporary = TempDir::new();
        let bundle = bundle_path(temporary.path());
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("cert.pem"), "not a certificate").unwrap();

        let error = lock_in(temporary.path(), None).unwrap_err();

        assert!(error.to_string().contains("invalid local link state"));
    }

    #[cfg(unix)]
    #[test]
    fn lock_in_pause_helper() {
        let Ok(journal) = std::env::var("SOL_LINK_TEST_JOURNAL") else {
            return;
        };
        lock_in(Path::new(&journal), None).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn crash_before_staging_dir_create_leaves_no_bundle_and_retries() {
        assert_crash_before_publish_then_retry("before-staging-dir-create");
    }

    #[cfg(unix)]
    #[test]
    fn crash_after_staging_dir_create_leaves_no_bundle_and_retries() {
        assert_crash_before_publish_then_retry("after-staging-dir-create");
    }

    #[cfg(unix)]
    #[test]
    fn crash_after_cert_write_leaves_no_bundle_and_retries() {
        assert_crash_before_publish_then_retry("mid-populate-cert");
    }

    #[cfg(unix)]
    #[test]
    fn crash_after_private_key_write_leaves_no_bundle_and_retries() {
        assert_crash_before_publish_then_retry("mid-populate-key");
    }

    #[cfg(unix)]
    #[test]
    fn crash_after_populate_leaves_no_bundle_and_retries() {
        assert_crash_before_publish_then_retry("after-populate");
    }

    #[cfg(unix)]
    #[test]
    fn crash_after_staging_sync_leaves_no_bundle_and_retries() {
        assert_crash_before_publish_then_retry("after-staging-sync");
    }

    #[cfg(unix)]
    #[test]
    fn crash_after_rename_leaves_complete_bundle() {
        let temporary = TempDir::new();
        run_child_until_pause(temporary.path(), "after-rename");
        assert_complete_bundle(temporary.path());
    }

    #[cfg(unix)]
    fn assert_crash_before_publish_then_retry(checkpoint: &str) {
        let temporary = TempDir::new();
        run_child_until_pause(temporary.path(), checkpoint);
        assert!(
            !bundle_path(temporary.path()).exists(),
            "checkpoint: {checkpoint}"
        );
        lock_in(temporary.path(), None).unwrap();
        assert_complete_bundle(temporary.path());
    }

    #[cfg(unix)]
    fn run_child_until_pause(journal: &Path, checkpoint: &str) {
        let marker = journal.join("pause-marker");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("establish::tests::lock_in_pause_helper")
            .arg("--exact")
            .env("SOL_LINK_TEST_JOURNAL", journal)
            .env("JOURNAL_IO_TEST_PAUSE_AT", checkpoint)
            .env("JOURNAL_IO_TEST_MARKER", &marker)
            .spawn()
            .unwrap();
        wait_for_marker(&mut child, &marker, checkpoint);
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert!(
            !status.success(),
            "child unexpectedly completed at {checkpoint}"
        );
    }

    #[cfg(unix)]
    fn wait_for_marker(child: &mut std::process::Child, marker: &Path, checkpoint: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if fs::read_to_string(marker).ok().as_deref() == Some(checkpoint) {
                return;
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("child exited before {checkpoint}: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {checkpoint}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn assert_complete_bundle(journal: &Path) {
        let bundle = bundle_path(journal);
        assert!(bundle.join("cert.pem").is_file());
        assert!(bundle.join("private.pem").is_file());
        assert!(bundle.join("state.json").is_file());
        assert_eq!(
            fs::metadata(bundle.join("private.pem"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let state = load_committed(journal).unwrap().unwrap();
        assert_eq!(
            state.instance_id,
            jid_from_spki(&load_ca_bundle_spki(&bundle)).unwrap()
        );
    }

    #[cfg(unix)]
    fn load_ca_bundle_spki(bundle: &Path) -> Vec<u8> {
        load_ca(
            &fs::read_to_string(bundle.join("cert.pem")).unwrap(),
            &fs::read_to_string(bundle.join("private.pem")).unwrap(),
        )
        .unwrap()
        .spki_der()
        .to_vec()
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-sol-link-establish-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
