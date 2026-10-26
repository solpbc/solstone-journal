// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed scheduled commands for the foreground host.
//!
//! Each arm calls the writer the desktop command already uses. This module
//! does not own a second scheduler, ledger, or publication schema.

use std::io::{BufRead, BufReader, ErrorKind};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, FixedOffset, Local, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::operational_log::{
    OplogFormat, OplogWriter, create_oplog_at_checked, fold_oplogs,
};
use solstone_core_maintenance::EmbeddedBackupRoutine;
use solstone_core_speaker_resolve::candidate_tracker::{
    CandidateTracker, SpeakerPoolCatchUp, backfill_speaker_pool_checked,
};
use solstone_core_speaker_resolve::discovery_helper::DiscoveryHelperError;
use solstone_core_speaker_resolve::discovery_scan::{
    DiscoveryRefresh, DiscoveryRefreshError, labels_as_i64, refresh_discovery_with_cluster_checked,
};

type StopCheckpoint = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

use super::HostedCommand;

const HEARTBEAT_RECENCY_MS: i64 = 12 * 3_600_000;
const DISCOVERY_MIN_CLUSTER_SIZE: usize = 5;
const DISCOVERY_MIN_SAMPLES: usize = 3;

pub(super) fn run(hosted: &HostedCommand, args: &[String], checkpoint: &StopCheckpoint) -> i32 {
    match args {
        [command, verb] if command == "indexer" && verb == "drain" => {
            match solstone_core_indexer_store::todo::drain_with_checkpoint(
                &hosted.journal,
                Default::default(),
                checkpoint.as_ref(),
            ) {
                Ok(report) if !report.timed_out && report.failed == 0 => 0,
                Ok(_) => 1,
                Err(error) => {
                    log::warn!("foreground search update: {error}");
                    1
                }
            }
        }
        [command, verb] if command == "indexer" && verb == "--rescan" => {
            match solstone_core_indexer_store::scan::scan_journal_with_checkpoint(
                &hosted.journal,
                false,
                checkpoint.as_ref(),
            ) {
                Ok(report) if report.failed == 0 => 0,
                Ok(_) => 1,
                Err(error) => {
                    log::warn!("foreground search check: {error}");
                    1
                }
            }
        }
        [command] if command == "heartbeat" => heartbeat(hosted, checkpoint),
        [command] if command == "facet-candidates" => facet_candidates(hosted, checkpoint),
        [command, verb] if command == "brain" && verb == "refresh" => {
            brain_refresh(hosted, checkpoint)
        }
        [command, flag] if command == "indexer" && flag == "--rebuild-edges" => {
            rebuild_edges(hosted, checkpoint)
        }
        [command, verb, id] if command == "maintenance" && verb == "run" => {
            maintenance(hosted, id, checkpoint)
        }
        _ => 1,
    }
}

fn stopped(checkpoint: &StopCheckpoint) -> bool {
    checkpoint().is_err()
}

fn heartbeat(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let now = Local::now().fixed_offset();
    if heartbeat_recent(&hosted.journal, now) {
        return 0;
    }
    if stopped(checkpoint) {
        return 1;
    }
    let started = Instant::now();
    let Ok(root) = JournalRoot::open(&hosted.journal) else {
        return 1;
    };
    let mut log = match create_oplog_at_checked(
        root,
        "heartbeat",
        "pass",
        OplogFormat::Jsonl,
        now,
        checkpoint.as_ref(),
    ) {
        Ok(log) => log,
        Err(error) => {
            log::warn!("foreground heartbeat oplog: {error}");
            return 1;
        }
    };
    let pass = write_heartbeat_record(
        &mut log,
        &json!({
            "data_source_errors": [],
            "escalated_targets": [],
            "event": "pass",
            "fired": [],
            "ts": Utc::now().timestamp_millis(),
        }),
        checkpoint.as_ref(),
    );
    if pass
        .as_ref()
        .is_err_and(|error| error.kind() == ErrorKind::Interrupted)
    {
        return 1;
    }
    let outcome = if pass.is_ok() { "success" } else { "error" };
    let recorded = write_heartbeat_record(
        &mut log,
        &json!({
            "duration_seconds": started.elapsed().as_secs(),
            "event": "pass.outcome",
            "outcome": outcome,
            "ts": Utc::now().timestamp_millis(),
        }),
        checkpoint.as_ref(),
    );
    if pass.is_err() || recorded.is_err() {
        1
    } else {
        0
    }
}

