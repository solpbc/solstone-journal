// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded, interruptible history work beside daily participation recovery.
use serde::Serialize;
use serde_json::Value;
use solstone_core_entity::faceprints::{self, cross};
use solstone_core_journal_io::{
    LockError, LockOptions, PathOrDay, contained_path, hold_lock, iter_segments,
};
use solstone_core_segment::output_attempt::OutputClaims;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const BATCH_SIZE: usize = 64;
const BUDGET: Duration = Duration::from_secs(solstone_core_segment::face_backfill::MAX_SECONDS);
/// Recording key -> the stamps of what a pass read to settle it: the screen
/// output of a recording that is not a meeting; the screen output and sidecar
/// of a meeting already described; or, for a meeting still owed (pending or
/// unsupported), the screen output, its body digest and the sidecar's stamp or
/// `-` when there is none. Unchanged since, it costs a stat on later passes,
/// not a read. A cache only: a miss, a damaged or a lost file just reads
/// again. Under `cache/`, so backups and portable copies leave it out.
/// Changing what `meeting_frames` or `valid_sidecar` accepts, or how
/// `screen_body_digest` is computed, must bump the version in this name, or
/// old entries keep their old answer.
const SCAN_CACHE: &str = "cache/faces/backfill-scan-v1.json";
/// A file modified this recently could change again inside one timestamp tick
/// without its stamp moving, so it is read again on the next pass instead.
const SETTLED: Duration = Duration::from_secs(2);
type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub processed: usize,
    pub processing_seconds: f64,
    pub skipped_unsupported: usize,
    pub already_skipped: usize,
    pub failed: usize,
    pub valid: usize,
    pub pending: usize,
    pub yielded: bool,
    pub added: usize,
    pub contested_pairs: usize,
    pub merge_pairs: usize,
    pub recovery_failed: bool,
}
#[derive(Clone)]
struct Candidate {
    day: String,
    stream: String,
    segment: String,
    raw: PathBuf,
    screen: PathBuf,
    key: String,
    binding: String,
}
fn options() -> LockOptions {
    LockOptions {
        timeout: Duration::ZERO,
        ..Default::default()
    }
}

