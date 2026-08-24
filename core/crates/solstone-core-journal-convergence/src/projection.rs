// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Legacy `stream.updated` / `daily.updated` projection after an exact proposed record.

use std::ffi::OsStr;
use std::os::fd::{AsFd, OwnedFd};

use solstone_core_journal_io::{
    BoundAtomicOutcome, atomic_replace_bound, create_directory_bound, read_bytes_bound,
    sync_dir_bound,
};

use crate::digest::{canonical_json_bytes, digest_bytes};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::layout::{CHRONICLE, DAILY_UPDATED, DayKey, HEALTH, STREAM_UPDATED};
use crate::lock::DayLockSet;
use crate::schema::{
    Adoption, Intent, PresentAbsent, ProjectionBinding, ROLE_STREAM_UPDATED, SCHEMA_VERSION,
    StreamUpdated, parse_json,
};
use crate::store::{ConvergenceStore, LoadDay};
use crate::walk::{open_dir, unlink_bound};

enum StreamClass {
    ExactPrior,
    ExactProposed,
}

enum DailyClass {
    ExactPrior,
    ExactProposed,
}

/// Build the proposed `stream.updated` Present slot. Bytes are canonical JSON
/// without the on-disk newline; digest is SHA-256 of those bytes.
pub(crate) fn marker_present(
    store: &ConvergenceStore,
    adoption: &Adoption,
    day: &DayKey,
    dirty_generation: u64,
    serial: u64,
) -> Result<PresentAbsent, ConvergenceError> {
    let marker = stream_marker(store, adoption, day, dirty_generation, serial);
    let bytes = canonical_json_bytes(&marker)?;
    let digest = digest_bytes(&bytes);
    Ok(PresentAbsent::Present {
        bytes: String::from_utf8(bytes).map_err(|source| ConvergenceError::Io {
            operation: "encode stream marker",
            role: DurableRole::StreamUpdated,
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        })?,
        digest: digest.as_hex().to_owned(),
    })
}

pub(crate) fn stream_marker(
    store: &ConvergenceStore,
    adoption: &Adoption,
    day: &DayKey,
    dirty_generation: u64,
    serial: u64,
) -> StreamUpdated {
    StreamUpdated {
        role: ROLE_STREAM_UPDATED.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        adoption_id: adoption.adoption_id.clone(),
        day: day.as_str().to_owned(),
        dirty_generation,
        author_serial: serial,
    }
}

pub(crate) fn verify_projection_binding(
    binding: &ProjectionBinding,
) -> Result<(), ConvergenceError> {
    verify_present_slot(&binding.prior_stream)?;
    verify_present_slot(&binding.prior_daily)?;
    match &binding.proposed_stream {
        PresentAbsent::Present { bytes, digest } => {
            if bytes.is_empty() {
                return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
            }
            verify_present_digest(bytes, digest)?;
        }
        PresentAbsent::Absent => {
            return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
        }
    }
    if !matches!(binding.proposed_daily, PresentAbsent::Absent) {
        return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
    }
    Ok(())
}

fn verify_present_slot(slot: &PresentAbsent) -> Result<(), ConvergenceError> {
    match slot {
        PresentAbsent::Absent => Ok(()),
        PresentAbsent::Present { bytes, digest } => verify_present_digest(bytes, digest),
    }
}

fn verify_present_digest(bytes: &str, digest: &str) -> Result<(), ConvergenceError> {
    if digest_bytes(bytes.as_bytes()).as_hex() != digest {
        return Err(ConvergenceError::Refused(Refusal::ProjectionByteMismatch));
    }
    Ok(())
}

pub(crate) fn refuse_mutated_projection(
    existing: &Intent,
    expected: &Intent,
) -> Result<(), ConvergenceError> {
    if existing.proposed_dirty_generations != expected.proposed_dirty_generations {
        return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
    }
    for (day, expected_binding) in &expected.projections {
        let Some(observed) = existing.projections.get(day) else {
            return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
        };
        classify_present_vs_expected(&observed.proposed_stream, &expected_binding.proposed_stream)?;
        classify_present_vs_expected(&observed.prior_stream, &expected_binding.prior_stream)?;
        if observed.proposed_daily != expected_binding.proposed_daily
            || observed.prior_daily != expected_binding.prior_daily
        {
            return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
        }
    }
    if existing.projections != expected.projections {
        return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
    }
    Ok(())
}

