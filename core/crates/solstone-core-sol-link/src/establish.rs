// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Atomic local-link identity establishment.
//!
//! The Rust-only identity bundle deliberately nests `state.json` in `link/ca/`
//! beside `cert.pem` and `private.pem`, so `publish_staged_dir` can publish the
//! complete identity with one create-only rename. Legacy journals keep
//! `state.json` at `link/state.json`; committed-identity and SPL readers accept
//! both layouts without copying or rewriting either one.

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

use crate::ca::{CaError, LocalCa, generate_ca, jid_from_spki, load_ca};
use crate::mark::{Mark, MarkError, mark_from_jid};
use crate::publish_checkpoint::PublishCheckpoint;

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
    Mark(MarkError),
    NoCandidate,
    State(String),
}

impl fmt::Display for EstablishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Publish(error) => error.fmt(formatter),
            Self::Ca(error) => error.fmt(formatter),
            Self::Mark(error) => error.fmt(formatter),
            Self::NoCandidate => formatter.write_str("no staged link identity candidate exists"),
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
            Self::Mark(error) => Some(error),
            Self::NoCandidate | Self::State(_) => None,
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

impl From<MarkError> for EstablishError {
    fn from(error: MarkError) -> Self {
        Self::Mark(error)
    }
}

pub fn lock_in(journal_root: &Path, home_label: Option<&str>) -> Result<LinkState, EstablishError> {
    lock_in_with_interruption(journal_root, home_label, &NoopInterruption)
}

trait PublishInterruption {
    fn at(&self, checkpoint: PublishCheckpoint) -> Result<(), EstablishError>;
}

struct NoopInterruption;

impl PublishInterruption for NoopInterruption {
    fn at(&self, _checkpoint: PublishCheckpoint) -> Result<(), EstablishError> {
        Ok(())
    }
}

fn lock_in_with_interruption(
    journal_root: &Path,
    home_label: Option<&str>,
    interruption: &dyn PublishInterruption,
) -> Result<LinkState, EstablishError> {
    let bundle = bundle_path(journal_root);
    let _lock = hold_lock(identity_lock_path(journal_root), LockOptions::default())?;
    if bundle.exists() {
        let state = load_bundle(&bundle)?;
        discard_candidate_unlocked(&candidate_path(journal_root));
        return Ok(state);
    }

    let candidate = candidate_path(journal_root);
    let ca = load_candidate_for_promotion(&candidate)?;
    let state = LinkState {
        instance_id: jid_from_spki(ca.spki_der())?,
        home_label: home_label.unwrap_or(DEFAULT_HOME_LABEL).to_owned(),
        locked_at: now_ms(),
    };
    publish_bundle(&bundle, &ca, &state, interruption)?;
    discard_candidate_unlocked(&candidate);
    Ok(state)
}

#[cfg(all(feature = "host", feature = "test-hooks"))]
#[doc(hidden)]
pub fn run_env_paused_lock_in() {
    let Ok(journal) = std::env::var("SOL_LINK_TEST_JOURNAL") else {
        return;
    };
    let name = std::env::var("JOURNAL_IO_TEST_PAUSE_AT")
        .unwrap_or_else(|_| panic!("JOURNAL_IO_TEST_PAUSE_AT is required"));
    let checkpoint = PublishCheckpoint::from_name(&name)
        .unwrap_or_else(|| panic!("unknown publication checkpoint {name}"));
    let marker = std::env::var("JOURNAL_IO_TEST_MARKER")
        .ok()
        .map(PathBuf::from);
    lock_in_with_interruption(
        Path::new(&journal),
        None,
        &EnvPauseInterruption {
            wanted: checkpoint,
            marker,
        },
    )
    .expect("env-paused lock_in");
}

#[cfg(feature = "test-hooks")]
struct EnvPauseInterruption {
    wanted: PublishCheckpoint,
    marker: Option<PathBuf>,
}