/// A fresh discovery snapshot is an admission hint only: it never establishes
/// completion. Pause historical inference while current or waiting work is owed.
fn live_work(journal: &Path) -> Result<bool> {
    Ok(
        solstone_core_system::work_inventory::newer_work_pending(journal)?
            || solstone_core_segment::face_backfill::priority_requested(journal)
                .map_err(|e| e.to_string())?,
    )
}
fn sidecar_path(raw: &Path) -> PathBuf {
    raw.with_file_name(format!(
        "{}.jsonl",
        faceprints::accumulation::source_prefix(
            raw.file_name().unwrap_or_default().to_str().unwrap_or("")
        )
    ))
}
fn stamp(path: &Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let modified = m.modified().ok()?;
    if SystemTime::now()
        .duration_since(modified)
        .map_or(true, |age| age < SETTLED)
    {
        return None;
    }
    let nanos = modified.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{}:{nanos}", m.len()))
}
/// The sidecar's part of an owed meeting's memo: its stamp, or `-` when the
/// meeting has none. `None` while it is still being written.
fn sidecar_token(raw: &Path) -> Option<String> {
    let path = sidecar_path(raw);
    if path.exists() {
        stamp(&path)
    } else {
        Some("-".into())
    }
}
/// A recording found unsupported is not tried again by the same journal
/// version. A new version may decode more, so it tries each one once more.
fn unsupported_mark(binding: &str) -> String {
    format!("{binding} {}", env!("CARGO_PKG_VERSION"))
}
fn load_scan(journal: &Path) -> BTreeMap<String, String> {
    contained_path(journal, SCAN_CACHE)
        .ok()
        .and_then(|p| {
            solstone_core_journal_io::read_optional_text(p)
                .ok()
                .flatten()
        })
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}
fn save_scan(journal: &Path, scan: &BTreeMap<String, String>) {
    let result = contained_path(journal, SCAN_CACHE)
        .map_err(|e| e.to_string())
        .and_then(|p| {
            let text = serde_json::to_string(scan).map_err(|e| e.to_string())?;
            solstone_core_journal_io::write_text(
                p,
                &text,
                solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
            )
            .map_err(|e| e.to_string())
        });
    if let Err(error) = result {
        log::warn!(
            "{}",
            solstone_core_cli_copy::display_for(
                solstone_core_cli_copy::manual_log_locale(),
                "journal.cli.maintenance.face.log.cache",
                [("detail", &error)]
            )
        );
    }
}
#[cfg(test)]
thread_local! {
    static BODY_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn body_read() {
    #[cfg(test)]
    BODY_READS.with(|n| n.set(n.get() + 1));
}
fn binding(raw: &Path, screen_digest: &str) -> Result<String> {
    let m = std::fs::metadata(raw).map_err(|e| e.to_string())?;
    let modified = m
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(format!("{}:{modified}:{screen_digest}", m.len()))
}
/// A recording whose screen output was never written, or was torn by an
/// interrupted write (a row that does not parse, or bytes that are not UTF-8),
/// is not a candidate: the face helper reads the same output strictly and
/// would fail on it. Neither stops the history; the catch-up moves on.
/// Its answer is cached by file stamp: a change here bumps `SCAN_CACHE`.
fn meeting_frames(screen: &Path) -> Result<bool> {
    body_read();
    let bytes = solstone_core_journal_io::read_bytes(screen, vec![]).map_err(|e| e.to_string())?;
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(false);
    };
    // Every row, the header included, must parse before any meeting row counts.
    let mut meeting = false;
    for (index, line) in text.lines().enumerate() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            return Ok(false);
        };
        meeting |= index > 0
            && row
                .pointer("/content/meeting/participants")
                .and_then(Value::as_array)
                .is_some_and(|p| !p.is_empty());
    }
    Ok(meeting)
}
/// Its answer is cached by file stamps: a change here bumps `SCAN_CACHE`.
fn valid_sidecar(raw: &Path, digest: &str) -> Result<bool> {
    let path = sidecar_path(raw);
    body_read();
    let Some(text) =
        solstone_core_journal_io::read_optional_text(path).map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };
    let Some(first) = text.lines().next() else {
        return Ok(false);
    };
    let Ok(header) = serde_json::from_str::<Value>(first) else {
        return Ok(false);
    };
    // The manifest is published last. F2 validates the referenced payloads when
    // accumulating it; a header with a stale screen binding is never complete.
    Ok(header["schema_version"] == 1
        && header["screen_body_sha256"] == digest
        && header["source_name"].as_str() == raw.file_name().and_then(|s| s.to_str()))
}
fn discover(
    journal: &Path,
    backfill: &mut cross::Backfill,
    stats: &mut Stats,
) -> Result<Vec<Candidate>> {
    let mut pending = vec![];
    let mut present = BTreeSet::new();
    let mut scanned = load_scan(journal);
    let mut seen = BTreeMap::new();
    let mut changed = false;
    for (day, _) in solstone_core_journal_io::day_dirs(journal).map_err(|e| e.to_string())? {
        for segment in iter_segments(journal, PathOrDay::Day(&day)).map_err(|e| e.to_string())? {
            if !matches!(
                solstone_core_segment::owner_deletion_state(segment.path()),
                Ok(solstone_core_segment::OwnerDeletionState::Live)
            ) {
                continue;
            }
            let discovery =
                solstone_core_segment::output_attempt::discover_output_claims(segment.path())
                    .map_err(|e| e.to_string())?;
            for screen in discovery.outputs() {
                let Some(stem) = screen.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !(stem == "screen" || stem.ends_with("_screen")) {
                    continue;
                }
                for extension in ["webm", "mp4", "mov", "mkv"] {
                    let raw = screen.with_extension(extension);
                    if !raw.is_file() {
                        continue;
                    }
                    let key = raw
                        .strip_prefix(journal)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/");
                    let raw = contained_path(journal, &key).map_err(|e| e.to_string())?;
                    present.insert(key.clone());
                    let Ok(_claim) = OutputClaims::acquire([screen.clone()], options()) else {
                        stats.yielded = true;
                        if let Some(memo) = scanned.remove(&key) {
                            seen.insert(key, memo);
                        }
                        continue;
                    };
                    // Stamp before reading, so a change during the read is
                    // seen as a change on the next pass.
                    let screen_stamp = stamp(screen);
                    let sidecar_stamp = stamp(&sidecar_path(&raw));
                    let memo = scanned.remove(&key);
                    let mut owed = None;
                    if let (Some(now), Some(memo)) = (&screen_stamp, &memo) {
                        match memo.split(' ').collect::<Vec<_>>()[..] {
                            [screen] if screen == now => {
                                seen.insert(key, memo.clone());
                                continue;
                            }
                            [screen, sidecar]
                                if screen == now && Some(sidecar) == sidecar_stamp.as_deref() =>
                            {
                                stats.valid += 1;
                                seen.insert(key, memo.clone());
                                continue;
                            }
                            [screen, digest, sidecar]
                                if screen == now
                                    && Some(sidecar) == sidecar_token(&raw).as_deref() =>
                            {
                                owed = Some(digest.to_owned());
                                seen.insert(key.clone(), memo.clone());
                            }
                            _ => {}
                        }
                    }
                    let digest = match owed {
                        Some(digest) => digest,
                        None => {
                            changed |= memo.is_some();
                            if !meeting_frames(screen)? {
                                if let Some(now) = screen_stamp {
                                    seen.insert(key, now);
                                    changed = true;
                                }
                                continue;
                            }
                            let digest = solstone_core_segment::screen_body_digest(screen)
                                .map_err(|e| e.to_string())?;
                            if valid_sidecar(&raw, &digest)? {
                                stats.valid += 1;
                                if let (Some(now), Some(sidecar)) = (screen_stamp, sidecar_stamp) {
                                    seen.insert(key, format!("{now} {sidecar}"));
                                    changed = true;
                                }
                                continue;
                            }
                            if let (Some(now), Some(sidecar)) = (screen_stamp, sidecar_token(&raw))
                            {
                                seen.insert(key.clone(), format!("{now} {digest} {sidecar}"));
                                changed = true;
                            }
                            digest
                        }
                    };
                    let binding = binding(&raw, &digest)?;
                    if backfill.unsupported.get(&key) == Some(&unsupported_mark(&binding)) {
                        stats.already_skipped += 1;
                        continue;
                    }
                    pending.push(Candidate {
                        day: day.clone(),
                        stream: segment
                            .stream()
                            .directory()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "_default".into()),
                        segment: segment.name().to_string_lossy().into_owned(),
                        raw,
                        screen: screen.clone(),
                        key,
                        binding,
                    });
                }
            }
        }
    }
    backfill.unsupported.retain(|key, _| present.contains(key));
    // Entries for recordings no longer present, or changed since, drop out.
    if changed || !scanned.is_empty() {
        save_scan(journal, &seen);
    }
    pending.sort_by(|a, b| a.key.cmp(&b.key));
    if let Some(cursor) = &backfill.cursor {
        let split = pending.partition_point(|c| c.key <= *cursor);
        pending.rotate_left(split);
    }
    stats.pending = pending.len();
    Ok(pending)
}