fn classify_present_vs_expected(
    observed: &PresentAbsent,
    expected: &PresentAbsent,
) -> Result<(), ConvergenceError> {
    match (observed, expected) {
        (
            PresentAbsent::Present { bytes, digest },
            PresentAbsent::Present {
                bytes: expected_bytes,
                digest: expected_digest,
            },
        ) => {
            let computed = digest_bytes(bytes.as_bytes());
            if computed.as_hex() != digest.as_str() {
                if bytes == expected_bytes {
                    return Err(ConvergenceError::Refused(Refusal::OldProjectionDigest));
                }
                return Err(ConvergenceError::Refused(Refusal::ProjectionByteMismatch));
            }
            if bytes != expected_bytes || digest != expected_digest {
                return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
            }
            Ok(())
        }
        (PresentAbsent::Absent, PresentAbsent::Absent) => Ok(()),
        _ => Err(ConvergenceError::Refused(Refusal::ChangedProjection)),
    }
}

/// Project after the day's record is exactly the intent's proposed revision.
pub(crate) fn project_day(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    day: &DayKey,
    intent: &Intent,
) -> Result<(), ConvergenceError> {
    store.revalidate()?;
    locks.matches(store.journal_id(), store.root_id(), store.object_identity())?;
    if !locks.contains(day) {
        return Err(ConvergenceError::Refused(Refusal::WrongDay {
            expected: day.as_str().to_owned(),
            observed: String::new(),
        }));
    }
    let proposed_rev = *intent
        .proposed_day_revisions
        .get(day.as_str())
        .ok_or(ConvergenceError::Refused(Refusal::ChangedProjection))?;
    match store.load_day(locks, day)? {
        LoadDay::Published(snapshot) if snapshot.record_revision == proposed_rev => {}
        _ => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Record,
            });
        }
    }
    let binding = intent
        .projections
        .get(day.as_str())
        .ok_or(ConvergenceError::Refused(Refusal::ChangedProjection))?;
    verify_projection_binding(binding)?;
    let proposed_gen = *intent
        .proposed_dirty_generations
        .get(day.as_str())
        .ok_or(ConvergenceError::Refused(Refusal::ChangedProjection))?;

    let health = ensure_chronicle_health(store, day)?;
    abort_after_health_dir()?;

    let stream_class = classify_stream(&health, binding, intent.serial, proposed_gen)?;
    let daily_class = classify_daily(&health, binding)?;
    match stream_class {
        StreamClass::ExactPrior => {
            write_proposed_stream(&health, &binding.proposed_stream)?;
            abort_after_stream()?;
        }
        StreamClass::ExactProposed => {
            resync_proposed_stream(&health, &binding.proposed_stream)?;
        }
    }

    match daily_class {
        DailyClass::ExactPrior => unlink_exact_prior_daily(&health)?,
        DailyClass::ExactProposed => {}
    }
    abort_after_daily()?;
    sync_dir_bound(&health).map_err(|source| ConvergenceError::Io {
        operation: "sync chronicle health directory",
        role: DurableRole::ChronicleHealth,
        source,
    })?;
    abort_after_sync()?;
    Ok(())
}

fn ensure_chronicle_health(
    store: &ConvergenceStore,
    day: &DayKey,
) -> Result<OwnedFd, ConvergenceError> {
    let chronicle = ensure_child_dir(store.root(), CHRONICLE)?;
    let day_dir = ensure_child_dir(&chronicle, day.as_str())?;
    ensure_child_dir(&day_dir, HEALTH)
}

fn ensure_child_dir(parent: &impl AsFd, name: &str) -> Result<OwnedFd, ConvergenceError> {
    if let Some(existing) = open_dir(parent, name)? {
        return Ok(existing);
    }
    create_directory_bound(parent, OsStr::new(name), 0o700).map_err(|error| {
        ConvergenceError::Io {
            operation: "create chronicle ancestor",
            role: DurableRole::Directory,
            source: std::io::Error::other(error.to_string()),
        }
    })?;
    sync_dir_bound(parent).map_err(|source| ConvergenceError::Io {
        operation: "sync parent after chronicle mkdir",
        role: DurableRole::Directory,
        source,
    })?;
    let created = open_dir(parent, name)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })?;
    sync_dir_bound(&created).map_err(|source| ConvergenceError::Io {
        operation: "sync created chronicle directory",
        role: DurableRole::Directory,
        source,
    })?;
    Ok(created)
}