fn heartbeat_recent(journal: &std::path::Path, now: DateTime<FixedOffset>) -> bool {
    let today = now.date_naive();
    let Some(previous) = today.pred_opt() else {
        return false;
    };
    let Ok(root) = JournalRoot::open(journal) else {
        return false;
    };
    let newest = fold_oplogs(
        root,
        &[previous, today],
        |newest: &mut Option<i64>, entry, input| {
            if entry.name().source().display_slug() != "heartbeat"
                || entry.name().run().display_slug() != "pass"
                || entry.name().format() != OplogFormat::Jsonl
            {
                return Ok(());
            }
            for line in BufReader::new(input).lines() {
                let line = line?;
                let Ok(Value::Object(row)) = serde_json::from_str(&line) else {
                    continue;
                };
                if row.get("event").and_then(Value::as_str) != Some("pass.outcome")
                    || row.get("outcome").and_then(Value::as_str) != Some("success")
                    || row
                        .get("duration_seconds")
                        .and_then(Value::as_u64)
                        .is_none()
                {
                    continue;
                }
                let Some(ts) = row.get("ts").and_then(Value::as_i64) else {
                    continue;
                };
                *newest = Some(newest.map_or(ts, |current| current.max(ts)));
            }
            Ok(())
        },
    )
    .ok()
    .flatten();
    newest.is_some_and(|ts| now.timestamp_millis().saturating_sub(ts) < HEARTBEAT_RECENCY_MS)
}

fn write_heartbeat_record(
    log: &mut OplogWriter,
    record: &Value,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    log.append_bytes_checked(&bytes, checkpoint)
}

fn facet_candidates(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let zone = solstone_core_journal_config::owner_zone(&hosted.journal);
    let today = Utc::now().with_timezone(&zone).date_naive();
    let day = today.format("%Y%m%d").to_string();
    let candidates = match solstone_core_facets::aggregate_speculative_facets(
        &hosted.journal,
        today,
        solstone_core_facets::FACET_CANDIDATE_MIN_SEGMENTS,
    ) {
        Ok(candidates) => candidates,
        Err(error) => {
            log::warn!("foreground facet candidates: {error}");
            return 1;
        }
    };
    if candidates.is_empty() {
        return 0;
    }
    if stopped(checkpoint) {
        return 1;
    }
    let admit = || checkpoint();
    match solstone_core_facets::record_facet_candidates_checked(
        &hosted.journal,
        &day,
        &candidates,
        &admit,
    ) {
        Ok(_) => 0,
        Err(error) => {
            log::warn!("foreground facet candidates: {error}");
            1
        }
    }
}

fn brain_refresh(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    let host_cancelled = &hosted.cancelled;
    let cancelled = || host_cancelled() || checkpoint().is_err();
    if cancelled() {
        return 2;
    }
    crate::brain::run_scheduled(&hosted.root, &cancelled)
}

fn rebuild_edges(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let admit = || checkpoint();
    match solstone_core_indexer_store::scan::rebuild_edges_with_checkpoint(&hosted.journal, &admit)
    {
        Ok(report) if report.failed == 0 => 0,
        Ok(_) => 1,
        Err(error) => {
            log::warn!("foreground edge rebuild: {error}");
            1
        }
    }
}

fn maintenance(hosted: &HostedCommand, id: &str, checkpoint: &StopCheckpoint) -> i32 {
    match id {
        "speakers:discover-voices" => discover_voices(hosted, checkpoint),
        "speakers:candidate-pair-suggestions" => candidate_pairs(hosted, checkpoint),
        "speakers:name-variants" => name_variants(hosted, checkpoint),
        "speakers:consolidate-pool" => consolidate_pool(hosted, checkpoint),
        "speakers:backfill-pool" => backfill_pool(hosted, checkpoint),
        "backup:run" => backup(hosted, EmbeddedBackupRoutine::Run, checkpoint),
        "backup:verify" => backup(hosted, EmbeddedBackupRoutine::Verify, checkpoint),
        "backup:prune" => backup(hosted, EmbeddedBackupRoutine::Prune, checkpoint),
        "health:mark-raw" | "health:prune-logs" => health(hosted, id, checkpoint),
        _ => 1,
    }
}

fn discover_voices(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let admit = || checkpoint();
    match refresh_discovery_with_cluster_checked(&hosted.journal, cluster_embeddings, &admit) {
        Ok(DiscoveryRefresh::IdentityInvalid | DiscoveryRefresh::NoConfirmedOwner) => 0,
        Ok(DiscoveryRefresh::Refreshed { .. }) => 0,
        Err(DiscoveryRefreshError::Helper(_)) => 2,
        Err(error) => {
            log::warn!("foreground speaker discovery: {error}");
            1
        }
    }
}