/// Public maintenance seam also used by offline validation. No providers.
pub fn reconcile(journal: &Path) -> Result<Stats> {
    run(
        journal,
        true,
        #[cfg(windows)]
        None,
    )
}

/// Hourly catch-up avoids replaying unaffected meetings.
pub fn backfill(journal: &Path) -> Result<Stats> {
    run(
        journal,
        false,
        #[cfg(windows)]
        None,
    )
}

#[cfg(windows)]
pub fn run_with_generation(
    journal: &Path,
    full_recovery: bool,
    generation: Option<&solstone_core_system::process::ChildLaunchContext>,
) -> Result<Stats> {
    run(journal, full_recovery, generation)
}
fn run(
    journal: &Path,
    full_recovery: bool,
    #[cfg(windows)] generation: Option<&solstone_core_system::process::ChildLaunchContext>,
) -> Result<Stats> {
    let mut stats = Stats::default();
    if !faceprints::enabled(journal)? || cfg!(any(target_os = "ios", target_os = "android")) {
        // Turned off, the catch-up keeps nothing of its own.
        #[expect(
            clippy::disallowed_methods,
            reason = "system internal: the catch-up's own scan cache, read again if missing"
        )]
        let _ = solstone_core_journal_io::remove_file(journal, SCAN_CACHE);
        return Ok(stats);
    }
    let path = cross::path(journal)?;
    let _run = match hold_lock(path.with_extension("backfill"), options()) {
        Ok(lock) => lock,
        Err(LockError::Timeout(_)) if full_recovery => {
            // The lock serializes history discovery and its cursor. Recovery
            // reads committed evidence and takes its own locks.
            return recover(journal, true, &[], stats);
        }
        Err(error) if full_recovery => return Err(error.to_string()),
        Err(_) => {
            stats.yielded = true;
            return Ok(stats);
        }
    };
    let start = Instant::now();
    let budget = if full_recovery {
        Duration::from_secs(8 * 60)
    } else {
        BUDGET
    };
    let mut backfill = cross::load(journal)?.backfill;
    let candidates = discover(journal, &mut backfill, &mut stats)?;
    let mut changed = vec![];
    cross::update_backfill(journal, backfill.clone())?;
    for candidate in candidates.iter().take(BATCH_SIZE) {
        if start.elapsed() >= budget || !faceprints::enabled(journal)? || live_work(journal)? {
            stats.yielded = true;
            break;
        }
        // Revalidate at the use boundary: describe or retention may have run
        // since discovery, and a current face pass must not be redone.
        let claim = match OutputClaims::acquire([candidate.screen.clone()], options()) {
            Ok(c) => c,
            Err(_) => {
                stats.yielded = true;
                continue;
            }
        };
        let digest = solstone_core_segment::screen_body_digest(&candidate.screen)
            .map_err(|e| e.to_string())?;
        if valid_sidecar(&candidate.raw, &digest)? {
            stats.pending -= 1;
            continue;
        }
        let current_binding = binding(&candidate.raw, &digest)?;
        if current_binding != candidate.binding {
            stats.yielded = true;
            continue;
        }
        drop(claim);
        let stop = || {
            start.elapsed() >= budget
                || live_work(journal).unwrap_or(true)
                || !faceprints::enabled(journal).unwrap_or(false)
        };
        let remaining = budget.saturating_sub(start.elapsed()).as_secs().max(1);
        let processing_started = Instant::now();
        let result = extract(
            journal,
            &candidate.raw,
            remaining,
            &stop,
            #[cfg(windows)]
            generation,
        );
        match result {
            Ok(0) => {
                changed.push(candidate);
                stats.processed += 1;
                stats.processing_seconds += processing_started.elapsed().as_secs_f64();
                stats.pending -= 1;
            }
            Ok(solstone_core_segment::face_backfill::UNSUPPORTED) => {
                backfill
                    .unsupported
                    .insert(candidate.key.clone(), unsupported_mark(&current_binding));
                stats.skipped_unsupported += 1;
                stats.pending -= 1;
            }
            Ok(solstone_core_segment::face_backfill::YIELDED) => {
                stats.yielded = true;
                break;
            }
            other => {
                log::warn!(
                    "{}",
                    solstone_core_cli_copy::display_for(
                        solstone_core_cli_copy::manual_log_locale(),
                        "journal.cli.maintenance.face.log.failed",
                        [("detail", &format!("{other:?}"))]
                    )
                );
                stats.failed += 1;
            }
        }
        // Failed inputs rotate too, so one damaged recording cannot block the
        // rest of the history. A crash before this write is harmless replay.
        backfill.cursor = Some(candidate.key.clone());
        cross::update_backfill(journal, backfill.clone())?;
    }
    recover(journal, full_recovery, &changed, stats)
}