fn read_stream_bytes(health: &impl AsFd) -> Result<Option<Vec<u8>>, ConvergenceError> {
    match read_bytes_bound(health, OsStr::new(STREAM_UPDATED)) {
        Ok(bytes) => Ok(bytes),
        Err(solstone_core_journal_io::ReadError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(solstone_core_journal_io::ReadError::Io { source, .. }) => Err(ConvergenceError::Io {
            operation: "read stream.updated",
            role: DurableRole::StreamUpdated,
            source,
        }),
        Err(solstone_core_journal_io::ReadError::Malformed(_)) => Err(ConvergenceError::Unknown {
            role: DurableRole::StreamUpdated,
        }),
    }
}

fn strip_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn classify_stream(
    health: &impl AsFd,
    binding: &ProjectionBinding,
    intent_serial: u64,
    proposed_gen: u64,
) -> Result<StreamClass, ConvergenceError> {
    let on_disk = read_stream_bytes(health)?;
    let payload = on_disk.as_deref().map(strip_newline);
    if let (PresentAbsent::Present { bytes, digest }, Some(raw)) =
        (&binding.proposed_stream, payload)
    {
        if raw == bytes.as_bytes() {
            return Ok(StreamClass::ExactProposed);
        }
        let computed = digest_bytes(raw);
        if computed.as_hex() == digest.as_str() && raw != bytes.as_bytes() {
            return Err(ConvergenceError::Refused(Refusal::ProjectionByteMismatch));
        }
    }
    if let PresentAbsent::Present { bytes, .. } = &binding.prior_stream {
        if payload == Some(bytes.as_bytes()) {
            return Ok(StreamClass::ExactPrior);
        }
    } else if payload.is_none() {
        return Ok(StreamClass::ExactPrior);
    }
    let Some(raw) = payload else {
        return Err(ConvergenceError::Refused(Refusal::ConflictingProjection));
    };
    named_stream_refusal(raw, intent_serial, proposed_gen)
}

fn named_stream_refusal(
    raw: &[u8],
    intent_serial: u64,
    proposed_gen: u64,
) -> Result<StreamClass, ConvergenceError> {
    let parsed = match parse_json::<StreamUpdated>(raw, DurableRole::StreamUpdated) {
        Ok(parsed) => parsed,
        Err(_) => {
            return Err(ConvergenceError::Refused(Refusal::ProjectionByteMismatch));
        }
    };
    if parsed.author_serial != intent_serial {
        return Err(ConvergenceError::Refused(Refusal::OldAuthorMarker));
    }
    if parsed.dirty_generation != proposed_gen {
        return Err(ConvergenceError::Refused(Refusal::WrongGenerationMarker));
    }
    Err(ConvergenceError::Refused(Refusal::ConflictingProjection))
}

fn write_proposed_stream(
    health: &impl AsFd,
    proposed: &PresentAbsent,
) -> Result<(), ConvergenceError> {
    let PresentAbsent::Present { bytes, .. } = proposed else {
        return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
    };
    replace_stream(health, bytes)
}

fn resync_proposed_stream(
    health: &impl AsFd,
    proposed: &PresentAbsent,
) -> Result<(), ConvergenceError> {
    let PresentAbsent::Present { bytes, .. } = proposed else {
        return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
    };
    match replace_stream(health, bytes) {
        Ok(()) => Ok(()),
        Err(ConvergenceError::Unknown {
            role: DurableRole::StreamUpdated,
        }) => {
            let again = read_stream_bytes(health)?;
            if strip_newline(again.as_deref().unwrap_or_default()) == bytes.as_bytes() {
                Ok(())
            } else {
                Err(ConvergenceError::Unknown {
                    role: DurableRole::StreamUpdated,
                })
            }
        }
        Err(error) => Err(error),
    }
}

fn replace_stream(health: &impl AsFd, bytes: &str) -> Result<(), ConvergenceError> {
    let mut disk = bytes.as_bytes().to_vec();
    disk.push(b'\n');
    let outcome = atomic_replace_bound(health, OsStr::new(STREAM_UPDATED), &disk, 0o600).map_err(
        |error| ConvergenceError::PreservedPrior {
            operation: error.operation,
            source: error.source,
        },
    )?;
    if uncertain(outcome) {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::StreamUpdated,
        });
    }
    Ok(())
}