fn cluster_embeddings(rows: Vec<Vec<f32>>) -> Result<Vec<i64>, DiscoveryHelperError> {
    let cols = rows.first().map(Vec::len).unwrap_or(0);
    if rows.iter().any(|row| row.len() != cols) {
        return Err(DiscoveryHelperError {
            stage: "cluster",
            reason: "ragged embeddings".into(),
        });
    }
    let flat = rows.iter().flatten().copied().collect::<Vec<_>>();
    solstone_core_speakers::discovery::cluster_embeddings(
        &flat,
        rows.len(),
        cols,
        DISCOVERY_MIN_CLUSTER_SIZE,
        DISCOVERY_MIN_SAMPLES,
    )
    .map(|labels| labels_as_i64(&labels))
    .map_err(|error| DiscoveryHelperError {
        stage: "cluster",
        reason: error.to_string(),
    })
}

fn candidate_pairs(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let admit = || checkpoint();
    match solstone_core_speaker_resolve::candidate_pair_suggestions::refresh_candidate_pair_suggestions_checked(
        &hosted.journal,
        &admit,
    ) {
        Ok(_) => 0,
        Err(error) => {
            log::warn!("foreground speaker candidate pairs: {error}");
            1
        }
    }
}

fn name_variants(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let scan =
        match solstone_core_speaker_resolve::name_variant_scan::detect_name_variant_candidates(
            &hosted.journal,
        ) {
            Ok(scan) => scan,
            Err(error) => {
                log::warn!("foreground speaker name variants: {error}");
                return 1;
            }
        };
    for candidate in &scan.candidates {
        if stopped(checkpoint) {
            return 1;
        }
        let admit = || checkpoint();
        if let Err(error) =
            solstone_core_speaker_resolve::speaker_review_candidates::record_name_variant_candidate_checked(
                &hosted.journal,
                candidate,
                &admit,
            )
        {
            log::warn!("foreground speaker name variants: {error}");
            return 1;
        }
    }
    0
}

fn consolidate_pool(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let admit = || checkpoint();
    match CandidateTracker::new(&hosted.journal).consolidate_dense_candidates_checked(&admit) {
        Ok(_) => 0,
        Err(error) => {
            log::warn!("foreground speaker pool consolidation: {error}");
            1
        }
    }
}

fn backfill_pool(hosted: &HostedCommand, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let admit = || checkpoint();
    match backfill_speaker_pool_checked(&hosted.journal, &admit) {
        SpeakerPoolCatchUp::Refuse(refusal) => {
            log::warn!("foreground speaker pool backfill: {refusal}");
            1
        }
        SpeakerPoolCatchUp::Done(value) => {
            if value.get("busy").and_then(Value::as_bool) == Some(true) {
                return super::UNCOMPLETED_HOLD_EXIT;
            }
            if stopped(checkpoint) {
                return 1;
            }
            match solstone_core_speaker_resolve::owner_voice::refresh_owner_voice_checked(
                &hosted.journal,
                checkpoint,
            ) {
                Ok(_) => 0,
                Err(error) => {
                    log::warn!("foreground owner voice: {error}");
                    1
                }
            }
        }
    }
}

fn health(hosted: &HostedCommand, id: &str, checkpoint: &StopCheckpoint) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let services = solstone_core_maintenance::HealthServices {
        now: Utc::now(),
        zone: solstone_core_journal_config::owner_zone(&hosted.journal),
    };
    let admit = || checkpoint();
    solstone_core_maintenance::bodies::health::run_checked(
        id,
        &[],
        &hosted.journal,
        &services,
        &admit,
    )
    .exit_code
}

fn backup(
    hosted: &HostedCommand,
    routine: EmbeddedBackupRoutine,
    checkpoint: &StopCheckpoint,
) -> i32 {
    if stopped(checkpoint) {
        return 1;
    }
    let reference = format!("scheduled-{}", crate::backup::routine_label(routine));
    let outcome = crate::backup::settle_embedded_backup(
        &hosted.root,
        routine,
        &hosted.backup,
        &reference,
        checkpoint,
    );
    crate::backup::log_backup_outcome(&outcome);
    crate::backup::report_scheduled_terminal(&hosted.root, &outcome);
    crate::backup::outcome_exit(&outcome)
}