fn recover(
    journal: &Path,
    full_recovery: bool,
    changed: &[&Candidate],
    mut stats: Stats,
) -> Result<Stats> {
    // Use committed current participation, including owner edits and undo.
    // Reconciliation handles every published activity affected by this batch,
    // and also recovers interruptions from earlier passes.
    let recovery = if full_recovery {
        solstone_core_facets::reconcile_faceprints(journal)
    } else {
        reconcile_changed(journal, changed)
    };
    stats.added = match recovery {
        Ok(added) => added,
        Err(error) => {
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    solstone_core_cli_copy::manual_log_locale(),
                    "journal.cli.maintenance.face.log.deferred",
                    [("detail", &error)]
                )
            );
            stats.recovery_failed = true;
            return Ok(stats);
        }
    };
    let rules = cross::reconcile(journal)?;
    stats.contested_pairs = rules.contested_pairs.len();
    stats.merge_pairs = rules.merge_pairs.len();
    Ok(stats)
}

fn extract(
    journal: &Path,
    raw: &Path,
    seconds: u64,
    stop: &dyn Fn() -> bool,
    #[cfg(windows)] generation: Option<&solstone_core_system::process::ChildLaunchContext>,
) -> Result<i32> {
    use solstone_core_system::process::{
        Disposition, ManagedLaunchRequest, SpawnOptions, launch_managed_request,
    };
    let binary = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name(if cfg!(windows) {
            "solstone-core-describe.exe"
        } else {
            "solstone-core-describe"
        });
    #[cfg(windows)]
    let generation = generation.ok_or_else(|| {
        solstone_core_cli_copy::display_for(
            solstone_core_cli_copy::manual_log_locale(),
            "journal.cli.maintenance.face.generation",
            [],
        )
    })?;
    let request = ManagedLaunchRequest {
        command: vec![
            binary.to_string_lossy().into_owned(),
            solstone_core_segment::face_backfill::OPTION.into(),
            journal.to_string_lossy().into_owned(),
            raw.to_string_lossy().into_owned(),
            seconds.to_string(),
        ],
        #[cfg(windows)]
        read_file_grants: generation.read_file_grants.clone(),
        options: SpawnOptions {
            journal_root: journal.to_owned(),
            reference: "faces".into(),
            day: None,
            sink: None,
            environment: {
                #[cfg(windows)]
                {
                    generation.environment.clone()
                }
                #[cfg(not(windows))]
                {
                    Default::default()
                }
            },
        },
    };
    let mut child = launch_managed_request(
        Disposition::IndependentBoundedHelper {
            timeout: Duration::from_secs(seconds),
        },
        request,
    )
    .map_err(|e| e.to_string())?;
    loop {
        if let Some(code) = child.poll().map_err(|e| e.to_string())? {
            child.cleanup();
            return Ok(code);
        }
        if stop() {
            child
                .terminate_exact(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            return Ok(solstone_core_segment::face_backfill::YIELDED);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn reconcile_changed(journal: &Path, changed: &[&Candidate]) -> Result<usize> {
    let coordinates: Vec<_> = changed
        .iter()
        .map(|c| (c.day.clone(), c.stream.clone(), c.segment.clone()))
        .collect();
    solstone_core_facets::reconcile_faceprints_for_segments(journal, &coordinates)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;