fn read_daily_bytes(health: &impl AsFd) -> Result<Option<Vec<u8>>, ConvergenceError> {
    match read_bytes_bound(health, OsStr::new(DAILY_UPDATED)) {
        Ok(bytes) => Ok(bytes),
        Err(solstone_core_journal_io::ReadError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(solstone_core_journal_io::ReadError::Io { source, .. }) => Err(ConvergenceError::Io {
            operation: "read daily.updated",
            role: DurableRole::DailyUpdated,
            source,
        }),
        Err(solstone_core_journal_io::ReadError::Malformed(_)) => {
            Err(ConvergenceError::Refused(Refusal::ConflictingProjection))
        }
    }
}

fn classify_daily(
    health: &impl AsFd,
    binding: &ProjectionBinding,
) -> Result<DailyClass, ConvergenceError> {
    if !matches!(binding.proposed_daily, PresentAbsent::Absent) {
        return Err(ConvergenceError::Refused(Refusal::ChangedProjection));
    }
    let on_disk = read_daily_bytes(health)?;
    let payload = on_disk.as_deref().map(strip_newline);
    if payload.is_none() {
        return Ok(DailyClass::ExactProposed);
    }
    if let PresentAbsent::Present { bytes, .. } = &binding.prior_daily
        && payload == Some(bytes.as_bytes())
    {
        return Ok(DailyClass::ExactPrior);
    }
    Err(ConvergenceError::Refused(Refusal::ConflictingProjection))
}

fn unlink_exact_prior_daily(health: &impl AsFd) -> Result<(), ConvergenceError> {
    unlink_bound(health, OsStr::new(DAILY_UPDATED), DurableRole::DailyUpdated)?;
    sync_dir_bound(health).map_err(|source| ConvergenceError::Io {
        operation: "sync chronicle health after daily unlink",
        role: DurableRole::ChronicleHealth,
        source,
    })?;
    if read_daily_bytes(health)?.is_some() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::DailyUpdated,
        });
    }
    Ok(())
}

#[cfg(test)]
fn classify_and_unlink_daily(
    health: &impl AsFd,
    binding: &ProjectionBinding,
) -> Result<(), ConvergenceError> {
    match classify_daily(health, binding)? {
        DailyClass::ExactProposed => Ok(()),
        DailyClass::ExactPrior => unlink_exact_prior_daily(health),
    }
}

fn uncertain(outcome: BoundAtomicOutcome) -> bool {
    #[cfg(test)]
    if crate::test_support::take_fail_dir_sync() {
        return true;
    }
    matches!(
        outcome,
        BoundAtomicOutcome::PublishedDurabilityUncertain { .. }
    )
}

#[cfg(test)]
fn abort_after(step: crate::test_support::PublishFault) -> Result<(), ConvergenceError> {
    if crate::test_support::take_publish_fault(step) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after projection step"),
        });
    }
    Ok(())
}