#[cfg(feature = "test-hooks")]
impl PublishInterruption for EnvPauseInterruption {
    fn at(&self, checkpoint: PublishCheckpoint) -> Result<(), EstablishError> {
        if checkpoint != self.wanted {
            return Ok(());
        }
        if let Some(path) = &self.marker {
            let _ = fs::write(path, checkpoint.as_str());
        }
        loop {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}

/// Return a valid staged candidate, regenerating a missing or invalid one.
pub fn current_candidate(journal_root: &Path) -> Result<LocalCa, EstablishError> {
    let candidate = candidate_path(journal_root);
    let _lock = hold_lock(identity_lock_path(journal_root), LockOptions::default())?;
    match load_candidate(&candidate) {
        Ok(ca) => Ok(ca),
        Err(_) => {
            discard_candidate_required(&candidate)?;
            generate_and_publish_candidate(&candidate)
        }
    }
}

/// Replace any staged candidate with a freshly generated CA.
pub fn regenerate_candidate(journal_root: &Path) -> Result<LocalCa, EstablishError> {
    let candidate = candidate_path(journal_root);
    let _lock = hold_lock(identity_lock_path(journal_root), LockOptions::default())?;
    discard_candidate_required(&candidate)?;
    generate_and_publish_candidate(&candidate)
}

/// Discard the staged candidate, if any.
pub fn discard_candidate(journal_root: &Path) -> Result<(), EstablishError> {
    let candidate = candidate_path(journal_root);
    let _lock = hold_lock(identity_lock_path(journal_root), LockOptions::default())?;
    discard_candidate_required(&candidate)
}

/// Derive the preview mark for a staged candidate CA.
pub fn candidate_mark(ca: &LocalCa) -> Result<Mark, EstablishError> {
    Ok(mark_from_jid(&jid_from_spki(ca.spki_der())?)?)
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

pub fn candidate_path(journal_root: &Path) -> PathBuf {
    journal_root.join("link").join("ca-staging")
}

fn identity_lock_path(journal_root: &Path) -> PathBuf {
    journal_root.join("link").join("identity")
}

fn load_candidate_for_promotion(candidate: &Path) -> Result<LocalCa, EstablishError> {
    match fs::symlink_metadata(candidate) {
        Ok(_) => load_candidate(candidate),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(EstablishError::NoCandidate),
        Err(error) => Err(EstablishError::State(format!(
            "candidate directory: {error}"
        ))),
    }
}

fn load_candidate(candidate: &Path) -> Result<LocalCa, EstablishError> {
    if !candidate.is_dir() {
        return Err(EstablishError::State(
            "candidate CA is not a directory".to_owned(),
        ));
    }
    let certificate_pem = fs::read_to_string(candidate.join("cert.pem"))
        .map_err(|error| EstablishError::State(format!("candidate cert.pem: {error}")))?;
    let private_key_pem = fs::read_to_string(candidate.join("private.pem"))
        .map_err(|error| EstablishError::State(format!("candidate private.pem: {error}")))?;
    Ok(load_ca(&certificate_pem, &private_key_pem)?)
}

fn generate_and_publish_candidate(candidate: &Path) -> Result<LocalCa, EstablishError> {
    let ca = generate_ca()?;
    publish_candidate(candidate, &ca)?;
    Ok(ca)
}

fn publish_candidate(candidate: &Path, ca: &LocalCa) -> Result<(), EstablishError> {
    publish_staged_dir(
        candidate,
        StagedDirOptions {
            directory_mode: Some(0o700),
        },
        |staging| {
            write_ca_material(staging, ca)?;
            Ok::<_, io::Error>(())
        },
    )?;
    Ok(())
}

fn discard_candidate_required(candidate: &Path) -> Result<(), EstablishError> {
    match fs::symlink_metadata(candidate) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(candidate)
            .map_err(|error| EstablishError::State(format!("candidate directory: {error}"))),
        Ok(_) => fs::remove_file(candidate)
            .map_err(|error| EstablishError::State(format!("candidate directory: {error}"))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(EstablishError::State(format!(
            "candidate directory: {error}"
        ))),
    }
}

fn discard_candidate_unlocked(candidate: &Path) {
    let _ = discard_candidate_required(candidate);
}

fn publish_bundle(
    bundle: &Path,
    ca: &crate::ca::LocalCa,
    state: &LinkState,
    interruption: &dyn PublishInterruption,
) -> Result<(), EstablishError> {
    interruption.at(PublishCheckpoint::BeforeStagingDirCreate)?;
    publish_staged_dir(
        bundle,
        StagedDirOptions {
            directory_mode: Some(0o700),
        },
        |staging| {
            interruption
                .at(PublishCheckpoint::AfterStagingDirCreate)
                .map_err(io::Error::other)?;
            write_certificate(staging, ca)?;
            interruption
                .at(PublishCheckpoint::MidPopulateCert)
                .map_err(io::Error::other)?;
            write_private_key(staging, ca)?;
            interruption
                .at(PublishCheckpoint::MidPopulateKey)
                .map_err(io::Error::other)?;
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
            interruption
                .at(PublishCheckpoint::AfterPopulate)
                .map_err(io::Error::other)?;
            Ok::<_, io::Error>(())
        },
    )?;
    Ok(())
}

fn write_ca_material(staging: &Path, ca: &LocalCa) -> Result<(), io::Error> {
    write_certificate(staging, ca)?;
    write_private_key(staging, ca)
}

fn write_certificate(staging: &Path, ca: &LocalCa) -> Result<(), io::Error> {
    write_bytes_exclusive(
        staging.join("cert.pem"),
        ca.certificate_pem().as_bytes(),
        AtomicWriteOptions { mode: Some(0o644) },
    )
    .map_err(io::Error::other)
}

fn write_private_key(staging: &Path, ca: &LocalCa) -> Result<(), io::Error> {
    write_bytes_exclusive(
        staging.join("private.pem"),
        ca.private_key_pem().as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(io::Error::other)
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

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn lock_in_publishes_one_complete_bundle_and_is_idempotent() {
        let temporary = TempDir::new();
        current_candidate(temporary.path()).unwrap();
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
        assert_complete_bundle(temporary.path());
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

    #[test]
    fn candidate_survives_between_previews() {
        let temporary = TempDir::new();
        let first = current_candidate(temporary.path()).unwrap();
        let second = current_candidate(temporary.path()).unwrap();

        assert_eq!(first.certificate_pem(), second.certificate_pem());
        assert_eq!(
            candidate_mark(&first).unwrap(),
            candidate_mark(&second).unwrap()
        );
        assert!(candidate_path(temporary.path()).join("cert.pem").is_file());
        assert!(
            candidate_path(temporary.path())
                .join("private.pem")
                .is_file()
        );
    }

    #[test]
    fn regenerate_candidate_replaces_the_existing_candidate() {
        let temporary = TempDir::new();
        let first = current_candidate(temporary.path()).unwrap();
        let first_certificate = first.certificate_pem().to_owned();
        let second = regenerate_candidate(temporary.path()).unwrap();

        assert_ne!(first_certificate, second.certificate_pem());
        assert_eq!(
            fs::read_to_string(candidate_path(temporary.path()).join("cert.pem")).unwrap(),
            second.certificate_pem()
        );
        assert_eq!(
            fs::read_dir(candidate_path(temporary.path()))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn discard_candidate_leaves_nothing_to_promote() {
        let temporary = TempDir::new();
        current_candidate(temporary.path()).unwrap();
        discard_candidate(temporary.path()).unwrap();

        assert!(!candidate_path(temporary.path()).exists());
        assert!(matches!(
            lock_in(temporary.path(), None),
            Err(EstablishError::NoCandidate)
        ));
    }

    #[test]
    fn lock_in_commits_the_last_previewed_candidate() {
        let temporary = TempDir::new();
        let first = current_candidate(temporary.path()).unwrap();
        let first_mark = candidate_mark(&first).unwrap();
        let regenerated = regenerate_candidate(temporary.path()).unwrap();
        let regenerated_mark = candidate_mark(&regenerated).unwrap();
        assert_ne!(first.certificate_pem(), regenerated.certificate_pem());
        let previewed = current_candidate(temporary.path()).unwrap();
        assert_eq!(candidate_mark(&previewed).unwrap(), regenerated_mark);

        lock_in(temporary.path(), None).unwrap();
        let committed = load_ca(
            &fs::read_to_string(bundle_path(temporary.path()).join("cert.pem")).unwrap(),
            &fs::read_to_string(bundle_path(temporary.path()).join("private.pem")).unwrap(),
        )
        .unwrap();
        assert_eq!(candidate_mark(&committed).unwrap(), regenerated_mark);
        assert_ne!(first_mark, regenerated_mark);
    }

    #[test]
    fn committed_lock_in_discards_a_stray_candidate() {
        let temporary = TempDir::new();
        current_candidate(temporary.path()).unwrap();
        let first = lock_in(temporary.path(), None).unwrap();
        regenerate_candidate(temporary.path()).unwrap();
        assert!(candidate_path(temporary.path()).exists());

        assert_eq!(lock_in(temporary.path(), None).unwrap(), first);
        assert!(!candidate_path(temporary.path()).exists());
    }

    #[test]
    fn lock_in_rejects_bundle_missing_cert_pem() {
        let temporary = committed_journal();
        fs::remove_file(bundle_path(temporary.path()).join("cert.pem")).unwrap();
        let error = lock_in(temporary.path(), None).unwrap_err();
        assert!(error.to_string().contains("cert.pem"));
    }

    #[test]
    fn lock_in_rejects_bundle_missing_private_pem() {
        let temporary = committed_journal();
        fs::remove_file(bundle_path(temporary.path()).join("private.pem")).unwrap();
        let error = lock_in(temporary.path(), None).unwrap_err();
        assert!(error.to_string().contains("private.pem"));
    }

    #[test]
    fn lock_in_rejects_bundle_missing_state_json() {
        let temporary = committed_journal();
        fs::remove_file(bundle_path(temporary.path()).join("state.json")).unwrap();
        let error = lock_in(temporary.path(), None).unwrap_err();
        assert!(error.to_string().contains("state.json"));
    }

    #[test]
    fn lock_in_rejects_bundle_with_invalid_private_key() {
        let temporary = committed_journal();
        fs::write(
            bundle_path(temporary.path()).join("private.pem"),
            "not a private key",
        )
        .unwrap();
        let error = lock_in(temporary.path(), None).unwrap_err();
        assert!(matches!(error, EstablishError::Ca(_)));
    }

    #[test]
    fn lock_in_rejects_bundle_with_malformed_state_json() {
        let temporary = committed_journal();
        fs::write(bundle_path(temporary.path()).join("state.json"), b"[]").unwrap();
        let error = lock_in(temporary.path(), None).unwrap_err();
        assert!(error.to_string().contains("state.json must be an object"));
    }

    #[test]
    fn lock_in_rejects_bundle_when_state_instance_id_does_not_match_ca() {
        let temporary = committed_journal();
        let path = bundle_path(temporary.path()).join("state.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["instance_id"] = json!("not-the-committed-jid");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let error = lock_in(temporary.path(), None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("state instance_id does not match the CA public key")
        );
    }

    #[test]
    fn interrupt_before_staging_dir_create_leaves_no_bundle_and_retries() {
        assert_interrupt_before_publish_then_retry(PublishCheckpoint::BeforeStagingDirCreate);
    }

    #[test]
    fn interrupt_after_staging_dir_create_leaves_no_bundle_and_retries() {
        assert_interrupt_before_publish_then_retry(PublishCheckpoint::AfterStagingDirCreate);
    }

    #[test]
    fn interrupt_after_cert_write_leaves_no_bundle_and_retries() {
        assert_interrupt_before_publish_then_retry(PublishCheckpoint::MidPopulateCert);
    }

    #[test]
    fn interrupt_after_private_key_write_leaves_no_bundle_and_retries() {
        assert_interrupt_before_publish_then_retry(PublishCheckpoint::MidPopulateKey);
    }

    #[test]
    fn interrupt_after_populate_leaves_no_bundle_and_retries() {
        assert_interrupt_before_publish_then_retry(PublishCheckpoint::AfterPopulate);
    }

    struct FailAt(PublishCheckpoint);

    impl PublishInterruption for FailAt {
        fn at(&self, checkpoint: PublishCheckpoint) -> Result<(), EstablishError> {
            if checkpoint == self.0 {
                Err(EstablishError::State(format!(
                    "injected at {}",
                    checkpoint.as_str()
                )))
            } else {
                Ok(())
            }
        }
    }

    fn committed_journal() -> TempDir {
        let temporary = TempDir::new();
        current_candidate(temporary.path()).unwrap();
        lock_in(temporary.path(), None).unwrap();
        temporary
    }

    fn assert_interrupt_before_publish_then_retry(checkpoint: PublishCheckpoint) {
        let temporary = TempDir::new();
        current_candidate(temporary.path()).unwrap();
        assert!(
            lock_in_with_interruption(temporary.path(), None, &FailAt(checkpoint)).is_err(),
            "checkpoint: {}",
            checkpoint.as_str()
        );
        assert!(
            !bundle_path(temporary.path()).exists(),
            "checkpoint: {}",
            checkpoint.as_str()
        );
        lock_in(temporary.path(), None).unwrap();
        assert_complete_bundle(temporary.path());
    }

    fn assert_complete_bundle(journal: &Path) {
        let bundle = bundle_path(journal);
        assert!(bundle.join("cert.pem").is_file());
        assert!(bundle.join("private.pem").is_file());
        assert!(bundle.join("state.json").is_file());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(bundle.join("private.pem"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let state = load_committed(journal).unwrap().unwrap();
        let ca = load_ca(
            &fs::read_to_string(bundle.join("cert.pem")).unwrap(),
            &fs::read_to_string(bundle.join("private.pem")).unwrap(),
        )
        .unwrap();
        assert_eq!(state.instance_id, jid_from_spki(ca.spki_der()).unwrap());
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
