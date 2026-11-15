// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded, interruptible history work beside daily participation recovery.
use serde::Serialize;
use serde_json::Value;
use solstone_core_entity::faceprints::{self, cross};
use solstone_core_journal_io::{LockOptions, PathOrDay, contained_path, hold_lock, iter_segments};
use solstone_core_segment::output_attempt::OutputClaims;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

pub const BATCH_SIZE: usize = 64;
const BUDGET: Duration = Duration::from_secs(solstone_core_segment::face_backfill::MAX_SECONDS);
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
fn meeting_frames(screen: &Path) -> Result<bool> {
    let text = solstone_core_journal_io::read_optional_text(screen)
        .map_err(|e| e.to_string())?
        .ok_or("screen disappeared")?;
    for line in text.lines().skip(1) {
        let row: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        if row
            .pointer("/content/meeting/participants")
            .and_then(Value::as_array)
            .is_some_and(|p| !p.is_empty())
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn valid_sidecar(raw: &Path, digest: &str) -> Result<bool> {
    let path = sidecar_path(raw);
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
                        continue;
                    };
                    if !meeting_frames(screen)? {
                        continue;
                    }
                    let digest = solstone_core_segment::screen_body_digest(screen)
                        .map_err(|e| e.to_string())?;
                    if valid_sidecar(&raw, &digest)? {
                        stats.valid += 1;
                        continue;
                    }
                    let binding = binding(&raw, &digest)?;
                    if backfill.unsupported.get(&key) == Some(&binding) {
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
        return Ok(stats);
    }
    let path = cross::path(journal)?;
    let _run = match hold_lock(path.with_extension("backfill"), options()) {
        Ok(lock) => lock,
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
                    .insert(candidate.key.clone(), current_binding);
                stats.skipped_unsupported += 1;
                stats.pending -= 1;
            }
            Ok(solstone_core_segment::face_backfill::YIELDED) => {
                stats.yielded = true;
                break;
            }
            other => {
                log::warn!("face backfill failed: {other:?}");
                stats.failed += 1;
            }
        }
        // Failed inputs rotate too, so one damaged recording cannot block the
        // rest of the history. A crash before this write is harmless replay.
        backfill.cursor = Some(candidate.key.clone());
        cross::update_backfill(journal, backfill.clone())?;
    }
    // Use committed current participation, including owner edits and undo.
    // Reconciliation handles every published activity affected by this batch,
    // and also recovers interruptions from earlier passes.
    let recovery = if full_recovery {
        solstone_core_facets::reconcile_faceprints(journal)
    } else {
        reconcile_changed(journal, &changed)
    };
    stats.added = match recovery {
        Ok(added) => added,
        Err(error) => {
            log::warn!("face participation recovery deferred: {error}");
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
    let generation = generation.ok_or("face backfill generation was not admitted")?;
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