#[cfg(test)]
fn abort_after_health_dir() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterHealthDir)
}
#[cfg(not(test))]
fn abort_after_health_dir() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_stream() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterProjectionStream)
}
#[cfg(not(test))]
fn abort_after_stream() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_daily() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterDailyUnlink)
}
#[cfg(not(test))]
fn abort_after_daily() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_sync() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterProjectionSync)
}
#[cfg(not(test))]
fn abort_after_sync() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::error::Refusal;
    use crate::schema::{Intent, intent_digest};
    use crate::test_support::{
        PublishFault, admit_days, continue_ok, continue_with_fault, sample_day, snapshot_tree,
    };
    use std::fs;
    use std::path::Path;

    fn marker_path(journal: &Path) -> std::path::PathBuf {
        journal.join("chronicle/20260823/health/stream.updated")
    }

    fn daily_path(journal: &Path) -> std::path::PathBuf {
        journal.join("chronicle/20260823/health/daily.updated")
    }

    fn health_path(journal: &Path) -> std::path::PathBuf {
        journal.join("chronicle/20260823/health")
    }

    fn read_intent_file(journal: &Path, serial: u64) -> Intent {
        let raw =
            fs::read(journal.join(format!("health/convergence/intents/{serial}.json"))).unwrap();
        serde_json::from_slice(raw.strip_suffix(b"\n").unwrap_or(&raw)).unwrap()
    }

    fn legacy_catchup_dirty(health: &Path) -> bool {
        let stream = health.join("stream.updated");
        if !stream.is_file() {
            return false;
        }
        let daily = health.join("daily.updated");
        if !daily.is_file() {
            return true;
        }
        let stream_mtime = fs::metadata(&stream).unwrap().modified().unwrap();
        let daily_mtime = fs::metadata(&daily).unwrap().modified().unwrap();
        daily_mtime < stream_mtime
    }

    fn legacy_source_complete(health: &Path) -> bool {
        let stream = health.join("stream.updated");
        if !stream.is_file() {
            return true;
        }
        let daily = health.join("daily.updated");
        if !daily.is_file() {
            return false;
        }
        let stream_mtime = fs::metadata(&stream).unwrap().modified().unwrap();
        let daily_mtime = fs::metadata(&daily).unwrap().modified().unwrap();
        stream_mtime <= daily_mtime
    }

    #[test]
    fn ac10_10_48_prior_writes_proposed() {
        let (temporary, admitted) = admit_days("p48", &["20260823"]);
        let _held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let marker = fs::read(marker_path(&journal)).unwrap();
        assert!(!marker.is_empty());
        assert!(!daily_path(&journal).exists());
        let intent = read_intent_file(&journal, 1);
        let proposed = intent.projections.get("20260823").unwrap();
        match &proposed.proposed_stream {
            PresentAbsent::Present { bytes, digest } => {
                assert!(!bytes.is_empty());
                assert_eq!(digest_bytes(bytes.as_bytes()).as_hex(), digest.as_str());
                assert_eq!(strip_newline(&marker), bytes.as_bytes());
            }
            PresentAbsent::Absent => panic!("proposed stream must be Present"),
        }
        assert!(matches!(proposed.proposed_daily, PresentAbsent::Absent));
    }

    #[test]
    fn ac10_10_49_proposed_is_idempotent() {
        let (temporary, admitted) = admit_days("p49", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let before = snapshot_tree(&journal);
        held.proceed().unwrap();
        let after = snapshot_tree(&journal);
        assert_eq!(
            before.get("chronicle/20260823/health/stream.updated"),
            after.get("chronicle/20260823/health/stream.updated")
        );
        assert!(!daily_path(&journal).exists());
    }

    #[test]
    fn ac10_10_50_conflict_no_overwrite_no_permit() {
        let (temporary, admitted) = admit_days("p50", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        fs::write(marker_path(&journal), b"not-a-marker\n").unwrap();
        let before = snapshot_tree(&journal);
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::ProjectionByteMismatch)
                    | ConvergenceError::Refused(Refusal::ConflictingProjection)
            ),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_50_11_planted_daily_absent_prior_conflicts() {
        let (temporary, admitted) = admit_days("p50-daily", &["20260823"]);
        let (mut held, error) = continue_with_fault(&admitted, PublishFault::AfterHealthDir);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        fs::write(daily_path(&temporary.journal_path()), b"").unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::ConflictingProjection)
            ),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        assert!(daily_path(&temporary.journal_path()).exists());
        assert!(!marker_path(&temporary.journal_path()).exists());
    }

    #[test]
    fn ac10_10_48_11_present_prior_daily_unlinks() {
        let temporary = crate::test_support::TempDir::new("prior-daily");
        let health_path = temporary.path().join("health");
        fs::create_dir(&health_path).unwrap();
        fs::write(health_path.join("daily.updated"), b"").unwrap();
        let health = fs::File::open(&health_path).unwrap();
        let digest = digest_bytes(b"").as_hex().to_owned();
        let binding = ProjectionBinding {
            prior_stream: PresentAbsent::Absent,
            prior_daily: PresentAbsent::Present {
                bytes: String::new(),
                digest,
            },
            proposed_stream: PresentAbsent::Present {
                bytes: "unused".into(),
                digest: digest_bytes(b"unused").as_hex().to_owned(),
            },
            proposed_daily: PresentAbsent::Absent,
        };
        super::classify_and_unlink_daily(&health, &binding).unwrap();
        assert!(!health_path.join("daily.updated").exists());
    }

    #[test]
    fn ac10_10_49_11_absent_daily_idempotent() {
        let (temporary, admitted) = admit_days("p49-daily", &["20260823"]);
        let mut held = continue_ok(&admitted);
        assert!(!daily_path(&temporary.journal_path()).exists());
        let before = snapshot_tree(&temporary.journal_path());
        held.proceed().unwrap();
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        assert!(!daily_path(&temporary.journal_path()).exists());
    }

    #[test]
    fn ac10_10_51_recompute_marker_and_intent_hex() {
        let (temporary, admitted) = admit_days("p51", &["20260823"]);
        let _held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let intent = read_intent_file(&journal, 1);
        let binding = intent.projections.get("20260823").unwrap();
        let PresentAbsent::Present { bytes, digest } = &binding.proposed_stream else {
            panic!("present");
        };
        let marker: StreamUpdated = serde_json::from_str(bytes).unwrap();
        let recomputed = canonical_json_bytes(&marker).unwrap();
        assert_eq!(recomputed, bytes.as_bytes());
        assert_eq!(digest_bytes(&recomputed).as_hex(), digest.as_str());
        let recomputed_intent = intent_digest(&intent).unwrap();
        assert_eq!(recomputed_intent.as_hex(), intent.intent_digest);
    }

    #[test]
    fn ac10_10_52_57_wrong_generation_marker() {
        let (temporary, admitted) = admit_days("p52", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let mut marker: StreamUpdated =
            serde_json::from_slice(&fs::read(marker_path(&journal)).unwrap()).unwrap();
        marker.dirty_generation = 9;
        let mut body = canonical_json_bytes(&marker).unwrap();
        body.push(b'\n');
        fs::write(marker_path(&journal), body).unwrap();
        let before = snapshot_tree(&journal);
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::WrongGenerationMarker)
            ),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_53_one_byte_marker_mismatch() {
        let (temporary, admitted) = admit_days("p53", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let mut raw = fs::read(marker_path(&journal)).unwrap();
        raw[0] ^= 0x01;
        fs::write(marker_path(&journal), &raw).unwrap();
        let before = snapshot_tree(&journal);
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::ProjectionByteMismatch)
            ),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_54_intent_projection_field_named_refusal() {
        let (temporary, admitted) = admit_days("p54", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let path = journal.join("health/convergence/intents/1.json");
        let mut intent = read_intent_file(&journal, 1);
        intent
            .proposed_dirty_generations
            .insert("20260823".into(), 9);
        intent.intent_digest = intent_digest(&intent).unwrap().as_hex().to_owned();
        let mut body = crate::digest::canonical_json_bytes(&intent).unwrap();
        body.push(b'\n');
        fs::write(&path, body).unwrap();
        let before = snapshot_tree(&journal);
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::ChangedProjection)),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_55_old_projection_digest() {
        let (temporary, admitted) = admit_days("p55", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let path = journal.join("health/convergence/intents/1.json");
        let mut intent = read_intent_file(&journal, 1);
        match intent.projections.get_mut("20260823").unwrap() {
            ProjectionBinding {
                proposed_stream: PresentAbsent::Present { digest, .. },
                ..
            } => *digest = "ab".repeat(32),
            _ => panic!("present"),
        }
        let mut body = crate::digest::canonical_json_bytes(&intent).unwrap();
        body.push(b'\n');
        fs::write(&path, body).unwrap();
        let before = snapshot_tree(&journal);
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::OldProjectionDigest)
            ),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_58_old_author_marker() {
        let (temporary, admitted) = admit_days("p58", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let mut marker: StreamUpdated =
            serde_json::from_slice(&fs::read(marker_path(&journal)).unwrap()).unwrap();
        marker.author_serial = 99;
        let mut body = canonical_json_bytes(&marker).unwrap();
        body.push(b'\n');
        fs::write(marker_path(&journal), body).unwrap();
        let before = snapshot_tree(&journal);
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::OldAuthorMarker)),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_56_154_g5_to_g6_changes_bytes_under_still_clock() {
        let (temporary, admitted) = admit_days("p56", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        for _ in 0..4 {
            crate::test_support::advance_dirty_ok(&mut held);
        }
        let g5 = fs::read(marker_path(&journal)).unwrap();
        crate::test_support::advance_dirty_ok(&mut held);
        let g6 = fs::read(marker_path(&journal)).unwrap();
        assert_ne!(g5, g6, "marker bytes must change G5→G6 with no clock input");
        assert!(!daily_path(&journal).exists());
        assert!(legacy_catchup_dirty(&health_path(&journal)));
        assert!(!legacy_source_complete(&health_path(&journal)));
    }

    #[test]
    fn ac10_legacy_mtime_tie_daily_wins_without_reader_crates() {
        let temporary = crate::test_support::TempDir::new("mtime-tie");
        let health = temporary.path().join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), b"").unwrap();
        fs::write(health.join("daily.updated"), b"").unwrap();
        let stream_mtime = fs::metadata(health.join("stream.updated"))
            .unwrap()
            .modified()
            .unwrap();
        let daily_mtime = fs::metadata(health.join("daily.updated"))
            .unwrap()
            .modified()
            .unwrap();
        if stream_mtime <= daily_mtime {
            assert!(!legacy_catchup_dirty(&health));
            assert!(legacy_source_complete(&health));
        } else {
            assert!(legacy_catchup_dirty(&health));
            assert!(!legacy_source_complete(&health));
        }
        fs::remove_file(health.join("daily.updated")).unwrap();
        assert!(legacy_catchup_dirty(&health));
        assert!(!legacy_source_complete(&health));
        fs::remove_file(health.join("stream.updated")).unwrap();
        assert!(!legacy_catchup_dirty(&health));
        assert!(legacy_source_complete(&health));
    }

    #[test]
    fn ac10_10_14_15_projection_phase_root_replacement() {
        for (name, poison) in [
            ("proj-ident", b"same-name".as_slice()),
            ("proj-div", b"divergent"),
        ] {
            let (temporary, admitted) = admit_days(name, &["20260823"]);
            let (mut held, error) = continue_with_fault(&admitted, PublishFault::AfterHealthDir);
            assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
            let journal = temporary.journal_path();
            let moved = temporary.path().join(format!("journal-moved-{name}"));
            fs::rename(&journal, &moved).unwrap();
            fs::create_dir(&journal).unwrap();
            fs::write(journal.join("poison"), poison).unwrap();
            held.proceed().unwrap();
            assert_eq!(fs::read(journal.join("poison")).unwrap(), poison);
            assert!(!journal.join("chronicle").exists());
            assert!(
                moved
                    .join("chronicle/20260823/health/stream.updated")
                    .is_file()
            );
        }
    }

    #[test]
    fn ac10_section_53_p0_p4_fault_boundaries() {
        struct Case {
            id: &'static str,
            fault: PublishFault,
            must: &'static [&'static str],
            must_not: &'static [&'static str],
        }
        let cases = [
            Case {
                id: "AC10-5.3-AfterHealthDir",
                fault: PublishFault::AfterHealthDir,
                must: &["chronicle/20260823/health"],
                must_not: &[
                    "chronicle/20260823/health/stream.updated",
                    "chronicle/20260823/health/daily.updated",
                ],
            },
            Case {
                id: "AC10-5.3-AfterProjectionStream",
                fault: PublishFault::AfterProjectionStream,
                must: &["chronicle/20260823/health/stream.updated"],
                must_not: &["chronicle/20260823/health/daily.updated"],
            },
            Case {
                id: "AC10-5.3-AfterDailyUnlink",
                fault: PublishFault::AfterDailyUnlink,
                must: &["chronicle/20260823/health/stream.updated"],
                must_not: &["chronicle/20260823/health/daily.updated"],
            },
            Case {
                id: "AC10-5.3-AfterProjectionSync",
                fault: PublishFault::AfterProjectionSync,
                must: &["chronicle/20260823/health/stream.updated"],
                must_not: &["chronicle/20260823/health/daily.updated"],
            },
        ];
        for case in cases {
            let (temporary, admitted) = admit_days(case.id, &["20260823"]);
            let (_held, error) = continue_with_fault(&admitted, case.fault);
            assert!(
                matches!(error, ConvergenceError::PreservedPrior { .. }),
                "{} {error:?}",
                case.id
            );
            let after = snapshot_tree(&temporary.journal_path());
            for path in case.must {
                assert!(after.contains_key(*path), "{} missing {path}", case.id);
            }
            for path in case.must_not {
                assert!(!after.contains_key(*path), "{} unexpected {path}", case.id);
            }
        }
    }

    #[test]
    fn ac10_10_21_27_33_39_45_lineage_projection_surfaces() {
        struct Case {
            id: &'static str,
            kind: &'static str,
        }
        let cases = [
            Case {
                id: "10.21",
                kind: "a",
            },
            Case {
                id: "10.27",
                kind: "b",
            },
            Case {
                id: "10.33",
                kind: "c",
            },
            Case {
                id: "10.39",
                kind: "d",
            },
            Case {
                id: "10.45",
                kind: "e",
            },
        ];
        for case in cases {
            let (src, admitted_src) = admit_days(&format!("{}-src", case.id), &["20260823"]);
            let mut held_src = continue_ok(&admitted_src);
            if case.kind == "e" {
                crate::test_support::advance_dirty_ok(&mut held_src);
            }
            drop(held_src);
            let dst_days: &[&str] = if case.kind == "b" {
                &["20260823", "20260824"]
            } else {
                &["20260823"]
            };
            let (dst, admitted_dst) = admit_days(&format!("{}-dst", case.id), dst_days);
            let mut held_dst = continue_ok(&admitted_dst);
            let from = src
                .journal_path()
                .join("chronicle/20260823/health/stream.updated");
            let to = if case.kind == "b" {
                dst.journal_path()
                    .join("chronicle/20260824/health/stream.updated")
            } else {
                dst.journal_path()
                    .join("chronicle/20260823/health/stream.updated")
            };
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut bytes = fs::read(&from).unwrap();
            if case.kind == "c" || case.kind == "d" {
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                value["journal_id"] = serde_json::Value::String("other".into());
                if case.kind == "d" {
                    value["root_id"] = serde_json::Value::String("mixed".into());
                }
                bytes = serde_json::to_vec(&value).unwrap();
                bytes.push(b'\n');
            }
            if case.kind == "e" {
                let dest_marker: serde_json::Value = serde_json::from_slice(
                    &fs::read(
                        dst.journal_path()
                            .join("chronicle/20260823/health/stream.updated"),
                    )
                    .unwrap(),
                )
                .unwrap();
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                value["journal_id"] = dest_marker["journal_id"].clone();
                value["root_id"] = dest_marker["root_id"].clone();
                value["adoption_id"] = dest_marker["adoption_id"].clone();
                value["day"] = dest_marker["day"].clone();
                bytes = serde_json::to_vec(&value).unwrap();
                bytes.push(b'\n');
            }
            fs::write(&to, &bytes).unwrap();
            let before = snapshot_tree(&dst.journal_path());
            let error = held_dst.proceed().unwrap_err();
            assert!(
                matches!(error, ConvergenceError::Refused(_)),
                "{} {error:?}",
                case.id
            );
            assert_eq!(
                before,
                snapshot_tree(&dst.journal_path()),
                "{} wrote during conflict",
                case.id
            );
        }
    }

    #[test]
    fn ac10_empty_proposed_stream_refused() {
        let present = PresentAbsent::Present {
            bytes: String::new(),
            digest: digest_bytes(b"").as_hex().to_owned(),
        };
        let binding = ProjectionBinding {
            prior_stream: PresentAbsent::Absent,
            prior_daily: PresentAbsent::Absent,
            proposed_stream: present,
            proposed_daily: PresentAbsent::Absent,
        };
        let error = verify_projection_binding(&binding).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ChangedProjection)
        ));
    }

    #[test]
    fn ac10_10_14_retained_projection_survives_rename() {
        let (temporary, admitted) = admit_days("proj-rename", &["20260823"]);
        let held = continue_ok(&admitted);
        let journal = temporary.journal_path();
        let moved = temporary.path().join("journal-moved");
        fs::rename(&journal, &moved).unwrap();
        fs::create_dir(&journal).unwrap();
        fs::write(journal.join("poison"), b"replacement").unwrap();
        admitted.store().revalidate().unwrap();
        assert!(held.snapshot(&sample_day()).is_ok());
        assert_eq!(fs::read(journal.join("poison")).unwrap(), b"replacement");
        assert!(!journal.join("chronicle").exists());
        let _ = moved;
    }
}
